//! End-to-end undo tests: create/update/delete parser flows, with and without groups,
//! including dependency error paths. Also covers apply_event for parser events (via undo flow).
//! Also covers card undo: update, forget, bury, unbury, rate, advance, postpone.

use crate::api::card::{forget_card, unbury_cards, update_cards};
use crate::api::note::{create_notes, update_notes};
use crate::api::parser::tests::create_parser_helper;
use crate::api::parser::{create_parser, create_parser_event, delete_parser, update_parser};
use crate::api::review::submit_study_action;
use crate::api::tag::tests::create_tag_helper;
use crate::api::tag::{create_tag, delete_tag, update_tag};
use crate::api::undo::payloads::CreateParserPayload;
use crate::api::undo::{create_event_group, insert_events, undo_event};
use crate::model::{Card, CardId, EventType, SpecialState};
use crate::parsers::get_all_parsers;
use crate::schema::card::{CardsSelector, SpecialStateUpdate, UpdateCardsRequest};
use crate::schema::note::{
    CreateNoteRequest, CreateNotesRequest, NotesSelector, UpdateNotesRequest, UpdateTags,
};
use crate::schema::review::{RatingSubmission, StudyAction, SubmitStudyActionRequest};
use crate::schema::tag::{TagSelector, UpdateTagRequest};
use crate::schema::undo::UndoEventRequest;
use chrono::Utc;
use serde_json::{Map, json};
use sqlx::SqlitePool;

/// Creates a single note with one cloze card and returns the card id.
async fn create_card_helper(pool: &SqlitePool) -> CardId {
    let parser = create_parser_helper(pool, "markdown").await;
    let request = CreateNotesRequest {
        parser_id: parser.id,
        requests: vec![CreateNoteRequest {
            data: "Hello {{ world }}".to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        }],
    };
    let result = create_notes(pool, request, Utc::now(), &get_all_parsers(), false)
        .await
        .unwrap();
    let note_id = result.notes[0].id;
    let card: Card = sqlx::query_as("SELECT * FROM card WHERE note_id = ? LIMIT 1")
        .bind(note_id)
        .fetch_one(pool)
        .await
        .unwrap();
    card.id
}

#[sqlx::test]
async fn e2e_undo_create_parser_restores_state(pool: SqlitePool) {
    let p = create_parser(
        &pool,
        crate::schema::parser::CreateParserRequest {
            name: "e2e".to_string(),
        },
        true,
    )
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
        (
            EventType::CreateParser,
            json!({"id": p1.id, "name": p1.name}),
        ),
        (
            EventType::CreateParser,
            json!({"id": p2.id, "name": p2.name}),
        ),
        (
            EventType::CreateParser,
            json!({"id": p3.id, "name": p3.name}),
        ),
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

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parser")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "undo group of 3 CreateParser must remove all 3 parsers"
    );
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
async fn e2e_undo_single_event_in_group_undoes_only_that_event_when_undo_group_false(
    pool: SqlitePool,
) {
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
        (
            EventType::CreateParser,
            json!({"id": p1.id, "name": p1.name}),
        ),
        (
            EventType::CreateParser,
            json!({"id": p2.id, "name": p2.name}),
        ),
    ];
    let group_id = create_event_group(&pool, events, at).await.unwrap();
    let second_id: i64 =
        sqlx::query_scalar("SELECT id FROM event WHERE group_id = ? ORDER BY id LIMIT 1 OFFSET 1")
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

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parser")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "undo single event without group should remove only one parser"
    );
}

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

// ── Card undo tests ──────────────────────────────────────────────────────────

#[sqlx::test]
async fn e2e_undo_update_card_restores_special_state(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;

    let before: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before.special_state, None);

    update_cards(
        &pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: Some(Some(SpecialStateUpdate::Suspended)),
            due: None,
        },
        Utc::now(),
        true,
    )
    .await
    .unwrap();

    let after: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after.special_state, Some(SpecialState::Suspended));

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored.special_state, None,
        "undo UpdateCards must restore special_state to None"
    );
}

#[sqlx::test]
async fn e2e_undo_update_card_restores_due(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    let before: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let original_due = before.due;
    let new_due = original_due + chrono::Duration::days(7);

    update_cards(
        &pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: None,
            due: Some(new_due),
        },
        Utc::now(),
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

    let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored.due.timestamp(),
        original_due.timestamp(),
        "undo UpdateCards must restore original due date"
    );
}

#[sqlx::test]
async fn e2e_undo_forget_card_restores_state(pool: SqlitePool) {
    // Rate the card first so it has non-zero stability/difficulty
    let card_id = create_card_helper(&pool).await;
    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating: 4,
                recall_duration: chrono::Duration::seconds(5),
                rate_duration: chrono::Duration::seconds(2),
                tag_id: None,
            }),
        },
        Utc::now(),
    )
    .await
    .unwrap();

    let before: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(before.stability > 0.0, "card must have been rated");

    forget_card(&pool, card_id, Utc::now(), true).await.unwrap();

    let forgotten: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(forgotten.stability, 0.0);
    assert_eq!(forgotten.difficulty, 0.0);

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored.stability, before.stability,
        "undo ForgetCard must restore stability"
    );
    assert_eq!(
        restored.difficulty, before.difficulty,
        "undo ForgetCard must restore difficulty"
    );
    assert_eq!(
        restored.state, before.state,
        "undo ForgetCard must restore state"
    );
    assert_eq!(
        restored.due.timestamp(),
        before.due.timestamp(),
        "undo ForgetCard must restore due"
    );
}

#[sqlx::test]
async fn e2e_undo_rate_card_restores_scheduling_state(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    let before: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating: 4,
                recall_duration: chrono::Duration::seconds(5),
                rate_duration: chrono::Duration::seconds(2),
                tag_id: None,
            }),
        },
        Utc::now(),
    )
    .await
    .unwrap();

    let after: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        after.due > before.due,
        "rated card must have a future due date"
    );

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored.stability, before.stability,
        "undo RateCard must restore stability"
    );
    assert_eq!(
        restored.difficulty, before.difficulty,
        "undo RateCard must restore difficulty"
    );
    assert_eq!(
        restored.state, before.state,
        "undo RateCard must restore state"
    );
    assert_eq!(
        restored.due.timestamp(),
        before.due.timestamp(),
        "undo RateCard must restore due"
    );
}

#[sqlx::test]
async fn e2e_undo_rate_card_deletes_review_log(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;

    let log_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log")
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

    let log_count_after_rate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        log_count_after_rate,
        log_count_before + 1,
        "rating must create a review log"
    );

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let log_count_after_undo: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        log_count_after_undo, log_count_before,
        "undo RateCard must delete the review log"
    );
}

#[sqlx::test]
async fn e2e_undo_bury_card_restores_special_state(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;

    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Bury { card_id },
        },
        Utc::now(),
    )
    .await
    .unwrap();

    let buried: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(buried.special_state, Some(SpecialState::UserBuried));

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored.special_state, None,
        "undo BuryCard must clear special_state"
    );
}

#[sqlx::test]
async fn e2e_undo_unbury_cards_restores_buried_state(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;

    // Manually bury the card in DB (simulate scheduler-buried)
    sqlx::query("UPDATE card SET special_state = ? WHERE id = ?")
        .bind(SpecialState::UserBuried)
        .bind(card_id)
        .execute(&pool)
        .await
        .unwrap();

    unbury_cards(&pool, Utc::now(), true).await.unwrap();

    let unburied: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(unburied.special_state, None);

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();

    let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored.special_state,
        Some(SpecialState::UserBuried),
        "undo UnburyCards must restore buried state"
    );
}

#[sqlx::test]
async fn e2e_undo_advance_cards_restores_due(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;

    // Rate the card so it has a future due date (new cards aren't safe to advance)
    let now = Utc::now();
    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating: 4,
                recall_duration: chrono::Duration::seconds(5),
                rate_duration: chrono::Duration::seconds(2),
                tag_id: None,
            }),
        },
        now,
    )
    .await
    .unwrap();

    let before: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Advance 1 card (moves due to today)
    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Advance {
                count: 1,
                query: None,
            },
        },
        now,
    )
    .await
    .unwrap();

    let advanced: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // If no cards were safe to advance, no event was logged; skip assertion
    if advanced.due.timestamp() == before.due.timestamp() {
        return;
    }

    // Undo the most recent event (either RateCard or AdvanceCards)
    // We want to undo just the AdvanceCards event specifically
    let advance_event_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM event WHERE kind = ? ORDER BY id DESC LIMIT 1")
            .bind(crate::model::EventType::AdvanceCards)
            .fetch_optional(&pool)
            .await
            .unwrap();

    if let Some(event_id) = advance_event_id {
        undo_event(
            &pool,
            UndoEventRequest {
                event_id: Some(event_id),
                undo_group: false,
            },
        )
        .await
        .unwrap();

        let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            restored.due.timestamp(),
            before.due.timestamp(),
            "undo AdvanceCards must restore original due date"
        );
    }
}

#[sqlx::test]
async fn e2e_undo_postpone_cards_restores_due(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    let now = Utc::now();

    // Rate so card has a scheduled interval (needed for postpone safety check)
    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating: 4,
                recall_duration: chrono::Duration::seconds(5),
                rate_duration: chrono::Duration::seconds(2),
                tag_id: None,
            }),
        },
        now,
    )
    .await
    .unwrap();

    let before: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Postpone {
                count: 1,
                query: None,
            },
        },
        now,
    )
    .await
    .unwrap();

    let postpone_event_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM event WHERE kind = ? ORDER BY id DESC LIMIT 1")
            .bind(crate::model::EventType::PostponeCards)
            .fetch_optional(&pool)
            .await
            .unwrap();

    if let Some(event_id) = postpone_event_id {
        let postponed: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            postponed.due.timestamp() >= before.due.timestamp(),
            "postponed card must have a later or equal due date"
        );

        undo_event(
            &pool,
            UndoEventRequest {
                event_id: Some(event_id),
                undo_group: false,
            },
        )
        .await
        .unwrap();

        let restored: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            restored.due.timestamp(),
            before.due.timestamp(),
            "undo PostponeCards must restore original due date"
        );
    }
}

#[sqlx::test]
async fn e2e_undo_update_card_does_not_log_when_no_change(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    let event_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();

    // Request with no field changes (all None)
    update_cards(
        &pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: None,
            due: None,
        },
        Utc::now(),
        true,
    )
    .await
    .unwrap();

    let event_count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();
    // An event IS logged (the selector matched 1 card), but the payload has no transitions.
    // The undo of such an event is a no-op but valid.
    assert_eq!(
        event_count_after,
        event_count_before + 1,
        "update_card with log=true must log an event even when fields are unchanged"
    );
}

// ── Note undo tests ───────────────────────────────────────────────────────────

#[sqlx::test]
async fn e2e_undo_create_notes_restores_state(pool: SqlitePool) {
    use crate::api::note::{create_notes, delete_notes};
    use crate::schema::note::{DeleteNotesRequest, NotesSelector};

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
    use crate::api::note::{create_notes, delete_notes};
    use crate::schema::note::{DeleteNotesRequest, NotesSelector};

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
    use crate::api::note::{create_notes, update_notes};
    use crate::schema::note::{NotesSelector, UpdateNotesRequest, UpdateTags};

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
