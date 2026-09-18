use chrono::DateTime;
use chrono::Utc;
use serde_json::Map;
use serde_json::Value;
use serde_json::to_value;
use sqlx::sqlite::SqlitePool;

use crate::ALLOWED_F64_ERROR;
use crate::Error;
use crate::api::MAX_ROWS_IN_QUERY;
use crate::api::execute_batched_query;
use crate::api::fetch_review_logs_for_replay;
use crate::api::placeholders_2d;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::ForgetCardPayload;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::api::validate_bury_target;
use crate::config::read_external_config;
use crate::model::Card;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::NEW_CARD_STATE;
use crate::model::NoteId;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::model::TagId;
use crate::schedulers::get_default_scheduler_name;
use crate::schedulers::get_scheduler_from_string;
use crate::schema::FilterOptions;
use crate::schema::card::CardResponse;
use crate::schema::card::CardsSelector;
use crate::schema::card::ForgetCardResponse;
use crate::schema::card::GetLeechesRequest;
use crate::schema::card::SpecialStateUpdate;
use crate::schema::card::UpdateCardsRequest;
use crate::schema::card::UpdateCardsResponse;
use crate::search::evaluator::Evaluator;

const DEFAULT_CARDS_LIMIT: usize = 100;

pub async fn get_card(db: &SqlitePool, id: CardId) -> Result<CardResponse, Error> {
    let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(CardResponse::new(&card))
}

pub async fn get_cards(db: &SqlitePool, note_id: NoteId) -> Result<Vec<CardResponse>, Error> {
    let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
        .bind(note_id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(cards
        .into_iter()
        .map(|card| CardResponse::new(&card))
        .collect::<Vec<_>>())
}

pub async fn list_cards(db: &SqlitePool, opts: FilterOptions) -> Result<Vec<CardResponse>, Error> {
    let limit = opts.limit.unwrap_or(DEFAULT_CARDS_LIMIT);
    let offset = (opts.page.unwrap_or(1).saturating_sub(1)) * limit;
    let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id LIMIT ? OFFSET ?")
        .bind(limit as u32)
        .bind(offset as u32)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(cards
        .into_iter()
        .map(|card| CardResponse::new(&card))
        .collect::<Vec<_>>())
}

fn build_update_card_payload(
    card_id: CardId,
    existing_card: &Card,
    final_card: &Card,
) -> UpdateCardPayload {
    UpdateCardPayload {
        card_id,
        order: None,
        back_type: None,
        due: (final_card.due != existing_card.due).then_some(Transition {
            before: existing_card.due,
            after: final_card.due,
        }),
        stability: ((final_card.stability - existing_card.stability).abs() > ALLOWED_F64_ERROR)
            .then_some(Transition {
                before: existing_card.stability,
                after: final_card.stability,
            }),
        difficulty: ((final_card.difficulty - existing_card.difficulty).abs() > ALLOWED_F64_ERROR)
            .then_some(Transition {
                before: existing_card.difficulty,
                after: final_card.difficulty,
            }),
        desired_retention: ((final_card.desired_retention - existing_card.desired_retention).abs()
            > ALLOWED_F64_ERROR)
            .then_some(Transition {
                before: existing_card.desired_retention,
                after: final_card.desired_retention,
            }),
        special_state: (final_card.special_state != existing_card.special_state).then_some(
            Transition {
                before: existing_card.special_state,
                after: final_card.special_state,
            },
        ),
        state: (final_card.state != existing_card.state).then_some(Transition {
            before: existing_card.state,
            after: final_card.state,
        }),
        custom_data: (final_card.custom_data != existing_card.custom_data).then_some(Transition {
            before: existing_card.custom_data.clone(),
            after: final_card.custom_data.clone(),
        }),
    }
}

pub async fn update_cards(
    db: &SqlitePool,
    body: UpdateCardsRequest,
    at: DateTime<Utc>,
    log: bool,
) -> Result<UpdateCardsResponse, Error> {
    let card_ids = match body.selector {
        CardsSelector::Ids(vec) => vec,
        CardsSelector::Query(query) => {
            let evaluator = Evaluator::new(&query);
            evaluator.get_card_ids(db).await?
        }
    };
    let mut card_responses = Vec::new();
    let mut card_payloads: Vec<UpdateCardPayload> = Vec::new();
    let requested_special_state = body.special_state.map(|x| {
        x.map(|y| match y {
            SpecialStateUpdate::Suspended => SpecialState::Suspended,
            SpecialStateUpdate::Buried => SpecialState::UserBuried,
            SpecialStateUpdate::BuriedUntilLaterToday => SpecialState::BuriedUntilLaterToday,
        })
    });
    for card_id in card_ids {
        let existing_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        // Update (if empty, use old value)
        let new_desired_retention = body
            .desired_retention
            .unwrap_or(existing_card.desired_retention);
        let new_special_state = requested_special_state.unwrap_or(existing_card.special_state);
        let new_due = body.due.unwrap_or(existing_card.due);
        if let Some(Some(SpecialState::UserBuried)) = requested_special_state {
            validate_bury_target(existing_card.special_state)?;
        }
        let updated_at: i64 =
        sqlx::query_scalar(r"UPDATE card SET desired_retention = ?, special_state = ?, due = ?, updated_at = ? WHERE id = ? RETURNING updated_at")
            .bind(new_desired_retention)
            .bind(new_special_state)
            .bind(new_due.timestamp())
            .bind(at.timestamp())
            .bind(card_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let updated_at = DateTime::from_timestamp(updated_at, 0).unwrap();
        let mut updated_card: Card = existing_card.clone();
        updated_card.desired_retention = new_desired_retention;
        updated_card.special_state = new_special_state;
        updated_card.due = new_due;
        updated_card.updated_at = updated_at;
        if let Some(new_desired_retention) = body.desired_retention
            && (new_desired_retention - existing_card.desired_retention).abs() > ALLOWED_F64_ERROR
            && updated_card.state != NEW_CARD_STATE
        {
            // Forget markers are kept: `compute_memory_state` needs them to know where to
            // restart the replay.
            let review_logs = fetch_review_logs_for_replay(db, updated_card.id).await?;
            // A card whose log holds nothing but forget markers has no memory state to recompute.
            if let Some(latest_review) = review_logs.iter().rev().find(|rl| rl.is_review()) {
                let scheduler = get_scheduler_from_string(latest_review.scheduler_name.as_str())?;

                let config = read_external_config()?;
                // Reschedule card
                scheduler
                    .reschedule(db, &config, vec![(updated_card.clone(), review_logs)], at)
                    .await?;
            }
        }
        // Read the final card state from DB (reschedule may have changed due/stability/difficulty)
        let final_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if log {
            card_payloads.push(build_update_card_payload(
                card_id,
                &existing_card,
                &final_card,
            ));
        }
        card_responses.push(CardResponse::new(&final_card));
    }
    let event_id = if log && !card_payloads.is_empty() {
        let event_ids = insert_events(
            db,
            &[(EventType::UpdateCards, to_value(&card_payloads).unwrap())],
            at,
            None,
        )
        .await?;
        Some(*event_ids.first().unwrap())
    } else {
        None
    };
    Ok(UpdateCardsResponse {
        cards: card_responses,
        event_id,
    })
}

/// Applies a list of card updates directly, restoring the `.after` value for each field.
/// Used by the undo system to replay or reverse card state changes.
pub async fn update_card_event(
    db: &SqlitePool,
    payloads: Vec<UpdateCardPayload>,
    log: bool,
) -> Result<(), Error> {
    let at = Utc::now();
    for payload in &payloads {
        let existing_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(payload.card_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let new_due = payload.due.as_ref().map_or(existing_card.due, |t| t.after);
        let new_stability = payload
            .stability
            .as_ref()
            .map_or(existing_card.stability, |t| t.after);
        let new_difficulty = payload
            .difficulty
            .as_ref()
            .map_or(existing_card.difficulty, |t| t.after);
        let new_desired_retention = payload
            .desired_retention
            .as_ref()
            .map_or(existing_card.desired_retention, |t| t.after);
        let new_special_state = payload
            .special_state
            .as_ref()
            .map_or(existing_card.special_state, |t| t.after);
        let new_state = payload
            .state
            .as_ref()
            .map_or(existing_card.state, |t| t.after);
        let new_custom_data = payload
            .custom_data
            .as_ref()
            .map_or_else(|| existing_card.custom_data.clone(), |t| t.after.clone());
        sqlx::query(
            r"UPDATE card SET due = ?, stability = ?, difficulty = ?, desired_retention = ?, special_state = ?, state = ?, custom_data = ?, updated_at = ? WHERE id = ?",
        )
        .bind(new_due.timestamp())
        .bind(new_stability)
        .bind(new_difficulty)
        .bind(new_desired_retention)
        .bind(new_special_state)
        .bind(new_state)
        .bind(&new_custom_data)
        .bind(at.timestamp())
        .bind(payload.card_id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    }
    if log && !payloads.is_empty() {
        insert_events(
            db,
            &[(EventType::UpdateCards, to_value(&payloads).unwrap())],
            at,
            None,
        )
        .await?;
    }
    Ok(())
}

pub async fn get_leeches(
    db: &SqlitePool,
    request: GetLeechesRequest,
) -> Result<Vec<CardResponse>, Error> {
    let GetLeechesRequest { scheduler_name } = request;
    let scheduler = get_scheduler_from_string(scheduler_name.as_str())?;
    let cards = scheduler.get_leeches(db).await?;
    let card_responses = cards
        .into_iter()
        .map(|card| CardResponse::new(&card))
        .collect::<Vec<_>>();
    Ok(card_responses)
}

// NOTE: Anki also has the option to "Reset reviews and lapses" when forgetting a card. This is
// never used since past reviews are always needed to keep track of how many cards were reviewed in
// the past on any given day.
pub async fn forget_card(
    db: &SqlitePool,
    card_id: CardId,
    now: DateTime<Utc>,
    log: bool,
) -> Result<ForgetCardResponse, Error> {
    let before_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let mut card = before_card.clone();
    card.stability = 0.0;
    card.difficulty = 0.0;
    card.due = now;
    card.state = NEW_CARD_STATE;
    card.updated_at = now;
    // Attribute the marker to whichever scheduler last graded this card, so that anything
    // resolving a scheduler from the newest row still finds the right one. A card that has never
    // been reviewed falls back to the default scheduler.
    let scheduler_name: Option<String> = sqlx::query_scalar(
        r"SELECT scheduler_name FROM review_log WHERE card_id = ? AND kind = ?
          ORDER BY reviewed_at DESC, id DESC LIMIT 1",
    )
    .bind(card_id)
    .bind(ReviewLogKind::Review)
    .fetch_optional(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let scheduler_name = scheduler_name.unwrap_or_else(|| get_default_scheduler_name().to_string());

    // The marker is written before the card is updated. Neither statement is transactional (the
    // same is true of `rate_card`), and this order fails safe: an interrupted forget leaves a
    // marker for a card that was not reset, so replay treats the card as newer than it is, rather
    // than resetting a card whose reset leaves no trace and is later undone by a reschedule.
    //
    // `rating`, `scheduled_time`, `recall_duration` and `rate_duration` are NULL: a forget is not
    // a review. `previous_state` is the state the card was in immediately before the forget.
    let review_log_id: i64 = sqlx::query_scalar(
        r"INSERT INTO review_log
            (card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
             recall_duration, rate_duration, previous_state, tag_id, custom_data)
          VALUES (?, ?, ?, NULL, ?, NULL, NULL, NULL, ?, NULL, ?) RETURNING id",
    )
    .bind(card_id)
    .bind(now.timestamp())
    .bind(ReviewLogKind::Forget)
    .bind(&scheduler_name)
    .bind(before_card.state)
    .bind(Value::Object(Map::new()))
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    sqlx::query("UPDATE card SET stability = ?, difficulty = ?, due = ?, state = ?, updated_at = ? WHERE id = ?")
        .bind(card.stability)
        .bind(card.difficulty)
        .bind(card.due.timestamp())
        .bind(card.state)
        .bind(card.updated_at.timestamp())
        .bind(card_id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    if log {
        let payload = UpdateCardPayload {
            card_id,
            order: None,
            back_type: None,
            due: Some(Transition {
                before: before_card.due,
                after: card.due,
            }),
            stability: Some(Transition {
                before: before_card.stability,
                after: card.stability,
            }),
            difficulty: Some(Transition {
                before: before_card.difficulty,
                after: card.difficulty,
            }),
            desired_retention: None,
            special_state: None,
            state: Some(Transition {
                before: before_card.state,
                after: card.state,
            }),
            custom_data: None,
        };
        let event_ids = insert_events(
            db,
            &[(
                EventType::ForgetCard,
                to_value(&ForgetCardPayload {
                    review_log_id,
                    card: payload,
                })
                .unwrap(),
            )],
            now,
            None,
        )
        .await?;
        return Ok(ForgetCardResponse {
            card: CardResponse::new(&card),
            event_id: Some(*event_ids.first().unwrap()),
        });
    }
    Ok(ForgetCardResponse {
        card: CardResponse::new(&card),
        event_id: None,
    })
}

pub async fn unbury_cards(
    db: &SqlitePool,
    query: Option<&str>,
    now: DateTime<Utc>,
    log: bool,
) -> Result<(), Error> {
    let card_id_filter = if let Some(q) = query {
        let evaluator = Evaluator::new(q);
        let card_ids = evaluator.get_card_ids(db).await?;
        if card_ids.is_empty() {
            return Ok(());
        }
        let ids_str = card_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(" AND id IN ({})", ids_str)
    } else {
        String::new()
    };
    if log {
        let select_query = format!(
            "SELECT * FROM card WHERE special_state IN (?, ?, ?){}",
            card_id_filter
        );
        let cards_to_unbury: Vec<Card> = sqlx::query_as(&select_query)
            .bind(SpecialState::UserBuried)
            .bind(SpecialState::SchedulerBuried)
            .bind(SpecialState::BuriedUntilLaterToday)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let update_query = format!(
            "UPDATE card SET special_state = NULL, updated_at = ? WHERE special_state IN (?, ?, ?){}",
            card_id_filter
        );
        sqlx::query(&update_query)
            .bind(now.timestamp())
            .bind(SpecialState::UserBuried)
            .bind(SpecialState::SchedulerBuried)
            .bind(SpecialState::BuriedUntilLaterToday)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if !cards_to_unbury.is_empty() {
            let payloads: Vec<UpdateCardPayload> = cards_to_unbury
                .iter()
                .map(|card| UpdateCardPayload {
                    card_id: card.id,
                    order: None,
                    back_type: None,
                    due: None,
                    stability: None,
                    difficulty: None,
                    desired_retention: None,
                    special_state: Some(Transition {
                        before: card.special_state,
                        after: None,
                    }),
                    state: None,
                    custom_data: None,
                })
                .collect();
            insert_events(
                db,
                &[(EventType::UnburyCards, to_value(&payloads).unwrap())],
                now,
                None,
            )
            .await?;
        }
    } else {
        let update_query = format!(
            "UPDATE card SET special_state = NULL, updated_at = ? WHERE special_state IN (?, ?, ?){}",
            card_id_filter
        );
        sqlx::query(&update_query)
            .bind(now.timestamp())
            .bind(SpecialState::UserBuried)
            .bind(SpecialState::SchedulerBuried)
            .bind(SpecialState::BuriedUntilLaterToday)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(())
}

pub async fn create_card_tags(
    db: &SqlitePool,
    card_tag_entries: &[(CardId, TagId)],
) -> Result<(), Error> {
    execute_batched_query(
        db,
        card_tag_entries,
        MAX_ROWS_IN_QUERY,
        async |db, chunk| {
            let query_str = format!(
                "INSERT INTO card_tag (card_id, tag_id) VALUES {}",
                placeholders_2d(chunk.len(), 2)
            );
            let mut query = sqlx::query(query_str.as_str());
            for (card_id, tag_id) in chunk {
                query = query.bind(card_id);
                query = query.bind(tag_id);
            }
            query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            Ok(())
        },
    )
    .await
}

pub async fn delete_card_tags(
    db: &SqlitePool,
    delete_card_tag_entries: &[(CardId, TagId)],
) -> Result<(), Error> {
    execute_batched_query(
        db,
        delete_card_tag_entries,
        MAX_ROWS_IN_QUERY,
        async |db, chunk| {
            let query_str = format!(
                "DELETE FROM card_tag WHERE (card_id, tag_id) IN ({})",
                placeholders_2d(chunk.len(), 2)
            );
            let mut query = sqlx::query(query_str.as_str());
            for (card_id, tag_id) in chunk {
                query = query.bind(card_id);
                query = query.bind(tag_id);
            }
            query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            Ok(())
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::model::SpecialState;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;

    #[sqlx::test]
    async fn test_update_card(pool: SqlitePool) -> () {
        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data {{1}}".to_string(),
            keywords: vec![],
            tags: vec!["test filtered tag".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone()],
        };
        let create_notes_res =
            create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Get card id
        let cards = get_cards(&pool, create_notes_response.notes[0].id)
            .await
            .unwrap();
        let card_id = cards[0].id;

        // Update card
        let update_card_request = UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: Some(Some(SpecialStateUpdate::Suspended)),
            due: None,
        };
        let update_card_response =
            update_cards(&pool, update_card_request, Utc::now(), false).await;
        assert!(update_card_response.is_ok());

        // Verify card is updated
        let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(card.special_state, Some(SpecialState::Suspended));
    }

    pub(super) async fn create_single_card(pool: &SqlitePool) -> CardId {
        let parser = create_parser_helper(pool, "markdown").await;
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Test {{1}}".to_string(),
                keywords: vec![],
                tags: vec![],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        };
        let created = create_notes(pool, request, Utc::now(), &get_all_parsers(), false)
            .await
            .unwrap();
        let cards = get_cards(pool, created.notes[0].id).await.unwrap();
        cards[0].id
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_sets_special_state(pool: SqlitePool) -> () {
        let card_id = create_single_card(&pool).await;
        let now = Utc::now();

        let result = update_cards(
            &pool,
            UpdateCardsRequest {
                selector: CardsSelector::Ids(vec![card_id]),
                desired_retention: None,
                special_state: Some(Some(SpecialStateUpdate::BuriedUntilLaterToday)),
                due: Some(now),
            },
            now,
            false,
        )
        .await;
        assert!(result.is_ok());

        let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            card.special_state,
            Some(SpecialState::BuriedUntilLaterToday)
        );
        assert_eq!(card.due.timestamp(), now.timestamp());
    }

    #[sqlx::test]
    async fn test_rebury_updates_due_timestamp(pool: SqlitePool) -> () {
        let card_id = create_single_card(&pool).await;
        let now = Utc::now();
        let t1 = now;
        let t2 = now + chrono::Duration::seconds(5);

        // Bury at t1
        update_cards(
            &pool,
            UpdateCardsRequest {
                selector: CardsSelector::Ids(vec![card_id]),
                desired_retention: None,
                special_state: Some(Some(SpecialStateUpdate::BuriedUntilLaterToday)),
                due: Some(t1),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Re-bury at t2 (should push to back of queue)
        update_cards(
            &pool,
            UpdateCardsRequest {
                selector: CardsSelector::Ids(vec![card_id]),
                desired_retention: None,
                special_state: Some(Some(SpecialStateUpdate::BuriedUntilLaterToday)),
                due: Some(t2),
            },
            now,
            false,
        )
        .await
        .unwrap();

        let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            card.special_state,
            Some(SpecialState::BuriedUntilLaterToday)
        );
        assert_eq!(card.due.timestamp(), t2.timestamp());
    }

    #[sqlx::test]
    async fn test_unbury_clears_buried_until_later_today(pool: SqlitePool) -> () {
        let card_id = create_single_card(&pool).await;
        let now = Utc::now();

        // Bury as BuriedUntilLaterToday
        update_cards(
            &pool,
            UpdateCardsRequest {
                selector: CardsSelector::Ids(vec![card_id]),
                desired_retention: None,
                special_state: Some(Some(SpecialStateUpdate::BuriedUntilLaterToday)),
                due: Some(now),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Unbury
        unbury_cards(&pool, None, now, false).await.unwrap();

        let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            card.special_state, None,
            "unbury_cards should clear BuriedUntilLaterToday"
        );
    }
}

#[cfg(test)]
mod forget_tests {
    use chrono::Duration;
    use serde_json::Map;

    use super::tests::create_single_card;
    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::review::submit_study_action;
    use crate::model::ReviewLog;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::review::RatingSubmission;
    use crate::schema::review::StudyAction;
    use crate::schema::review::SubmitStudyActionRequest;

    async fn rate(pool: &SqlitePool, card_id: CardId, rating: u32, at: DateTime<Utc>) {
        submit_study_action(
            pool,
            SubmitStudyActionRequest {
                scheduler_name: "fsrs".to_string(),
                action: StudyAction::Rate(RatingSubmission {
                    card_id,
                    rating,
                    recall_duration: Duration::seconds(5),
                    rate_duration: Duration::seconds(2),
                    tag_id: None,
                }),
            },
            at,
        )
        .await
        .unwrap();
    }

    async fn fetch_card(pool: &SqlitePool, card_id: CardId) -> Card {
        sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn forget_card_writes_marker_row(pool: SqlitePool) {
        let card_id = create_single_card(&pool).await;
        let now = Utc::now();
        rate(&pool, card_id, 3, now - Duration::days(2)).await;
        let before = fetch_card(&pool, card_id).await;
        assert!(before.stability > 0.0, "precondition: card was rated");

        forget_card(&pool, card_id, now, true).await.unwrap();

        let markers: Vec<ReviewLog> =
            sqlx::query_as(r"SELECT * FROM review_log WHERE card_id = ? AND kind = ?")
                .bind(card_id)
                .bind(ReviewLogKind::Forget)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(markers.len(), 1, "exactly one marker row per forget");
        let marker = &markers[0];
        assert_eq!(marker.rating, None, "a forget is not a graded review");
        assert_eq!(marker.recall_duration, None);
        assert_eq!(marker.rate_duration, None);
        assert_eq!(marker.scheduled_time, None);
        assert_eq!(marker.tag_id, None);
        assert_eq!(
            marker.previous_state, before.state,
            "the marker records the state the card was in before the forget"
        );
        assert_eq!(
            marker.scheduler_name, "fsrs",
            "the marker is attributed to the scheduler that last graded the card"
        );
    }

    #[sqlx::test]
    async fn forget_card_on_never_reviewed_card_writes_marker(pool: SqlitePool) {
        let card_id = create_single_card(&pool).await;
        forget_card(&pool, card_id, Utc::now(), true).await.unwrap();

        let (count, scheduler_name): (i64, String) = sqlx::query_as(
            r"SELECT COUNT(*), MAX(scheduler_name) FROM review_log WHERE card_id = ? AND kind = ?",
        )
        .bind(card_id)
        .bind(ReviewLogKind::Forget)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            scheduler_name, "fsrs",
            "with no review to attribute it to, the marker falls back to the default scheduler"
        );
    }

    /// Guards every other `reschedule` assertion in this module. `reschedule`'s `UPDATE` once
    /// bound its parameters out of order, so it matched zero rows and silently did nothing —
    /// which made "forget survives reschedule" pass for entirely the wrong reason.
    #[sqlx::test]
    async fn reschedule_actually_updates_the_card(pool: SqlitePool) {
        let card_id = create_single_card(&pool).await;
        let now = Utc::now();
        rate(&pool, card_id, 3, now - Duration::days(3)).await;
        rate(&pool, card_id, 3, now - Duration::days(2)).await;

        // Move the card somewhere a correct reschedule will not leave it.
        let bogus_due = (now + Duration::days(3650)).timestamp();
        sqlx::query(r"UPDATE card SET due = ? WHERE id = ?")
            .bind(bogus_due)
            .bind(card_id)
            .execute(&pool)
            .await
            .unwrap();

        submit_study_action(
            &pool,
            SubmitStudyActionRequest {
                scheduler_name: "fsrs".to_string(),
                action: StudyAction::Reschedule,
            },
            now,
        )
        .await
        .unwrap();

        let after = fetch_card(&pool, card_id).await;
        assert_ne!(
            after.due.timestamp(),
            bogus_due,
            "reschedule must actually write to the card row"
        );
    }

    #[sqlx::test]
    async fn forget_survives_reschedule(pool: SqlitePool) {
        let card_id = create_single_card(&pool).await;
        let now = Utc::now();
        rate(&pool, card_id, 3, now - Duration::days(4)).await;
        rate(&pool, card_id, 3, now - Duration::days(3)).await;
        rate(&pool, card_id, 4, now - Duration::days(2)).await;
        assert!(fetch_card(&pool, card_id).await.stability > 0.0);

        forget_card(&pool, card_id, now, true).await.unwrap();

        submit_study_action(
            &pool,
            SubmitStudyActionRequest {
                scheduler_name: "fsrs".to_string(),
                action: StudyAction::Reschedule,
            },
            now,
        )
        .await
        .unwrap();

        let after = fetch_card(&pool, card_id).await;
        assert_eq!(
            after.stability, 0.0,
            "replaying the log must not resurrect pre-forget stability"
        );
        assert_eq!(after.difficulty, 0.0);
        assert_eq!(after.state, NEW_CARD_STATE);
    }

    /// The other path into `reschedule`. Note the card must be rated *after* the forget: that
    /// block is guarded on `state != NEW_CARD_STATE`, so a just-forgotten card never reaches it
    /// and asserting on one would prove nothing.
    #[sqlx::test]
    async fn forget_survives_desired_retention_change(pool: SqlitePool) {
        let now = Utc::now();
        let (forgotten_id, fresh_id) = create_two_cards(&pool).await;

        rate(&pool, forgotten_id, 3, now - Duration::days(30)).await;
        rate(&pool, forgotten_id, 4, now - Duration::days(20)).await;
        forget_card(&pool, forgotten_id, now - Duration::days(10), true)
            .await
            .unwrap();
        rate(&pool, forgotten_id, 3, now - Duration::days(5)).await;

        // The control: the same single post-forget review, with no history behind it.
        rate(&pool, fresh_id, 3, now - Duration::days(5)).await;

        update_cards(
            &pool,
            UpdateCardsRequest {
                selector: CardsSelector::Ids(vec![forgotten_id, fresh_id]),
                desired_retention: Some(0.85),
                special_state: None,
                due: None,
            },
            now,
            false,
        )
        .await
        .unwrap();

        let forgotten = fetch_card(&pool, forgotten_id).await;
        let fresh = fetch_card(&pool, fresh_id).await;
        assert_eq!(
            forgotten.stability, fresh.stability,
            "replay after a desired-retention change must not resurrect pre-forget history"
        );
        assert_eq!(forgotten.difficulty, fresh.difficulty);
    }

    /// Two cards on separate notes, sharing one parser (parser names must be unique).
    async fn create_two_cards(pool: &SqlitePool) -> (CardId, CardId) {
        let parser = create_parser_helper(pool, "markdown").await;
        let note = |data: &str| CreateNoteRequest {
            data: data.to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let created = create_notes(
            pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![note("First {{1}}"), note("Second {{1}}")],
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();
        let first = get_cards(pool, created.notes[0].id).await.unwrap()[0].id;
        let second = get_cards(pool, created.notes[1].id).await.unwrap()[0].id;
        (first, second)
    }

    #[sqlx::test]
    async fn rate_after_forget_schedules_as_new(pool: SqlitePool) {
        let now = Utc::now();
        let (forgotten_id, fresh_id) = create_two_cards(&pool).await;

        // A card that was rated, forgotten, then rated again.
        rate(&pool, forgotten_id, 3, now - Duration::days(40)).await;
        rate(&pool, forgotten_id, 3, now - Duration::days(30)).await;
        forget_card(&pool, forgotten_id, now - Duration::days(1), true)
            .await
            .unwrap();
        rate(&pool, forgotten_id, 3, now).await;

        // A card whose only review is that same one.
        rate(&pool, fresh_id, 3, now).await;

        let forgotten = fetch_card(&pool, forgotten_id).await;
        let fresh = fetch_card(&pool, fresh_id).await;
        assert_eq!(
            forgotten.stability, fresh.stability,
            "a review after a forget must be scheduled as a first review, not a lapse"
        );
        assert_eq!(forgotten.difficulty, fresh.difficulty);
        assert_eq!(forgotten.state, fresh.state);
    }

    /// `copy_review_logs_on` backs the `inh:` cloze setting, which promises the new card inherits
    /// the source's "full review history". A forget is part of that history: dropping it would
    /// give the inheriting card the memory state the source would have had if it had never been
    /// forgotten.
    #[sqlx::test]
    async fn inherited_review_logs_carry_forget_markers(pool: SqlitePool) {
        use crate::api::note::copy_review_logs_on;

        let now = Utc::now();
        let (src_id, dst_id) = create_two_cards(&pool).await;
        rate(&pool, src_id, 3, now - Duration::days(30)).await;
        rate(&pool, src_id, 4, now - Duration::days(20)).await;
        forget_card(&pool, src_id, now - Duration::days(10), true)
            .await
            .unwrap();
        rate(&pool, src_id, 3, now - Duration::days(5)).await;

        let mut conn = pool.acquire().await.unwrap();
        copy_review_logs_on(&mut conn, src_id, &[dst_id])
            .await
            .unwrap();
        drop(conn);

        let copied = crate::api::fetch_review_logs_for_replay(&pool, dst_id)
            .await
            .unwrap();
        assert_eq!(copied.len(), 4, "all four rows are inherited");
        assert_eq!(
            copied.iter().filter(|rl| !rl.is_review()).count(),
            1,
            "the forget marker is inherited too"
        );

        // The inherited history must replay to the same place as the source's.
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        let src_logs = crate::api::fetch_review_logs_for_replay(&pool, src_id)
            .await
            .unwrap();
        let from_src = scheduler.compute_memory_state(src_logs).unwrap();
        let from_dst = scheduler.compute_memory_state(copied).unwrap();
        assert_eq!(from_dst.stability, from_src.stability);
        assert_eq!(from_dst.difficulty, from_src.difficulty);
    }

    #[sqlx::test]
    async fn forget_does_not_count_as_a_study(pool: SqlitePool) {
        use crate::api::statistics::get_statistics;
        use crate::schema::review::StatisticsRequest;

        let card_id = create_single_card(&pool).await;
        let now = Utc::now();
        rate(&pool, card_id, 3, now).await;

        let request = || StatisticsRequest {
            scheduler_name: "fsrs".to_string(),
            date: now,
        };
        let before = get_statistics(&pool, request()).await.unwrap();
        forget_card(&pool, card_id, now, true).await.unwrap();
        let after = get_statistics(&pool, request()).await.unwrap();

        assert_eq!(
            before.cards_studied_count, after.cards_studied_count,
            "forgetting a card is not studying it"
        );
        assert_eq!(before.recall_duration, after.recall_duration);
        assert_eq!(before.rate_duration, after.rate_duration);
    }
}
