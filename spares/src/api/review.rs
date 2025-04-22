use super::note::delete_empty_tags;
use crate::{
    Error, LibraryError, SchedulerErrorKind, TagErrorKind,
    api::card::delete_card_tags,
    config::{read_external_config, read_internal_config, write_internal_config},
    helpers::get_start_end_local_date,
    model::{
        Card, CardId, NEW_CARD_STATE, NoteId, RatingId, ReviewLog, SpecialState, StateId, Tag,
    },
    parsers::{
        BackType, Parseable, RenderOutputDirectoryType, find_parser,
        generate_files::{CardSide, RenderOutputType},
        get_output_raw_dir,
    },
    schedulers::{SrsScheduler, get_scheduler_from_string},
    schema::review::{
        CardBackRenderedPath, GetReviewCardFilterRequest, GetReviewCardRequest,
        GetReviewCardResponse, RatingSubmission, StudyAction, SubmitStudyActionRequest,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Days, Duration, Utc};
use indoc::indoc;
use itertools::Itertools;
use serde_json::Value;
use sqlx::{FromRow, sqlite::SqlitePool};

async fn unbury_cards(db: &SqlitePool) -> Result<(), Error> {
    let mut config = read_internal_config()?;
    let now = Utc::now();
    let unburied_limit =
        config
            .last_unburied
            .checked_add_days(Days::new(1))
            .ok_or(Error::Library(LibraryError::InvalidConfig(
                "Failed to add days since `config.last_unburied` is past limit.".to_string(),
            )))?;
    if unburied_limit < now {
        // Unbury
        let _unbury_result = sqlx::query(
            r"UPDATE card SET special_state = NULL, updated_at = ? WHERE special_state IN (?, ?)",
        )
        .bind(now.timestamp())
        .bind(SpecialState::UserBuried)
        .bind(SpecialState::SchedulerBuried)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

        // Update config
        config.last_unburied = now;
        write_internal_config(&config)?;
    }
    Ok(())
}

// Note that `requested_date` is not in `ReviewOptions` since we don't want the user to be able to edit it. However, for testing purposes, we still want to be able to mimic calling this function on different days, so it is included as an argument.
#[allow(clippy::too_many_lines)]
pub async fn get_review_card(
    db: &SqlitePool,
    body: GetReviewCardRequest,
    requested_date: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Option<GetReviewCardResponse>, Error> {
    #[derive(Clone, Debug, Default, FromRow)]
    struct ReviewCard {
        note_id: NoteId,
        parser_name: String,
        card_order: u32,
        card_back_type: BackType,
        card_id: i64,
    }
    let GetReviewCardRequest { filter } = body;

    // Unbury cards, if needed
    unbury_cards(db).await?;

    // Get cards reviewed on `requested_date`
    let (lower_limit, upper_limit) = get_start_end_local_date(&requested_date);
    let cards_studied_on_requested_date: Vec<(i64, i64, StateId)> = sqlx::query_as(
        r"SELECT card_id, duration, previous_state FROM review_log WHERE reviewed_at >= ? AND reviewed_at <= ?",
    )
    .bind(lower_limit.timestamp())
    .bind(upper_limit.timestamp())
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let new_cards_studied_on_requested_date = cards_studied_on_requested_date
        .iter()
        .unique_by(|(card_id, _, _)| card_id)
        .filter(|(_, _, state)| *state == NEW_CARD_STATE)
        .count() as u32;
    let config = read_external_config()?;
    let card_due_limit = upper_limit;
    let not_new_card_str = if new_cards_studied_on_requested_date >= config.new_cards_daily_limit {
        format!("AND c.state != {}", NEW_CARD_STATE)
    } else {
        String::new()
    };
    let note_id_query_str = if let Some(GetReviewCardFilterRequest::Query(ref query)) = filter {
        let evaluator = Evaluator::new(query);
        let note_ids_str = evaluator.get_note_ids(db).await?.into_iter().join(", ");
        format!("AND n.id IN ({})", note_ids_str)
    } else {
        String::new()
    };
    let restrictions = if let Some(GetReviewCardFilterRequest::FilteredTag { tag_id }) = filter {
        // Verify tag has a query
        let tag_query_opt: Option<(Option<String>,)> =
            sqlx::query_as(r"SELECT query FROM tag WHERE id = ?")
                .bind(tag_id)
                .fetch_optional(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        if let Some((tag_query,)) = tag_query_opt {
            if tag_query.is_none() {
                return Err(Error::Library(LibraryError::Tag(
                    TagErrorKind::InvalidInput(
                        "Cannot study a tag that does not have a query.".to_string(),
                    ),
                )));
            }
        } else {
            return Ok(None);
            // return Err(Error::Library(LibraryError::Tag(
            //     TagErrorKind::InvalidInput("Tag does not exist.".to_string()),
            // )));
        }
        // Get all review cards that match the tag, regardless of whether they are due today
        format!(
            "AND c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.id = {})",
            tag_id
        )
    } else {
        format!("AND c.due <= ? {} {}", not_new_card_str, note_id_query_str)
    };
    // Sort by `n.created_at` after `c.due` so cards from older notes are shown first. This ensures that notes that depend on previous knowledge are shown in the right order.
    let query_str = format!(
        indoc! {
        "SELECT
            n.id as note_id,
            p.name as parser_name,
            c.\"order\" as card_order,
            c.back_type as card_back_type,
            c.id as card_id
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE c.special_state IS NULL
            {}
        ORDER BY c.due ASC, n.created_at ASC
        LIMIT 1"
        },
        restrictions
    );
    dbg!(&query_str);
    let mut query = sqlx::query_as(&query_str);
    if !matches!(filter, Some(GetReviewCardFilterRequest::FilteredTag { .. })) {
        query = query.bind(card_due_limit.timestamp());
    }

    let review_card_opt: Option<ReviewCard> = query
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    if let Some(ReviewCard {
        note_id,
        parser_name,
        card_order,
        card_back_type,
        card_id,
    }) = review_card_opt
    {
        let parser = find_parser(parser_name.as_str(), all_parsers)?;
        // Card front rendered path
        let mut card_front_rendered_path =
            parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
        card_front_rendered_path.push(parser.get_output_filename(
            RenderOutputType::Card(card_order as usize, CardSide::Front),
            note_id,
        ));

        // Note raw path
        let mut note_raw_path =
            get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
        note_raw_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
        note_raw_path.set_extension(parser.file_extension());

        let card_back_rendered_path = match card_back_type {
            BackType::FullNote => {
                // Note rendered path
                let mut note_rendered_path =
                    parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
                note_rendered_path
                    .push(parser.get_output_filename(RenderOutputType::Note, note_id));
                CardBackRenderedPath::Note(note_rendered_path)
            }
            BackType::OnlyAnswered => {
                // Card back rendered path
                let mut card_back_rendered_path =
                    parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
                card_back_rendered_path.push(parser.get_output_filename(
                    RenderOutputType::Card(card_order as usize, CardSide::Back),
                    note_id,
                ));
                CardBackRenderedPath::CardBack(card_back_rendered_path)
            }
        };
        let review_card_response = GetReviewCardResponse {
            note_id,
            card_order,
            card_id,
            card_front_rendered_path,
            card_back_rendered_path,
            note_raw_path,
            parser_name,
        };
        return Ok(Some(review_card_response));
    }
    Ok(None)
}

pub async fn update_filtered_tag_scheduler_data(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    filtered_tag: Tag,
    updated_card: &mut Card,
    rating: RatingId,
    duration: Duration,
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
        duration,
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

pub async fn rate_card(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    RatingSubmission {
        card_id,
        rating,
        duration,
        tag_id,
    }: RatingSubmission,
    reviewed_at: DateTime<Utc>,
) -> Result<(), Error> {
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

    let card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    // Get review logs for this card
    let mut review_logs: Vec<ReviewLog> =
        sqlx::query_as(r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC")
            .bind(card_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    // Schedule card
    let latest_review_log = review_logs.last().cloned();
    let (mut updated_card, new_review_log) =
        scheduler.schedule(&card, latest_review_log, rating, reviewed_at, duration)?;
    // Validate scheduler's output
    assert!(matches!(updated_card.custom_data, Value::Object(_)));
    assert!(matches!(new_review_log.custom_data, Value::Object(_)));

    // Smart schedule
    review_logs.push(new_review_log.clone());
    let siblings: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ? AND id != ?")
        .bind(card.note_id)
        .bind(card.id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let mut siblings_with_review_logs = vec![];
    // Get latest reviews for this card
    for card in siblings {
        let review_logs: Vec<ReviewLog> =
            sqlx::query_as(r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC")
                .bind(card.id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        siblings_with_review_logs.push((card, review_logs));
    }
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
            duration,
            reviewed_at,
        )
        .await?;
    }

    // Add entry to review_log
    let _insert_result =
        sqlx::query(r"INSERT INTO review_log (card_id, reviewed_at, rating, scheduler_name, scheduled_time, duration, previous_state, custom_data) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(new_review_log.card_id)
            .bind(new_review_log.reviewed_at.timestamp())
            .bind(new_review_log.rating)
            .bind(new_review_log.scheduler_name)
            .bind(new_review_log.scheduled_time)
            .bind(new_review_log.duration)
            .bind(new_review_log.previous_state)
            .bind(&new_review_log.custom_data)
            .execute(db)
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
    .bind(updated_card.custom_data)
    .bind(card.id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    Ok(())
}

pub async fn bury_card(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    card_id: CardId,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    if let Some(special_state) = card.special_state {
        match special_state {
            SpecialState::Suspended => {
                return Err(Error::Library(LibraryError::Scheduler(
                    SchedulerErrorKind::Suspended,
                )));
            }
            SpecialState::UserBuried | SpecialState::SchedulerBuried => {
                return Err(Error::Library(LibraryError::Scheduler(
                    SchedulerErrorKind::AlreadyBuried,
                )));
            }
        }
    }

    let Card {
        id: _,
        note_id: _,
        order: _,
        back_type: _,
        created_at: _,
        updated_at: _,
        due,
        stability,
        difficulty,
        desired_retention: _,
        special_state,
        state,
        custom_data: _,
    } = scheduler.bury(&card)?;

    // Update card with all new properties from updated_card
    let _update_card_result = sqlx::query(
        r"UPDATE card SET due = ?, stability = ?, difficulty = ?, special_state = ?, state = ?, updated_at = ? WHERE id = ?",
    )
    .bind(due.timestamp())
    .bind(stability)
    .bind(difficulty)
    .bind(special_state)
    .bind(state)
    .bind(at.timestamp())
    .bind(card_id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}

// Note that `reviewed_at` is not present in the request body since we don't want the user to be able to edit it. However, for testing purposes, we still want to be able to mimic calling this function on different days, so it is included as an argument.
pub async fn submit_study_action(
    db: &SqlitePool,
    body: SubmitStudyActionRequest,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    let SubmitStudyActionRequest {
        scheduler_name,
        action,
    } = body;

    let scheduler = get_scheduler_from_string(scheduler_name.as_str())?;

    let config = read_external_config()?;
    match action {
        StudyAction::Rate(rating_submission) => {
            rate_card(db, scheduler.as_ref(), rating_submission, at).await?;
        }
        StudyAction::Bury { card_id } => {
            bury_card(db, scheduler.as_ref(), card_id, at).await?;
        }
        StudyAction::Advance { count } => {
            let _message = scheduler.advance(db, &config, count, at).await?;
        }
        StudyAction::Postpone { count } => {
            let _message = scheduler.postpone(db, &config, count, at).await?;
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
    Ok(())
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
                duration: Duration::seconds(5),
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
}
