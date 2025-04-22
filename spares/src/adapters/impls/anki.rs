use super::spares::{SparesAdapter, SparesRequestProcessor};
use crate::adapters::SrsAdapter;
use crate::adapters::migration::{MigrationData, MigrationFunc, create_notes};
use crate::api::card::update_card;
use crate::api::review::submit_study_action;
use crate::config::get_data_dir;
use crate::helpers::parse_list;
use crate::model::{Card, CustomData, DEFAULT_DESIRED_RETENTION, NOTE_ID_KEY, NoteId, RatingId};
use crate::parsers::{
    NoteImportAction, NotePart, NoteSettings, Parseable, generate_files::GenerateNoteFilesRequest,
    get_adapter_note_id_key, get_cards, image_occlusion::ConstructImageOcclusionType,
};
use crate::schema::card::{CardsSelector, UpdateCardRequest};
use crate::schema::note::{NoteResponse, NotesResponse};
use crate::schema::review::{RatingSubmission, StudyAction, SubmitStudyActionRequest};
use crate::{AdapterErrorKind, Error, LibraryError};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use indicatif::ProgressIterator;
use inquire::Select;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{FromRow, SqlitePool};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SPARES_KEYWORDS_FIELD_NAME: &str = "KEYWORDS";
const SPARES_ID_FIELD_NAME: &str = "SparesId";
const SPARES_PARSER_NAME_FIELD_NAME: &str = "SparesParserName";

#[derive(Debug)]
pub struct AnkiAdapter {
    confirm_bypass: bool,
}

impl Default for AnkiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AnkiAdapter {
    pub fn new() -> Self {
        Self {
            confirm_bypass: false,
        }
    }

    fn note_action_to_anki(note_action: NoteImportAction) -> ApiAction {
        match note_action {
            NoteImportAction::Add => ApiAction::AddNote,
            NoteImportAction::Update(_) => ApiAction::UpdateNote,
            NoteImportAction::Delete(_) => ApiAction::DeleteNote,
        }
    }

    // fn get_gui_browse_request(query: &str) -> ApiRequest {
    //     ApiRequest {
    //         action: ApiAction::GuiBrowse,
    //         params: ApiRequestParams::GuiBrowse(GuiBrowseApiRequestData {
    //             query: query.to_owned(),
    //         }),
    //         version: 6,
    //     }
    // }

    fn verify_anki_is_open(&mut self) -> Result<(), Error> {
        if self.confirm_bypass {
            return Ok(());
        }
        let abort_str = "Abort";
        let short_confirm_str = "Confirm";
        let long_confirm_str = "Confirm all";
        let options = vec![abort_str, short_confirm_str, long_confirm_str];
        let ans = Select::new("Please confirm that Anki is open.", options)
            .with_help_message("This is needed to import data into Anki.")
            .prompt();
        let abort = match ans {
            Ok(choice) => {
                if choice == abort_str {
                    true
                } else if choice == short_confirm_str {
                    false
                } else if choice == long_confirm_str {
                    self.confirm_bypass = true;
                    false
                } else {
                    unreachable!()
                }
            }
            Err(_) => true,
        };
        if abort {
            return Err(Error::Library(LibraryError::Adapter(
                AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: "Aborting since Anki is not open.".to_string(),
                },
            )));
        }
        Ok(())
    }

    fn to_anki_html(data: &str, is_latex: bool) -> String {
        let mut new_data = String::new();
        if is_latex {
            new_data.push_str("[latex]<br/>");
        }
        new_data.push_str(data);
        if is_latex {
            new_data.push_str("<br/>[/latex]");
        }
        new_data = new_data.replace('\n', "<br/>");
        new_data
    }

    pub fn note_parts_to_data(data: &[NotePart], parser: &dyn Parseable) -> String {
        data.iter()
            .map(|p| match p {
                NotePart::SurroundingData(text)
                | NotePart::ClozeData(text, _)
                | NotePart::ClozeStart(text)
                | NotePart::ClozeEnd(text) => text.to_string(),
                NotePart::ImageOcclusion { data, .. } => {
                    parser.construct_image_occlusion(data, ConstructImageOcclusionType::Note)
                }
            })
            .collect::<String>()
    }

    async fn create_field(field_name: &str, client: &Client) -> Result<(), Error> {
        let params = ApiRequestParams::AddFieldToModel(AddFieldToModelApiRequestData {
            model_name: ModelName::Basic,
            field_name: field_name.to_string(),
            index: None,
        });
        let api_request = ApiRequest {
            action: ApiAction::GetModelFieldNames,
            params,
            version: 6,
        };
        let _response = AnkiAdapter::execute_request(&api_request, client).await?;
        Ok(())
    }

    async fn execute_request(request: &ApiRequest, client: &Client) -> Result<Value, Error> {
        let api_url = "http://localhost:8765";
        let body = serde_json::to_string_pretty(&request).map_err(|e| {
            Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: e.to_string(),
            }))
        })?;
        // println!("{}", serde_json::to_string_pretty(&request).unwrap());
        let response = client.post(api_url).body(body).send().await.map_err(|e| {
            Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: format!("Failed to send the API request: {}", e),
            }))
        })?;
        if response.status().is_success() {
            let response_value = response.json::<Value>().await.map_err(|e| {
                Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: format!("Failed to get response body: {}", e),
                }))
            })?;
            // <https://git.foosoft.net/alex/anki-connect#sample-invocation>
            let response_result = response_value.get("result");
            if let Some(response) = response_result {
                return Ok(response.clone());
            }
            let response_error =
                response_value
                    .get("error")
                    .ok_or(Error::Library(LibraryError::Adapter(
                        AdapterErrorKind::Custom {
                            adapter_name: ANKI_ADAPTER_NAME.to_string(),
                            error: "Failed to get 'result'".to_string(),
                        },
                    )))?;
            Err(Error::Library(LibraryError::Adapter(
                AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: format!(
                        "Failed to get 'result'. Got an 'error' of: {}",
                        response_error
                    ),
                },
            )))
        } else {
            Err(Error::Library(LibraryError::Adapter(
                AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: format!("Request failed with status code: {}", response.status()),
                },
            )))
        }
    }

    async fn execute_requests(
        requests: &[ApiRequest],
        run: bool,
        quiet: bool,
        client: &Client,
    ) -> Result<Vec<Value>, Error> {
        let mut results = Vec::new();
        for (i, request) in requests.iter().enumerate().progress() {
            if run {
                let result = AnkiAdapter::execute_request(request, client).await?;
                if !quiet {
                    println!("{}: {}", i, result);
                }
                results.push(result);
            }
        }
        Ok(results)
    }

    fn format_side(data: &str) -> String {
        let mut data = data.to_string();

        // Latex prefix/suffix
        let latex_start = "[latex]\n";
        if data.starts_with(latex_start) {
            data = data[latex_start.len()..].to_string();
        }
        let latex_end = "\n[/latex]";
        if data.ends_with(latex_end) {
            data = data[..data.len() - latex_end.len()].to_string();
        }
        data
    }

    fn get_note_id(note_response: &NoteResponse) -> Result<i64, Error> {
        let anki_note_id_str = note_response
            .custom_data
            .iter()
            .find(|(k, _v)| **k == get_adapter_note_id_key(AnkiAdapter::new().get_adapter_name()))
            .ok_or(Error::Library(LibraryError::Adapter(
                AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: "Failed to get anki note id custom field".to_string(),
                },
            )))?
            .1
            .as_str()
            .unwrap();
        anki_note_id_str.trim().parse::<i64>().map_err(|e| {
            Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: e.to_string(),
            }))
        })
    }

    async fn add_spares_id(
        &mut self,
        notes_responses: &[NotesResponse],
        client: &Client,
        run: bool,
    ) -> Result<(), Error> {
        let mut requests = Vec::new();
        for note_response in notes_responses {
            for note in &note_response.notes {
                let anki_note_id = AnkiAdapter::get_note_id(note).map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: ANKI_ADAPTER_NAME.to_string(),
                        error: e.to_string(),
                    }))
                })?;
                let note_data = UpdateNoteApiRequestNoteData {
                    deck_name: "Default".to_string(),
                    model_name: ModelName::Basic,
                    id: anki_note_id,
                    fields: NoteFields {
                        front: None,
                        back: None,
                        keywords: None,
                        spares_id: Some(note.id.to_string()),
                        spares_parser_name: None,
                    },
                    tags: None,
                };

                let api_request = ApiRequest {
                    action: ApiAction::UpdateNote,
                    params: ApiRequestParams::UpdateNote(UpdateNoteApiRequestData {
                        note: note_data,
                    }),
                    version: 6,
                };
                requests.push(api_request);
            }
        }
        if run && !requests.is_empty() {
            self.verify_anki_is_open()?;
        }
        AnkiAdapter::execute_requests(&requests, run, true, client).await?;
        Ok(())
    }

    fn parse_anki_revlog_rows(
        review_log_rows: &[DbRevLogRow],
        card_id: i64,
    ) -> Result<Vec<(RatingSubmission, DateTime<Utc>)>, String> {
        let review_logs = review_log_rows
        .iter()
        .enumerate()
        .map(|(i, review_log_row)| {
            // Anki stores time in milliseconds
            let reviewed_at = DateTime::from_timestamp_millis(review_log_row.id);
            if reviewed_at.is_none() {
                info!(
                    "[Card {}] Skipping the {}th review log because reviewed at is none.",
                    card_id, i
                );
                return Ok(None);
            }
            let rating: Option<RatingId> = match review_log_row.ease {
                // Manual reschedule
                0 => {
                    info!(
                        "[Card {}] Skipping the {}th review log because manually rescheduled, so the rating is none.",
                        card_id, i
                    );
                    Ok(None)
                }
                // Wrong
                1 => Ok(Some(1)), // Again
                // Hard
                2 => Ok(Some(2)), // Hard
                // Ok
                3 => Ok(Some(3)), // Good
                // Easy
                4 => Ok(Some(4)), // Easy
                x => Err(format!("Got an invalid rating of: {}", x)),
            }?;
            if rating.is_none() {
                return Ok(None);
            }
            // let scheduled_time = if review_log_row.ivl < 0 {
            //     // Negative = seconds, positive = days
            //     Duration::try_seconds(-review_log_row.ivl)
            // } else {
            //     Duration::try_days(-review_log_row.ivl)
            // };
            // if scheduled_time.is_none() {
            //     info!(
            //         "[Card {}] Skipping the {}th review log because scheduled time is none.",
            //         card_id, i
            //     );
            //     return Ok(None);
            // }
            Ok(Some((RatingSubmission {
                card_id,
                rating: rating.unwrap(),
                duration: Duration::try_milliseconds(review_log_row.time)
                    .unwrap_or(Duration::zero()),
                tag_id: None,
            }, reviewed_at.unwrap())))
            // let previous_state: Option<StateId> = if i > 0 {
            //     let prev_review_log_row = review_log_rows.get(i - 1).unwrap();
            //     match prev_review_log_row.r#type {
            //         // Learn
            //         0 => Ok(Some(1)),
            //         // Review
            //         1 => Ok(Some(2)),
            //         // Relearn
            //         2 => Ok(Some(3)),
            //         // Filtered
            //         3 => {
            //             info!(
            //                 "[Card {}] Skipping the {}th review log because filtered, so the previous state cannot be determined.",
            //                 card_id, i
            //             );
            //             Ok(None)
            //         },
            //         // Manual
            //         // "When cards are manually rescheduled using the "reset" or "set due date" actions, the type will be listed as Manual and the rating as 0." <https://docs.ankiweb.net/stats.html>
            //         4 => {
            //             info!(
            //                 "[Card {}] Skipping the {}th review log because manually rescheduled, so previous state cannot be determined.",
            //                 card_id, i
            //             );
            //             Ok(None)
            //         },
            //         x => Err(format!("Got an invalid previous state of: {}", x)),
            //     }
            // } else {
            //     // The first review, so the previous state is new
            //     Ok(Some(NEW_CARD_STATE))
            // }?;
            // if previous_state.is_none() {
            //     return Ok(None);
            // }
            // let custom_data = Value::Null;
            // Ok(Some(ReviewLog {
            //     // Unused
            //     id: i64::default(),
            //     card_id,
            //     reviewed_at: reviewed_at.unwrap(),
            //     rating: rating.unwrap(),
            //     scheduler_name: "fsrs".to_string(),
            //     scheduled_time: scheduled_time.unwrap().num_seconds(),
            //     duration: Duration::try_milliseconds(review_log_row.time)
            //         .unwrap_or(Duration::zero())
            //         .num_seconds(),
            //     previous_state: previous_state.unwrap(),
            //     custom_data,
            // }))
        })
        .collect::<Result<Vec<Option<_>>, String>>()?;
        Ok(review_logs.into_iter().flatten().collect::<Vec<_>>())
    }

    #[allow(clippy::too_many_lines)]
    async fn populate_reviews(
        run: bool,
        spares_and_anki_note_ids: Vec<(NoteId, i64)>,
        spares_pool: &SqlitePool,
        anki_db_path: &Path,
    ) -> Result<(), Error> {
        // Get Anki pool
        let anki_pool = AnkiAdapter::read_database_file(anki_db_path).await?;

        // Modify cards
        if run {
            let total = spares_and_anki_note_ids.len();
            for (note_id, anki_note_id) in spares_and_anki_note_ids
                .into_iter()
                .progress_count(total.try_into().unwrap())
            {
                // Get card rows
                let card_rows: Vec<DbCardRow> = sqlx::query_as(
                    "SELECT id, queue, type, due, data FROM cards WHERE nid = ? ORDER BY id ASC",
                )
                .bind(anki_note_id)
                .fetch_all(&anki_pool)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

                let cards: Vec<Card> =
                    sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER by "order""#)
                        .bind(note_id)
                        .fetch_all(spares_pool)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;

                // `.zip()` stops when one iterator is None. This is what we want since we only need to update cards that have a corresponding card in Anki.
                let mut zipped_cards = card_rows.into_iter().zip(cards);
                // Count Anki notes with more than 1 card: `SELECT *, COUNT(*) c FROM cards GROUP BY nid HAVING c > 1;`
                for (anki_card, card) in &mut zipped_cards {
                    // State
                    // See <https://github.com/ankidroid/Anki-Android/wiki/Database-Structure> and <https://github.com/open-spaced-repetition/rs-fsrs/blob/master/src/models.rs>.
                    // let state = anki_card.r#type;
                    // Skip if card is new
                    // let anki_new_card_state = 0;
                    // if state == anki_new_card_state {
                    //     info!(
                    //         "[Note {}, Card {}, Anki Card {}] Skipping because new.",
                    //         card.note_id, card.id, anki_card.id
                    //     );
                    //     continue;
                    // }

                    // FSRS
                    // let stability = anki_card.data.get("s").and_then(|val| val.as_f64());
                    // if stability.is_none() {
                    //     info!(
                    //         "[Note {}, Card {}, Anki Card {}] Skipping because stability is missing.",
                    //         card.note_id, card.id, anki_card.id
                    //     );
                    //     continue;
                    // }
                    // let difficulty = anki_card.data.get("d").and_then(|val| val.as_f64());
                    // if difficulty.is_none() {
                    //     info!(
                    //         "[Note {}, Card {}, Anki Card {}] Skipping because difficulty is missing.",
                    //         card.note_id, card.id, anki_card.id
                    //     );
                    //     continue;
                    // }
                    let desired_retention = anki_card
                        .data
                        .get("dr")
                        .and_then(|val| val.as_f64())
                        .unwrap_or(DEFAULT_DESIRED_RETENTION);
                    if desired_retention != DEFAULT_DESIRED_RETENTION {
                        let body = UpdateCardRequest {
                            selector: CardsSelector::Ids(vec![card.id]),
                            desired_retention: Some(desired_retention),
                            special_state: None,
                        };
                        update_card(spares_pool, body, card.created_at).await?;
                    }

                    // Add review logs
                    let review_log_rows: Vec<DbRevLogRow> =
                        sqlx::query_as("SELECT * FROM revlog WHERE cid = ? ORDER BY id ASC")
                            .bind(anki_card.id)
                            .fetch_all(&anki_pool)
                            .await
                            .map_err(|e| Error::Sqlx { source: e })?;
                    let review_histories = AnkiAdapter::parse_anki_revlog_rows(&review_log_rows, card.id)
                    .map_err(|e| {
                        info!(
                            "[Note {}, Card {}, Anki Card {}] Skipping this card because the review log failed to parse: {}",
                            card.note_id,
                            card.id,
                            anki_card.id,
                            e
                        );
                        e
                    })
                            .map_err(|e| {
            Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: e,
            }))
                        })?;
                    for (rating_submission, reviewed_at) in review_histories {
                        let body = SubmitStudyActionRequest {
                            scheduler_name: "fsrs".to_string(),
                            action: StudyAction::Rate(rating_submission),
                        };
                        submit_study_action(spares_pool, body, reviewed_at).await?;
                    }
                    // for review_log in review_logs {
                    //     let _insert_result =
                    //     sqlx::query(r"INSERT INTO review_log (card_id, reviewed_at, rating, scheduler_name, scheduled_time, duration, previous_state, custom_data) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                    //         .bind(review_log.card_id)
                    //         .bind(review_log.reviewed_at.timestamp())
                    //         .bind(review_log.rating)
                    //         .bind(review_log.scheduler_name)
                    //         .bind(review_log.scheduled_time)
                    //         .bind(review_log.duration)
                    //         .bind(review_log.previous_state)
                    //         .bind(review_log.custom_data)
                    //         .execute(&spares_pool)
                    //         .await
                    //         .map_err(|e| format!("{}", e))?;
                    // }

                    // Update database
                    // let _update_card_result = sqlx::query(
                    //         r"UPDATE card SET stability = ?, difficulty = ?, desired_retention = ?, state = ?, updated_at = strftime('%s', 'now') WHERE id = ?",
                    //     )
                    //     .bind(card.stability)
                    //     .bind(card.difficulty)
                    //     .bind(card.desired_retention)
                    //     .bind(card.state)
                    //     .bind(card.id)
                    //     .execute(&spares_pool)
                    //     .await
                    //     .map_err(|e| format!("{}", e))?;
                }
            }
        }
        Ok(())
    }

    async fn db_row_to_request(
        row: &DbNoteRow,
        pool: &SqlitePool,
        migration_func: Option<MigrationFunc>,
    ) -> Result<(String, GenerateNoteFilesRequest), Error> {
        #[derive(Clone, Debug, Default, Deserialize, FromRow, Serialize)]
        struct DbCardRow {
            queue: i64,
            data: Value,
        }

        let card_rows: Vec<DbCardRow> =
            sqlx::query_as("SELECT queue, data FROM cards WHERE nid = ?")
                .bind(row.id)
                .fetch_all(pool)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        assert!(!card_rows.is_empty());
        let is_suspended = card_rows.into_iter().any(|c| c.queue == -1);

        let tags = row
            .tags
            .split(' ')
            .map(|v| v.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>();

        // Extract note_data and keywords
        let mut fields = row.flds.clone();
        let replacements = [
            ("<br>", "\n"),
            ("<br/>", "\n"),
            ("&amp;", "&"),
            ("&nbsp;", " "),
            ("&gt;", ">"),
            ("&lt;", "<"),
        ];
        for (from, to) in replacements {
            fields = fields.replace(from, to);
        }

        let flds = fields.split('\u{1f}').collect::<Vec<_>>();

        #[allow(clippy::get_first, reason = "symmetry")]
        let mut front = (*flds.get(0).unwrap_or(&"")).to_string();
        let mut back = (*flds.get(1).unwrap_or(&"")).to_string();
        let keywords_str = (*flds.get(2).unwrap_or(&"")).to_string();
        let spares_id_str = (*flds.get(3).unwrap_or(&"")).to_string();
        let spares_parser_name_string = (*flds.get(4).unwrap_or(&"")).to_string();

        let spares_id = spares_id_str.trim().parse::<i64>().ok();
        let keywords = parse_list(keywords_str.as_str());

        front = AnkiAdapter::format_side(&front);
        back = AnkiAdapter::format_side(&back);

        if let Some(ref migration_func) = migration_func {
            let migration_data = MigrationData {
                front,
                back,
                parser_name: spares_parser_name_string.clone(),
                is_suspended,
            };
            let (new_front, new_back) = migration_func(migration_data);
            front = new_front;
            back = new_back;
        }
        let note_data = format!("{}{}", front, back);

        let mut custom_data = Map::new();
        let note_id_key = format!("{}-{}", "anki", NOTE_ID_KEY);
        custom_data.insert(note_id_key, Value::String(format!("{}", row.id)));

        if spares_id.is_none() {
            info!("Failed to parse spares id.");
        }

        // Create requests
        let parse_note_request = GenerateNoteFilesRequest {
            note_id: spares_id.unwrap_or(-1),
            note_data: note_data.clone(),
            keywords: keywords.clone(),
            linked_notes: None,
            custom_data: custom_data.clone(),
            tags: tags.clone(),
        };

        Ok((spares_parser_name_string, parse_note_request))
    }

    async fn read_database_file(original_db_path: &Path) -> Result<SqlitePool, Error> {
        // Copy to prevent corrupting the database
        let mut db_path = get_data_dir();
        db_path.push(original_db_path.file_name().unwrap());
        fs::copy(original_db_path, &db_path).map_err(|e| Error::Io {
            source: e,
            description: "Failed to copy Anki's DB.".to_string(),
        })?;
        info!("Database copied to: {:?}", db_path);

        // Create a connection pool
        let db_url = format!("sqlite://{}", db_path.to_str().unwrap());
        let pool = SqlitePool::connect(&db_url)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(pool)
    }

    pub async fn database_to_requests(
        original_db_path: &Path,
        migration_func: Option<MigrationFunc>,
    ) -> Result<Vec<(String, GenerateNoteFilesRequest)>, Error> {
        let pool = AnkiAdapter::read_database_file(original_db_path).await?;

        // Run the query
        // The field `notes.id` is the epoch milliseconds of when the note was created, so ordering
        // ascending means the notes are inserted the order in which they were created.
        let rows: Vec<DbNoteRow> =
            sqlx::query_as("SELECT id, flds, tags FROM notes ORDER BY id ASC")
                .fetch_all(&pool)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

        let mut requests = Vec::new();
        for row in rows.iter().progress() {
            let request = AnkiAdapter::db_row_to_request(row, &pool, migration_func).await?;
            requests.push(request);
        }

        Ok(requests)
    }
}

#[derive(Debug, Deserialize, FromRow, Serialize)]
struct DbNoteRow {
    id: i64,
    flds: String,
    tags: String,
}

#[derive(Debug, Deserialize, FromRow, Serialize)]
struct DbCardRow {
    id: i64,
    queue: i64,
    r#type: i64,
    due: i64,
    data: Value,
}

#[derive(Debug, Deserialize, FromRow, Serialize)]
struct DbRevLogRow {
    id: i64,     // reviewed_at, but this is in milliseconds
    ease: i64,   // rating
    r#type: i64, // new state of the card
    ivl: i64,    // scheduled_time
    time: i64,   // duration, but this is in milliseconds
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum ModelName {
    #[serde(rename = "Basic")]
    Basic,
    #[serde(rename = "Basic (and reversed card)")]
    BasicAndReversed,
    #[serde(rename = "Cloze")]
    Cloze,
}

// API
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApiRequest {
    pub action: ApiAction,
    pub params: ApiRequestParams,
    pub version: u32, // 6
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ApiAction {
    #[serde(rename = "addNote")]
    AddNote,
    #[serde(rename = "updateNote")]
    UpdateNote,
    #[serde(rename = "deleteNotes")]
    DeleteNote,
    #[serde(rename = "guiBrowse")]
    GuiBrowse,
    #[serde(rename = "findCards")]
    FindCards,
    #[serde(rename = "suspend")]
    Suspend,
    #[serde(rename = "modelFieldNames")]
    GetModelFieldNames,
    #[serde(rename = "modelFieldAdd")]
    AddFieldToModel,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ApiRequestParams {
    AddNote(AddNoteApiRequestData),
    UpdateNote(UpdateNoteApiRequestData),
    DeleteNote(DeleteNoteApiRequestData),
    GuiBrowse(GuiBrowseApiRequestData),
    FindCards(FindCardsApiRequestData),
    Suspend(SuspendApiRequestData),
    GetModelFieldNames(GetModelFieldNamesApiRequestData),
    AddFieldToModel(AddFieldToModelApiRequestData),
}

// General Note Fields
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NoteFields {
    #[serde(rename = "Front", skip_serializing_if = "Option::is_none")]
    pub front: Option<String>,
    #[serde(rename = "Back", skip_serializing_if = "Option::is_none")]
    pub back: Option<String>,
    #[serde(rename = "Keywords", skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    #[serde(rename = "SparesId", skip_serializing_if = "Option::is_none")]
    pub spares_id: Option<String>,
    #[serde(rename = "SparesParserName", skip_serializing_if = "Option::is_none")]
    pub spares_parser_name: Option<String>,
}

// Add Note
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddNoteApiRequestData {
    pub note: AddNoteApiRequestNoteData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddNoteApiRequestNoteData {
    #[serde(rename = "deckName")]
    pub deck_name: String,
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
    pub fields: NoteFields,
    pub tags: Vec<String>,
    pub options: AddNoteApiRequestOptions,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct AddNoteApiRequestOptions {
    #[serde(rename = "allowDuplicate")]
    pub allow_duplicate: bool, // True
}

// Update Note Fields
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateNoteApiRequestData {
    pub note: UpdateNoteApiRequestNoteData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateNoteApiRequestNoteData {
    #[serde(rename = "deckName")]
    pub deck_name: String,
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
    pub id: i64,
    pub fields: NoteFields,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

// Delete Note
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteNoteApiRequestData {
    pub notes: Vec<i64>,
}

// Gui Browse
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GuiBrowseApiRequestData {
    pub query: String,
}

// Find Cards
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FindCardsApiRequestData {
    pub query: String,
}

// Suspend
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SuspendApiRequestData {
    pub cards: Vec<i64>,
}

// Get model field names
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetModelFieldNamesApiRequestData {
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
}

// Add field to model
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddFieldToModelApiRequestData {
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
    #[serde(rename = "fieldName")]
    pub field_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

const ANKI_ADAPTER_NAME: &str = "anki";

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
        run: bool,
    ) -> Result<(), Error> {
        let client = Client::new();

        // Update Anki model's fields, if needed
        if initial_migration {
            if run {
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
            let model_field_names_value =
                AnkiAdapter::execute_request(&api_request, &client).await?;
            let model_field_names: Vec<String> =
                serde_json::from_value(model_field_names_value.clone()).map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: ANKI_ADAPTER_NAME.to_string(),
                        error: e.to_string(),
                    }))
                })?;
            if !model_field_names.contains(&SPARES_KEYWORDS_FIELD_NAME.to_string()) {
                AnkiAdapter::create_field(SPARES_KEYWORDS_FIELD_NAME, &client).await?;
            }
            if !model_field_names.contains(&SPARES_ID_FIELD_NAME.to_string()) {
                AnkiAdapter::create_field(SPARES_ID_FIELD_NAME, &client).await?;
            }
            if !model_field_names.contains(&SPARES_PARSER_NAME_FIELD_NAME.to_string()) {
                AnkiAdapter::create_field(SPARES_PARSER_NAME_FIELD_NAME, &client).await?;
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
        let notes_responses = create_notes(&client, base_url, parse_note_requests, run)
            .await
            .map_err(|e| {
                Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: e.to_string(),
                }))
            })?;

        // Add Anki's reviews
        if initial_migration {
            let spares_and_anki_note_ids = notes_responses
                .iter()
                .flat_map(|x| &x.notes)
                .map(|note_response| -> Result<(i64, i64), String> {
                    let anki_note_id =
                        AnkiAdapter::get_note_id(note_response).map_err(|e| format!("{}", e))?;
                    Ok((note_response.id, anki_note_id))
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: ANKI_ADAPTER_NAME.to_string(),
                        error: e.to_string(),
                    }))
                })?;
            println!("Modifying cards and review log...");
            let start = Instant::now();
            AnkiAdapter::populate_reviews(
                run,
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
                .add_spares_id(&notes_responses, &client, run)
                .await?;
            let duration = start.elapsed();
            println!("Add SparesId to Anki duration: {:?}", duration);
        }

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn process_data(
        &mut self,
        notes: Vec<(NoteSettings, Option<String>)>,
        parser: &dyn Parseable,
        run: bool,
        quiet: bool,
        _at: DateTime<Utc>,
    ) -> Result<(), Error> {
        if run {
            self.verify_anki_is_open()?;
        }
        let mut requests: Vec<ApiRequest> = Vec::new();
        let parser_name = parser.get_parser_name();
        let is_latex = parser_name.contains("latex");
        let client = Client::new();
        let mut added_notes = Vec::new();
        for (local_settings, note_data_res) in notes.clone() {
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
                let front = AnkiAdapter::to_anki_html(front_data.as_str(), is_latex);
                let back_data = AnkiAdapter::note_parts_to_data(
                    &first_card.data[first_cloze_index..],
                    // &card.grouping,
                    parser,
                );
                let back = AnkiAdapter::to_anki_html(back_data.as_str(), is_latex);
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
                        action: AnkiAdapter::note_action_to_anki(local_settings.action),
                        params,
                        version: 6,
                    };

                    if local_settings.is_suspended {
                        let created_note_id = AnkiAdapter::execute_request(&api_request, &client)
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
                        action: AnkiAdapter::note_action_to_anki(local_settings.action),
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
                        action: AnkiAdapter::note_action_to_anki(local_settings.action),
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
                let cards_result_res = AnkiAdapter::execute_request(&api_request, &client).await;
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

        let anki_results = AnkiAdapter::execute_requests(&requests, run, quiet, &client).await?;

        // Update Spares with Anki note id if it was:
        // 1. Already added to Spares
        // 2. Just added to Anki
        let relevant_data: Vec<(String, NoteId, CustomData)> = requests
            .into_iter()
            .zip(anki_results)
            .filter(|(request, _)| matches!(request.action, ApiAction::AddNote))
            .filter(|(request, _)| match &request.params {
                ApiRequestParams::AddNote(AddNoteApiRequestData { note }) => {
                    note.fields.spares_id.is_some()
                }
                _ => unreachable!(),
            })
            .map(|(_request, result)| serde_json::from_value::<i64>(result))
            // Since we are only looking at the relevant requests/results now, the data is now lined up. We can simply zip up the spares id and custom data.
            .zip(added_notes)
            .filter_map(|(anki_note_id, (spares_id, custom_data))| {
                match (anki_note_id, spares_id) {
                    (Ok(anki_note_id), Some(spares_id)) => {
                        Some((anki_note_id.to_string(), spares_id, custom_data))
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        if run {
            let spares_adapter = SparesAdapter::new(SparesRequestProcessor::Server);
            let new_key = format!("{}-{}", self.get_adapter_name(), NOTE_ID_KEY);
            for (anki_note_id, spares_note_id, mut custom_data) in relevant_data {
                custom_data.remove(NOTE_ID_KEY);
                custom_data.insert(new_key.clone(), Value::String(anki_note_id.to_string()));
                spares_adapter
                    .update_custom_data(spares_note_id, custom_data, run, Utc::now())
                    .await?;
            }
        }

        Ok(())
    }
}
