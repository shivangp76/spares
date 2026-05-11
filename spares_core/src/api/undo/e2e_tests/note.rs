// ── Note undo tests ───────────────────────────────────────────────────────────

use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::api::note::create_notes;
use crate::api::note::update_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::tag::tests::create_tag_helper;
use crate::api::undo::undo_event;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::undo::UndoEventRequest;

#[sqlx::test]
async fn e2e_undo_create_notes_restores_state(pool: SqlitePool) {
    use crate::api::note::create_notes;
    use crate::api::note::delete_notes;
    use crate::schema::note::DeleteNotesRequest;
    use crate::schema::note::NotesSelector;

    let parser = create_parser_helper(&pool, "markdown").await;
    let request = CreateNotesRequest {
        parser_id: parser.id,
        requests: vec![CreateNoteRequest {
            data: "Undo test {{ cloze }}".to_string(),
            keywords: vec!["kw1".to_string()],
            tags: vec!["tag1".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        }],
    };
    let result = create_notes(&pool, request, Utc::now(), &get_all_parsers(), true)
        .await
        .unwrap();
    let note_id = result.notes[0].id;

    // Verify note exists
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "note must exist after creation");

    // Undo CreateNotes
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "undo CreateNotes must remove the note");

    // Cards must also be gone
    let card_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE note_id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(card_count, 0, "undo CreateNotes must remove cards");
}

#[sqlx::test]
async fn e2e_undo_delete_notes_restores_note(pool: SqlitePool) {
    use crate::api::note::create_notes;
    use crate::api::note::delete_notes;
    use crate::schema::note::DeleteNotesRequest;
    use crate::schema::note::NotesSelector;

    let parser = create_parser_helper(&pool, "markdown").await;
    let request = CreateNotesRequest {
        parser_id: parser.id,
        requests: vec![CreateNoteRequest {
            data: "Delete undo {{ cloze }}".to_string(),
            keywords: vec!["kw_del".to_string()],
            tags: vec!["tag_del".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        }],
    };
    let result = create_notes(&pool, request, Utc::now(), &get_all_parsers(), false)
        .await
        .unwrap();
    let note_id = result.notes[0].id;

    // Capture card count before delete
    let card_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE note_id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(card_count_before > 0, "note must have cards");

    // Delete note with logging
    delete_notes(
        &pool,
        DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
        },
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "note must be deleted");

    // Undo DeleteNotes
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    // Note must be restored with the same ID
    let restored_data: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        restored_data.contains("Delete undo"),
        "undo DeleteNotes must restore note data"
    );

    // Cards must be restored
    let card_count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE note_id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        card_count_after, card_count_before,
        "undo DeleteNotes must restore cards"
    );
}

#[sqlx::test]
async fn e2e_undo_update_notes_restores_data(pool: SqlitePool) {
    use crate::api::note::create_notes;
    use crate::api::note::update_notes;
    use crate::schema::note::NotesSelector;
    use crate::schema::note::UpdateNotesRequest;
    use crate::schema::note::UpdateTags;

    let parser = create_parser_helper(&pool, "markdown").await;
    let request = CreateNotesRequest {
        parser_id: parser.id,
        requests: vec![CreateNoteRequest {
            data: "Original {{ data }}".to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        }],
    };
    let result = create_notes(&pool, request, Utc::now(), &get_all_parsers(), false)
        .await
        .unwrap();
    let note_id = result.notes[0].id;
    let original_data = result.notes[0].data.clone();

    // Update note with logging
    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: Some("Updated {{ data }}".to_string()),
            parser_id: None,
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();

    let updated_data: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        updated_data.contains("Updated"),
        "note data must be updated"
    );

    // Undo UpdateNotes
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let restored_data: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored_data, original_data,
        "undo UpdateNotes must restore original data"
    );
}

#[sqlx::test]
async fn e2e_create_notes_with_new_tag_groups_events(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;

    let last_event_id_before: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();

    let result = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Group test {{ cloze }}".to_string(),
                keywords: vec![],
                tags: vec!["brand_new_tag".to_string()],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();

    // Both CreateTag and CreateNotes events must share the same group_id
    let group_ids: Vec<Option<i64>> =
        sqlx::query_scalar("SELECT group_id FROM event WHERE id > ? ORDER BY id ASC")
            .bind(last_event_id_before)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        group_ids.len(),
        2,
        "must have exactly 2 events (CreateTag + CreateNotes)"
    );
    assert!(
        group_ids[0].is_some() && group_ids[0] == group_ids[1],
        "CreateTag and CreateNotes must share the same group_id, got {:?}",
        group_ids
    );

    let _ = result.notes[0].id;
}

#[sqlx::test]
async fn e2e_undo_create_notes_with_new_tag_removes_tag_when_undo_group(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Tag undo test {{ cloze }}".to_string(),
                keywords: vec![],
                tags: vec!["auto_created_tag".to_string()],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();

    // Tag must exist after creation
    let tag_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tag WHERE name = 'auto_created_tag'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tag_count_before, 1, "tag must exist after note creation");

    // Undo with undo_group: true — must remove both note and tag
    let latest_event_id: i64 = sqlx::query_scalar("SELECT id FROM event ORDER BY id DESC LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(latest_event_id),
            undo_group: true,
        },
    )
    .await
    .unwrap();

    let tag_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tag WHERE name = 'auto_created_tag'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        tag_count_after, 0,
        "undo with undo_group must remove the implicitly-created tag"
    );
}

#[sqlx::test]
async fn e2e_create_notes_with_existing_tag_does_not_group(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    // Pre-create the tag so it already exists
    create_tag_helper(&pool, "existing_tag", "desc").await;

    let event_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();

    create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "No-group test {{ cloze }}".to_string(),
                keywords: vec![],
                tags: vec!["existing_tag".to_string()],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();

    let event_count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        event_count_after,
        event_count_before + 1,
        "only one CreateNotes event must be logged when tag already exists"
    );

    let group_id: Option<i64> =
        sqlx::query_scalar("SELECT group_id FROM event ORDER BY id DESC LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        group_id.is_none(),
        "CreateNotes with existing tag must not be grouped"
    );
}

#[sqlx::test]
async fn e2e_update_notes_with_new_tag_groups_events(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    let result = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Update group test {{ cloze }}".to_string(),
                keywords: vec![],
                tags: vec![],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    let note_id = result.notes[0].id;

    let last_event_id_before: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();

    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_add: Some(vec!["update_new_tag".to_string()]),
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

    let group_ids: Vec<Option<i64>> =
        sqlx::query_scalar("SELECT group_id FROM event WHERE id > ? ORDER BY id ASC")
            .bind(last_event_id_before)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        group_ids.len(),
        2,
        "must have CreateTag + UpdateNotes events"
    );
    assert!(
        group_ids[0].is_some() && group_ids[0] == group_ids[1],
        "CreateTag and UpdateNotes must share the same group_id, got {:?}",
        group_ids
    );
}

#[sqlx::test]
async fn e2e_undo_update_notes_with_new_tag_removes_tag_when_undo_group(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    let result = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Update tag undo {{ cloze }}".to_string(),
                keywords: vec![],
                tags: vec![],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    let note_id = result.notes[0].id;

    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_add: Some(vec!["update_auto_tag".to_string()]),
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

    let tag_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tag WHERE name = 'update_auto_tag'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tag_count_before, 1, "tag must exist after update");

    // Undo the UpdateNotes event with undo_group: true
    let latest_event_id: i64 = sqlx::query_scalar("SELECT id FROM event ORDER BY id DESC LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(latest_event_id),
            undo_group: true,
        },
    )
    .await
    .unwrap();

    let tag_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tag WHERE name = 'update_auto_tag'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        tag_count_after, 0,
        "undo with undo_group must remove the tag created during note update"
    );
}
