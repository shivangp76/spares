use crate::adapters::impls::anki::types::{DbCardRow, DbNoteRow, DbRevLogRow};
use crate::adapters::impls::anki::utils::format_side;
use crate::adapters::impls::anki::{ANKI_ADAPTER_NAME, AnkiAdapter};
use crate::adapters::migration::{MigrationData, MigrationFunc};
use crate::api::card::update_card;
use crate::api::review::submit_study_action;
use crate::config::get_data_dir;
use crate::helpers::parse_list;
use crate::model::{Card, DEFAULT_DESIRED_RETENTION, NOTE_ID_KEY, NoteId, RatingId};
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::schema::card::{CardsSelector, UpdateCardsRequest};
use crate::schema::review::{RatingSubmission, StudyAction, SubmitStudyActionRequest};
use crate::{AdapterErrorKind, Error, LibraryError};
use chrono::{DateTime, Duration, Utc};
use indicatif::ProgressIterator;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{FromRow, SqlitePool};
use std::fs;
use std::path::Path;

pub fn parse_anki_revlog_rows(
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
                recall_duration: Duration::try_milliseconds(review_log_row.time)
                    .unwrap_or(Duration::zero()),
                rate_duration: Duration::zero(),
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

pub async fn populate_reviews(
    dry_run: bool,
    spares_and_anki_note_ids: Vec<(NoteId, i64)>,
    spares_pool: &SqlitePool,
    anki_db_path: &Path,
) -> Result<(), Error> {
    // Get Anki pool
    let anki_pool = read_database_file(anki_db_path).await?;

    // Modify cards
    if !dry_run {
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
                    let body = UpdateCardsRequest {
                        selector: CardsSelector::Ids(vec![card.id]),
                        desired_retention: Some(desired_retention),
                        special_state: None,
                        due: None,
                    };
                    update_card(spares_pool, body, card.created_at, false).await?;
                }

                // Add review logs
                let review_log_rows: Vec<DbRevLogRow> =
                    sqlx::query_as("SELECT * FROM revlog WHERE cid = ? ORDER BY id ASC")
                        .bind(anki_card.id)
                        .fetch_all(&anki_pool)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                let review_histories = parse_anki_revlog_rows(&review_log_rows, card.id)
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

pub async fn db_row_to_request(
    row: &DbNoteRow,
    pool: &SqlitePool,
    migration_func: Option<MigrationFunc>,
) -> Result<(String, GenerateNoteFilesRequest), Error> {
    #[derive(Clone, Debug, Default, Deserialize, FromRow, Serialize)]
    struct DbCardRow {
        queue: i64,
        data: Value,
    }

    let card_rows: Vec<DbCardRow> = sqlx::query_as("SELECT queue, data FROM cards WHERE nid = ?")
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

    front = format_side(&front);
    back = format_side(&back);

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

pub async fn read_database_file(original_db_path: &Path) -> Result<SqlitePool, Error> {
    // Copy to prevent corrupting the database
    let mut db_path = get_data_dir();
    db_path.push(original_db_path.file_name().unwrap());
    fs::copy(original_db_path, &db_path).map_err(|e| Error::Io {
        source: e,
        description: "Failed to copy Anki's DB.".to_string(),
    })?;
    info!("Database copied to: {}", db_path.display());

    // Create a connection pool
    let db_url = format!("sqlite://{}", db_path.to_str().unwrap());
    let pool = SqlitePool::connect(&db_url)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(pool)
}

impl AnkiAdapter {
    pub async fn database_to_requests(
        original_db_path: &Path,
        migration_func: Option<MigrationFunc>,
    ) -> Result<Vec<(String, GenerateNoteFilesRequest)>, Error> {
        let pool = read_database_file(original_db_path).await?;

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
            let request = db_row_to_request(row, &pool, migration_func).await?;
            requests.push(request);
        }

        Ok(requests)
    }
}
