//! Note undo tests

use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::api::note::create_notes;
use crate::api::note::delete_notes;
use crate::api::note::update_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::tag::tests::create_tag_helper;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::model::CardId;
use crate::model::Tag;
use crate::model::TagId;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::UndoEventRequest;

#[sqlx::test]
async fn e2e_undo_create_notes_restores_state(pool: SqlitePool) {
    use crate::api::note::create_notes;

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

/// An auto-delete tag with a description, which a note created after it is the only one to have
async fn create_lonely_tagged_note(pool: &SqlitePool) -> (crate::model::NoteId, Tag) {
    use crate::api::tag::create_tag;
    use crate::schema::tag::CreateTagRequest;

    let parser = create_parser_helper(pool, "markdown").await;
    create_tag(
        pool,
        CreateTagRequest {
            name: "lonely".to_string(),
            description: "kept on undo".to_string(),
            query: None,
            auto_delete: true,
            reschedule: true,
        },
        false,
    )
    .await
    .unwrap();
    let request = CreateNotesRequest {
        parser_id: parser.id,
        requests: vec![CreateNoteRequest {
            data: "Hello {{ world }}".to_string(),
            keywords: vec![],
            tags: vec!["lonely".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        }],
    };
    let note_id = create_notes(pool, request, Utc::now(), &get_all_parsers(), false)
        .await
        .unwrap()
        .notes[0]
        .id;
    let tag = tag_named(pool, "lonely").await.unwrap();
    (note_id, tag)
}

async fn tag_named(pool: &SqlitePool, name: &str) -> Option<Tag> {
    sqlx::query_as("SELECT * FROM tag WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn note_tag_ids(pool: &SqlitePool, note_id: crate::model::NoteId) -> Vec<TagId> {
    sqlx::query_scalar("SELECT tag_id FROM note_tag WHERE note_id = ?")
        .bind(note_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn undo_latest(pool: &SqlitePool) {
    undo_event(
        pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

async fn redo_latest(pool: &SqlitePool) {
    redo_event(
        pool,
        RedoEventRequest {
            event_id: None,
            redo_group: false,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

async fn delete_note(pool: &SqlitePool, note_id: crate::model::NoteId) {
    delete_notes(
        pool,
        DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
        },
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
}

#[sqlx::test]
async fn e2e_undo_delete_notes_restores_auto_deleted_tag_and_card_links(
    pool_options: sqlx::sqlite::SqlitePoolOptions,
    options: sqlx::sqlite::SqliteConnectOptions,
) {
    use crate::api::review::submit_study_action;
    use crate::api::tag::create_tag;
    use crate::schema::review::RatingSubmission;
    use crate::schema::review::StudyAction;
    use crate::schema::review::SubmitStudyActionRequest;
    use crate::schema::tag::CreateTagRequest;

    // The filtered tag searches text, which needs `REGEXP`.
    let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
    let (note_id, tag) = create_lonely_tagged_note(&pool).await;
    let card_id: CardId = sqlx::query_scalar("SELECT id FROM card WHERE note_id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating: 3,
                recall_duration: chrono::Duration::seconds(5),
                rate_duration: chrono::Duration::seconds(2),
                tag_id: None,
            }),
        },
        Utc::now(),
    )
    .await
    .unwrap();
    let filtered = create_tag(
        &pool,
        CreateTagRequest {
            name: "filtered".to_string(),
            description: String::new(),
            query: Some("Hello".to_string()),
            auto_delete: false,
            reschedule: true,
        },
        false,
    )
    .await
    .unwrap();
    let card_tags = async || -> Vec<TagId> {
        sqlx::query_scalar("SELECT tag_id FROM card_tag WHERE card_id = ?")
            .bind(card_id)
            .fetch_all(&pool)
            .await
            .unwrap()
    };
    let reviews_of_card = async || -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM review_log WHERE card_id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(card_tags().await, vec![filtered.id]);
    assert_eq!(reviews_of_card().await, 1);

    delete_note(&pool, note_id).await;
    assert!(tag_named(&pool, "lonely").await.is_none());
    assert_eq!(reviews_of_card().await, 0);

    // A plain undo reverts the delete, with the tag it deleted and what its card had
    undo_latest(&pool).await;
    let restored = tag_named(&pool, "lonely").await.unwrap();
    assert_eq!(restored.id, tag.id);
    assert_eq!(restored.description, "kept on undo");
    assert_eq!(note_tag_ids(&pool, note_id).await, vec![tag.id]);
    assert_eq!(reviews_of_card().await, 1);
    assert_eq!(card_tags().await, vec![filtered.id]);

    redo_latest(&pool).await;
    assert!(tag_named(&pool, "lonely").await.is_none());
    assert_eq!(reviews_of_card().await, 0);

    // And again, from the redo's undo
    undo_latest(&pool).await;
    assert_eq!(tag_named(&pool, "lonely").await.unwrap().id, tag.id);
    assert_eq!(reviews_of_card().await, 1);
}

#[sqlx::test]
async fn e2e_undo_delete_notes_links_to_tag_that_took_the_name(pool: SqlitePool) {
    let (note_id, tag) = create_lonely_tagged_note(&pool).await;
    delete_note(&pool, note_id).await;
    // Not auto-delete, so it outlives the note again below
    let other = create_tag_helper(&pool, "lonely", "another").await;
    assert_ne!(other.id, tag.id);

    undo_latest(&pool).await;
    assert_eq!(note_tag_ids(&pool, note_id).await, vec![other.id]);
    assert_eq!(
        tag_named(&pool, "lonely").await.unwrap().description,
        "another"
    );

    redo_latest(&pool).await;
    assert_eq!(tag_named(&pool, "lonely").await.unwrap().id, other.id);
}

#[sqlx::test]
async fn e2e_undo_update_notes_restores_auto_deleted_tag(pool: SqlitePool) {
    let (note_id, tag) = create_lonely_tagged_note(&pool).await;
    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            parser_id: None,
            data: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_remove: Some(vec!["lonely".to_string()]),
                tags_to_add: None,
            },
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    assert!(tag_named(&pool, "lonely").await.is_none());

    undo_latest(&pool).await;
    let restored = tag_named(&pool, "lonely").await.unwrap();
    assert_eq!(restored.id, tag.id);
    assert_eq!(restored.description, "kept on undo");
    assert_eq!(note_tag_ids(&pool, note_id).await, vec![tag.id]);

    redo_latest(&pool).await;
    assert!(tag_named(&pool, "lonely").await.is_none());
    assert_eq!(note_tag_ids(&pool, note_id).await, Vec::<TagId>::new());
}
