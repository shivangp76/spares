use crate::{
    Error, LibraryError, SchedulerErrorKind,
    config::read_external_config,
    model::{Card, CardId, NEW_CARD_STATE, NoteId, ReviewLog, SpecialState, TagId},
    schedulers::get_scheduler_from_string,
    schema::card::{
        CardResponse, CardsSelector, GetLeechesRequest, SpecialStateUpdate, UpdateCardRequest,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Utc};
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

pub async fn update_card(
    db: &SqlitePool,
    body: UpdateCardRequest,
    at: DateTime<Utc>,
) -> Result<Vec<CardResponse>, Error> {
    let card_ids = match body.selector {
        CardsSelector::Ids(vec) => vec,
        CardsSelector::Query(query) => {
            let evaluator = Evaluator::new(&query);
            evaluator.get_card_ids(db).await?
        }
    };
    let mut card_responses = Vec::new();
    let requested_special_state = body.special_state.map(|x| {
        x.map(|y| match y {
            SpecialStateUpdate::Suspended => SpecialState::Suspended,
            SpecialStateUpdate::Buried => SpecialState::UserBuried,
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
        if let Some(Some(SpecialState::UserBuried)) = requested_special_state {
            if let Some(special_state) = existing_card.special_state {
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
        }
        let (updated_at,): (i64,) =
        sqlx::query_as(r"UPDATE card SET desired_retention = ?, special_state = ?, updated_at = ? WHERE id = ? RETURNING updated_at")
            .bind(new_desired_retention)
            .bind(new_special_state)
            .bind(at.timestamp())
            .bind(card_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let updated_at = DateTime::from_timestamp(updated_at, 0).unwrap();
        let mut updated_item: Card = existing_card.clone();
        updated_item.desired_retention = new_desired_retention;
        updated_item.special_state = new_special_state;
        updated_item.updated_at = updated_at;
        if let Some(new_desired_retention) = body.desired_retention {
            if (new_desired_retention - existing_card.desired_retention).abs() > f64::EPSILON
                && updated_item.state != NEW_CARD_STATE
            {
                let review_logs: Vec<ReviewLog> = sqlx::query_as(
                    r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC",
                )
                .bind(updated_item.id)
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
                        .reschedule(db, &config, vec![(updated_item.clone(), review_logs)], at)
                        .await?;
                }
            }
        }
        card_responses.push(CardResponse::new(&updated_item));
    }
    Ok(card_responses)
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

pub async fn create_card_tags(
    db: &SqlitePool,
    card_tag_entries: &[(CardId, TagId)],
) -> Result<(), Error> {
    if !card_tag_entries.is_empty() {
        let insert_card_tag_query_str = format!(
            "INSERT INTO card_tag (card_id, tag_id) VALUES {}",
            vec!["(?, ?)"; card_tag_entries.len()].join(", ")
        );
        let mut query = sqlx::query(insert_card_tag_query_str.as_str());
        for (card_id, tag_id) in card_tag_entries {
            query = query.bind(card_id);
            query = query.bind(tag_id);
        }
        let _insert_result = query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(())
}

pub async fn delete_card_tags(
    db: &SqlitePool,
    delete_card_tag_entries: &[(CardId, TagId)],
) -> Result<(), Error> {
    for (card_id, tag_id) in delete_card_tag_entries {
        let _delete_card_tag_result =
            sqlx::query(r"DELETE FROM card_tag WHERE card_id = ? AND tag_id = ?")
                .bind(card_id)
                .bind(tag_id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(())
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
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Get card id
        let cards = get_cards(&pool, create_notes_response.notes[0].id)
            .await
            .unwrap();
        let card_id = cards[0].id;

        // Update card
        let update_card_request = UpdateCardRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: Some(Some(SpecialStateUpdate::Suspended)),
        };
        let update_card_response = update_card(&pool, update_card_request, Utc::now()).await;
        assert!(update_card_response.is_ok());

        // Verify card is updated
        let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(card.special_state, Some(SpecialState::Suspended));
    }
}
