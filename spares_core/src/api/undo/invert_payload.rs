use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::Error;
use crate::api::undo::payloads::CreateNotesPayload;
use crate::api::undo::payloads::CreateParserPayload;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::DeleteNotesPayload;
use crate::api::undo::payloads::DeleteParserPayload;
use crate::api::undo::payloads::DeleteTagPayload;
use crate::api::undo::payloads::RateCardPayload;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::api::undo::payloads::UpdateNotePayload;
use crate::api::undo::payloads::UpdateNotesPayload;
use crate::api::undo::payloads::UpdateParserPayload;
use crate::api::undo::payloads::UpdateTagPayload;
use crate::model::Event;
use crate::model::EventType;

pub async fn create_undo_event(
    db: &SqlitePool,
    event: &Event,
    at: chrono::DateTime<Utc>,
) -> Result<Event, Error> {
    let undo_event_type = match event.kind {
        EventType::CreateParser => EventType::DeleteParser,
        EventType::UpdateParser => EventType::UpdateParser,
        EventType::DeleteParser => EventType::CreateParser,
        EventType::CreateTag => EventType::DeleteTag,
        EventType::UpdateTag => EventType::UpdateTag,
        EventType::DeleteTag => EventType::CreateTag,
        EventType::CreateNotes => EventType::DeleteNotes,
        EventType::UpdateNotes => EventType::UpdateNotes,
        EventType::DeleteNotes => EventType::CreateNotes,
        EventType::UpdateCards
        | EventType::RateCard
        | EventType::ForgetCard
        | EventType::AdvanceCards
        | EventType::PostponeCards
        | EventType::BuryCards
        | EventType::UnburyCards => EventType::UpdateCards,
    };

    let undo_payload = create_undo_payload(db, event).await?;

    // Insert the undo event
    let id: i64 = sqlx::query_scalar(
        r"INSERT INTO event (kind, created_at, version, group_id, payload) VALUES (?, ?, ?, ?, ?) RETURNING id"
    )
    .bind(undo_event_type)
    .bind(at.timestamp())
    .bind(event.version) // Use same version as original event
    .bind(event.group_id) // Preserve group_id if undoing a grouped event
    .bind(&undo_payload)
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    Ok(Event {
        id,
        kind: undo_event_type,
        created_at: at,
        version: event.version,
        group_id: event.group_id,
        payload: undo_payload,
    })
}

#[expect(clippy::too_many_lines)]
async fn create_undo_payload(db: &SqlitePool, event: &Event) -> Result<Value, Error> {
    match event.kind {
        EventType::CreateParser => {
            let payload: CreateParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            // To undo CreateParser, we need DeleteParser with the parser and note_ids
            let note_ids: Vec<i64> = sqlx::query_scalar(r"SELECT id FROM note WHERE parser_id = ?")
                .bind(payload.id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let delete_payload = DeleteParserPayload {
                id: payload.id,
                name: payload.name,
                note_ids,
            };
            Ok(serde_json::to_value(delete_payload).unwrap())
        }
        EventType::UpdateParser => {
            let payload: UpdateParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            // Swap old and new
            let undo_payload = UpdateParserPayload {
                id: payload.id,
                name: payload.name.map(|t| t.swap()),
            };
            Ok(serde_json::to_value(undo_payload).unwrap())
        }
        EventType::DeleteParser => {
            let payload: DeleteParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            // To undo DeleteParser, we create the parser again
            // Note: We don't need note_ids for CreateParser, only for DeleteParser validation
            let create_payload = CreateParserPayload {
                id: payload.id,
                name: payload.name,
            };
            Ok(serde_json::to_value(create_payload).unwrap())
        }
        EventType::CreateTag => {
            let payload: CreateTagPayload = serde_json::from_value(event.payload.clone()).unwrap();
            // Capture current note/card associations before deleting the tag
            let note_ids: Vec<i64> = if let Some(id) = payload.id {
                sqlx::query_scalar(r"SELECT note_id FROM note_tag WHERE tag_id = ?")
                    .bind(id)
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?
            } else {
                vec![]
            };
            let card_ids: Vec<i64> = if let Some(id) = payload.id {
                sqlx::query_scalar(r"SELECT card_id FROM card_tag WHERE tag_id = ?")
                    .bind(id)
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?
            } else {
                vec![]
            };
            // To undo CreateTag, we need DeleteTag with the tag info
            let delete_payload = DeleteTagPayload {
                id: payload.id,
                name: payload.name,
                description: payload.description,
                query: payload.query,
                auto_delete: payload.auto_delete,
                note_ids,
                card_ids,
            };
            Ok(serde_json::to_value(delete_payload).unwrap())
        }
        EventType::UpdateTag => {
            let payload: UpdateTagPayload = serde_json::from_value(event.payload.clone()).unwrap();
            // Swap old and new for each field
            let undo_payload = UpdateTagPayload {
                id: payload.id,
                name: payload.name.map(|t| t.swap()),
                description: payload.description.map(|t| t.swap()),
                query: payload.query.map(|t| t.swap()),
                auto_delete: payload.auto_delete.map(|t| t.swap()),
            };
            Ok(serde_json::to_value(undo_payload).unwrap())
        }
        EventType::DeleteTag => {
            let payload: DeleteTagPayload = serde_json::from_value(event.payload.clone()).unwrap();
            // To undo DeleteTag, we create the tag again and restore its associations
            let create_payload = CreateTagPayload {
                id: payload.id,
                name: payload.name,
                description: payload.description,
                query: payload.query,
                auto_delete: payload.auto_delete,
                note_ids: payload.note_ids,
                card_ids: payload.card_ids,
            };
            Ok(serde_json::to_value(create_payload).unwrap())
        }
        EventType::CreateNotes => {
            let payload: CreateNotesPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            // To undo CreateNotes, we need DeleteNotes with the same full snapshots
            let delete_payload = DeleteNotesPayload {
                notes: payload.notes,
            };
            Ok(serde_json::to_value(delete_payload).unwrap())
        }
        EventType::UpdateNotes => {
            let payload: UpdateNotesPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            // Swap before/after for each field in each note
            let undo_payload = UpdateNotesPayload {
                notes: payload
                    .notes
                    .into_iter()
                    .map(|p| UpdateNotePayload {
                        id: p.id,
                        data: p.data.map(|t| t.swap()),
                        parser_id: p.parser_id.map(|t| t.swap()),
                        keywords: p.keywords.map(|t| t.swap()),
                        tags: p.tags.map(|t| t.swap()),
                        custom_data: p.custom_data.map(|t| t.swap()),
                        cards: p.cards.map(|t| t.swap()),
                    })
                    .collect(),
            };
            Ok(serde_json::to_value(undo_payload).unwrap())
        }
        EventType::DeleteNotes => {
            let payload: DeleteNotesPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            // To undo DeleteNotes, we recreate the notes from the saved snapshots
            let create_payload = CreateNotesPayload {
                notes: payload.notes,
            };
            Ok(serde_json::to_value(create_payload).unwrap())
        }
        EventType::RateCard => {
            let payload: RateCardPayload = serde_json::from_value(event.payload.clone()).unwrap();
            sqlx::query(r"DELETE FROM review_log WHERE id = ?")
                .bind(payload.review_log_id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let p = payload.card;
            let undo_payloads = vec![UpdateCardPayload {
                card_id: p.card_id,
                order: p.order.map(|t| t.swap()),
                back_type: p.back_type.map(|t| t.swap()),
                due: p.due.map(|t| t.swap()),
                stability: p.stability.map(|t| t.swap()),
                difficulty: p.difficulty.map(|t| t.swap()),
                desired_retention: p.desired_retention.map(|t| t.swap()),
                special_state: p.special_state.map(|t| t.swap()),
                state: p.state.map(|t| t.swap()),
                custom_data: p.custom_data.map(|t| t.swap()),
            }];
            Ok(serde_json::to_value(undo_payloads).unwrap())
        }
        EventType::UpdateCards
        | EventType::ForgetCard
        | EventType::AdvanceCards
        | EventType::PostponeCards
        | EventType::BuryCards
        | EventType::UnburyCards => {
            let payloads: Vec<UpdateCardPayload> =
                serde_json::from_value(event.payload.clone()).unwrap();
            let undo_payloads: Vec<UpdateCardPayload> = payloads
                .into_iter()
                .map(|p| UpdateCardPayload {
                    card_id: p.card_id,
                    order: p.order.map(|t| t.swap()),
                    back_type: p.back_type.map(|t| t.swap()),
                    due: p.due.map(|t| t.swap()),
                    stability: p.stability.map(|t| t.swap()),
                    difficulty: p.difficulty.map(|t| t.swap()),
                    desired_retention: p.desired_retention.map(|t| t.swap()),
                    special_state: p.special_state.map(|t| t.swap()),
                    state: p.state.map(|t| t.swap()),
                    custom_data: p.custom_data.map(|t| t.swap()),
                })
                .collect();
            Ok(serde_json::to_value(undo_payloads).unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::undo::insert_events;

    #[sqlx::test]
    async fn create_undo_event_create_parser_produces_delete_parser(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "to_undo").await;
        let at = Utc::now();
        let payload = json!({"id": parser.id, "name": parser.name});
        let ids = insert_events(&pool, &[(EventType::CreateParser, payload)], at, None)
            .await
            .unwrap();
        let event: Event = sqlx::query_as("SELECT * FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();

        let undo = create_undo_event(&pool, &event, at).await.unwrap();
        assert_eq!(undo.kind, EventType::DeleteParser);
        let delete_payload: DeleteParserPayload = serde_json::from_value(undo.payload).unwrap();
        assert_eq!(delete_payload.id, Some(parser.id));
        assert_eq!(delete_payload.name, "to_undo");
    }

    #[sqlx::test]
    async fn create_undo_event_create_parser_collects_note_ids(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "with_notes").await;
        let ts = Utc::now().timestamp();
        let custom_data = json!({}).to_string();
        let id1: i64 = sqlx::query_scalar(
        r"INSERT INTO note (data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind("n1")
    .bind(ts)
    .bind(ts)
    .bind(parser.id)
    .bind(&custom_data)
    .fetch_one(&pool)
    .await
    .unwrap();
        let id2: i64 = sqlx::query_scalar(
        r"INSERT INTO note (data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind("n2")
    .bind(ts)
    .bind(ts)
    .bind(parser.id)
    .bind(&custom_data)
    .fetch_one(&pool)
    .await
    .unwrap();

        let at = Utc::now();
        let payload = json!({"id": parser.id, "name": parser.name});
        let ids = insert_events(&pool, &[(EventType::CreateParser, payload)], at, None)
            .await
            .unwrap();
        let event: Event = sqlx::query_as("SELECT * FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();

        let undo = create_undo_event(&pool, &event, at).await.unwrap();
        let delete_payload: DeleteParserPayload = serde_json::from_value(undo.payload).unwrap();
        assert_eq!(delete_payload.note_ids.len(), 2);
        assert!(delete_payload.note_ids.contains(&id1));
        assert!(delete_payload.note_ids.contains(&id2));
    }

    #[sqlx::test]
    async fn create_undo_event_update_parser_swaps_before_after(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "old_name").await;
        let at = Utc::now();
        let payload = json!({
            "id": parser.id,
            "name": {"b": "old_name", "a": "new_name"}
        });
        let ids = insert_events(&pool, &[(EventType::UpdateParser, payload)], at, None)
            .await
            .unwrap();
        let event: Event = sqlx::query_as("SELECT * FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();

        let undo = create_undo_event(&pool, &event, at).await.unwrap();
        assert_eq!(undo.kind, EventType::UpdateParser);
        let name = undo.payload.get("name").unwrap();
        assert_eq!(name.get("b").unwrap(), "new_name");
        assert_eq!(name.get("a").unwrap(), "old_name");
    }

    #[sqlx::test]
    async fn create_undo_event_delete_parser_produces_create_parser(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "deleted").await;
        let payload = json!({
            "id": parser.id,
            "name": parser.name,
            "note_ids": []
        });
        let ids = insert_events(
            &pool,
            &[(EventType::DeleteParser, payload)],
            Utc::now(),
            None,
        )
        .await
        .unwrap();

        let event: Event = sqlx::query_as("SELECT * FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        let at = Utc::now();
        let undo = create_undo_event(&pool, &event, at).await.unwrap();
        assert_eq!(undo.kind, EventType::CreateParser);
        assert_eq!(
            undo.payload.get("id").and_then(|v| v.as_i64()),
            Some(parser.id)
        );
        assert_eq!(
            undo.payload.get("name").and_then(|v| v.as_str()),
            Some("deleted")
        );
    }

    #[sqlx::test]
    async fn create_undo_event_create_tag_produces_delete_tag(pool: SqlitePool) {
        use crate::api::tag::create_tag;
        use crate::schema::tag::CreateTagRequest;

        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "to_undo".to_string(),
                description: "desc".to_string(),
                query: None,
                auto_delete: false,
            },
            false,
        )
        .await
        .unwrap();
        let at = Utc::now();
        let payload = json!({"id": tag.id, "name": tag.name, "description": tag.description, "query": null, "auto_delete": false});
        let ids = insert_events(&pool, &[(EventType::CreateTag, payload)], at, None)
            .await
            .unwrap();
        let event: Event = sqlx::query_as("SELECT * FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();

        let undo = create_undo_event(&pool, &event, at).await.unwrap();
        assert_eq!(undo.kind, EventType::DeleteTag);
        assert_eq!(
            undo.payload.get("id").and_then(|v| v.as_i64()),
            Some(tag.id)
        );
        assert_eq!(
            undo.payload.get("name").and_then(|v| v.as_str()),
            Some("to_undo")
        );
    }

    #[sqlx::test]
    async fn create_undo_event_update_tag_swaps_before_after(pool: SqlitePool) {
        use crate::api::tag::create_tag;
        use crate::schema::tag::CreateTagRequest;

        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "old_name".to_string(),
                description: "desc".to_string(),
                query: None,
                auto_delete: false,
            },
            false,
        )
        .await
        .unwrap();
        let at = Utc::now();
        let payload = json!({
            "id": tag.id,
            "name": {"b": "old_name", "a": "new_name"}
        });
        let ids = insert_events(&pool, &[(EventType::UpdateTag, payload)], at, None)
            .await
            .unwrap();
        let event: Event = sqlx::query_as("SELECT * FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();

        let undo = create_undo_event(&pool, &event, at).await.unwrap();
        assert_eq!(undo.kind, EventType::UpdateTag);
        let name = undo.payload.get("name").unwrap();
        assert_eq!(name.get("b").unwrap(), "new_name");
        assert_eq!(name.get("a").unwrap(), "old_name");
    }

    #[sqlx::test]
    async fn create_undo_event_delete_tag_produces_create_tag(pool: SqlitePool) {
        use crate::api::tag::create_tag;
        use crate::schema::tag::CreateTagRequest;

        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "deleted_tag".to_string(),
                description: "desc".to_string(),
                query: None,
                auto_delete: false,
            },
            false,
        )
        .await
        .unwrap();
        let payload = json!({
            "id": tag.id,
            "name": tag.name,
            "description": tag.description,
            "query": null,
            "auto_delete": false
        });
        let ids = insert_events(&pool, &[(EventType::DeleteTag, payload)], Utc::now(), None)
            .await
            .unwrap();
        let event: Event = sqlx::query_as("SELECT * FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();

        let undo = create_undo_event(&pool, &event, Utc::now()).await.unwrap();
        assert_eq!(undo.kind, EventType::CreateTag);
        assert_eq!(
            undo.payload.get("id").and_then(|v| v.as_i64()),
            Some(tag.id)
        );
        assert_eq!(
            undo.payload.get("name").and_then(|v| v.as_str()),
            Some("deleted_tag")
        );
    }
}
