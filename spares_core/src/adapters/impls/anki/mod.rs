use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

use super::spares::SparesAdapter;
use super::spares::SparesRequestProcessor;
use crate::AdapterErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::adapters::SrsAdapter;
use crate::adapters::impls::anki::api::create_field;
use crate::adapters::impls::anki::api::execute_request;
use crate::adapters::impls::anki::api::execute_requests;
use crate::adapters::impls::anki::database::populate_reviews;
use crate::adapters::impls::anki::types::AddNoteApiRequestData;
use crate::adapters::impls::anki::types::AddNoteApiRequestNoteData;
use crate::adapters::impls::anki::types::AddNoteApiRequestOptions;
use crate::adapters::impls::anki::types::ApiAction;
use crate::adapters::impls::anki::types::ApiRequest;
use crate::adapters::impls::anki::types::ApiRequestParams;
use crate::adapters::impls::anki::types::DeleteNoteApiRequestData;
use crate::adapters::impls::anki::types::FindCardsApiRequestData;
use crate::adapters::impls::anki::types::GetModelFieldNamesApiRequestData;
use crate::adapters::impls::anki::types::ModelName;
use crate::adapters::impls::anki::types::NoteFields;
use crate::adapters::impls::anki::types::SuspendApiRequestData;
use crate::adapters::impls::anki::types::UpdateNoteApiRequestData;
use crate::adapters::impls::anki::types::UpdateNoteApiRequestNoteData;
use crate::adapters::impls::anki::utils::get_note_id;
use crate::adapters::impls::anki::utils::note_action_to_anki;
use crate::adapters::impls::anki::utils::to_anki_html;
use crate::adapters::migration::MigrationFunc;
use crate::adapters::migration::create_notes;
use crate::model::CustomData;
use crate::model::NOTE_ID_KEY;
use crate::model::NoteId;
use crate::parsers::NoteImportAction;
use crate::parsers::NotePart;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;
use crate::parsers::get_cards;

mod api;
mod database;
mod types;
mod utils;

const SPARES_KEYWORDS_FIELD_NAME: &str = "KEYWORDS";
const SPARES_ID_FIELD_NAME: &str = "SparesId";
const SPARES_PARSER_NAME_FIELD_NAME: &str = "SparesParserName";
const ANKI_ADAPTER_NAME: &str = "anki";

#[derive(Debug, Default)]
pub struct AnkiAdapter {
    confirm_bypass: bool,
}

// impl Default for AnkiAdapter {
//     fn default() -> Self {
//         Self::new()
//     }
// }

impl AnkiAdapter {
    pub fn new() -> Self {
        Self {
            confirm_bypass: false,
        }
    }

    /// After non-suspended Add requests have been executed, correlates each result with its
    /// `spares_id` and updates the corresponding Spares note's `custom_data` to record the
    /// newly assigned Anki note ID.
    async fn update_spares_with_anki_ids(
        &self,
        requests: &[ApiRequest],
        anki_results: &[Value],
        added_notes: Vec<(Option<NoteId>, CustomData)>,
        dry_run: bool,
    ) -> Result<(), Error> {
        let relevant_data: Vec<(String, NoteId, CustomData)> = requests
            .iter()
            .zip(anki_results.iter())
            .filter(|(request, _)| matches!(request.action, ApiAction::AddNote))
            .filter(|(request, _)| match &request.params {
                ApiRequestParams::AddNote(AddNoteApiRequestData { note }) => {
                    note.fields.spares_id.is_some()
                }
                _ => unreachable!(),
            })
            .map(|(_request, result)| serde_json::from_value::<i64>(result.clone()))
            .zip(added_notes)
            .filter_map(|(anki_note_id, (spares_id, custom_data))| {
                match (anki_note_id, spares_id) {
                    (Ok(anki_note_id), Some(spares_id)) => {
                        Some((anki_note_id.to_string(), spares_id, custom_data))
                    }
                    _ => None,
                }
            })
            .collect();
        if !dry_run {
            let spares_adapter = SparesAdapter::new(SparesRequestProcessor::Server);
            let new_key = format!("{}-{}", self.get_adapter_name(), NOTE_ID_KEY);
            for (anki_note_id, spares_note_id, mut custom_data) in relevant_data {
                custom_data.remove(NOTE_ID_KEY);
                custom_data.insert(new_key.clone(), Value::String(anki_note_id));
                spares_adapter
                    .update_custom_data(spares_note_id, custom_data, dry_run, Utc::now())
                    .await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SrsAdapter for AnkiAdapter {
    fn get_adapter_name(&self) -> &'static str {
        ANKI_ADAPTER_NAME
    }

    async fn migrate(
        &mut self,
        base_url: &str,
        spares_pool: &SqlitePool,
        migration_function: Option<MigrationFunc>,
        initial_migration: bool,
        dry_run: bool,
    ) -> Result<(), Error> {
        let client = Client::new();

        // Update Anki model's fields, if needed
        if initial_migration {
            if !dry_run {
                self.verify_anki_is_open()?;
            }
            let params = ApiRequestParams::GetModelFieldNames(GetModelFieldNamesApiRequestData {
                model_name: ModelName::Basic,
            });
            let api_request = ApiRequest {
                action: ApiAction::GetModelFieldNames,
                params,
                version: 6,
            };
            let model_field_names_value = execute_request(&api_request, &client).await?;
            let model_field_names: Vec<String> =
                serde_json::from_value(model_field_names_value.clone()).map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: ANKI_ADAPTER_NAME.to_string(),
                        error: e.to_string(),
                    }))
                })?;
            if !model_field_names.contains(&SPARES_KEYWORDS_FIELD_NAME.to_string()) {
                create_field(SPARES_KEYWORDS_FIELD_NAME, &client).await?;
            }
            if !model_field_names.contains(&SPARES_ID_FIELD_NAME.to_string()) {
                create_field(SPARES_ID_FIELD_NAME, &client).await?;
            }
            if !model_field_names.contains(&SPARES_PARSER_NAME_FIELD_NAME.to_string()) {
                create_field(SPARES_PARSER_NAME_FIELD_NAME, &client).await?;
            }
        }

        let anki_db_path = std::env::var("ANKI_DB_PATH").map_err(|_| {
            Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: "ANKI_DB_PATH environment variable is not set.".to_string(),
            }))
        })?;
        let anki_db_path = PathBuf::from(anki_db_path);
        let parse_note_requests =
            AnkiAdapter::database_to_requests(anki_db_path.as_path(), migration_function).await?;
        let row_count = parse_note_requests.len();
        println!("Row count: {}", row_count);
        let notes_responses = create_notes(&client, base_url, parse_note_requests, dry_run)
            .await
            .map_err(|e| {
                Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: e.clone(),
                }))
            })?;

        // Add Anki's reviews
        if initial_migration {
            let spares_and_anki_note_ids = notes_responses
                .iter()
                .flat_map(|x| &x.notes)
                .map(|note_response| -> Result<(i64, i64), String> {
                    let anki_note_id = get_note_id(note_response).map_err(|e| format!("{}", e))?;
                    Ok((note_response.id, anki_note_id))
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: ANKI_ADAPTER_NAME.to_string(),
                        error: e.clone(),
                    }))
                })?;
            println!("Modifying cards and review log...");
            let start = Instant::now();
            populate_reviews(
                dry_run,
                spares_and_anki_note_ids,
                spares_pool,
                &anki_db_path,
            )
            .await?;
            let duration = start.elapsed();
            println!("Add Anki's review log duration: {:?}", duration);
        }

        // This is deterministic, so the ids should always be the same. This means it can be run once and only needs to be rerun if:
        // - a new note is added without a SparesId in Anki
        // - if notes are added/deleted in Anki
        if initial_migration {
            println!("Populating SparesId in Anki...");
            let start = Instant::now();
            let mut adapter = AnkiAdapter::default();
            adapter
                .add_spares_id(&notes_responses, &client, dry_run)
                .await?;
            let duration = start.elapsed();
            println!("Add SparesId to Anki duration: {:?}", duration);
        }

        Ok(())
    }

    #[expect(clippy::too_many_lines)]
    async fn process_data(
        &mut self,
        notes: Vec<(NoteSettings, Option<String>)>,
        parser: &dyn Parseable,
        dry_run: bool,
        quiet: bool,
        _at: DateTime<Utc>,
        _live_update_note_ids: Vec<NoteId>,
    ) -> Result<(), Error> {
        if !dry_run {
            self.verify_anki_is_open()?;
        }
        let mut requests: Vec<ApiRequest> = Vec::new();
        let parser_name = parser.get_parser_name();
        let is_latex = parser_name.contains("latex");
        let client = Client::new();
        let mut added_notes = Vec::new();
        for (local_settings, note_data_res) in notes {
            if note_data_res.is_none() {
                continue;
            }
            let note_data = note_data_res.unwrap();
            // The note's cards should be validated before being passed in, so this should not error.
            let cards = get_cards(parser, None, note_data.as_str(), false, true)?;
            // NOTE: Workaround: Only extract first card in Anki
            let (front, back) = if let Some(first_card) = cards.first() {
                // NOTE: Workaround: Add data after cloze to back
                let first_cloze_index = first_card
                    .data
                    .iter()
                    .position(|p| matches!(*p, NotePart::ClozeStart(_)))
                    .unwrap_or(cards.len());
                let front_data = AnkiAdapter::note_parts_to_data(
                    &first_card.data[..first_cloze_index],
                    // &card.grouping,
                    parser,
                );
                let front = to_anki_html(front_data.as_str(), is_latex);
                let back_data = AnkiAdapter::note_parts_to_data(
                    &first_card.data[first_cloze_index..],
                    // &card.grouping,
                    parser,
                );
                let back = to_anki_html(back_data.as_str(), is_latex);
                (front, back)
            } else {
                (note_data, String::new())
            };
            let mut final_note_id: Option<i64> = None;
            // NOTE: Anki stores all tags sorted alphabetically, so for syncing notes between Spares and Anki, tags are inserted and updated alphabetically as well. This way when rendering notes for syncing, the diff is empty.
            let tags = local_settings.tags.clone();
            let keywords = if local_settings.keywords.is_empty() {
                None
            } else {
                Some(local_settings.keywords.join(", "))
            };
            let spares_parser_name = Some(parser_name.to_string());
            let spares_id: Option<String> = local_settings
                .custom_data
                .get(NOTE_ID_KEY)
                .map(|x: &Value| {
                    serde_json::from_value(x.clone()).map_err(|_| {
                        Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                            adapter_name: ANKI_ADAPTER_NAME.to_string(),
                            error: "Failed to parse spares note id".to_string(),
                        }))
                    })
                })
                .transpose()?;
            if spares_id.is_none() {
                println!(
                    "WARNING: spares id is missing. If you are using spares, this will cause data to go out of sync."
                );
            }

            // See <https://git.foosoft.net/alex/anki-connect>
            match local_settings.action {
                NoteImportAction::Add => {
                    if let Some(ref spares_id) = spares_id {
                        let spares_id_parsed = spares_id.parse::<NoteId>().ok();
                        added_notes.push((spares_id_parsed, local_settings.custom_data));
                    }
                    let params = ApiRequestParams::AddNote(AddNoteApiRequestData {
                        note: AddNoteApiRequestNoteData {
                            deck_name: "Default".to_owned(),
                            model_name: ModelName::Basic,
                            fields: NoteFields {
                                front: Some(front),
                                back: Some(back),
                                keywords,
                                spares_id,
                                spares_parser_name,
                            },
                            tags,
                            options: AddNoteApiRequestOptions {
                                allow_duplicate: true,
                            },
                        },
                    });
                    let api_request = ApiRequest {
                        action: note_action_to_anki(local_settings.action),
                        params,
                        version: 6,
                    };

                    if local_settings.is_suspended {
                        let created_note_id = execute_request(&api_request, &client)
                            .await?
                            .as_i64()
                            .ok_or(Error::Library(LibraryError::Adapter(
                                AdapterErrorKind::Custom {
                                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                                    error: "Failed to get note id".to_string(),
                                },
                            )))?;
                        final_note_id = Some(created_note_id);
                    } else {
                        requests.push(api_request);
                    }
                }
                NoteImportAction::Update(note_id) => {
                    final_note_id = Some(note_id);
                    // Workaround for issue that prevents request if note is open in browser
                    // see <https://github.com/FooSoft/anki-connect/issues/82#issuecomment-1221895385>
                    // The workaround causes the request to not go through since focus is lost.
                    // requests.push(AnkiAdapter::get_gui_browse_request("nid:1"));
                    let params = ApiRequestParams::UpdateNote(UpdateNoteApiRequestData {
                        note: UpdateNoteApiRequestNoteData {
                            deck_name: "Default".to_owned(),
                            model_name: ModelName::Basic,
                            id: note_id,
                            fields: NoteFields {
                                front: Some(front),
                                back: Some(back),
                                keywords,
                                spares_id,
                                spares_parser_name,
                            },
                            tags: Some(tags),
                        },
                    });
                    let api_request = ApiRequest {
                        action: note_action_to_anki(local_settings.action),
                        params,
                        version: 6,
                    };
                    requests.push(api_request);
                    // let note_id_query = format!("nid:{}", local_settings.note_id.unwrap());
                    // requests.push(AnkiAdapter::get_gui_browse_request(note_id_query.as_str()));
                }
                NoteImportAction::Delete(note_id) => {
                    let params = ApiRequestParams::DeleteNote(DeleteNoteApiRequestData {
                        notes: vec![note_id],
                    });
                    let api_request = ApiRequest {
                        action: note_action_to_anki(local_settings.action),
                        params,
                        version: 6,
                    };
                    requests.push(api_request);
                }
            }

            if !matches!(local_settings.action, NoteImportAction::Delete(_))
                && local_settings.is_suspended
            {
                let query = format!("nid:{}", final_note_id.unwrap());
                let api_request = ApiRequest {
                    action: ApiAction::FindCards,
                    params: ApiRequestParams::FindCards(FindCardsApiRequestData { query }),
                    version: 6,
                };
                let cards_result_res = execute_request(&api_request, &client).await;
                if let Ok(cards_result) = cards_result_res {
                    let card_ids_res =
                        cards_result
                            .as_array()
                            .ok_or(Error::Library(LibraryError::Adapter(
                                AdapterErrorKind::Custom {
                                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                                    error: "Failed to get card ids".to_string(),
                                },
                            )))?;
                    let cards = card_ids_res
                        .iter()
                        .map(|c| {
                            c.as_i64().ok_or(Error::Library(LibraryError::Adapter(
                                AdapterErrorKind::Custom {
                                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                                    error: "Failed to get card id as i64".to_string(),
                                },
                            )))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let api_request = ApiRequest {
                        action: ApiAction::Suspend,
                        params: ApiRequestParams::Suspend(SuspendApiRequestData { cards }),
                        version: 6,
                    };
                    requests.push(api_request);
                }
            }
        }

        let anki_results = execute_requests(&requests, dry_run, quiet, &client).await?;

        // Update Spares with Anki note id if it was:
        // 1. Already added to Spares
        // 2. Just added to Anki
        self.update_spares_with_anki_ids(&requests, &anki_results, added_notes, dry_run)
            .await?;

        Ok(())
    }
}
