use super::note::delete_empty_tags;
use crate::{
    ALLOWED_F64_ERROR, Error, LibraryError, SchedulerErrorKind, TagErrorKind,
    api::{
        card::{delete_card_tags, unbury_cards},
        undo::{
            insert_events,
            payloads::{RateCardPayload, Transition, UpdateCardPayload},
        },
    },
    config::{read_external_config, read_internal_config, write_internal_config},
    helpers::get_start_end_local_date,
    model::{
        Card, CardId, EventType, NEW_CARD_STATE, NoteId, RatingId, ReviewLog, SpecialState,
        StateId, Tag,
    },
    parsers::{
        BackType, Parseable, RenderOutputDirectoryType, find_parser,
        generate_files::{CardSide, RenderOutputType},
        get_output_raw_dir,
    },
    schedulers::{SrsScheduler, get_scheduler_from_string},
    schema::review::{
        CardBackRenderedPath, GetReviewCardFilterRequest, GetReviewCardRequest,
        GetReviewCardResponse, RatingSubmission, ReviewLinkedNote, StudyAction,
        SubmitStudyActionRequest, SubmitStudyActionResponse,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use indoc::indoc;
use itertools::Itertools;
use log::info;
use serde_json::{Value, to_value};
use sqlx::{FromRow, sqlite::SqlitePool};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, FromRow)]
struct ReviewCard {
    note_id: NoteId,
    parser_name: String,
    card_order: u32,
    card_back_type: BackType,
    card_id: i64,
    card_state: StateId,
}

#[derive(Clone, Debug, FromRow)]
struct LinkedNoteRow {
    searched_keyword: String,
    linked_note_id: NoteId,
    matched_keyword: Option<String>,
    parser_name: String,
}

async fn get_linked_notes_for_review(
    db: &SqlitePool,
    note_id: NoteId,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Vec<ReviewLinkedNote>, Error> {
    let rows: Vec<LinkedNoteRow> = sqlx::query_as(
        r"SELECT nl.searched_keyword, nl.linked_note_id, nl.matched_keyword, p.name as parser_name
          FROM note_link nl
          JOIN note n ON nl.linked_note_id = n.id
          JOIN parser p ON n.parser_id = p.id
          WHERE nl.parent_note_id = ?
          AND nl.linked_note_id IS NOT NULL",
    )
    .bind(note_id)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    let mut linked_notes = Vec::new();
    for row in rows {
        let parser = find_parser(&row.parser_name, all_parsers)?;
        let mut note_raw_path =
            get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
        note_raw_path.push(parser.get_output_filename(RenderOutputType::Note, row.linked_note_id));
        note_raw_path.set_extension(parser.file_extension());
        linked_notes.push(ReviewLinkedNote {
            searched_keyword: row.searched_keyword,
            note_id: row.linked_note_id,
            matched_keyword: row.matched_keyword,
            note_raw_path,
        });
    }
    Ok(linked_notes)
}

fn build_review_card_response(
    ReviewCard {
        note_id,
        parser_name,
        card_order,
        card_back_type,
        card_id,
        card_state,
    }: ReviewCard,
    cards_left_by_state: HashMap<StateId, u32>,
    time_estimate: Duration,
    all_parsers: &[fn() -> Box<dyn Parseable>],
    linked_notes: Vec<ReviewLinkedNote>,
) -> Result<GetReviewCardResponse, Error> {
    let parser = find_parser(parser_name.as_str(), all_parsers)?;

    let mut card_front_rendered_path =
        parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
    card_front_rendered_path.push(parser.get_output_filename(
        RenderOutputType::Card(card_order as usize, CardSide::Front),
        note_id,
    ));

    let mut note_raw_path =
        get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
    note_raw_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    note_raw_path.set_extension(parser.file_extension());

    let card_back_rendered_path = match card_back_type {
        BackType::NoteFilePath => {
            let mut note_rendered_path =
                parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
            note_rendered_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
            CardBackRenderedPath::Note(note_rendered_path)
        }
        BackType::CardFilePath => {
            let mut card_back_rendered_path =
                parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
            card_back_rendered_path.push(parser.get_output_filename(
                RenderOutputType::Card(card_order as usize, CardSide::Back),
                note_id,
            ));
            CardBackRenderedPath::CardBack(card_back_rendered_path)
        }
    };

    Ok(GetReviewCardResponse {
        note_id,
        card_order,
        card_id,
        card_state,
        card_front_rendered_path,
        card_back_rendered_path,
        note_raw_path,
        parser_name,
        cards_left_by_state,
        time_estimate,
        linked_notes,
    })
}

const DEFAULT_ESTIMATED_CARD_REVIEW_SECONDS: f64 = 30.0;

async fn unbury_cards_and_update_config(db: &SqlitePool) -> Result<(), Error> {
    let mut config = read_internal_config(db).await?;
    let now = Utc::now();
    // Compare dates in local timezone to determine if cards should be unburied.
    // This ensures that cards are unburied when the calendar date changes in the user's timezone,
    // regardless of when reviews were performed (e.g., late at night vs. early morning).
    // For example, if a user reviews at 11 PM on day 1 and then at 6 AM on day 2, the cards
    // will be unburied on day 2 even though less than 24 hours have passed.
    let now_local_date = now.with_timezone(&Local).date_naive();
    // Handle the case where last_unburied might be MIN_UTC or out of range for local timezone
    // If it's MIN_UTC, treat it as never unburied, so we should always unbury
    let last_unburied_local_date = if config.last_unburied == DateTime::<Utc>::MIN_UTC {
        // Use a date far in the past that's safe for comparison
        // This ensures we always unbury on the first run
        NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
    } else {
        config.last_unburied.with_timezone(&Local).date_naive()
    };
    if now_local_date > last_unburied_local_date {
        // Unbury
        unbury_cards(db, None, now, false).await?;

        // Update config
        config.last_unburied = now;
        write_internal_config(db, &config).await?;
    }
    Ok(())
}

// Note that `requested_date` is not in `ReviewOptions` since we don't want the user to be able to edit it. However, for testing purposes, we still want to be able to mimic calling this function on different days, so it is included as an argument.
#[expect(clippy::too_many_lines)]
pub async fn get_review_card(
    db: &SqlitePool,
    body: GetReviewCardRequest,
    requested_date: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Option<GetReviewCardResponse>, Error> {
    let GetReviewCardRequest { filter } = body;

    // Unbury cards, if needed
    unbury_cards_and_update_config(db).await?;

    // Get cards reviewed on `requested_date`
    let (lower_limit, upper_limit) = get_start_end_local_date(&requested_date);
    let new_cards_studied_on_requested_date: u32 = sqlx::query_scalar(
        r"SELECT COUNT(DISTINCT card_id) FROM review_log
      WHERE reviewed_at >= ? AND reviewed_at <= ? AND previous_state = ?",
    )
    .bind(lower_limit.timestamp())
    .bind(upper_limit.timestamp())
    .bind(NEW_CARD_STATE)
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let config = read_external_config()?;
    let card_due_limit = upper_limit;
    let not_new_card_str = if new_cards_studied_on_requested_date >= config.new_cards_daily_limit {
        format!("\nAND c.state != {}", NEW_CARD_STATE)
    } else {
        String::new()
    };
    let card_id_query_str = if let Some(GetReviewCardFilterRequest::Query(ref query)) = filter {
        let evaluator = Evaluator::new(query);
        let card_ids_str = evaluator.get_card_ids(db).await?.into_iter().join(", ");
        format!("\nAND c.id IN ({})", card_ids_str)
    } else {
        String::new()
    };
    let is_filtered_tag = matches!(filter, Some(GetReviewCardFilterRequest::FilteredTag { .. }));
    let buried_state = SpecialState::BuriedUntilLaterToday as u8;
    let where_clause = if let Some(GetReviewCardFilterRequest::FilteredTag { tag_id }) = filter {
        // Verify tag has a query
        let tag_query_opt: Option<(Option<String>,)> =
            sqlx::query_as(r"SELECT query FROM tag WHERE id = ?")
                .bind(tag_id)
                .fetch_optional(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        if tag_query_opt.is_none() {
            return Ok(None);
            // return Err(Error::Library(LibraryError::Tag(
            //     TagErrorKind::InvalidInput("Tag does not exist.".to_string()),
            // )));
        }
        if let Some((tag_query,)) = tag_query_opt
            && tag_query.is_none()
        {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput(
                    "Cannot study a tag that does not have a query.".to_string(),
                ),
            )));
        }
        // Get all review cards that match the tag, regardless of whether they are due today
        format!(
            "(c.special_state IS NULL OR c.special_state = {buried_state})\n    AND c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.id = {tag_id})"
        )
    } else {
        format!(
            "((c.special_state IS NULL AND c.due <= ?{not_new_card_str}{card_id_query_str})\n    OR (c.special_state = {buried_state}{card_id_query_str}))"
        )
    };
    // Sort by `n.created_at` after `c.due` so cards from older notes are shown first. This ensures that notes that depend on previous knowledge are shown in the right order.
    // BuriedUntilLaterToday cards are shown after normal cards, ordered by their burial timestamp (FIFO).
    let query_str = format!(
        indoc! {
        "SELECT
            n.id as note_id,
            p.name as parser_name,
            c.\"order\" as card_order,
            c.back_type as card_back_type,
            c.id as card_id,
            c.state as card_state
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE {}
        ORDER BY
            CASE WHEN c.special_state IS NULL THEN 0 ELSE 1 END ASC,
            c.due ASC,
            n.created_at ASC
        LIMIT 1"
        },
        where_clause
    );
    info!("{}", &query_str);
    let mut query = sqlx::query_as(&query_str);
    if !is_filtered_tag {
        query = query.bind(card_due_limit.timestamp());
    }
    let review_card_opt: Option<ReviewCard> = query
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    // Get count of cards by state with the same filters
    // These are the remaining cards that are due on `requested_date` grouped by state. Note that
    // this is _not_ equivalent to the cards by state in `get_statistics()` because that does not
    // filter by `requested_date`.
    let count_query_str = format!(
        indoc! {
        "SELECT
            c.state,
            COUNT(*) as count
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE {}
        GROUP BY c.state"
        },
        where_clause
    );
    let mut count_query = sqlx::query_as(&count_query_str);
    if !is_filtered_tag {
        count_query = count_query.bind(card_due_limit.timestamp());
    }
    let cards_by_state_vec: Vec<(StateId, u32)> = count_query
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let cards_left_by_state = cards_by_state_vec
        .into_iter()
        .collect::<HashMap<StateId, u32>>();

    // Calculate time estimate
    // The cast is necassary since the SUM function returns a different type depending on whether
    // the arguments are all null or not.
    let time_estimate_query_str = format!(
        indoc! {
        "SELECT
            SUM(CAST(COALESCE(avg_duration, {}) as REAL)) as total_time
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        LEFT JOIN (
            SELECT card_id, AVG(recall_duration + rate_duration) as avg_duration
            FROM review_log
            GROUP BY card_id
        ) rl ON rl.card_id = c.id
        WHERE {}"
        },
        DEFAULT_ESTIMATED_CARD_REVIEW_SECONDS, where_clause
    );
    let mut time_estimate_query = sqlx::query_scalar(&time_estimate_query_str);
    if !is_filtered_tag {
        time_estimate_query = time_estimate_query.bind(card_due_limit.timestamp());
    }
    let total_time_seconds: f64 = time_estimate_query
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let time_estimate = Duration::seconds(total_time_seconds as i64);

    if let Some(review_card) = review_card_opt {
        let linked_notes =
            get_linked_notes_for_review(db, review_card.note_id, all_parsers).await?;
        return Ok(Some(build_review_card_response(
            review_card,
            cards_left_by_state,
            time_estimate,
            all_parsers,
            linked_notes,
        )?));
    }
    Ok(None)
}

pub async fn get_review_card_by_id(
    db: &SqlitePool,
    card_id: CardId,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Option<GetReviewCardResponse>, Error> {
    let review_card_opt: Option<ReviewCard> = sqlx::query_as(
        r#"SELECT
            n.id as note_id,
            p.name as parser_name,
            c."order" as card_order,
            c.back_type as card_back_type,
            c.id as card_id,
            c.state as card_state
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE c.id = ?"#,
    )
    .bind(card_id)
    .fetch_optional(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    match review_card_opt {
        Some(rc) => {
            let linked_notes = get_linked_notes_for_review(db, rc.note_id, all_parsers).await?;
            Ok(Some(build_review_card_response(
                rc,
                HashMap::new(),
                Duration::zero(),
                all_parsers,
                linked_notes,
            )?))
        }
        None => Ok(None),
    }
}

pub async fn update_filtered_tag_scheduler_data(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    filtered_tag: Tag,
    updated_card: &mut Card,
    rating: RatingId,
    recall_duration: Duration,
    reviewed_at: DateTime<Utc>,
) -> Result<(), Error> {
    let scheduler_name = scheduler.get_scheduler_name();
    let tag_id_str = filtered_tag.id.to_string();
    assert!(matches!(updated_card.custom_data, Value::Object(_)));
    let custom_data = updated_card.custom_data.as_object().unwrap();
    let filtered_tag_scheduler_data = custom_data
        .get(&tag_id_str)
        .and_then(|x| x.get(scheduler_name));
    let new_filtered_tag_scheduler_data_opt = scheduler.filtered_tag_schedule(
        filtered_tag_scheduler_data,
        updated_card,
        rating,
        reviewed_at,
        recall_duration,
    )?;
    let custom_data = updated_card.custom_data.as_object_mut().unwrap();
    if let Some(new_filtered_tag_scheduler_data) = new_filtered_tag_scheduler_data_opt {
        custom_data
            .entry(tag_id_str)
            .and_modify(|v| {
                if let Some(tag_object) = v.as_object_mut() {
                    tag_object.insert(
                        scheduler_name.to_string(),
                        new_filtered_tag_scheduler_data.clone(),
                    );
                }
            })
            .or_insert_with(|| {
                Value::Object(serde_json::Map::from_iter([(
                    scheduler_name.to_string(),
                    new_filtered_tag_scheduler_data,
                )]))
            });
    } else {
        // Delete scheduler data for the filtered tag
        if let Some(tag_object) = custom_data
            .get_mut(&tag_id_str)
            .and_then(|v| v.as_object_mut())
        {
            tag_object.remove(scheduler_name);
            if tag_object.is_empty() {
                custom_data.remove(&tag_id_str);
            }
        }

        // Remove filtered tag from card
        delete_card_tags(db, &[(updated_card.id, filtered_tag.id)]).await?;

        // Delete filtered tag if there are no more notes
        if filtered_tag.auto_delete {
            delete_empty_tags(db, &[filtered_tag.id]).await?;
        }
    }
    Ok(())
}

/// Fetches all sibling cards for a note (excluding `card_id`) together with
/// their full review-log history, ordered by `reviewed_at ASC`.
async fn fetch_siblings_with_review_logs(
    db: &SqlitePool,
    note_id: NoteId,
    card_id: CardId,
) -> Result<Vec<(Card, Vec<ReviewLog>)>, Error> {
    let siblings: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ? AND id != ?")
        .bind(note_id)
        .bind(card_id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let mut siblings_with_review_logs = Vec::with_capacity(siblings.len());
    for sibling in siblings {
        let review_logs: Vec<ReviewLog> =
            sqlx::query_as(r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC")
                .bind(sibling.id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        siblings_with_review_logs.push((sibling, review_logs));
    }
    Ok(siblings_with_review_logs)
}

#[expect(clippy::too_many_lines)]
pub async fn rate_card(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    RatingSubmission {
        card_id,
        rating,
        recall_duration,
        rate_duration,
        tag_id,
    }: RatingSubmission,
    reviewed_at: DateTime<Utc>,
    log: bool,
) -> Result<Option<i64>, Error> {
    // Validate input
    let filtered_tag_opt = if let Some(tag_id) = tag_id {
        let tag: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(tag_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if tag.query.is_none() {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput("Supplied tag id is not a filtered tag.".to_string()),
            )));
        }
        Some(tag)
    } else {
        None
    };

    let before_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let card = before_card.clone();

    // Get review logs for this card
    let mut review_logs: Vec<ReviewLog> =
        sqlx::query_as(r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC")
            .bind(card_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    // Schedule card
    let latest_review_log = review_logs.last().cloned();
    let (mut updated_card, new_review_log) = scheduler.schedule(
        &card,
        latest_review_log,
        rating,
        reviewed_at,
        recall_duration,
        rate_duration,
    )?;
    // Validate scheduler's output
    assert!(matches!(updated_card.custom_data, Value::Object(_)));
    assert!(matches!(new_review_log.custom_data, Value::Object(_)));

    // Smart schedule
    review_logs.push(new_review_log.clone());
    let siblings_with_review_logs =
        fetch_siblings_with_review_logs(db, card.note_id, card.id).await?;
    let config = read_external_config()?;
    updated_card.due = scheduler
        .smart_schedule(
            &config,
            &(updated_card.clone(), review_logs),
            &siblings_with_review_logs,
            reviewed_at,
        )
        .await?;

    // Update filtered tag scheduler data
    if let Some(filtered_tag) = filtered_tag_opt {
        update_filtered_tag_scheduler_data(
            db,
            scheduler,
            filtered_tag,
            &mut updated_card,
            rating,
            recall_duration,
            reviewed_at,
        )
        .await?;
    }

    // Add entry to review_log
    let review_log_id: i64 =
        sqlx::query_scalar(r"INSERT INTO review_log (card_id, reviewed_at, rating, scheduler_name, scheduled_time, recall_duration, rate_duration, previous_state, custom_data) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id")
            .bind(new_review_log.card_id)
            .bind(new_review_log.reviewed_at.timestamp())
            .bind(new_review_log.rating)
            .bind(new_review_log.scheduler_name)
            .bind(new_review_log.scheduled_time)
            .bind(new_review_log.recall_duration)
            .bind(new_review_log.rate_duration)
            .bind(new_review_log.previous_state)
            .bind(&new_review_log.custom_data)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    // Update card with all new properties from updated_card
    let _update_card_result = sqlx::query(
        r"UPDATE card SET due = ?, stability = ?, difficulty = ?, state = ?, updated_at = ?, custom_data = ? WHERE id = ?",
    )
    .bind(updated_card.due.timestamp())
    .bind(updated_card.stability)
    .bind(updated_card.difficulty)
    .bind(updated_card.state)
    .bind(updated_card.updated_at.timestamp())
    .bind(updated_card.custom_data.clone())
    .bind(card.id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    if log {
        let payload = RateCardPayload {
            review_log_id,
            card: UpdateCardPayload {
                card_id,
                order: None,
                back_type: None,
                due: Some(Transition {
                    before: before_card.due,
                    after: updated_card.due,
                }),
                stability: Some(Transition {
                    before: before_card.stability,
                    after: updated_card.stability,
                }),
                difficulty: Some(Transition {
                    before: before_card.difficulty,
                    after: updated_card.difficulty,
                }),
                desired_retention: None,
                special_state: None,
                state: Some(Transition {
                    before: before_card.state,
                    after: updated_card.state,
                }),
                custom_data: (updated_card.custom_data != before_card.custom_data).then_some(
                    Transition {
                        before: before_card.custom_data,
                        after: updated_card.custom_data,
                    },
                ),
            },
        };
        let event_ids = insert_events(
            db,
            &[(EventType::RateCard, to_value(&payload).unwrap())],
            reviewed_at,
            None,
        )
        .await?;
        return Ok(Some(*event_ids.first().unwrap()));
    }

    Ok(None)
}

pub async fn bury_card(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    card_id: CardId,
    at: DateTime<Utc>,
    log: bool,
) -> Result<Option<i64>, Error> {
    let before_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    if let Some(special_state) = before_card.special_state {
        match special_state {
            SpecialState::Suspended => {
                return Err(Error::Library(LibraryError::Scheduler(
                    SchedulerErrorKind::Suspended,
                )));
            }
            SpecialState::UserBuried
            | SpecialState::SchedulerBuried
            | SpecialState::BuriedUntilLaterToday => {
                return Err(Error::Library(LibraryError::Scheduler(
                    SchedulerErrorKind::AlreadyBuried,
                )));
            }
        }
    }

    let buried_card = scheduler.bury(&before_card)?;

    // Update card with all new properties from buried_card
    let _update_card_result = sqlx::query(
        r"UPDATE card SET due = ?, stability = ?, difficulty = ?, special_state = ?, state = ?, updated_at = ? WHERE id = ?",
    )
    .bind(buried_card.due.timestamp())
    .bind(buried_card.stability)
    .bind(buried_card.difficulty)
    .bind(buried_card.special_state)
    .bind(buried_card.state)
    .bind(at.timestamp())
    .bind(card_id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    if log {
        let payload = vec![UpdateCardPayload {
            card_id,
            order: None,
            back_type: None,
            due: (buried_card.due != before_card.due).then_some(Transition {
                before: before_card.due,
                after: buried_card.due,
            }),
            stability: ((buried_card.stability - before_card.stability).abs() > ALLOWED_F64_ERROR)
                .then_some(Transition {
                    before: before_card.stability,
                    after: buried_card.stability,
                }),
            difficulty: ((buried_card.difficulty - before_card.difficulty).abs()
                > ALLOWED_F64_ERROR)
                .then_some(Transition {
                    before: before_card.difficulty,
                    after: buried_card.difficulty,
                }),
            desired_retention: None,
            special_state: Some(Transition {
                before: before_card.special_state,
                after: buried_card.special_state,
            }),
            state: (buried_card.state != before_card.state).then_some(Transition {
                before: before_card.state,
                after: buried_card.state,
            }),
            custom_data: None,
        }];
        let event_ids = insert_events(
            db,
            &[(EventType::BuryCards, to_value(&payload).unwrap())],
            at,
            None,
        )
        .await?;
        return Ok(Some(*event_ids.first().unwrap()));
    }
    Ok(None)
}

// Note that `reviewed_at` is not present in the request body since we don't want the user to be able to edit it. However, for testing purposes, we still want to be able to mimic calling this function on different days, so it is included as an argument.
pub async fn submit_study_action(
    db: &SqlitePool,
    body: SubmitStudyActionRequest,
    at: DateTime<Utc>,
) -> Result<SubmitStudyActionResponse, Error> {
    let SubmitStudyActionRequest {
        scheduler_name,
        action,
    } = body;

    let scheduler = get_scheduler_from_string(scheduler_name.as_str())?;

    let config = read_external_config()?;
    match action {
        StudyAction::Rate(rating_submission) => {
            return rate_card(db, scheduler.as_ref(), rating_submission, at, true)
                .await
                .map(|event_id| SubmitStudyActionResponse { event_id });
        }
        StudyAction::Bury { card_id } => {
            return bury_card(db, scheduler.as_ref(), card_id, at, true)
                .await
                .map(|event_id| SubmitStudyActionResponse { event_id });
        }
        StudyAction::Advance { count, query } => {
            let move_cards_result = scheduler.advance(db, &config, count, query, at).await?;
            if !move_cards_result.card_payloads.is_empty() {
                insert_events(
                    db,
                    &[(
                        EventType::AdvanceCards,
                        to_value(&move_cards_result.card_payloads).unwrap(),
                    )],
                    at,
                    None,
                )
                .await?;
            }
        }
        StudyAction::Postpone { count, query } => {
            let move_cards_result = scheduler.postpone(db, &config, count, query, at).await?;
            if !move_cards_result.card_payloads.is_empty() {
                insert_events(
                    db,
                    &[(
                        EventType::PostponeCards,
                        to_value(&move_cards_result.card_payloads).unwrap(),
                    )],
                    at,
                    None,
                )
                .await?;
            }
        }
        StudyAction::Reschedule => {
            let cards: Vec<Card> =
                sqlx::query_as(r"SELECT * FROM card WHERE special_state IS NULL")
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
            // Get all review logs for cards
            let mut query = sqlx::query_as(r"SELECT * FROM review_log WHERE card_id IN (?)");
            for card in &cards {
                query = query.bind(card.id);
            }
            let all_review_logs: Vec<ReviewLog> = query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let grouped_review_logs = all_review_logs
                .into_iter()
                .map(|rl| (rl.card_id, rl))
                .into_group_map();
            let cards_with_review_logs = cards
                .into_iter()
                .map(|card| {
                    (
                        card.clone(),
                        grouped_review_logs.get(&card.id).unwrap().clone(),
                    )
                })
                .collect::<Vec<_>>();
            scheduler
                .reschedule(db, &config, cards_with_review_logs, at)
                .await?;
        }
    }
    Ok(SubmitStudyActionResponse { event_id: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{note::tests::tests::create_note_helper, statistics::get_statistics},
        model::Card,
        parsers::get_all_parsers,
        schema::{note::NoteResponse, review::StatisticsRequest},
    };

    async fn create_note(pool: &sqlx::SqlitePool) -> (NoteResponse, Vec<Card>) {
        // Create note
        let created_notes = create_note_helper(pool).await;
        let last_note = created_notes.last().unwrap();

        // Get card_id for note
        let cards_res: Result<Vec<Card>, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE note_id = ? ORDER BY due")
                .bind(last_note.id)
                .fetch_all(pool)
                .await;
        assert!(cards_res.is_ok());
        let cards = cards_res.unwrap();
        assert!(!cards.is_empty());

        (last_note.clone(), cards)
    }

    #[sqlx::test]
    async fn test_get_and_update_review(pool: sqlx::SqlitePool) -> () {
        // Create note
        let (_note, _cards) = create_note(&pool).await;
        let now = Utc::now();

        // Get review
        let review_res = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await;
        assert!(review_res.is_ok());
        let review_card_opt = review_res.unwrap();
        assert!(review_card_opt.is_some());
        let review_card = review_card_opt.unwrap();
        // These assertions can't be made because all notes are created at the same time, so the cards are also created at the same time, so they are due at the same time. Thus, the order of cards being due is not guaranteed to be the same.
        // assert_eq!(review_card.note_id, note_id);
        // assert_eq!(review_card.card_id, card_id);
        // assert_eq!(
        //     review_card
        //         .card_rendered_path
        //         .file_stem()
        //         .unwrap()
        //         .to_str()
        //         .unwrap(),
        //     format!("{:0>4}-{:0>1}", note_id, 1)
        // );
        assert_eq!(review_card.parser_name, "markdown".to_string());

        // Get old card
        let old_card_res: Result<Card, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
                .bind(review_card.card_id)
                .fetch_one(&pool)
                .await;
        assert!(old_card_res.is_ok());
        let old_card = old_card_res.unwrap();

        // Get statistics
        let request = StatisticsRequest {
            scheduler_name: "fsrs".to_string(),
            date: now,
        };
        let statistics_res = get_statistics(&pool, request).await;
        assert!(statistics_res.is_ok());
        let statistics_response = statistics_res.unwrap();
        assert_eq!(
            statistics_response.due_count_by_state.get(&NEW_CARD_STATE),
            Some(&3)
        );
        assert_eq!(
            statistics_response
                .due_count_by_state
                .iter()
                .map(|(_, x)| x)
                .sum::<u32>(),
            3
        );
        assert_eq!(statistics_response.advance_safe_count, 0);
        assert_eq!(statistics_response.postpone_safe_count, 0);

        // Update review
        let request = SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id: review_card.card_id,
                rating: 4,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(5),
                tag_id: None,
            }),
        };
        let submit_review_res = submit_study_action(&pool, request, now).await;
        assert!(submit_review_res.is_ok());

        // Check database and verify card is now due later
        let new_card_res: Result<Card, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
                .bind(review_card.card_id)
                .fetch_one(&pool)
                .await;
        assert!(new_card_res.is_ok());
        let new_card = new_card_res.unwrap();
        assert!(new_card.due > old_card.due);
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_hides_card_while_others_remain(
        pool: sqlx::SqlitePool,
    ) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(all_cards.len(), 3);

        let now = Utc::now();

        // Initialize last_unburied = today so subsequent get_review_card calls don't unbury
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        // Bury card_a as BuriedUntilLaterToday
        let card_a_id = all_cards[0].id;
        crate::api::card::update_cards(
            &pool,
            crate::schema::card::UpdateCardsRequest {
                selector: crate::schema::card::CardsSelector::Ids(vec![card_a_id]),
                desired_retention: None,
                special_state: Some(Some(
                    crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                )),
                due: Some(now),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Next review card should NOT be card_a (it's buried, 2 normal cards remain)
        let review = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_ne!(
            review.card_id, card_a_id,
            "BuriedUntilLaterToday card should not be shown while normal cards remain"
        );

        // Confirm the card is still in BuriedUntilLaterToday state
        let card_a: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_a_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            card_a.special_state,
            Some(SpecialState::BuriedUntilLaterToday)
        );
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_reappears_when_no_normal_cards(
        pool: sqlx::SqlitePool,
    ) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(all_cards.len(), 3);

        let now = Utc::now();

        // Initialize last_unburied = today
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        // Bury card_a (index 0) as BuriedUntilLaterToday
        let card_a_id = all_cards[0].id;
        crate::api::card::update_cards(
            &pool,
            crate::schema::card::UpdateCardsRequest {
                selector: crate::schema::card::CardsSelector::Ids(vec![card_a_id]),
                desired_retention: None,
                special_state: Some(Some(
                    crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                )),
                due: Some(now),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Rate the remaining 2 normal cards (rating=4 schedules them far in the future)
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        for card in &all_cards[1..] {
            rate_card(
                &pool,
                scheduler.as_ref(),
                crate::schema::review::RatingSubmission {
                    card_id: card.id,
                    rating: 4,
                    recall_duration: Duration::seconds(5),
                    rate_duration: Duration::seconds(2),
                    tag_id: None,
                },
                now,
                false,
            )
            .await
            .unwrap();
        }

        // card_a (BuriedUntilLaterToday) should now be the only reviewable card
        let review = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();
        assert!(
            review.is_some(),
            "Expected BuriedUntilLaterToday card to reappear when no normal cards remain"
        );
        assert_eq!(
            review.unwrap().card_id,
            card_a_id,
            "Expected card_a to be returned"
        );
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_fifo_ordering(pool: sqlx::SqlitePool) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(all_cards.len(), 3);

        let now = Utc::now();

        // Initialize last_unburied = today
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        let card_a_id = all_cards[0].id;
        let card_b_id = all_cards[1].id;
        let card_c_id = all_cards[2].id;

        // Bury all 3 cards with increasing timestamps (t1 < t2 < t3)
        let t1 = now;
        let t2 = now + Duration::seconds(1);
        let t3 = now + Duration::seconds(2);

        for (id, t) in [(card_a_id, t1), (card_b_id, t2), (card_c_id, t3)] {
            crate::api::card::update_cards(
                &pool,
                crate::schema::card::UpdateCardsRequest {
                    selector: crate::schema::card::CardsSelector::Ids(vec![id]),
                    desired_retention: None,
                    special_state: Some(Some(
                        crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                    )),
                    due: Some(t),
                },
                now,
                false,
            )
            .await
            .unwrap();
        }

        // First review should be card_a (earliest burial timestamp t1)
        let review1 = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            review1.card_id, card_a_id,
            "card_a (earliest burial) should come first"
        );

        // Re-bury card_a with a later timestamp (t4 > t3), pushing it to the back
        let t4 = now + Duration::seconds(3);
        crate::api::card::update_cards(
            &pool,
            crate::schema::card::UpdateCardsRequest {
                selector: crate::schema::card::CardsSelector::Ids(vec![card_a_id]),
                desired_retention: None,
                special_state: Some(Some(
                    crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                )),
                due: Some(t4),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Now card_b (t2) should be first, then card_c (t3), then card_a (t4)
        let review2 = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            review2.card_id, card_b_id,
            "card_b should be shown after card_a is re-buried to the back"
        );
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_unburied_next_day(pool: sqlx::SqlitePool) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();

        let now = Utc::now();

        // Initialize last_unburied = today to prevent immediate unburying
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        // Bury all cards as BuriedUntilLaterToday
        for card in &all_cards {
            crate::api::card::update_cards(
                &pool,
                crate::schema::card::UpdateCardsRequest {
                    selector: crate::schema::card::CardsSelector::Ids(vec![card.id]),
                    desired_retention: None,
                    special_state: Some(Some(
                        crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                    )),
                    due: Some(now),
                },
                now,
                false,
            )
            .await
            .unwrap();
        }

        // Verify all cards are buried
        let buried_count: i64 =
            sqlx::query_scalar(r"SELECT COUNT(*) FROM card WHERE special_state = ?")
                .bind(SpecialState::BuriedUntilLaterToday)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(buried_count, 3);

        // Simulate next day by calling unbury_cards directly
        let tomorrow = now + Duration::days(1);
        crate::api::card::unbury_cards(&pool, None, tomorrow, false)
            .await
            .unwrap();

        // All cards should now have special_state = NULL
        let still_buried_count: i64 =
            sqlx::query_scalar(r"SELECT COUNT(*) FROM card WHERE special_state = ?")
                .bind(SpecialState::BuriedUntilLaterToday)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            still_buried_count, 0,
            "All BuriedUntilLaterToday cards should be unburied at next-day rollover"
        );
    }
}
