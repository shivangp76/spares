//! End-to-end redo tests: the undo/redo stack, review log restoration, groups, and error paths.

use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use super::create_card_helper;
use crate::api::card::forget_card;
use crate::api::note::create_notes;
use crate::api::note::delete_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::parser::update_parser;
use crate::api::review::bury_card;
use crate::api::review::submit_study_action;
use crate::api::tag::delete_tag;
use crate::api::tag::tests::create_tag_helper;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::model::Card;
use crate::model::EventAction;
use crate::model::ReviewLog;
use crate::parsers::get_all_parsers;
use crate::schedulers::get_scheduler_from_string;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::parser::UpdateParserRequest;
use crate::schema::review::RatingSubmission;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;
use crate::schema::undo::EventSummary;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::RedoEventResponse;
use crate::schema::undo::UndoEventRequest;
use crate::schema::undo::UndoEventResponse;

async fn undo_latest(pool: &SqlitePool) -> Option<UndoEventResponse> {
    undo_event(
        pool,
        UndoEventRequest {
            event_id: None,
            undo_group: true,
        },
    )
    .await
    .unwrap()
}

async fn redo_latest(pool: &SqlitePool) -> Option<RedoEventResponse> {
    redo_event(
        pool,
        RedoEventRequest {
            event_id: None,
            redo_group: true,
        },
    )
    .await
    .unwrap()
}

async fn rename_parser(pool: &SqlitePool, id: i64, name: &str) {
    update_parser(
        pool,
        UpdateParserRequest {
            name: Some(name.to_string()),
        },
        id,
        true,
    )
    .await
    .unwrap();
}

async fn parser_name(pool: &SqlitePool, id: i64) -> String {
    sqlx::query_scalar("SELECT name FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn fetch_card(pool: &SqlitePool, card_id: i64) -> Card {
    sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn review_logs(pool: &SqlitePool, card_id: i64) -> Vec<ReviewLog> {
    sqlx::query_as("SELECT * FROM review_log WHERE card_id = ? ORDER BY id")
        .bind(card_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn rate(pool: &SqlitePool, card_id: i64, rating: u32) {
    submit_study_action(
        pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating,
                recall_duration: chrono::Duration::seconds(5),
                rate_duration: chrono::Duration::seconds(2),
                tag_id: None,
            }),
        },
        Utc::now(),
    )
    .await
    .unwrap();
}

#[sqlx::test]
async fn e2e_redo_follows_stack_order(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "a").await;
    rename_parser(&pool, p.id, "b").await;
    rename_parser(&pool, p.id, "c").await;

    undo_latest(&pool).await.unwrap();
    assert_eq!(parser_name(&pool, p.id).await, "b");
    undo_latest(&pool).await.unwrap();
    assert_eq!(parser_name(&pool, p.id).await, "a");

    redo_latest(&pool).await.unwrap();
    assert_eq!(parser_name(&pool, p.id).await, "b");
    redo_latest(&pool).await.unwrap();
    assert_eq!(parser_name(&pool, p.id).await, "c");

    assert!(redo_latest(&pool).await.is_none(), "nothing left to redo");
}

#[sqlx::test]
async fn e2e_undo_latest_twice_never_undoes_an_undo(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "a").await;
    rename_parser(&pool, p.id, "b").await;
    rename_parser(&pool, p.id, "c").await;

    undo_latest(&pool).await.unwrap();
    undo_latest(&pool).await.unwrap();
    assert_eq!(
        parser_name(&pool, p.id).await,
        "a",
        "the second undo must undo the earlier rename, not the first undo"
    );
}

#[sqlx::test]
async fn e2e_new_action_clears_redo(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "a").await;
    rename_parser(&pool, p.id, "b").await;
    undo_latest(&pool).await.unwrap();
    rename_parser(&pool, p.id, "x").await;
    assert!(redo_latest(&pool).await.is_none());

    // Undoing the new action makes it redoable, but the undo from before it stays cleared
    undo_latest(&pool).await.unwrap();
    assert_eq!(parser_name(&pool, p.id).await, "a");
    redo_latest(&pool).await.unwrap();
    assert_eq!(parser_name(&pool, p.id).await, "x");
    assert!(redo_latest(&pool).await.is_none());
}

#[sqlx::test]
async fn e2e_undo_redo_cycles(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "a").await;
    rename_parser(&pool, p.id, "b").await;
    let action_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();

    for _ in 0..2 {
        undo_latest(&pool).await.unwrap();
        assert_eq!(parser_name(&pool, p.id).await, "a");
        redo_latest(&pool).await.unwrap();
        assert_eq!(parser_name(&pool, p.id).await, "b");
    }

    let actions: Vec<EventAction> =
        sqlx::query_scalar("SELECT action FROM event_action WHERE id >= ? ORDER BY id")
            .bind(action_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        actions,
        vec![
            EventAction::Do,
            EventAction::Undo,
            EventAction::Redo,
            EventAction::Undo,
            EventAction::Redo,
        ]
    );
}

#[sqlx::test]
async fn e2e_redo_and_undo_by_id_reject_wrong_events(pool: SqlitePool) {
    let p = create_parser_helper(&pool, "a").await;
    rename_parser(&pool, p.id, "b").await;
    let action_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();

    let redo_action = redo_event(
        &pool,
        RedoEventRequest {
            event_id: Some(action_id),
            redo_group: false,
        },
    )
    .await;
    assert!(redo_action.is_err(), "an action is not an undo");

    let undo_id = undo_latest(&pool).await.unwrap().undo_event_ids[0];
    let undo_again = undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(action_id),
            undo_group: false,
        },
    )
    .await;
    assert!(undo_again.is_err(), "an action can only be undone once");
    let undo_undo = undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(undo_id),
            undo_group: false,
        },
    )
    .await;
    assert!(undo_undo.is_err(), "an undo is reversed with redo");

    redo_latest(&pool).await.unwrap();
    let redo_again = redo_event(
        &pool,
        RedoEventRequest {
            event_id: Some(undo_id),
            redo_group: false,
        },
    )
    .await;
    assert!(redo_again.is_err(), "an undo can only be redone once");
    assert_eq!(parser_name(&pool, p.id).await, "b");
}

#[sqlx::test]
async fn e2e_redo_rate_card_restores_review_log(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    rate(&pool, card_id, 3).await;
    let rated = fetch_card(&pool, card_id).await;
    let logs = review_logs(&pool, card_id).await;
    assert_eq!(logs.len(), 1);

    undo_latest(&pool).await.unwrap();
    assert_eq!(review_logs(&pool, card_id).await.len(), 0);

    redo_latest(&pool).await.unwrap();
    let redone = fetch_card(&pool, card_id).await;
    assert_eq!(redone.due, rated.due);
    assert_eq!(redone.stability, rated.stability);
    assert_eq!(redone.difficulty, rated.difficulty);
    assert_eq!(redone.state, rated.state);
    let restored = review_logs(&pool, card_id).await;
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].rating, logs[0].rating);
    assert_eq!(restored[0].reviewed_at, logs[0].reviewed_at);
    assert_eq!(restored[0].recall_duration, logs[0].recall_duration);
    assert_eq!(restored[0].scheduled_time, logs[0].scheduled_time);

    // Undoing the redo deletes the reinserted row, not the old id
    undo_latest(&pool).await.unwrap();
    assert_eq!(review_logs(&pool, card_id).await.len(), 0);
    assert_eq!(fetch_card(&pool, card_id).await.state, 0);
}

#[sqlx::test]
async fn e2e_redo_forget_card_restores_marker(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    rate(&pool, card_id, 4).await;
    let rated = fetch_card(&pool, card_id).await;
    forget_card(&pool, card_id, Utc::now(), true).await.unwrap();
    let forgotten = fetch_card(&pool, card_id).await;

    undo_latest(&pool).await.unwrap();
    assert_eq!(fetch_card(&pool, card_id).await.stability, rated.stability);
    assert_eq!(review_logs(&pool, card_id).await.len(), 1);

    redo_latest(&pool).await.unwrap();
    let redone = fetch_card(&pool, card_id).await;
    assert_eq!(redone.stability, forgotten.stability);
    assert_eq!(redone.state, forgotten.state);
    let logs = review_logs(&pool, card_id).await;
    assert_eq!(logs.len(), 2);
    assert!(!logs[1].is_review(), "the forget marker must be restored");
}

#[sqlx::test]
async fn e2e_redo_group_recreates_note_and_tag(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Redo group {{ cloze }}".to_string(),
                keywords: vec![],
                tags: vec!["redo_tag".to_string()],
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

    let counts = async |pool: &SqlitePool| -> (i64, i64) {
        sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM note), (SELECT COUNT(*) FROM tag WHERE name = 'redo_tag')",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    };
    assert_eq!(counts(&pool).await, (1, 1));

    let undo = undo_latest(&pool).await.unwrap();
    assert_eq!(undo.undone_events.len(), 2);
    assert_eq!(undo.undo_event_ids.len(), 2);
    assert_eq!(counts(&pool).await, (0, 0));

    let redo = redo_event(
        &pool,
        RedoEventRequest {
            event_id: Some(undo.undo_event_ids[0]),
            redo_group: true,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        redo.redone_events.len(),
        2,
        "redo must take the whole undo group"
    );
    assert_eq!(counts(&pool).await, (1, 1));

    undo_latest(&pool).await.unwrap();
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test]
async fn e2e_redo_delete_tag_removes_restored_associations(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    create_tag_helper(&pool, "assoc", "desc").await;
    let result = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Tagged {{ cloze }}".to_string(),
                keywords: vec![],
                tags: vec!["assoc".to_string()],
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
    let note_id = result.notes[0].id;
    let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tag WHERE name = 'assoc'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let note_tags = async |pool: &SqlitePool| -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM note_tag WHERE note_id = ? AND tag_id = ?")
            .bind(note_id)
            .bind(tag_id)
            .fetch_one(pool)
            .await
            .unwrap()
    };
    assert_eq!(note_tags(&pool).await, 1);

    delete_tag(&pool, tag_id, true).await.unwrap();
    undo_latest(&pool).await.unwrap();
    assert_eq!(note_tags(&pool).await, 1);

    // The undo restored the associations, which must not block redoing the delete
    redo_latest(&pool).await.unwrap();
    assert_eq!(note_tags(&pool).await, 0);

    undo_latest(&pool).await.unwrap();
    assert_eq!(
        note_tags(&pool).await,
        1,
        "a second undo restores them again"
    );
}

#[sqlx::test]
async fn e2e_redo_delete_notes(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    let note_id: i64 = sqlx::query_scalar("SELECT note_id FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
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
    let note_count = async |pool: &SqlitePool| -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM note WHERE id = ?")
            .bind(note_id)
            .fetch_one(pool)
            .await
            .unwrap()
    };

    undo_latest(&pool).await.unwrap();
    assert_eq!(note_count(&pool).await, 1);
    redo_latest(&pool).await.unwrap();
    assert_eq!(note_count(&pool).await, 0);
    undo_latest(&pool).await.unwrap();
    assert_eq!(note_count(&pool).await, 1);
}

fn descriptions(events: &[EventSummary]) -> Vec<&str> {
    events.iter().map(|e| e.description.as_str()).collect()
}

#[sqlx::test]
async fn e2e_undo_and_redo_describe_the_original_action(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    rate(&pool, card_id, 3).await;
    let rated = format!("Rate card {card_id} (Good)");

    // An undo of the action, a redo, and an undo of the redo all describe the rating itself
    let undo = undo_latest(&pool).await.unwrap();
    assert_eq!(descriptions(&undo.undone_events), [rated.as_str()]);
    let redo = redo_latest(&pool).await.unwrap();
    assert_eq!(descriptions(&redo.redone_events), [rated.as_str()]);
    let undo = undo_latest(&pool).await.unwrap();
    assert_eq!(descriptions(&undo.undone_events), [rated.as_str()]);

    // A redone bury is stored as `UpdateCards`, but is still described as a bury
    let scheduler = get_scheduler_from_string("fsrs").unwrap();
    bury_card(&pool, scheduler.as_ref(), card_id, Utc::now(), true)
        .await
        .unwrap();
    let buried = format!("Bury card {card_id}");
    undo_latest(&pool).await.unwrap();
    redo_latest(&pool).await.unwrap();
    let undo = undo_latest(&pool).await.unwrap();
    assert_eq!(descriptions(&undo.undone_events), [buried.as_str()]);

    let p = create_parser_helper(&pool, "a").await;
    rename_parser(&pool, p.id, "b").await;
    let undo = undo_latest(&pool).await.unwrap();
    assert_eq!(
        descriptions(&undo.undone_events),
        ["Rename parser 'a' to 'b'"]
    );
    let redo = redo_latest(&pool).await.unwrap();
    assert_eq!(
        descriptions(&redo.redone_events),
        ["Rename parser 'a' to 'b'"]
    );
}
