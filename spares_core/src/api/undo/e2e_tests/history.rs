use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::api::note::create_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::tag::create_tag;
use crate::api::undo::group_events;
use crate::api::undo::list_events;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::model::EventAction;
use crate::model::EventType;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::tag::CreateTagRequest;
use crate::schema::undo::GroupEventsRequest;
use crate::schema::undo::ListEventsRequest;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::UndoEventRequest;

async fn tag(pool: &SqlitePool, name: &str) -> i64 {
    create_tag(
        pool,
        CreateTagRequest {
            name: name.to_string(),
            description: String::new(),
            query: None,
            auto_delete: false,
            reschedule: true,
        },
        true,
    )
    .await
    .unwrap();
    sqlx::query_scalar("SELECT MAX(id) FROM event")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The history lists a group as one entry, by its first event, says what each entry is and
/// whether it was reverted, and pages back with `before`.
#[sqlx::test]
async fn history_lists_groups_once_and_what_was_reverted(pool: SqlitePool) {
    let first = tag(&pool, "a").await;
    let second = tag(&pool, "b").await;
    group_events(
        &pool,
        GroupEventsRequest {
            event_ids: vec![first, second],
        },
    )
    .await
    .unwrap();
    let third = tag(&pool, "c").await;
    let undo = undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(first),
            undo_group: true,
        },
    )
    .await
    .unwrap()
    .unwrap();

    let entries = list_events(&pool, ListEventsRequest::default())
        .await
        .unwrap();
    let summary = entries
        .iter()
        .map(|e| (e.action, e.group_size, e.reverted))
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            // The undo of the group, a group of its own
            (EventAction::Undo, 2, false),
            (EventAction::Do, 1, false),
            (EventAction::Do, 2, true),
        ],
        "{entries:?}"
    );
    assert_eq!(entries[0].id, undo.undo_event_ids[0]);
    assert_eq!(entries[1].id, third);
    assert_eq!(entries[2].id, first);
    assert_eq!(entries[1].kind, EventType::CreateTag);
    assert!(entries[1].description.contains('c'), "{entries:?}");
    // A group with no note events is described by its first event
    assert_eq!(entries[2].description, "Create tag 'a' (and 1 more change)");
    // An undo is described as the action it undid. A group is undone newest first, so the undo's
    // first event undid the group's last.
    assert_eq!(entries[0].description, "Create tag 'b' (and 1 more change)");

    // Redoing the undo reverts it, and the redo is listed as one
    redo_event(
        &pool,
        RedoEventRequest {
            event_id: Some(entries[0].id),
            redo_group: true,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let entries = list_events(&pool, ListEventsRequest::default())
        .await
        .unwrap();
    assert_eq!(
        (entries[0].action, entries[1].action, entries[1].reverted),
        (EventAction::Redo, EventAction::Undo, true)
    );

    let page = list_events(
        &pool,
        ListEventsRequest {
            before: Some(third),
            limit: Some(1),
            ratings: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(page.iter().map(|e| e.id).collect::<Vec<_>>(), [first]);
}

/// A group with notes, such as an import's, which also creates the tags its notes name, is
/// described by its notes, whether it is listed as the action or as its undo.
#[sqlx::test]
async fn history_describes_groups_by_their_notes(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    let tag_event = tag(&pool, "geo").await;
    let created = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "{{ Paris }} is the capital".to_string(),
                keywords: vec![],
                tags: vec!["geo".to_string()],
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
    let note = created.notes[0].id.letters();
    group_events(
        &pool,
        GroupEventsRequest {
            event_ids: vec![tag_event, created.event_id.unwrap()],
        },
    )
    .await
    .unwrap();
    let expected = format!("Create note {note} (and 1 more change)");
    let entries = list_events(&pool, ListEventsRequest::default())
        .await
        .unwrap();
    assert_eq!(entries[0].description, expected, "{entries:?}");

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(tag_event),
            undo_group: true,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let entries = list_events(&pool, ListEventsRequest::default())
        .await
        .unwrap();
    assert_eq!(
        (entries[0].action, entries[0].description.as_str()),
        (EventAction::Undo, expected.as_str()),
        "{entries:?}"
    );
}
