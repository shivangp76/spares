use crate::adapters::SrsAdapter;
use crate::adapters::migration::MigrationFunc;
use crate::api::note::render_notes;
use crate::api::{
    note::{create_notes, delete_notes, update_notes},
    parser::list_parsers,
};
use crate::config::{Environment, get_env_config};
use crate::model::CustomData;
use crate::parsers::{NoteImportAction, NoteSettings, Parseable, get_all_parsers};
use crate::schema::FilterOptions;
use crate::schema::note::{
    CreateNoteRequest, CreateNotesRequest, DeleteNotesRequest, NotesResponse, NotesSelector,
    RenderNotesRequest, UpdateNotesRequest, UpdateTags,
};
use crate::schema::parser::ParserResponse;
use crate::{AdapterErrorKind, Error, LibraryError, ParserErrorKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::info;
use reqwest::StatusCode;
use reqwest::{Client, Response};
use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug)]
pub struct SparesAdapter {
    request_processor: SparesRequestProcessor,
}

#[derive(Debug)]
pub enum SparesRequestProcessor {
    Server,
    /// For testing use only
    Database {
        pool: SqlitePool,
    },
}

#[derive(Debug)]
enum SparesRequestProcessorInternal {
    Server { base_url: String, client: Client },
    Database { pool: SqlitePool },
}

impl SparesAdapter {
    pub fn new(request_processor: SparesRequestProcessor) -> Self {
        Self { request_processor }
    }

    async fn handle_response(&self, response: Response) -> Result<Response, Error> {
        let status = response.status();
        if status != StatusCode::OK {
            let response_json: Value = response.json().await.map_err(Error::ApiRequest)?;
            let message = response_json.get("message");
            info!("{:?}", &message);
            return Err(Error::Library(LibraryError::Adapter(
                AdapterErrorKind::Custom {
                    adapter_name: self.get_adapter_name().to_string(),
                    error: response_json.to_string(),
                },
            )));
        }
        Ok(response)
    }

    pub async fn update_custom_data(
        &self,
        note_id: i64,
        custom_data: CustomData,
        dry_run: bool,
        at: DateTime<Utc>,
    ) -> Result<(), Error> {
        let request_processor = match self.request_processor {
            SparesRequestProcessor::Server => {
                let env_config = get_env_config(Environment::Production);
                let base_url = format!("http://{}", env_config.socket_address);
                SparesRequestProcessorInternal::Server {
                    base_url,
                    client: Client::new(),
                }
            }
            SparesRequestProcessor::Database { ref pool } => {
                SparesRequestProcessorInternal::Database { pool: pool.clone() }
            }
        };
        let update_note_request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            custom_data: Some(custom_data),
            parser_id: None,
            data: None,
            keywords: None,
            tags: UpdateTags::None,
        };
        if !dry_run {
            match &request_processor {
                SparesRequestProcessorInternal::Server { base_url, client } => {
                    let url = format!("{}/api/notes", base_url);
                    let response = client
                        .patch(url)
                        .json(&update_note_request)
                        .send()
                        .await
                        .map_err(Error::ApiRequest)?;
                    self.handle_response(response).await?;
                }
                SparesRequestProcessorInternal::Database { pool } => {
                    let _notes_res =
                        update_notes(pool, update_note_request, at, &get_all_parsers(), false).await?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SrsAdapter for SparesAdapter {
    fn get_adapter_name(&self) -> &'static str {
        "spares"
    }

    async fn migrate(
        &mut self,
        _base_url: &str,
        _spares_pool: &SqlitePool,
        _migration_function: Option<MigrationFunc>,
        _initial_migration: bool,
        _dry_run: bool,
    ) -> Result<(), Error> {
        unreachable!("The default adapter has no migrations.");
    }

    #[expect(clippy::too_many_lines)]
    async fn process_data(
        &mut self,
        notes: Vec<(NoteSettings, Option<String>)>,
        parser: &dyn Parseable,
        dry_run: bool,
        _quiet: bool,
        at: DateTime<Utc>,
    ) -> Result<(), Error> {
        let request_processor = match self.request_processor {
            SparesRequestProcessor::Server => {
                let env_config = get_env_config(Environment::Production);
                let base_url = format!("http://{}", env_config.socket_address);
                SparesRequestProcessorInternal::Server {
                    base_url,
                    client: Client::new(),
                }
            }
            SparesRequestProcessor::Database { ref pool } => {
                SparesRequestProcessorInternal::Database { pool: pool.clone() }
            }
        };

        // Get parser id
        let parser_responses: Vec<ParserResponse> = match &request_processor {
            SparesRequestProcessorInternal::Server { base_url, client } => {
                let url = format!("{}/api/parsers", base_url);
                let mut response = client.get(url).send().await.map_err(Error::ApiRequest)?;
                response = self.handle_response(response).await?;
                response.json().await.map_err(Error::ApiRequest)?
            }
            SparesRequestProcessorInternal::Database { pool } => {
                list_parsers(pool, FilterOptions::default()).await?
            }
        };
        let parser_response = parser_responses
            .into_iter()
            .find(|p| p.name == parser.get_parser_name())
            .ok_or(Error::Library(LibraryError::Parser(
                ParserErrorKind::NotFound(parser.get_parser_name().to_string()),
            )))?;
        let parser_id = parser_response.id;

        let mut create_note_requests: Vec<CreateNoteRequest> = Vec::new();
        let mut delete_note_ids: Vec<i64> = Vec::new();
        for (local_settings, note_data_res) in notes {
            if note_data_res.is_none() {
                continue;
            }
            let note_data = note_data_res.unwrap();
            match local_settings.action {
                NoteImportAction::Add => {
                    let create_note_request = CreateNoteRequest {
                        data: note_data,
                        keywords: local_settings.keywords,
                        tags: local_settings.tags,
                        is_suspended: local_settings.is_suspended,
                        custom_data: local_settings.custom_data,
                    };
                    create_note_requests.push(create_note_request);
                }
                NoteImportAction::Update(note_id) => {
                    let update_note_request = UpdateNotesRequest {
                        selector: NotesSelector::Ids(vec![note_id]),
                        parser_id: Some(parser_id),
                        data: Some(note_data),
                        keywords: Some(local_settings.keywords),
                        tags: UpdateTags::SetTags(local_settings.tags),
                        custom_data: Some(local_settings.custom_data),
                    };
                    if !dry_run {
                        match &request_processor {
                            SparesRequestProcessorInternal::Server { base_url, client } => {
                                let url = format!("{}/api/notes", base_url);
                                let response = client
                                    .patch(url)
                                    .json(&update_note_request)
                                    .send()
                                    .await
                                    .map_err(Error::ApiRequest)?;
                                self.handle_response(response).await?;
                            }
                            SparesRequestProcessorInternal::Database { pool } => {
                                let _notes_res =
                                    update_notes(pool, update_note_request, at, &get_all_parsers(), false)
                                        .await?;
                            }
                        }
                    }
                }
                NoteImportAction::Delete(note_id) => {
                    delete_note_ids.push(note_id);
                }
            }
        }

        // Issue a single bulk delete for all collected note IDs.
        if !delete_note_ids.is_empty() && !dry_run {
            let request = DeleteNotesRequest {
                selector: NotesSelector::Ids(delete_note_ids),
            };
            match &request_processor {
                SparesRequestProcessorInternal::Server { base_url, client } => {
                    let url = format!("{}/api/notes", base_url);
                    let response = client
                        .delete(url)
                        .json(&request)
                        .send()
                        .await
                        .map_err(Error::ApiRequest)?;
                    self.handle_response(response).await?;
                }
                SparesRequestProcessorInternal::Database { pool } => {
                    delete_notes(pool, request, &get_all_parsers(), false).await?;
                }
            }
        }

        let create_notes_request = CreateNotesRequest {
            parser_id,
            requests: create_note_requests,
        };
        // Create notes and render created notes
        // We cannot render updated notes since that might overwrite unsaved changes.
        if !dry_run {
            let created_note_ids = match &request_processor {
                SparesRequestProcessorInternal::Server { base_url, client } => {
                    let url = format!("{}/api/notes", base_url);
                    let response = client
                        .post(url)
                        .json(&create_notes_request)
                        .send()
                        .await
                        .map_err(Error::ApiRequest)?;
                    let response = self.handle_response(response).await?;
                    let response: NotesResponse = response.json().await.map_err(|e| {
                        Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                            adapter_name: self.get_adapter_name().to_string(),
                            error: e.to_string(),
                        }))
                    })?;
                    response.notes.into_iter().map(|n| n.id).collect::<Vec<_>>()
                }
                SparesRequestProcessorInternal::Database { pool } => {
                    let notes_res =
                        create_notes(pool, create_notes_request, at, &get_all_parsers(), false).await?;
                    notes_res
                        .notes
                        .into_iter()
                        .map(|n| n.id)
                        .collect::<Vec<_>>()
                }
            };
            // If testing, then we
            // - do not want to render notes to save time. We can explicitly call render notes if we need to.
            // - want to render notes in a different directory, so we don't overwrite user data.
            if !cfg!(test) {
                let request = RenderNotesRequest {
                    selector: NotesSelector::Ids(created_note_ids),
                    immutable_note_ids: None,
                    overridden_output_raw_dir: None,
                    include_linked_notes: true,
                    include_cards: true,
                    generate_rendered: true,
                    force_generate_rendered: false,
                };
                match &request_processor {
                    SparesRequestProcessorInternal::Server { base_url, client } => {
                        let url = format!("{}/api/notes/generate_files", base_url);
                        let response = client
                            .post(url)
                            .json(&request)
                            .send()
                            .await
                            .map_err(Error::ApiRequest)?;
                        let _response = self.handle_response(response).await?;
                    }
                    SparesRequestProcessorInternal::Database { pool } => {
                        render_notes(pool, request, &get_all_parsers()).await?;
                    }
                }
            }
        }

        Ok(())
    }
}
