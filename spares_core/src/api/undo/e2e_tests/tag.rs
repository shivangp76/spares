use chrono::Utc;
use serde_json::Map;
use serde_json::json;
use sqlx::SqlitePool;

use crate::api::note::create_notes;
use crate::api::note::update_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::tag::create_tag;
use crate::api::tag::delete_tag;
use crate::api::tag::tests::create_tag_helper;
use crate::api::tag::update_tag;
use crate::api::undo::undo_event;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::tag::TagSelector;
use crate::schema::tag::UpdateTagRequest;
use crate::schema::undo::UndoEventRequest;

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

#[sqlx::test]
async fn e2e_undo_delete_tag_restores_note_and_card_tags(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;

    // Step 1: Create tag
    let tag = create_tag_helper(&pool, "restore_test", "desc").await;

    // Step 2: Create two notes via public API (data contains a cloze so cards are created)
    let result = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![
                CreateNoteRequest {
                    data: "n1 {{ cloze }}".to_string(),
                    keywords: vec![],
                    tags: vec![],
                    is_suspended: false,
                    custom_data: Map::new(),
                },
                CreateNoteRequest {
                    data: "n2 {{ cloze }}".to_string(),
                    keywords: vec![],
                    tags: vec![],
                    is_suspended: false,
                    custom_data: Map::new(),
                },
            ],
        },
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    let note_id_1 = result.notes[0].id;
    let note_id_2 = result.notes[1].id;

    // Step 3: Add tag to both notes via public API
    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id_1, note_id_2]),
            data: None,
            parser_id: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_add: Some(vec!["restore_test".to_string()]),
                tags_to_remove: None,
            },
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();

    // Preserve card_tag restore coverage: pick a card parsed from note_id_1 and tag it via raw SQL
    let card_id: i64 = sqlx::query_scalar("SELECT id FROM card WHERE note_id = ? LIMIT 1")
        .bind(note_id_1)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(r"INSERT INTO card_tag (card_id, tag_id) VALUES (?, ?)")
        .bind(card_id)
        .bind(tag.id)
        .execute(&pool)
        .await
        .unwrap();

    // Step 4: Delete tag
    delete_tag(&pool, tag.id, true).await.unwrap();

    // Step 5: Verify note (and card) has no tag
    let note_tag_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM note_tag WHERE tag_id = ?")
            .bind(tag.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(note_tag_count_before, 0, "note_tag rows deleted by cascade");
    let card_tag_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM card_tag WHERE tag_id = ?")
            .bind(tag.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(card_tag_count_before, 0, "card_tag rows deleted by cascade");

    // Step 6: Undo delete tag
    let res = undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .expect("undo delete tag should succeed");
    assert!(res.is_some(), "undo_event should return Some response");

    // Step 7: Verify note (and card) still has tag
    let name: String = sqlx::query_scalar("SELECT name FROM tag WHERE id = ?")
        .bind(tag.id)
        .fetch_one(&pool)
        .await
        .expect("tag should exist after undo");
    assert_eq!(name, "restore_test");
    let note_tag_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note_tag WHERE tag_id = ?")
        .bind(tag.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(note_tag_count, 2, "note_tag associations restored on undo");
    let card_tag_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card_tag WHERE tag_id = ?")
        .bind(tag.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(card_tag_count, 1, "card_tag associations restored on undo");
}
