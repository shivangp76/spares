//! End-to-end checks of the due dates the scheduler assigns, through the same entry points the
//! server uses.

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::api::card::get_cards;
use crate::api::card::get_leeches;
use crate::api::note::create_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::review::submit_study_action;
use crate::config::read_external_config;
use crate::helpers::FractionalDays;
use crate::model::Card;
use crate::model::CardId;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::parsers::get_all_parsers;
use crate::schedulers::optimal_interval_days;
use crate::schema::card::GetLeechesRequest;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::review::RatingSubmission;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;

const LEARNING: u32 = 1;
const REVIEW: u32 = 2;
const AGAIN: u32 = 1;
const GOOD: u32 = 3;

/// Creates one note per entry of `notes` and returns each note's card ids, in card order.
async fn create_cards(pool: &SqlitePool, notes: &[&str]) -> Vec<Vec<CardId>> {
    let parser = create_parser_helper(pool, "markdown").await;
    let created = create_notes(
        pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: notes
                .iter()
                .map(|data| CreateNoteRequest {
                    data: (*data).to_string(),
                    keywords: vec![],
                    tags: vec![],
                    is_suspended: false,
                    custom_data: Map::new(),
                })
                .collect(),
        },
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    let mut card_ids = Vec::new();
    for note in created.notes {
        let mut cards = get_cards(pool, note.id).await.unwrap();
        cards.sort_by_key(|card| card.order);
        card_ids.push(cards.into_iter().map(|card| card.id).collect());
    }
    card_ids
}

async fn study(pool: &SqlitePool, action: StudyAction, at: DateTime<Utc>) {
    submit_study_action(
        pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action,
        },
        at,
    )
    .await
    .unwrap();
}

async fn rate(pool: &SqlitePool, card_id: CardId, rating: u32, at: DateTime<Utc>) {
    let submission = RatingSubmission {
        card_id,
        rating,
        recall_duration: Duration::seconds(5),
        rate_duration: Duration::seconds(2),
        tag_id: None,
    };
    study(pool, StudyAction::Rate(submission), at).await;
}

async fn fetch_card(pool: &SqlitePool, card_id: CardId) -> Card {
    sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn set_desired_retention(pool: &SqlitePool, card_id: CardId, desired_retention: f64) {
    sqlx::query(r"UPDATE card SET desired_retention = ? WHERE id = ?")
        .bind(desired_retention)
        .bind(card_id)
        .execute(pool)
        .await
        .unwrap();
}

/// Rates `card_id` Good on each of `days_ago`, oldest first, leaving it in review.
async fn graduate(pool: &SqlitePool, card_id: CardId, now: DateTime<Utc>, days_ago: &[i64]) {
    for days in days_ago {
        rate(pool, card_id, GOOD, now - Duration::days(*days)).await;
    }
    assert_eq!(
        fetch_card(pool, card_id).await.state,
        REVIEW,
        "precondition: card graduated"
    );
}

/// Asserts that `card` is due within a fuzz-sized window around its optimal interval after
/// `last_review`. The window is deliberately loose: it pins the target, not the fuzz.
fn assert_due_near_optimal(card: &Card, last_review: DateTime<Utc>) {
    let maximum = read_external_config()
        .unwrap()
        .maximum_interval
        .num_fractional_days();
    let optimal = optimal_interval_days(card.stability, card.desired_retention).min(maximum);
    let actual = (card.due - last_review).num_fractional_days();
    let slack = 1.0 + optimal * 0.2;
    assert!(
        (actual - optimal).abs() <= slack,
        "card {} is due {actual:.2} days after its last review, expected about {optimal:.2} \
         (stability {:.2}, desired retention {})",
        card.id,
        card.stability,
        card.desired_retention,
    );
}

#[sqlx::test]
async fn learning_steps_are_not_spread_out(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    rate(&pool, card_id, GOOD, now).await;

    let card = fetch_card(&pool, card_id).await;
    assert_eq!(
        card.state, LEARNING,
        "precondition: card is in a learning step"
    );
    assert!(
        card.due - now < Duration::hours(1),
        "a learning step must keep its short interval, but the card is due in {} hours",
        (card.due - now).num_hours()
    );
}

#[sqlx::test]
async fn interval_follows_desired_retention(pool: SqlitePool) {
    let cards = create_cards(&pool, &["a {{ b }} c", "d {{ e }} f"]).await;
    let (low, high) = (cards[0][0], cards[1][0]);
    set_desired_retention(&pool, low, 0.7).await;
    set_desired_retention(&pool, high, 0.97).await;
    let now = Utc::now();
    for card_id in [low, high] {
        graduate(&pool, card_id, now, &[10, 9, 5]).await;
    }

    let (low, high) = (fetch_card(&pool, low).await, fetch_card(&pool, high).await);
    assert!(
        (low.stability - high.stability).abs() < 1e-9,
        "precondition: identical histories give identical memory states"
    );
    let last_review = now - Duration::days(5);
    assert_due_near_optimal(&low, last_review);
    assert_due_near_optimal(&high, last_review);
}

#[sqlx::test]
async fn reschedule_moves_card_to_its_fsrs_interval(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    sqlx::query(r"UPDATE card SET due = ? WHERE id = ?")
        .bind((now + Duration::days(3650)).timestamp())
        .bind(card_id)
        .execute(&pool)
        .await
        .unwrap();

    study(&pool, StudyAction::Reschedule, now).await;

    assert_due_near_optimal(&fetch_card(&pool, card_id).await, now - Duration::days(5));
}

#[sqlx::test]
async fn reschedule_respects_a_changed_desired_retention(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    set_desired_retention(&pool, card_id, 0.97).await;

    study(&pool, StudyAction::Reschedule, now).await;

    assert_due_near_optimal(&fetch_card(&pool, card_id).await, now - Duration::days(5));
}

#[sqlx::test]
async fn review_log_records_the_interval_actually_given(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;

    let card = fetch_card(&pool, card_id).await;
    let latest: ReviewLog = sqlx::query_as(
        r"SELECT * FROM review_log WHERE card_id = ? AND kind = ?
          ORDER BY reviewed_at DESC, id DESC LIMIT 1",
    )
    .bind(card_id)
    .bind(ReviewLogKind::Review)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        latest.scheduled_time,
        Some((card.due - latest.reviewed_at).num_seconds())
    );
}

#[sqlx::test]
async fn leeches_are_cards_with_many_lapses(pool: SqlitePool) {
    let cards = create_cards(&pool, &["a {{ b }} c", "d {{ e }} f"]).await;
    let (leech, struggling_learner) = (cards[0][0], cards[1][0]);
    let mut at = Utc::now() - Duration::days(400);
    // Graduate, then lapse and recover repeatedly. Each `Again` is on a card in review.
    rate(&pool, leech, GOOD, at).await;
    rate(&pool, leech, GOOD, at + Duration::days(1)).await;
    for _ in 0..9 {
        at += Duration::days(10);
        rate(&pool, leech, AGAIN, at).await;
        rate(&pool, leech, GOOD, at + Duration::hours(1)).await;
        rate(&pool, leech, GOOD, at + Duration::days(1)).await;
    }
    // Fails just as often, but only ever during its first learning step.
    for i in 0..12 {
        rate(&pool, struggling_learner, AGAIN, at + Duration::minutes(i)).await;
    }

    let leeches = get_leeches(
        &pool,
        GetLeechesRequest {
            scheduler_name: "fsrs".to_string(),
        },
    )
    .await
    .unwrap();
    let leech_ids = leeches.iter().map(|card| card.id).collect::<Vec<_>>();
    assert_eq!(leech_ids, vec![leech]);
}

#[sqlx::test]
async fn advance_and_postpone_accept_a_query(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    let query = Some("c.stability>=0".to_string());
    study(
        &pool,
        StudyAction::Advance {
            count: 1,
            query: query.clone(),
        },
        now,
    )
    .await;
    study(&pool, StudyAction::Postpone { count: 1, query }, now).await;
}

fn distinct_local_days(cards: &[Card]) -> usize {
    cards
        .iter()
        .map(|card| card.due.with_timezone(&chrono::Local).date_naive())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

#[sqlx::test]
async fn siblings_reviewed_together_are_due_on_different_days(pool: SqlitePool) {
    let siblings = create_cards(&pool, &["a {{ b }} c {{ d }} e {{ f }}"]).await[0].clone();
    assert_eq!(siblings.len(), 3, "precondition: one card per cloze");
    let now = Utc::now();
    for card_id in &siblings {
        graduate(&pool, *card_id, now, &[10, 9, 5]).await;
    }

    let mut cards = Vec::new();
    for card_id in &siblings {
        cards.push(fetch_card(&pool, *card_id).await);
    }
    for card in &cards {
        assert_due_near_optimal(card, now - Duration::days(5));
    }
    assert_eq!(distinct_local_days(&cards), 3, "{cards:#?}");
}

#[sqlx::test]
async fn reschedule_spreads_siblings_apart(pool: SqlitePool) {
    let siblings = create_cards(&pool, &["a {{ b }} c {{ d }} e {{ f }}"]).await[0].clone();
    let now = Utc::now();
    for card_id in &siblings {
        graduate(&pool, *card_id, now, &[10, 9, 5]).await;
    }
    // Bunch them up on a single day.
    sqlx::query(r"UPDATE card SET due = ?")
        .bind((now + Duration::days(10)).timestamp())
        .execute(&pool)
        .await
        .unwrap();

    study(&pool, StudyAction::Reschedule, now).await;

    let mut cards = Vec::new();
    for card_id in &siblings {
        cards.push(fetch_card(&pool, *card_id).await);
    }
    for card in &cards {
        assert_due_near_optimal(card, now - Duration::days(5));
    }
    assert_eq!(distinct_local_days(&cards), 3, "{cards:#?}");
}
