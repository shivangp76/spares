//! End-to-end undo tests: create/update/delete parser flows, with and without groups,
//! including dependency error paths. Also covers apply_event for parser events (via undo flow).

use crate::api::parser::tests::create_parser_helper;
use crate::api::parser::{create_parser, create_parser_event, delete_parser, update_parser};
use crate::api::undo::payloads::CreateParserPayload;
use crate::api::undo::{create_event_group, insert_events, undo_event};
use crate::model::EventType;
use crate::schema::undo::UndoEventRequest;
use chrono::Utc;
use serde_json::json;
use sqlx::SqlitePool;

#[sqlx::test]
async fn e2e_undo_create_parser_restores_state(pool: SqlitePool) {
    let p = create_parser(&pool, crate::schema::parser::CreateParserRequest { name: "e2e".to_string() }, true)
        .await
        .unwrap();
    let id = p.id;

    let res = undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();
    assert!(res.is_some());
    assert_eq!(res.unwrap().undone_event_ids.len(), 1);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "undo CreateParser must remove parser");
}

#[sqlx::test]
async fn e2e_undo_delete_parser_restores_parser(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "to_delete_then_undo").await;
    delete_parser(&pool, p.id, true).await.unwrap();

    let res = undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();
    assert!(res.is_some());

    let name: String = sqlx::query_scalar("SELECT name FROM parser WHERE id = ?")
        .bind(p.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "to_delete_then_undo");
}

#[sqlx::test]
async fn e2e_undo_update_parser_restores_previous_name(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "original").await;
    update_parser(
        &pool,
        crate::schema::parser::UpdateParserRequest {
            name: Some("updated".to_string()),
        },
        p.id,
        true,
    )
    .await
    .unwrap();

    let _ = undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let name: String = sqlx::query_scalar("SELECT name FROM parser WHERE id = ?")
        .bind(p.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "original");
}

#[sqlx::test]
async fn e2e_undo_group_undoes_all_events_in_group(pool: SqlitePool) {
    let at = Utc::now();
    let p1 = create_parser_event(
        &pool,
        CreateParserPayload {
            id: None,
            name: "g1".to_string(),
        },
        false,
    )
    .await
    .unwrap();
    let p2 = create_parser_event(
        &pool,
        CreateParserPayload {
            id: None,
            name: "g2".to_string(),
        },
        false,
    )
    .await
    .unwrap();
    let p3 = create_parser_event(
        &pool,
        CreateParserPayload {
            id: None,
            name: "g3".to_string(),
        },
        false,
    )
    .await
    .unwrap();
    let events = vec![
        (EventType::CreateParser, json!({"id": p1.id, "name": p1.name})),
        (EventType::CreateParser, json!({"id": p2.id, "name": p2.name})),
        (EventType::CreateParser, json!({"id": p3.id, "name": p3.name})),
    ];
    let group_id = create_event_group(&pool, events, at).await.unwrap();

    let rows: Vec<(i64,)> = sqlx::query_as("SELECT id FROM event WHERE group_id = ? ORDER BY id")
        .bind(group_id)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);

    let _ = undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(group_id),
            undo_group: true,
        },
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parser").fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0, "undo group of 3 CreateParser must remove all 3 parsers");
}

#[sqlx::test]
async fn e2e_undo_delete_parser_with_notes_fails_with_dependency_error(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "with_notes").await;
    let ts = Utc::now().timestamp();
    let custom_data = json!({}).to_string();
    let note_id: i64 = sqlx::query_scalar(
        r"INSERT INTO note (data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind("n1")
    .bind(ts)
    .bind(ts)
    .bind(p.id)
    .bind(&custom_data)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Insert a DeleteParser event without actually deleting (simulates log entry). Undoing it would
    // create the parser again, but notes still depend on it, so validation must fail.
    let payload = json!({
        "id": p.id,
        "name": p.name,
        "note_ids": [note_id]
    });
    let ids = insert_events(
        &pool,
        &[(EventType::DeleteParser, payload)],
        Utc::now(),
        None,
    )
    .await
    .unwrap();
    let event_id = ids[0];

    let res = undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(event_id),
            undo_group: false,
        },
    )
    .await;
    assert!(res.is_err());
    let err_msg = format!("{:?}", res.unwrap_err());
    assert!(
        err_msg.contains("notes still depend") || err_msg.contains("Cannot undo DeleteParser"),
        "expected dependency error: {}",
        err_msg
    );
}

#[sqlx::test]
async fn e2e_undo_single_event_in_group_undoes_only_that_event_when_undo_group_false(pool: SqlitePool) {
    let at = Utc::now();
    let p1 = create_parser_event(
        &pool,
        CreateParserPayload {
            id: None,
            name: "s1".to_string(),
        },
        false,
    )
    .await
    .unwrap();
    let p2 = create_parser_event(
        &pool,
        CreateParserPayload {
            id: None,
            name: "s2".to_string(),
        },
        false,
    )
    .await
    .unwrap();
    let events = vec![
        (EventType::CreateParser, json!({"id": p1.id, "name": p1.name})),
        (EventType::CreateParser, json!({"id": p2.id, "name": p2.name})),
    ];
    let group_id = create_event_group(&pool, events, at).await.unwrap();
    let second_id: i64 = sqlx::query_scalar("SELECT id FROM event WHERE group_id = ? ORDER BY id LIMIT 1 OFFSET 1")
        .bind(group_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let _ = undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(second_id),
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parser").fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1, "undo single event without group should remove only one parser");
}
