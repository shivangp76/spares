use crate::{
    ALLOWED_F64_ERROR, Error, LibraryError, SchedulerErrorKind,
    api::{
        execute_batched_query, placeholders_2d,
        undo::{
            insert_events,
            payloads::{Transition, UpdateCardPayload},
        },
    },
    config::read_external_config,
    model::{Card, CardId, EventType, NEW_CARD_STATE, NoteId, ReviewLog, SpecialState, TagId},
    schedulers::get_scheduler_from_string,
    schema::card::{
        CardResponse, CardsSelector, ForgetCardResponse, GetLeechesRequest, SpecialStateUpdate,
        UpdateCardsRequest, UpdateCardsResponse,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Utc};
use serde_json::to_value;
use sqlx::sqlite::SqlitePool;

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

#[expect(clippy::too_many_lines)]
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
        if let Some(Some(SpecialState::UserBuried)) = requested_special_state
            && let Some(special_state) = existing_card.special_state
        {
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
                // Exclude `SpecialState::BuriedUntilLaterToday` since this can be overriden to buried
                SpecialState::BuriedUntilLaterToday => {}
            }
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
            let review_logs: Vec<ReviewLog> = sqlx::query_as(
                r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC",
            )
            .bind(updated_card.id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
            if !review_logs.is_empty() {
                let latest_review_log = review_logs.last().unwrap();
                let scheduler =
                    get_scheduler_from_string(latest_review_log.scheduler_name.as_str())?;

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
        let payload = vec![UpdateCardPayload {
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
        }];
        let event_ids = insert_events(
            db,
            &[(EventType::ForgetCard, to_value(&payload).unwrap())],
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
    execute_batched_query(db, card_tag_entries, async |db, chunk| {
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
    })
    .await
}

pub async fn delete_card_tags(
    db: &SqlitePool,
    delete_card_tag_entries: &[(CardId, TagId)],
) -> Result<(), Error> {
    execute_batched_query(db, delete_card_tag_entries, async |db, chunk| {
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
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{note::create_notes, parser::tests::create_parser_helper},
        model::SpecialState,
        parsers::get_all_parsers,
        schema::note::{CreateNoteRequest, CreateNotesRequest},
    };
    use serde_json::Map;

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

    async fn create_single_card(pool: &SqlitePool) -> CardId {
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
