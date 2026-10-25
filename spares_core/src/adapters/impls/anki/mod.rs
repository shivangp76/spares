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
pub mod backend;
mod database;
pub use database::AnkiNote;
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

    /// An adapter that never asks whether Anki is open, for the server, which cannot prompt.
    /// Callers check that `AnkiConnect` answers instead (see [`backend::anki_connect_is_up`]).
    pub fn without_prompt() -> Self {
        Self {
            confirm_bypass: true,
        }
    }

    /// Correlates each executed Add request that carried a `SparesId` with its result, the
    /// newly assigned Anki note id.
    fn added_anki_ids(
        requests: &[ApiRequest],
        anki_results: &[Value],
        added_notes: Vec<(Option<NoteId>, CustomData)>,
    ) -> Vec<(NoteId, i64, CustomData)> {
        requests
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
                        Some((spares_id, anki_note_id, custom_data))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Records in the spares notes the Anki note ids they were just added as, through the server.
    async fn update_spares_with_anki_ids(
        &self,
        added: Vec<(NoteId, i64, CustomData)>,
        dry_run: bool,
    ) -> Result<(), Error> {
        if dry_run {
            return Ok(());
        }
        let spares_adapter = SparesAdapter::new(SparesRequestProcessor::Server);
        let new_key = format!("{}-{}", self.get_adapter_name(), NOTE_ID_KEY);
        for (spares_note_id, anki_note_id, mut custom_data) in added {
            custom_data.remove(NOTE_ID_KEY);
            custom_data.insert(new_key.clone(), Value::String(anki_note_id.to_string()));
            spares_adapter
                .update_custom_data(spares_note_id, custom_data, dry_run, Utc::now())
                .await?;
        }
        Ok(())
    }
}

/// What spares writes to an Anki note for a note: the fields of the `Basic` model and the tags.
///
/// Anki only gets the note's first card, split into front and back at its first cloze, with the
/// rest of the note's markup lost. So a note read back from Anki is compared with this projection
/// of the spares note, never with the note itself.
#[derive(Clone, Debug, PartialEq)]
pub struct AnkiFields {
    pub front: String,
    pub back: String,
    pub keywords: Option<String>,
    pub spares_id: Option<String>,
    pub spares_parser_name: String,
    pub tags: Vec<String>,
}

impl AnkiFields {
    /// The fields as Anki stores them: in the model's order, separated by U+001F.
    pub fn joined(&self) -> String {
        [
            self.front.as_str(),
            self.back.as_str(),
            self.keywords.as_deref().unwrap_or_default(),
            self.spares_id.as_deref().unwrap_or_default(),
            self.spares_parser_name.as_str(),
        ]
        .join("\u{1f}")
    }
}

/// The Anki fields of a note, as [`AnkiAdapter::process_data`] writes them. The spares id is
/// `settings.custom_data[NOTE_ID_KEY]`.
pub fn to_anki_fields(
    parser: &dyn Parseable,
    settings: &NoteSettings,
    note_data: &str,
) -> Result<AnkiFields, Error> {
    let parser_name = parser.get_parser_name();
    let is_latex = parser_name.contains("latex");
    // The note's cards should be validated before being passed in, so this should not error.
    let cards = get_cards(parser, None, note_data, false, true)?;
    // NOTE: Workaround: Only extract first card in Anki
    let (front, back) = if let Some(first_card) = cards.first() {
        // NOTE: Workaround: Add data after cloze to back
        let first_cloze_index = first_card
            .data
            .iter()
            .position(|p| matches!(*p, NotePart::ClozeStart(_)))
            .unwrap_or(cards.len());
        let front_data =
            AnkiAdapter::note_parts_to_data(&first_card.data[..first_cloze_index], parser);
        let front = to_anki_html(front_data.as_str(), is_latex);
        let back_data =
            AnkiAdapter::note_parts_to_data(&first_card.data[first_cloze_index..], parser);
        let back = to_anki_html(back_data.as_str(), is_latex);
        (front, back)
    } else {
        (note_data.to_string(), String::new())
    };
    // NOTE: Anki stores all tags sorted alphabetically, so for syncing notes between Spares and Anki, tags are inserted and updated alphabetically as well. This way when rendering notes for syncing, the diff is empty.
    let tags = settings.tags.clone();
    let keywords = if settings.keywords.is_empty() {
        None
    } else {
        Some(settings.keywords.join(", "))
    };
    let spares_id: Option<String> = settings
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
    Ok(AnkiFields {
        front,
        back,
        keywords,
        spares_id,
        spares_parser_name: parser_name.to_string(),
        tags,
    })
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
                .map(|note_response| -> Result<(NoteId, i64), String> {
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

    async fn process_data(
        &mut self,
        notes: Vec<(NoteSettings, Option<String>)>,
        parser: &dyn Parseable,
        dry_run: bool,
        quiet: bool,
        _at: DateTime<Utc>,
        _live_update_note_ids: Vec<NoteId>,
    ) -> Result<(), Error> {
        let added = self.apply_notes(notes, parser, dry_run, quiet).await?;
        // Update Spares with Anki note id if it was:
        // 1. Already added to Spares
        // 2. Just added to Anki
        self.update_spares_with_anki_ids(added, dry_run).await
    }
}

impl AnkiAdapter {
    /// Adds, updates and deletes Anki notes. Entries without note data are skipped. Returns the
    /// spares id, new Anki note id and custom data of each note added with a spares id, which the
    /// caller records in spares.
    #[expect(clippy::too_many_lines)]
    pub async fn apply_notes(
        &mut self,
        notes: Vec<(NoteSettings, Option<String>)>,
        parser: &dyn Parseable,
        dry_run: bool,
        quiet: bool,
    ) -> Result<Vec<(NoteId, i64, CustomData)>, Error> {
        if !dry_run {
            self.verify_anki_is_open()?;
        }
        let mut requests: Vec<ApiRequest> = Vec::new();
        let client = Client::new();
        let mut added_notes = Vec::new();
        // Suspended notes are added right away, to suspend their cards
        let mut added_right_away = Vec::new();
        for (local_settings, note_data_res) in notes {
            let Some(note_data) = note_data_res else {
                continue;
            };
            let AnkiFields {
                front,
                back,
                keywords,
                spares_id,
                spares_parser_name,
                tags,
            } = to_anki_fields(parser, &local_settings, &note_data)?;
            let spares_parser_name = Some(spares_parser_name);
            let mut final_note_id: Option<i64> = None;
            if spares_id.is_none() && !quiet {
                println!(
                    "WARNING: spares id is missing. If you are using spares, this will cause data to go out of sync."
                );
            }

            // See <https://git.foosoft.net/alex/anki-connect>
            match local_settings.action {
                NoteImportAction::Add => {
                    let spares_id_parsed = spares_id
                        .as_ref()
                        .and_then(|spares_id| spares_id.parse::<NoteId>().ok());
                    let params = ApiRequestParams::AddNote(AddNoteApiRequestData {
                        note: AddNoteApiRequestNoteData {
                            deck_name: "Default".to_owned(),
                            model_name: ModelName::Basic,
                            fields: NoteFields {
                                front: Some(front),
                                back: Some(back),
                                keywords,
                                spares_id: spares_id.clone(),
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
                        if let Some(spares_id) = spares_id_parsed {
                            added_right_away.push((
                                spares_id,
                                created_note_id,
                                local_settings.custom_data.clone(),
                            ));
                        }
                    } else {
                        if spares_id.is_some() {
                            added_notes.push((spares_id_parsed, local_settings.custom_data));
                        }
                        requests.push(api_request);
                    }
                }
                // For this adapter, the id is Anki's (see `NoteImportAction`).
                NoteImportAction::Update(note_id) => {
                    final_note_id = Some(note_id.packed());
                    // Workaround for issue that prevents request if note is open in browser
                    // see <https://github.com/FooSoft/anki-connect/issues/82#issuecomment-1221895385>
                    // The workaround causes the request to not go through since focus is lost.
                    // requests.push(AnkiAdapter::get_gui_browse_request("nid:1"));
                    let params = ApiRequestParams::UpdateNote(UpdateNoteApiRequestData {
                        note: UpdateNoteApiRequestNoteData {
                            deck_name: "Default".to_owned(),
                            model_name: ModelName::Basic,
                            id: note_id.packed(),
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
                        notes: vec![note_id.packed()],
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
        let mut added = AnkiAdapter::added_anki_ids(&requests, &anki_results, added_notes);
        added.extend(added_right_away);
        Ok(added)
    }
}
