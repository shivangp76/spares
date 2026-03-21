use crate::api::parser::tests::create_parser_helper;
use crate::api::tag::tests::create_tag_helper;
use crate::api::tag::{create_tag, delete_tag, update_tag};
use crate::api::undo::undo_event;
use crate::schema::tag::{TagSelector, UpdateTagRequest};
use crate::schema::undo::UndoEventRequest;
use chrono::Utc;
use serde_json::json;
use sqlx::SqlitePool;

#[sqlx::test]
async fn e2e_undo_create_tag_restores_state(pool: SqlitePool) {
    let tag = create_tag(
        &pool,
        crate::schema::tag::CreateTagRequest {
            name: "e2e_tag".to_string(),
            description: "desc".to_string(),
            query: None,
            auto_delete: false,
        },
        true,
    )
    .await
    .unwrap();
    let id = tag.id;

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

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tag WHERE id = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "undo CreateTag must remove tag");
}

#[sqlx::test]
async fn e2e_undo_delete_tag_restores_tag(pool: SqlitePool) {
    let tag = create_tag_helper(&pool, "to_delete_then_undo", "desc").await;
    delete_tag(&pool, tag.id, true).await.unwrap();

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

    let name: String = sqlx::query_scalar("SELECT name FROM tag WHERE id = ?")
        .bind(tag.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "to_delete_then_undo");
}

#[sqlx::test]
async fn e2e_undo_update_tag_restores_previous_name(pool: SqlitePool) {
    let tag = create_tag_helper(&pool, "original_tag", "desc").await;
    update_tag(
        &pool,
        UpdateTagRequest {
            tag_to_modify: TagSelector::Id(tag.id),
            name: Some("updated_tag".to_string()),
            description: None,
            query: None,
            auto_delete: None,
        },
        true,
    )
    .await
    .unwrap();

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let name: String = sqlx::query_scalar("SELECT name FROM tag WHERE id = ?")
        .bind(tag.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "original_tag");
}

#[sqlx::test]
async fn e2e_undo_create_tag_with_note_tags_fails_with_dependency_error(pool: SqlitePool) {
    let tag = create_tag(
        &pool,
        crate::schema::tag::CreateTagRequest {
            name: "tagged".to_string(),
            description: "desc".to_string(),
            query: None,
            auto_delete: false,
        },
        true,
    )
    .await
    .unwrap();
    let tag_event_id: i64 = sqlx::query_scalar("SELECT id FROM event ORDER BY id DESC LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();

    // Create a note and tag it so undoing the CreateTag (which deletes the tag) must fail.
    // The parser creation adds more events, so we pass the tag_event_id explicitly.
    let parser = create_parser_helper(&pool, "markdown").await;
    let ts = Utc::now().timestamp();
    let note_id: i64 = sqlx::query_scalar(
        r"INSERT INTO note (data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind("n1")
    .bind(ts)
    .bind(ts)
    .bind(parser.id)
    .bind(json!({}).to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(r"INSERT INTO note_tag (note_id, tag_id) VALUES (?, ?)")
        .bind(note_id)
        .bind(tag.id)
        .execute(&pool)
        .await
        .unwrap();

    let res = undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(tag_event_id),
            undo_group: false,
        },
    )
    .await;
    assert!(res.is_err());
    let err_msg = format!("{:?}", res.unwrap_err());
    assert!(
        err_msg.contains("note tags") || err_msg.contains("Cannot undo CreateTag"),
        "expected dependency error: {}",
        err_msg
    );
}
