use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use log::warn;
use reqwest::Client;
use reqwest::RequestBuilder;
use reqwest::Response;
use reqwest::StatusCode;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::AdapterErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::ParserErrorKind;
use crate::adapters::SrsAdapter;
use crate::adapters::migration::MigrationFunc;
use crate::api::note::create_notes;
use crate::api::note::delete_notes;
use crate::api::note::relocate_live_cards;
use crate::api::note::render_notes;
use crate::api::note::update_notes;
use crate::api::parser::list_parsers;
use crate::api::undo::group_events;
use crate::config::Environment;
use crate::config::get_env_config;
use crate::config::is_test_mode;
use crate::config::local_api_key;
use crate::config::read_external_config;
use crate::model::CustomData;
use crate::model::NoteId;
use crate::parsers::NoteImportAction;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;
use crate::parsers::add_order_to_note_data;
use crate::parsers::get_all_parsers;
use crate::schema::FilterOptions;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::DeleteNotesResponse;
use crate::schema::note::FindLiveNoteRequest;
use crate::schema::note::FindLiveNoteResponse;
use crate::schema::note::LiveNoteClozes;
use crate::schema::note::NoteResponse;
use crate::schema::note::NotesResponse;
use crate::schema::note::NotesSelector;
use crate::schema::note::RelocateLiveCardsRequest;
use crate::schema::note::RelocateLiveCardsResponse;
use crate::schema::note::RenderNotesRequest;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateNotesResponse;
use crate::schema::note::UpdateTags;
use crate::schema::parser::ParserResponse;
use crate::schema::undo::GroupEventsRequest;

/// A note of an import: one it updates, or the one it creates for that index of its create
/// requests.
enum ImportedNote {
    Updated(NoteId),
    Created(usize),
}

#[derive(Debug)]
pub struct SparesAdapter {
    request_processor: SparesRequestProcessorInternal,
    /// The first event this adapter's imports logged, whose group the later ones join, so one
    /// undo of the group reverts the whole import
    import_group: Mutex<Option<i64>>,
}

#[derive(Debug)]
pub enum SparesRequestProcessor {
    Server,
    /// For testing use only. Logs undo events, as the server's handlers do.
    Database {
        pool: SqlitePool,
    },
}

/// Sends the local server's API key, which it requires when it has one.
fn authed(request: RequestBuilder, api_key: Option<&str>) -> RequestBuilder {
    match api_key {
        Some(key) => request.bearer_auth(key),
        None => request,
    }
}

#[derive(Debug)]
enum SparesRequestProcessorInternal {
    Server {
        base_url: String,
        client: Client,
        api_key: Option<String>,
    },
    Database {
        pool: SqlitePool,
    },
}

impl SparesAdapter {
    pub fn new(request_processor: SparesRequestProcessor) -> Self {
        let internal = match request_processor {
            SparesRequestProcessor::Server => {
                let env_config = get_env_config(Environment::Production);
                let base_url = format!("http://{}", env_config.socket_address);
                SparesRequestProcessorInternal::Server {
                    base_url,
                    client: Client::new(),
                    api_key: local_api_key(),
                }
            }
            SparesRequestProcessor::Database { pool } => {
                SparesRequestProcessorInternal::Database { pool }
            }
        };
        Self {
            request_processor: internal,
            import_group: Mutex::new(None),
        }
    }

    /// Puts `event_ids`, the events of part of an import, in the group of the import's first
    /// event. A failure is only logged: the import itself succeeded, and is only harder to undo.
    async fn group_import_events(&self, event_ids: Vec<i64>) {
        let leader = *self.import_group.lock().unwrap();
        let mut event_ids: Vec<i64> = leader.into_iter().chain(event_ids).collect();
        event_ids.sort_unstable();
        event_ids.dedup();
        let Some(&first) = event_ids.first() else {
            return;
        };
        *self.import_group.lock().unwrap() = Some(first);
        if event_ids.len() < 2 {
            return;
        }
        let request = GroupEventsRequest { event_ids };
        let result = match &self.request_processor {
            SparesRequestProcessorInternal::Server {
                base_url,
                client,
                api_key,
            } => {
                let url = format!("{}/api/undo/group", base_url);
                match authed(client.post(url), api_key.as_deref())
                    .json(&request)
                    .send()
                    .await
                {
                    Ok(response) => self.handle_response(response).await.map(|_| ()),
                    Err(e) => Err(Error::ApiRequest(e)),
                }
            }
            SparesRequestProcessorInternal::Database { pool } => group_events(pool, request).await,
        };
        if let Err(e) = result {
            warn!("Could not group the import's undo events, so it is undone in parts: {e}");
        }
    }

    async fn handle_response(&self, response: Response) -> Result<Response, Error> {
        let status = response.status();
        if status != StatusCode::OK {
            let response_json: Value = response.json().await.map_err(Error::ApiRequest)?;
            let message = response_json.get("message");
            warn!("{:?}", message);
            return Err(Error::Library(LibraryError::Adapter(
                AdapterErrorKind::Custom {
                    adapter_name: self.get_adapter_name().to_string(),
                    error: response_json.to_string(),
                },
            )));
        }
        Ok(response)
    }

    /// Returns the event to undo the update with, if any
    async fn update_note(
        &self,
        update_note_request: UpdateNotesRequest,
        at: DateTime<Utc>,
    ) -> Result<Option<i64>, Error> {
        let response: UpdateNotesResponse = match &self.request_processor {
            SparesRequestProcessorInternal::Server {
                base_url,
                client,
                api_key,
            } => {
                let url = format!("{}/api/notes", base_url);
                let response = authed(client.patch(url), api_key.as_deref())
                    .json(&update_note_request)
                    .send()
                    .await
                    .map_err(Error::ApiRequest)?;
                let response = self.handle_response(response).await?;
                response.json().await.map_err(Error::ApiRequest)?
            }
            SparesRequestProcessorInternal::Database { pool } => {
                update_notes(pool, update_note_request, at, &get_all_parsers(), true).await?
            }
        };
        Ok(response.event_id)
    }

    async fn relocate_live_cards(
        &self,
        request: RelocateLiveCardsRequest,
        at: DateTime<Utc>,
    ) -> Result<RelocateLiveCardsResponse, Error> {
        match &self.request_processor {
            SparesRequestProcessorInternal::Server {
                base_url,
                client,
                api_key,
            } => {
                let url = format!("{}/api/notes/live/relocate-cards", base_url);
                let response = authed(client.post(url), api_key.as_deref())
                    .json(&request)
                    .send()
                    .await
                    .map_err(Error::ApiRequest)?;
                let response = self.handle_response(response).await?;
                response.json().await.map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: self.get_adapter_name().to_string(),
                        error: e.to_string(),
                    }))
                })
            }
            SparesRequestProcessorInternal::Database { pool } => {
                relocate_live_cards(pool, request, at, true).await
            }
        }
    }

    pub async fn update_custom_data(
        &self,
        note_id: NoteId,
        custom_data: CustomData,
        dry_run: bool,
        at: DateTime<Utc>,
    ) -> Result<(), Error> {
        let update_note_request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            custom_data: Some(custom_data),
            parser_id: None,
            data: None,
            keywords: None,
            tags: UpdateTags::None,
        };
        if !dry_run {
            self.update_note(update_note_request, at).await?;
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
        live_update_note_ids: Vec<NoteId>,
    ) -> Result<(), Error> {
        // Get parser id
        let parser_responses: Vec<ParserResponse> = match &self.request_processor {
            SparesRequestProcessorInternal::Server {
                base_url,
                client,
                api_key,
            } => {
                let url = format!("{}/api/parsers", base_url);
                let mut response = authed(client.get(url), api_key.as_deref())
                    .send()
                    .await
                    .map_err(Error::ApiRequest)?;
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
        let mut update_note_requests: Vec<UpdateNotesRequest> = Vec::new();
        let mut delete_note_ids: Vec<NoteId> = Vec::new();
        // The live notes by live-sync-name, with their new data
        let mut live_notes: BTreeMap<String, Vec<(ImportedNote, String)>> = BTreeMap::new();
        for (local_settings, note_data_res) in notes {
            if note_data_res.is_none() {
                continue;
            }
            let note_data = note_data_res.unwrap();
            let live_sync_name = local_settings
                .custom_data
                .get("live_sync_name")
                .and_then(Value::as_str)
                .map(str::to_string);
            match local_settings.action {
                NoteImportAction::Add => {
                    if let Some(live_sync_name) = live_sync_name {
                        live_notes.entry(live_sync_name).or_default().push((
                            ImportedNote::Created(create_note_requests.len()),
                            note_data.clone(),
                        ));
                    }
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
                    if let Some(live_sync_name) = live_sync_name {
                        live_notes
                            .entry(live_sync_name)
                            .or_default()
                            .push((ImportedNote::Updated(note_id), note_data.clone()));
                    }
                    let update_note_request = UpdateNotesRequest {
                        selector: NotesSelector::Ids(vec![note_id]),
                        parser_id: Some(parser_id),
                        data: Some(note_data),
                        keywords: Some(local_settings.keywords),
                        tags: UpdateTags::SetTags(local_settings.tags),
                        custom_data: Some(local_settings.custom_data),
                    };
                    update_note_requests.push(update_note_request);
                }
                NoteImportAction::Delete(note_id) => {
                    delete_note_ids.push(note_id);
                }
            }
        }

        if dry_run {
            return Ok(());
        }

        // Notes are created first, so that every note a live note's cloze can move to exists when
        // the cards are relocated, before the updates.
        let create_notes_request = CreateNotesRequest {
            parser_id,
            requests: create_note_requests,
        };
        let mut event_ids = Vec::new();
        let created_note_ids = match &self.request_processor {
            SparesRequestProcessorInternal::Server {
                base_url,
                client,
                api_key,
            } => {
                let url = format!("{}/api/notes", base_url);
                let response = authed(client.post(url), api_key.as_deref())
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
                event_ids.extend(response.event_id);
                response.notes.into_iter().map(|n| n.id).collect::<Vec<_>>()
            }
            SparesRequestProcessorInternal::Database { pool } => {
                let notes_res =
                    create_notes(pool, create_notes_request, at, &get_all_parsers(), true).await?;
                event_ids.extend(notes_res.event_id);
                notes_res
                    .notes
                    .into_iter()
                    .map(|n| n.id)
                    .collect::<Vec<_>>()
            }
        };

        let overlapper = read_external_config().ok().map(|c| c.overlapper);
        for (live_sync_name, notes) in live_notes {
            let notes = notes
                .into_iter()
                .map(|(note, data)| {
                    let (note_id, created) = match note {
                        ImportedNote::Updated(note_id) => (note_id, false),
                        ImportedNote::Created(i) => (created_note_ids[i], true),
                    };
                    let (_, cards) = add_order_to_note_data(parser, &data, overlapper.as_ref())?;
                    Ok(LiveNoteClozes {
                        note_id,
                        cloze_uids: cards
                            .iter()
                            .filter_map(|c| c.cloze_uid.map(|uid| uid.to_string()))
                            .collect(),
                        created,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let request = RelocateLiveCardsRequest {
                live_sync_name,
                notes,
            };
            event_ids.extend(self.relocate_live_cards(request, at).await?.event_id);
        }

        for update_note_request in update_note_requests {
            event_ids.extend(self.update_note(update_note_request, at).await?);
        }

        // Issue a single bulk delete for all collected note IDs.
        if !delete_note_ids.is_empty() {
            let request = DeleteNotesRequest {
                selector: NotesSelector::Ids(delete_note_ids),
            };
            match &self.request_processor {
                SparesRequestProcessorInternal::Server {
                    base_url,
                    client,
                    api_key,
                } => {
                    let url = format!("{}/api/notes", base_url);
                    let response = authed(client.delete(url), api_key.as_deref())
                        .json(&request)
                        .send()
                        .await
                        .map_err(Error::ApiRequest)?;
                    let response = self.handle_response(response).await?;
                    // A server from before deletes returned their event responds with no body
                    let response: Option<DeleteNotesResponse> = response.json().await.ok();
                    event_ids.extend(response.and_then(|r| r.event_id));
                }
                SparesRequestProcessorInternal::Database { pool } => {
                    let response = delete_notes(pool, request, &get_all_parsers(), true).await?;
                    event_ids.extend(response.event_id);
                }
            }
        }
        self.group_import_events(event_ids).await;

        // Render created notes + live-updated notes
        // We cannot render non-live updated notes since that might overwrite unsaved changes.
        // If testing, then we
        // - do not want to render notes to save time. We can explicitly call render notes if we need to.
        // - want to render notes in a different directory, so we don't overwrite user data.
        if !is_test_mode() {
            let mut all_render_ids = created_note_ids;
            all_render_ids.extend(live_update_note_ids);
            if !all_render_ids.is_empty() {
                let request = RenderNotesRequest {
                    selector: NotesSelector::Ids(all_render_ids),
                    immutable_note_ids: None,
                    overridden_output_raw_dir: None,
                    include_linked_notes: true,
                    include_cards: true,
                    generate_rendered: true,
                    force_generate_rendered: false,
                };
                match &self.request_processor {
                    SparesRequestProcessorInternal::Server {
                        base_url,
                        client,
                        api_key,
                    } => {
                        let url = format!("{}/api/notes/generate_files", base_url);
                        let response = authed(client.post(url), api_key.as_deref())
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

    async fn find_live_note_by_block_order(
        &self,
        live_sync_name: &str,
        block_order: i64,
    ) -> Result<Option<NoteId>, Error> {
        match &self.request_processor {
            SparesRequestProcessorInternal::Server {
                base_url,
                client,
                api_key,
            } => {
                let url = format!("{}/api/notes/live", base_url);
                let body = FindLiveNoteRequest {
                    live_sync_name: live_sync_name.to_string(),
                    block_order,
                };
                let response = authed(client.post(url), api_key.as_deref())
                    .json(&body)
                    .send()
                    .await
                    .map_err(Error::ApiRequest)?;
                let response = self.handle_response(response).await?;
                let response: FindLiveNoteResponse = response.json().await.map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: self.get_adapter_name().to_string(),
                        error: e.to_string(),
                    }))
                })?;
                Ok(response.id)
            }
            SparesRequestProcessorInternal::Database { pool } => {
                let note_id: Option<NoteId> = sqlx::query_scalar(
                    r"SELECT id FROM note WHERE json_extract(custom_data, '$.live_sync_name') = ? AND json_extract(custom_data, '$.live_block_order') = ?",
                )
                .bind(live_sync_name)
                .bind(block_order)
                .fetch_optional(pool)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
                Ok(note_id)
            }
        }
    }

    async fn get_live_sync_name(&self, note_id: NoteId) -> Result<Option<String>, Error> {
        match &self.request_processor {
            SparesRequestProcessorInternal::Server {
                base_url,
                client,
                api_key,
            } => {
                let url = format!("{}/api/notes/{}", base_url, note_id);
                let response = authed(client.get(url), api_key.as_deref())
                    .send()
                    .await
                    .map_err(Error::ApiRequest)?;
                let response = self.handle_response(response).await?;
                let response: NoteResponse = response.json().await.map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: self.get_adapter_name().to_string(),
                        error: e.to_string(),
                    }))
                })?;
                Ok(response
                    .custom_data
                    .get("live_sync_name")
                    .and_then(Value::as_str)
                    .map(str::to_string))
            }
            SparesRequestProcessorInternal::Database { pool } => {
                let live_sync_name: Option<Option<String>> = sqlx::query_scalar(
                    r"SELECT json_extract(custom_data, '$.live_sync_name') FROM note WHERE id = ?",
                )
                .bind(note_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
                Ok(live_sync_name.flatten())
            }
        }
    }
}
