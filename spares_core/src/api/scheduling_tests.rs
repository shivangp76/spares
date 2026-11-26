//! End-to-end checks of the due dates the scheduler assigns, through the same entry points the
//! server uses.

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::api::card::get_cards;
use crate::api::card::get_leeches;
use crate::api::card::move_cards_past_break;
use crate::api::card::update_cards;
use crate::api::note::create_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::review::reschedule_all_cards;
use crate::api::review::submit_study_action;
use crate::api::scheduler::reschedule_scheduler;
use crate::api::undo::undo_event;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::helpers::FractionalDays;
use crate::model::Card;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::RescheduleOrigin;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::parsers::get_all_parsers;
use crate::schedulers::BreakPlan;
use crate::schedulers::get_scheduler_from_string;
use crate::schedulers::optimal_interval_days;
use crate::schema::card::CardsSelector;
use crate::schema::card::GetLeechesRequest;
use crate::schema::card::UpdateCardsRequest;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::review::RatingSubmission;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;
use crate::schema::undo::UndoEventRequest;

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
    let optimal = optimal_interval_days(card).min(maximum);
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

    reschedule_scheduler(&pool, "fsrs", now).await.unwrap();

    assert_due_near_optimal(&fetch_card(&pool, card_id).await, now - Duration::days(5));
}

#[sqlx::test]
async fn reschedule_respects_a_changed_desired_retention(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    set_desired_retention(&pool, card_id, 0.97).await;

    reschedule_scheduler(&pool, "fsrs", now).await.unwrap();

    assert_due_near_optimal(&fetch_card(&pool, card_id).await, now - Duration::days(5));
}

#[sqlx::test]
async fn reschedule_is_logged_with_the_cards_it_changed(pool: SqlitePool) {
    let cards = create_cards(&pool, &["a {{ b }} c", "d {{ e }} f"]).await;
    let (changed, unreviewed) = (cards[0][0], cards[1][0]);
    let now = Utc::now();
    graduate(&pool, changed, now, &[10, 9, 5]).await;
    set_desired_retention(&pool, changed, 0.97).await;
    let before = fetch_card(&pool, changed).await;

    let response = reschedule_scheduler(&pool, "fsrs", now).await.unwrap();
    let rescheduled = fetch_card(&pool, changed).await;
    assert_ne!(rescheduled.due, before.due);

    let (event_id, payload): (i64, serde_json::Value) =
        sqlx::query_as("SELECT id, payload FROM event WHERE kind = ?")
            .bind(EventType::RescheduleCards)
            .fetch_one(&pool)
            .await
            .unwrap();
    let card_ids: Vec<i64> = payload
        .as_array()
        .unwrap()
        .iter()
        .map(|card| card["card_id"].as_i64().unwrap())
        .collect();
    assert_eq!(card_ids, [changed], "{unreviewed} did not change");
    // Returned, so the web app can offer to undo it
    assert_eq!(response.event_id, Some(event_id));

    // A plain undo skips it, as an automatic change, but it can be undone by id
    let latest = undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!latest.undone_events.iter().any(|e| e.id == event_id));
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(event_id),
            undo_group: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(fetch_card(&pool, changed).await.due, before.due);
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
    let threshold = read_external_config().unwrap().leech.lapses_threshold;
    let cards = create_cards(&pool, &["a {{ b }} c", "d {{ e }} f", "g {{ h }} i"]).await;
    let (leech, almost_leech, struggling_learner) = (cards[0][0], cards[1][0], cards[2][0]);
    let mut at = Utc::now() - Duration::days(400);
    // Graduate, then lapse and recover repeatedly. Each `Again` is on a card in review. A card
    // becomes a leech on reaching the threshold, not on exceeding it.
    for (card_id, lapses) in [(leech, threshold), (almost_leech, threshold - 1)] {
        let mut at = at;
        rate(&pool, card_id, GOOD, at).await;
        rate(&pool, card_id, GOOD, at + Duration::days(1)).await;
        for _ in 0..lapses {
            at += Duration::days(10);
            rate(&pool, card_id, AGAIN, at).await;
            rate(&pool, card_id, GOOD, at + Duration::hours(1)).await;
            rate(&pool, card_id, GOOD, at + Duration::days(1)).await;
        }
    }
    // Fails more often, but only ever during its first learning step.
    at += Duration::days(10 * i64::from(threshold + 1));
    for i in 0..i64::from(threshold + 4) {
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
        .map(|card| crate::helpers::study_date(card.due, crate::api::queue::tests::zone()))
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

    reschedule_scheduler(&pool, "fsrs", now).await.unwrap();

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
async fn reschedule_includes_suspended_and_buried_cards(pool: SqlitePool) {
    let cards = create_cards(&pool, &["a {{ b }} c", "d {{ e }} f"]).await;
    let (suspended, buried) = (cards[0][0], cards[1][0]);
    let now = Utc::now();
    let bogus_due = now + Duration::days(3650);
    for (card_id, special_state) in [
        (suspended, SpecialState::Suspended),
        (buried, SpecialState::UserBuried),
    ] {
        graduate(&pool, card_id, now, &[10, 9, 5]).await;
        sqlx::query(r"UPDATE card SET due = ?, stability = 1000, special_state = ? WHERE id = ?")
            .bind(bogus_due.timestamp())
            .bind(special_state)
            .bind(card_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    reschedule_scheduler(&pool, "fsrs", now).await.unwrap();

    for (card_id, special_state) in [
        (suspended, SpecialState::Suspended),
        (buried, SpecialState::UserBuried),
    ] {
        let card = fetch_card(&pool, card_id).await;
        assert_eq!(
            card.special_state,
            Some(special_state),
            "the special state is kept"
        );
        assert!(card.stability < 1000.0, "the memory state is recomputed");
        assert_due_near_optimal(&card, now - Duration::days(5));
    }
}

#[sqlx::test]
async fn rating_walks_the_learning_steps(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let steps = read_external_config().unwrap().learning_steps;
    assert_eq!(
        steps.len(),
        2,
        "precondition: the default two learning steps"
    );
    // Whole seconds, as the database stores them.
    let mut at = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap() - Duration::days(1);

    rate(&pool, card_id, GOOD, at).await;
    let card = fetch_card(&pool, card_id).await;
    assert_eq!((card.state, card.due - at), (LEARNING, steps[1]));

    at += steps[1];
    rate(&pool, card_id, AGAIN, at).await;
    let card = fetch_card(&pool, card_id).await;
    assert_eq!((card.state, card.due - at), (LEARNING, steps[0]));

    at += steps[0];
    rate(&pool, card_id, GOOD, at).await;
    let card = fetch_card(&pool, card_id).await;
    assert_eq!(
        (card.state, card.due - at),
        (LEARNING, steps[1]),
        "Good on the first step moves to the second, read back from the stored delay"
    );

    at += steps[1];
    rate(&pool, card_id, GOOD, at).await;
    assert_eq!(fetch_card(&pool, card_id).await.state, REVIEW);
}

/// `at` without its fraction of a second, as the database stores it.
fn whole_seconds(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(at.timestamp(), 0).unwrap()
}

/// Gives `card_id` a due date by hand, as "Set Card Due Date" does, and returns the event.
async fn set_due(pool: &SqlitePool, card_id: CardId, due: DateTime<Utc>, at: DateTime<Utc>) -> i64 {
    update_cards(
        pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: None,
            due: Some(due),
            tags: UpdateTags::None,
        },
        at,
        true,
    )
    .await
    .unwrap()
    .event_id
    .unwrap()
}

async fn reschedule_with(pool: &SqlitePool, config: &SparesExternalConfig, at: DateTime<Utc>) {
    let scheduler = get_scheduler_from_string("fsrs").unwrap();
    reschedule_all_cards(pool, scheduler.as_ref(), config, at)
        .await
        .unwrap();
}

async fn reschedule_rows(pool: &SqlitePool, card_id: CardId) -> Vec<ReviewLog> {
    sqlx::query_as(r"SELECT * FROM review_log WHERE card_id = ? AND kind = ? ORDER BY id")
        .bind(card_id)
        .bind(ReviewLogKind::Reschedule)
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Sets a bogus stability, so a test can tell that rescheduling recomputed it.
async fn spoil_stability(pool: &SqlitePool, card_id: CardId) {
    sqlx::query(r"UPDATE card SET stability = 1000 WHERE id = ?")
        .bind(card_id)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test]
async fn reschedule_keeps_a_due_date_set_by_hand(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    let stability = fetch_card(&pool, card_id).await.stability;
    let exam = whole_seconds(now + Duration::days(40));
    set_due(&pool, card_id, exam, now).await;
    let rows = reschedule_rows(&pool, card_id).await;
    assert_eq!(rows.len(), 1, "the move is logged");
    assert_eq!(
        rows[0].reschedule_data().unwrap().origin,
        RescheduleOrigin::Manual
    );
    spoil_stability(&pool, card_id).await;

    reschedule_with(&pool, &read_external_config().unwrap(), now).await;

    let card = fetch_card(&pool, card_id).await;
    assert_eq!(card.due, exam, "the due date set by hand is kept");
    assert!(
        (card.stability - stability).abs() < 1e-6,
        "the memory state is still recomputed: {} != {stability}",
        card.stability
    );
}

#[sqlx::test]
async fn reschedule_keeps_a_postponed_card(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    // Postpone moves cards due today.
    let before = whole_seconds(now);
    sqlx::query(r"UPDATE card SET due = ? WHERE id = ?")
        .bind(before.timestamp())
        .bind(card_id)
        .execute(&pool)
        .await
        .unwrap();
    study(
        &pool,
        StudyAction::Postpone {
            count: 1,
            query: None,
        },
        now,
    )
    .await;
    let postponed = fetch_card(&pool, card_id).await;
    assert_ne!(postponed.due, before, "precondition: postponed");
    spoil_stability(&pool, card_id).await;

    reschedule_with(&pool, &read_external_config().unwrap(), now).await;

    let card = fetch_card(&pool, card_id).await;
    assert_eq!(card.due, postponed.due);
    assert!(card.stability < 1000.0, "the memory state is recomputed");
}

#[sqlx::test]
async fn a_review_after_a_move_lets_reschedule_move_the_card(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9]).await;
    set_due(
        &pool,
        card_id,
        whole_seconds(now - Duration::days(5)),
        now - Duration::days(6),
    )
    .await;
    rate(&pool, card_id, GOOD, now - Duration::days(5)).await;
    sqlx::query(r"UPDATE card SET due = ? WHERE id = ?")
        .bind((now + Duration::days(3650)).timestamp())
        .bind(card_id)
        .execute(&pool)
        .await
        .unwrap();

    reschedule_with(&pool, &read_external_config().unwrap(), now).await;

    assert_due_near_optimal(&fetch_card(&pool, card_id).await, now - Duration::days(5));
}

#[sqlx::test]
async fn reschedule_moved_cards_moves_them_too(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    set_due(&pool, card_id, now + Duration::days(150), now).await;
    let mut config = read_external_config().unwrap();
    config.reschedule_moved_cards = true;

    reschedule_with(&pool, &config, now).await;

    assert_due_near_optimal(&fetch_card(&pool, card_id).await, now - Duration::days(5));
}

#[sqlx::test]
async fn changing_desired_retention_keeps_a_due_date_set_by_hand(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    let exam = whole_seconds(now + Duration::days(40));
    set_due(&pool, card_id, exam, now).await;

    update_cards(
        &pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: Some(0.97),
            special_state: None,
            due: None,
            tags: UpdateTags::None,
        },
        now,
        true,
    )
    .await
    .unwrap();

    assert_eq!(fetch_card(&pool, card_id).await.due, exam);
}

#[sqlx::test]
async fn reschedule_places_other_cards_around_a_kept_card(pool: SqlitePool) {
    let siblings = create_cards(&pool, &["a {{ b }} c {{ d }} e"]).await[0].clone();
    let (kept, other) = (siblings[0], siblings[1]);
    let now = Utc::now();
    for card_id in &siblings {
        graduate(&pool, *card_id, now, &[10, 9, 5]).await;
    }
    // Onto the day the other sibling was placed on, which is where rescheduling would put it.
    let other_due = fetch_card(&pool, other).await.due;
    set_due(&pool, kept, other_due, now).await;
    let mut config = read_external_config().unwrap();
    config.disperse_siblings = true;

    reschedule_with(&pool, &config, now).await;

    let cards = [
        fetch_card(&pool, kept).await,
        fetch_card(&pool, other).await,
    ];
    assert_eq!(cards[0].due, other_due);
    assert_due_near_optimal(&cards[1], now - Duration::days(5));
    assert_eq!(
        distinct_local_days(&cards),
        2,
        "the other sibling moves off the kept one's day: {cards:#?}"
    );
}

#[sqlx::test]
async fn undoing_a_due_date_set_by_hand_deletes_its_row(pool: SqlitePool) {
    let card_id = create_cards(&pool, &["a {{ b }} c"]).await[0][0];
    let now = Utc::now();
    graduate(&pool, card_id, now, &[10, 9, 5]).await;
    let before = fetch_card(&pool, card_id).await.due;
    let event_id = set_due(&pool, card_id, now + Duration::days(40), now).await;
    let kind: EventType = sqlx::query_scalar("SELECT kind FROM event WHERE id = ?")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kind, EventType::SetCardsDue);

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(event_id),
            undo_group: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(fetch_card(&pool, card_id).await.due, before);
    assert_eq!(reschedule_rows(&pool, card_id).await.len(), 0);
}

async fn set_raw_due(pool: &SqlitePool, card_id: CardId, due: DateTime<Utc>) {
    sqlx::query(r"UPDATE card SET due = ? WHERE id = ?")
        .bind(due.timestamp())
        .bind(card_id)
        .execute(pool)
        .await
        .unwrap();
}

fn break_config() -> SparesExternalConfig {
    let mut config = read_external_config().unwrap();
    config.load_balance = true;
    config.disperse_siblings = true;
    config.easy_days.specific_dates.clear();
    config
}

/// A break of 5 days from today, spread over the 4 days after it.
fn break_from_today(config: &SparesExternalConfig, now: DateTime<Utc>) -> BreakPlan {
    BreakPlan {
        start: config.zone().date(now),
        days: 5,
        spread: 4,
    }
}

async fn take_break(
    pool: &SqlitePool,
    config: &SparesExternalConfig,
    plan: BreakPlan,
    now: DateTime<Utc>,
) -> Option<i64> {
    let scheduler = get_scheduler_from_string("fsrs").unwrap();
    move_cards_past_break(pool, scheduler.as_ref(), config, plan, now, false)
        .await
        .unwrap()
        .1
}

/// `count` cards in review, one per note, each due on a day of the break or overdue, and the
/// cards of `extra` other new notes, one each.
async fn cards_due_in_break(
    pool: &SqlitePool,
    count: usize,
    extra: usize,
    now: DateTime<Utc>,
) -> (Vec<CardId>, Vec<CardId>) {
    let notes = (0..count + extra)
        .map(|i| format!("note {i} {{{{ answer {i} }}}}"))
        .collect::<Vec<_>>();
    let notes = notes.iter().map(String::as_str).collect::<Vec<_>>();
    let mut created = create_cards(pool, &notes)
        .await
        .into_iter()
        .map(|cards| cards[0])
        .collect::<Vec<_>>();
    let extras = created.split_off(count);
    let mut card_ids = Vec::new();
    for (i, card_id) in (0_i64..).zip(created) {
        graduate(pool, card_id, now, &[10, 9, 5]).await;
        // From two days overdue to the break's fourth day
        set_raw_due(pool, card_id, now + Duration::days(i % 6 - 2)).await;
        card_ids.push(card_id);
    }
    (card_ids, extras)
}

#[sqlx::test]
async fn a_break_moves_the_cards_due_in_it_onto_the_days_after_it(pool: SqlitePool) {
    let now = Utc::now();
    let config = break_config();
    let plan = break_from_today(&config, now);
    let (moved, others) = cards_due_in_break(&pool, 20, 3, now).await;
    let (after_break, suspended, learning) = (others[0], others[1], others[2]);
    graduate(&pool, after_break, now, &[10, 9, 5]).await;
    set_raw_due(&pool, after_break, now + Duration::days(12)).await;
    graduate(&pool, suspended, now, &[10, 9, 5]).await;
    set_raw_due(&pool, suspended, now + Duration::days(1)).await;
    sqlx::query(r"UPDATE card SET special_state = ? WHERE id = ?")
        .bind(SpecialState::Suspended)
        .bind(suspended)
        .execute(&pool)
        .await
        .unwrap();
    rate(&pool, learning, GOOD, now).await;
    let untouched = [after_break, suspended, learning];
    let mut before = Vec::new();
    for card_id in untouched {
        before.push(fetch_card(&pool, card_id).await.due);
    }

    take_break(&pool, &config, plan, now).await.unwrap();

    let spread_days = plan.spread_days();
    let mut days = std::collections::HashMap::new();
    for card_id in &moved {
        let day = config.zone().date(fetch_card(&pool, *card_id).await.due);
        assert!(spread_days.contains(&day), "card {card_id} is due on {day}");
        *days.entry(day).or_insert(0) += 1;
        let rows = reschedule_rows(&pool, *card_id).await;
        assert_eq!(
            rows.last().unwrap().reschedule_data().unwrap().origin,
            RescheduleOrigin::Break
        );
    }
    for (card_id, due) in untouched.into_iter().zip(before) {
        assert_eq!(fetch_card(&pool, card_id).await.due, due, "card {card_id}");
    }
    // Load balancing spreads them rather than piling them onto the first day.
    assert_eq!(days.len(), spread_days.len(), "{days:?}");
    assert!(days.values().all(|count| *count <= 8), "{days:?}");
}

#[sqlx::test]
async fn a_break_that_starts_later_leaves_the_cards_due_before_it(pool: SqlitePool) {
    let now = Utc::now();
    let config = break_config();
    let today = config.zone().date(now);
    let plan = BreakPlan {
        start: today + chrono::Days::new(3),
        days: 4,
        spread: 3,
    };
    let cards = cards_due_in_break(&pool, 6, 0, now).await.0;
    let mut before = Vec::new();
    for card_id in &cards {
        before.push(fetch_card(&pool, *card_id).await);
    }

    take_break(&pool, &config, plan, now).await;

    for card in before {
        let due = fetch_card(&pool, card.id).await.due;
        if config.zone().date(card.due) < plan.start {
            assert_eq!(due, card.due, "card {} was due before the break", card.id);
        } else {
            let day = config.zone().date(due);
            assert!(
                plan.spread_days().contains(&day),
                "card {} on {day}",
                card.id
            );
        }
    }
}

#[sqlx::test]
async fn a_break_keeps_siblings_apart(pool: SqlitePool) {
    let now = Utc::now();
    let config = break_config();
    let siblings = create_cards(&pool, &["a {{ b }} c {{ d }} e {{ f }}"]).await[0].clone();
    for card_id in &siblings {
        graduate(&pool, *card_id, now, &[10, 9, 5]).await;
        set_raw_due(&pool, *card_id, now + Duration::days(2)).await;
    }

    take_break(&pool, &config, break_from_today(&config, now), now).await;

    let mut cards = Vec::new();
    for card_id in &siblings {
        cards.push(fetch_card(&pool, *card_id).await);
    }
    assert_eq!(distinct_local_days(&cards), 3, "{cards:#?}");
}

#[sqlx::test]
async fn a_dry_run_break_moves_nothing(pool: SqlitePool) {
    let now = Utc::now();
    let config = break_config();
    let cards = cards_due_in_break(&pool, 6, 0, now).await.0;
    let mut before = Vec::new();
    for card_id in &cards {
        before.push(fetch_card(&pool, *card_id).await.due);
    }
    let scheduler = get_scheduler_from_string("fsrs").unwrap();

    let (outcome, event_id) = move_cards_past_break(
        &pool,
        scheduler.as_ref(),
        &config,
        break_from_today(&config, now),
        now,
        true,
    )
    .await
    .unwrap();

    assert_eq!(outcome.card_payloads.len(), cards.len());
    let (recall_before, recall_after) = outcome.mean_retrievability.unwrap();
    assert!(recall_after < recall_before, "waiting lowers recall");
    assert_eq!(event_id, None);
    for (card_id, due) in cards.into_iter().zip(before) {
        assert_eq!(fetch_card(&pool, card_id).await.due, due);
        assert_eq!(reschedule_rows(&pool, card_id).await.len(), 0);
    }
}

#[sqlx::test]
async fn undoing_a_break_puts_every_card_back(pool: SqlitePool) {
    let now = Utc::now();
    let config = break_config();
    let cards = cards_due_in_break(&pool, 10, 0, now).await.0;
    let mut before = Vec::new();
    for card_id in &cards {
        before.push(fetch_card(&pool, *card_id).await.due);
    }
    let event_id = take_break(&pool, &config, break_from_today(&config, now), now)
        .await
        .unwrap();

    undo_event(
        &pool,
        UndoEventRequest {
            event_id: Some(event_id),
            undo_group: false,
        },
    )
    .await
    .unwrap();

    for (card_id, due) in cards.into_iter().zip(before) {
        assert_eq!(fetch_card(&pool, card_id).await.due, due);
        assert_eq!(reschedule_rows(&pool, card_id).await.len(), 0);
    }
}

#[sqlx::test]
async fn reschedule_keeps_cards_where_a_break_put_them(pool: SqlitePool) {
    let now = Utc::now();
    let config = break_config();
    let cards = cards_due_in_break(&pool, 6, 0, now).await.0;
    take_break(&pool, &config, break_from_today(&config, now), now).await;
    let mut after_break = Vec::new();
    for card_id in &cards {
        after_break.push(fetch_card(&pool, *card_id).await.due);
    }

    reschedule_with(&pool, &config, now).await;

    for (card_id, due) in cards.into_iter().zip(after_break) {
        assert_eq!(fetch_card(&pool, card_id).await.due, due);
    }
}
