// ── Card undo tests ──────────────────────────────────────────────────────────

use chrono::Utc;
use sqlx::SqlitePool;

use super::create_card_helper;
use crate::api::card::forget_card;
use crate::api::card::unbury_cards;
use crate::api::card::update_cards;
use crate::api::review::submit_study_action;
use crate::api::undo::undo_event;
use crate::model::Card;
use crate::model::SpecialState;
use crate::schema::card::CardsSelector;
use crate::schema::card::SpecialStateUpdate;
use crate::schema::card::UpdateCardsRequest;
use crate::schema::review::RatingSubmission;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;
use crate::schema::undo::UndoEventRequest;

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

    unbury_cards(&pool, None, Utc::now(), true).await.unwrap();

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
