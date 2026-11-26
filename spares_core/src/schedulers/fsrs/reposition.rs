use std::cmp;
use std::collections::HashMap;

use chrono::DateTime;
use chrono::Days;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use itertools::Itertools;
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;
use serde_json::to_value;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use super::SCHEDULER_NAME;
use super::utils::Curve;
use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::api::review::insert_review;
use crate::api::statistics::local_time::Zone;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::config::SparesExternalConfig;
use crate::helpers::FractionalDays;
use crate::helpers::mean;
use crate::model::Card;
use crate::model::CardId;
use crate::model::RescheduleData;
use crate::model::RescheduleOrigin;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::schedulers::BreakOutcome;
use crate::schedulers::BreakPlan;
use crate::schedulers::CardMemory;
use crate::schedulers::MoveCardsResult;
use crate::schedulers::placement::PlacementContext;
use crate::schedulers::state_ids_sql;
use crate::schema::review::StateRole;

#[derive(Debug)]
pub struct CardInternal<'a> {
    card: &'a Card,
    estimate: Estimate,
}

/// Where a card in review stands, to tell whether moving it is safe.
#[derive(Clone, Copy, Debug)]
struct Estimate {
    memory: CardMemory,
    desired_retention: f64,
    // Advance
    current_elapsed_time: Duration,
    scheduled_time: Option<Duration>,
    current_retention: f64,
    // Postpone
    approx_retention_after_postpone: f64,
}

/// A card's last graded review: when, and the interval it gave, in seconds.
type LastReview = (i64, Option<i64>);

#[derive(Debug, PartialEq)]
pub enum MoveCardAction {
    Advance,
    Postpone,
}

/// How many of the cards in review matching `where_clause` (over `card c`, with `?1` bound to
/// `bound`) are safe to move. One query, reading only what the estimate needs, so it stays quick
/// with tens of thousands of cards: statistics show it on every load.
pub async fn count_safe_cards(
    db: &SqlitePool,
    where_clause: &str,
    bound: i64,
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
    curve: &Curve,
) -> Result<u32, Error> {
    let (json,): (String,) = sqlx::query_as(AssertSqlSafe(format!(
        r"SELECT json_group_array(json_array(c.stability, c.stability_fast, c.difficulty,
                 c.desired_retention, l.reviewed_at, l.scheduled_time))
          FROM card c LEFT JOIN review_log l ON l.id = ({LAST_REVIEW_ID})
          WHERE {where_clause}"
    )))
    .bind(bound)
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    #[expect(clippy::type_complexity, reason = "one row of the query")]
    let cards: Vec<(f64, f64, f64, f64, Option<i64>, Option<i64>)> = serde_json::from_str(&json)
        .map_err(|e| Error::Sqlx {
            source: sqlx::Error::Decode(Box::new(e)),
        })?;
    let count = cards
        .into_iter()
        .filter_map(
            |(
                stability,
                stability_fast,
                difficulty,
                desired_retention,
                reviewed_at,
                scheduled_time,
            )| {
                estimate(
                    CardMemory {
                        stability,
                        stability_fast,
                        difficulty,
                    },
                    desired_retention,
                    reviewed_at.map(|at| (at, scheduled_time)),
                    requested_date,
                    curve,
                )
            },
        )
        .filter(|estimate| is_card_safe(estimate, action))
        .count();
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// The id of the last graded review of `c.id`. Only graded reviews carry a `scheduled_time`,
/// and a forget marker would anchor the elapsed time on a history the user discarded.
const LAST_REVIEW_ID: &str =
    "SELECT rl.id FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = 0
     ORDER BY rl.reviewed_at DESC, rl.id DESC LIMIT 1";

fn is_card_safe(d: &Estimate, action: &MoveCardAction) -> bool {
    match action {
        MoveCardAction::Advance => {
            1. - (1. / d.current_retention - 1.) / (1. / d.desired_retention - 1.) < 0.13
        }
        MoveCardAction::Postpone => {
            (1. / d.approx_retention_after_postpone - 1.) / (1. / d.desired_retention - 1.) - 1.
                < 0.15
        }
    }
}

fn card_sort_key(d: &Estimate, action: &MoveCardAction) -> (f64, f64) {
    match action {
        // sort by (1 - elapsed_day / scheduled_day)
        // = 1-ln(current retention)/ln(requested retention), -stability (ascending)
        MoveCardAction::Advance => (
            1. - (1. / d.current_retention - 1.) / (1. / d.desired_retention - 1.),
            -d.memory.stability,
        ),
        // sort by (elapsed_days / scheduled_days - 1)
        // = ln(current retention)/ln(requested retention)-1, -stability (ascending)
        MoveCardAction::Postpone => (
            ((1. / d.approx_retention_after_postpone - 1.) / (1. / d.desired_retention - 1.) - 1.),
            -d.memory.stability,
        ),
    }
}

/// `None` if the card was last reviewed after `requested_date` (e.g. statistics for a past date),
/// so its current schedule did not exist yet at that date and it cannot be moved from there.
fn estimate(
    memory: CardMemory,
    desired_retention: f64,
    last_review: Option<LastReview>,
    requested_date: DateTime<Utc>,
    curve: &Curve,
) -> Option<Estimate> {
    let scheduled_time = last_review
        .and_then(|(_, scheduled_time)| scheduled_time)
        .map(Duration::seconds);

    // Advance
    let current_elapsed_time = last_review.map_or_else(Duration::zero, |(reviewed_at, _)| {
        requested_date - DateTime::from_timestamp(reviewed_at, 0).unwrap_or_default()
    });
    if current_elapsed_time < Duration::zero() {
        return None;
    }
    // Equivalent to `current_retrievability`.
    let current_retention =
        curve.retrievability(memory, current_elapsed_time.num_fractional_days());
    assert!(
        !current_retention.is_nan(),
        "card in review has memory {memory:?}"
    );

    // Postpone
    let approx_elapsed_time_after_postpone =
        scheduled_time
            .as_ref()
            .map_or_else(Duration::zero, |scheduled_time| {
                Duration::fractional_days(
                    current_elapsed_time.num_fractional_days()
                        + scheduled_time.num_fractional_days() * 0.075,
                )
            });
    let approx_retention_after_postpone = curve.retrievability(
        memory,
        approx_elapsed_time_after_postpone.num_fractional_days(),
    );
    Some(Estimate {
        memory,
        desired_retention,
        current_elapsed_time,
        scheduled_time,
        current_retention,
        approx_retention_after_postpone,
    })
}

/// The last graded review of each of `cards` that has one, in one query.
async fn last_reviews(
    db: &SqlitePool,
    cards: &[Card],
) -> Result<HashMap<CardId, LastReview>, Error> {
    let ids = format!("[{}]", cards.iter().map(|card| card.id).join(","));
    let (json,): (String,) = sqlx::query_as(AssertSqlSafe(format!(
        r"SELECT json_group_array(json_array(c.id, l.reviewed_at, l.scheduled_time))
          FROM (SELECT value AS id FROM json_each(?)) c
          JOIN review_log l ON l.id = ({LAST_REVIEW_ID})"
    )))
    .bind(ids)
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let rows: Vec<(CardId, i64, Option<i64>)> =
        serde_json::from_str(&json).map_err(|e| Error::Sqlx {
            source: sqlx::Error::Decode(Box::new(e)),
        })?;
    Ok(rows
        .into_iter()
        .map(|(card_id, reviewed_at, scheduled_time)| (card_id, (reviewed_at, scheduled_time)))
        .collect())
}

async fn get_all_cards_internal<'a>(
    db: &SqlitePool,
    cards: &'a [Card],
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
    curve: &Curve,
) -> Result<Vec<CardInternal<'a>>, Error> {
    let last_reviews = last_reviews(db, cards).await?;
    let mut cards_internal = cards
        .iter()
        .filter_map(|card| {
            let estimate = estimate(
                CardMemory::from(card),
                card.desired_retention,
                last_reviews.get(&card.id).copied(),
                requested_date,
                curve,
            )?;
            Some(CardInternal { card, estimate })
        })
        .collect::<Vec<_>>();
    cards_internal.sort_by(|a, b| {
        let (a_primary, a_secondary) = card_sort_key(&a.estimate, action);
        let (b_primary, b_secondary) = card_sort_key(&b.estimate, action);
        a_primary
            .total_cmp(&b_primary)
            .then(a_secondary.total_cmp(&b_secondary))
    });
    Ok(cards_internal)
}

// DB is used to:
// 1. get latest review log for each card
// 2. update card with new due date
// 3. log the move as a `ReviewLogKind::Reschedule`, at `moved_at`
#[expect(clippy::too_many_lines)]
pub async fn move_cards(
    db: &SqlitePool,
    count: u32,
    cards: &[Card],
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
    moved_at: DateTime<Utc>,
    config: &SparesExternalConfig,
) -> Result<MoveCardsResult, Error> {
    let curve = Curve::new(config);
    let cards_internal = get_all_cards_internal(db, cards, action, requested_date, &curve).await?;
    let (mut safe_cards, not_safe_cards): (Vec<_>, Vec<_>) = cards_internal
        .into_iter()
        .partition(|x| is_card_safe(&x.estimate, action));
    let cards_internal = if count as usize > safe_cards.len() {
        safe_cards.extend(
            not_safe_cards
                .into_iter()
                .take(count as usize - safe_cards.len()),
        );
        safe_cards
    } else {
        safe_cards
            .into_iter()
            .take(count as usize)
            .collect::<Vec<_>>()
    };

    let mut card_changes = Vec::new();
    let mut prev_target_retentions = Vec::new();
    let mut new_target_retentions = Vec::new();
    for card_internal in cards_internal.into_iter().take(count as usize) {
        // Set card's due date
        let (new_due, new_scheduled_time) = match action {
            MoveCardAction::Advance => {
                (requested_date, card_internal.estimate.current_elapsed_time)
            }
            MoveCardAction::Postpone => {
                let delay_time = card_internal
                    .estimate
                    .scheduled_time
                    .map_or_else(Duration::zero, |scheduled_time| {
                        card_internal.estimate.current_elapsed_time - scheduled_time
                    });
                let mut rng = rand::rng();
                let rand_float: f64 = rng.random();
                let new_scheduled_time = cmp::min(
                    cmp::max(
                        config.minimum_interval,
                        Duration::fractional_days(
                            card_internal
                                .estimate
                                .scheduled_time
                                .unwrap_or_else(Duration::zero)
                                .num_fractional_days()
                                * (1.05 + 0.05 * rand_float),
                        ) + delay_time,
                    ),
                    config.maximum_interval,
                );
                (requested_date + new_scheduled_time, new_scheduled_time)
            }
        };
        let _update_card_result =
            sqlx::query(r"UPDATE card SET due = ?, updated_at = ? WHERE id = ?")
                .bind(new_due.timestamp())
                .bind(requested_date.timestamp())
                .bind(card_internal.card.id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        let data = RescheduleData {
            previous_due: card_internal.card.due,
            due: new_due,
            origin: match action {
                MoveCardAction::Advance => RescheduleOrigin::Advance,
                MoveCardAction::Postpone => RescheduleOrigin::Postpone,
            },
        };
        insert_review(
            db,
            &ReviewLog {
                card_id: Some(card_internal.card.id),
                reviewed_at: moved_at,
                kind: ReviewLogKind::Reschedule,
                scheduler_name: SCHEDULER_NAME.to_string(),
                previous_state: card_internal.card.state,
                custom_data: to_value(data).unwrap(),
                ..ReviewLog::new()
            },
            None,
        )
        .await?;
        card_changes.push(UpdateCardPayload {
            card_id: card_internal.card.id,
            order: None,
            back_type: None,
            due: Some(Transition {
                before: card_internal.card.due,
                after: new_due,
            }),
            stability: None,
            stability_fast: None,
            difficulty: None,
            desired_retention: None,
            special_state: None,
            state: None,
            custom_data: None,
        });

        let prev_target_retention = curve.retrievability(
            card_internal.estimate.memory,
            card_internal
                .estimate
                .scheduled_time
                .unwrap_or_else(Duration::zero)
                .num_fractional_days(),
        );
        prev_target_retentions.push(prev_target_retention);
        let new_target_retention = curve.retrievability(
            card_internal.estimate.memory,
            new_scheduled_time.num_fractional_days(),
        );
        new_target_retentions.push(new_target_retention);
    }

    let message = if !prev_target_retentions.is_empty() && !new_target_retentions.is_empty() {
        format!(
            "Mean target retention of moved cards: {:?} -> {:?}",
            mean(&prev_target_retentions).unwrap(),
            mean(&new_target_retentions).unwrap()
        )
    } else {
        String::new()
    };
    Ok(MoveCardsResult {
        card_payloads: card_changes,
        message,
    })
}

/// `due` moved to `day`, at the same time of day, so that a card due at the hour it was reviewed
/// keeps that hour. The start of `day` if a daylight saving change would move it off the day.
fn on_day(zone: Zone, due: DateTime<Utc>, day: NaiveDate) -> DateTime<Utc> {
    let moved = due + Duration::days((day - zone.date(due)).num_days());
    if zone.date(moved) == day {
        moved
    } else {
        zone.start_of(day)
    }
}

/// The cards in review, not suspended or buried, due before `plan` ends: from its start, or
/// overdue too if it starts today.
async fn cards_due_in_break(
    db: &SqlitePool,
    zone: Zone,
    today: NaiveDate,
    plan: BreakPlan,
) -> Result<Vec<Card>, Error> {
    let from = if plan.start <= today {
        DateTime::<Utc>::MIN_UTC
    } else {
        zone.start_of(plan.start)
    };
    let to = zone.start_of(plan.end());
    sqlx::query_as(AssertSqlSafe(format!(
        r"SELECT * FROM card
          WHERE state IN ({})
            AND special_state IS NULL
            AND due >= ? AND due < ?
          ORDER BY due, id",
        state_ids_sql(&[StateRole::Review])
    )))
    .bind(from.timestamp())
    .bind(to.timestamp())
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })
}

/// Moves `card` to `due`, logged as a [`ReviewLogKind::Reschedule`] row of a break.
async fn move_past_break(
    db: &SqlitePool,
    card: &Card,
    due: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    sqlx::query(r"UPDATE card SET due = ?, updated_at = ? WHERE id = ?")
        .bind(due.timestamp())
        .bind(at.timestamp())
        .bind(card.id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let data = RescheduleData {
        previous_due: card.due,
        due,
        origin: RescheduleOrigin::Break,
    };
    insert_review(
        db,
        &ReviewLog {
            card_id: Some(card.id),
            reviewed_at: at,
            kind: ReviewLogKind::Reschedule,
            scheduler_name: SCHEDULER_NAME.to_string(),
            previous_state: card.state,
            custom_data: to_value(data).unwrap(),
            ..ReviewLog::new()
        },
        None,
    )
    .await?;
    Ok(())
}

/// See [`SrsScheduler::schedule_break`](crate::schedulers::SrsScheduler::schedule_break).
///
/// Every card is moved onto one of the days of the spread, never to an earlier date than it had.
/// The cards are placed one at a time with [`PlacementContext`], so load balancing, easy days and
/// sibling dispersal apply as they do when rating, except that any day of the spread is a
/// candidate rather than only those in a card's fuzz range.
///
/// The cards are first ordered by how much of their recall they would lose if moved to the end
/// of the spread (from their old due date, with their stability), most first. The card at rank
/// `i` of `n` may only take the first `⌈(i + 1)·spread / n⌉ + 1` days, so the cards with the most
/// to lose fill the first days, and the others the days left. This is fsrs4anki-helper's
/// "minimise the deviation from the original due date", in the terms postpone already uses.
pub async fn schedule_break(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    plan: BreakPlan,
    at: DateTime<Utc>,
    dry_run: bool,
) -> Result<BreakOutcome, Error> {
    let zone = config.zone();
    let today = zone.date(at);
    let invalid = |message: String| {
        Error::Library(LibraryError::Scheduler(SchedulerErrorKind::InvalidInput(
            message,
        )))
    };
    if plan.start < today {
        return Err(invalid(format!(
            "the break starts on {}, which has passed",
            plan.start
        )));
    }
    if plan.days == 0 || plan.spread == 0 {
        return Err(invalid(
            "a break lasts at least one day, and spreads its reviews over at least one".to_string(),
        ));
    }
    let spread_days = plan.spread_days();
    let cards = cards_due_in_break(db, zone, today, plan).await?;

    let curve = Curve::new(config);
    let last_reviews = last_reviews(db, &cards).await?;
    // The predicted recall of `card` if it is due at `due`, if it has a review to predict from.
    let recall_at = |card: &Card, due: DateTime<Utc>| {
        last_reviews.get(&card.id).map(|(reviewed_at, _)| {
            let last_review = DateTime::from_timestamp(*reviewed_at, 0).unwrap_or_default();
            curve.retrievability(
                CardMemory::from(card),
                (due - last_review).num_fractional_days().max(0.0),
            )
        })
    };
    let spread_end = zone.start_of(plan.end() + Days::new(u64::from(plan.spread)));
    let mut ordered = cards
        .iter()
        .map(|card| {
            let loss = recall_at(card, card.due)
                .zip(recall_at(card, spread_end))
                .map_or(0.0, |(before, after)| before - after);
            (card, loss)
        })
        .collect::<Vec<_>>();
    // Stable, so cards that lose as much keep their due date order.
    ordered.sort_by(|(_, a), (_, b)| b.total_cmp(a));

    let mut context = PlacementContext::load_all(db, at, zone).await?;
    // `ThreadRng` is not `Send`, and this future is held across awaits.
    let mut rng = StdRng::from_rng(&mut rand::rng());
    let count = ordered.len();
    let moves = ordered
        .into_iter()
        .enumerate()
        .map(|(rank, (card, _))| {
            let reach = ((rank + 1) * spread_days.len())
                .div_ceil(count)
                .saturating_add(1)
                .min(spread_days.len());
            let candidates = spread_days[..reach]
                .iter()
                .map(|day| on_day(zone, card.due, *day))
                .collect::<Vec<_>>();
            (card, context.place(config, card, &candidates, &mut rng))
        })
        .collect::<Vec<_>>();

    let busiest_day = spread_days
        .iter()
        .map(|day| (*day, context.due_count(*day)))
        .max_by_key(|(day, count)| (*count, cmp::Reverse(*day)));
    let (before, after): (Vec<f64>, Vec<f64>) = moves
        .iter()
        .filter_map(|(card, due)| recall_at(card, card.due).zip(recall_at(card, *due)))
        .unzip();
    let mean_retrievability = mean(&before).zip(mean(&after));

    let mut card_payloads = Vec::new();
    for (card, due) in moves {
        if !dry_run {
            move_past_break(db, card, due, at).await?;
        }
        card_payloads.push(UpdateCardPayload {
            card_id: card.id,
            order: None,
            back_type: None,
            due: Some(Transition {
                before: card.due,
                after: due,
            }),
            stability: None,
            stability_fast: None,
            difficulty: None,
            desired_retention: None,
            special_state: None,
            state: None,
            custom_data: None,
        });
    }
    Ok(BreakOutcome {
        card_payloads,
        busiest_day,
        mean_retrievability,
    })
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use super::*;

    /// A minimal card with two graded reviews, inserted directly so this test does not depend on
    /// the note/parser creation pipeline.
    async fn seed_card_with_two_reviews(
        pool: &SqlitePool,
        older: DateTime<Utc>,
        newer: DateTime<Utc>,
    ) -> Card {
        sqlx::query("INSERT INTO parser (name, usn) VALUES ('markdown', -1)")
            .execute(pool)
            .await
            .unwrap();
        let note_id = crate::ids::insert_test_note(pool, "n", 1).await;
        let card_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
               desired_retention, state, custom_data, created_at, updated_at, usn, stability_fast)
               VALUES (?, 1, 1, ?, 5.0, 4.0, 0.9, 2, '{}', strftime('%s', 'now'), strftime('%s', 'now'), -1, 5.0) RETURNING id"#,
        )
        .bind(note_id)
        .bind(newer.timestamp())
        .fetch_one(pool)
        .await
        .unwrap();
        for (reviewed_at, scheduled_time) in [(older, 86_400_i64), (newer, 172_800_i64)] {
            sqlx::query(
                r"INSERT INTO review_log
                    (card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
                     recall_duration, rate_duration, previous_state, custom_data, usn)
                  VALUES (?, ?, 0, 3, 'fsrs', ?, 5, 2, 2, '{}', -1)",
            )
            .bind(card_id)
            .bind(reviewed_at.timestamp())
            .bind(scheduled_time)
            .execute(pool)
            .await
            .unwrap();
        }
        sqlx::query_as("SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// `get_all_cards_internal` fetches "the latest review" to compute elapsed time. Before the
    /// fix, `ORDER BY reviewed_at ASC` with no `LIMIT` returned the OLDEST matching row instead,
    /// so a card with more than one review had its elapsed time computed against its first-ever
    /// review rather than its most recent one.
    #[sqlx::test]
    async fn uses_the_latest_review_not_the_oldest(pool: SqlitePool) {
        let now = Utc::now();
        let older = now - Duration::days(30);
        let newer = now - Duration::days(3);
        let card = seed_card_with_two_reviews(&pool, older, newer).await;

        let cards_internal = get_all_cards_internal(
            &pool,
            std::slice::from_ref(&card),
            &MoveCardAction::Advance,
            now,
            &Curve::new(&SparesExternalConfig::default()),
        )
        .await
        .unwrap();
        assert_eq!(cards_internal.len(), 1);

        let elapsed = cards_internal[0].estimate.current_elapsed_time;
        let expected_from_newer = now - newer;
        let would_be_from_older = now - older;
        assert_eq!(
            elapsed.num_seconds(),
            expected_from_newer.num_seconds(),
            "elapsed time must be computed against the most recent review"
        );
        assert_ne!(
            elapsed.num_seconds(),
            would_be_from_older.num_seconds(),
            "must not fall back to the oldest review"
        );
    }
}
