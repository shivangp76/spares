use std::cmp;

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use rand::RngExt;
use sqlx::SqlitePool;

use super::utils::decay;
use super::utils::retrievability;
use crate::Error;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::config::SparesExternalConfig;
use crate::helpers::FractionalDays;
use crate::helpers::mean;
use crate::model::Card;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::schedulers::MoveCardsResult;

#[derive(Debug)]
pub struct CardInternal<'a> {
    card: &'a Card,
    // Advance
    current_elapsed_time: Duration,
    scheduled_time: Option<Duration>,
    current_retention: f64,
    // Postpone
    approx_retention_after_postpone: f64,
}

#[derive(Debug, PartialEq)]
pub enum MoveCardAction {
    Advance,
    Postpone,
}

pub async fn get_safe_cards<'a>(
    db: &SqlitePool,
    cards: &'a [Card],
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
    decay: f32,
) -> Result<Vec<CardInternal<'a>>, Error> {
    let cards_internal = get_all_cards_internal(db, cards, action, requested_date, decay).await?;
    let safe_cards = cards_internal
        .into_iter()
        .filter(|x| is_card_safe(x, action))
        .collect::<Vec<_>>();
    Ok(safe_cards)
}

fn is_card_safe(d: &CardInternal, action: &MoveCardAction) -> bool {
    match action {
        MoveCardAction::Advance => {
            1. - (1. / d.current_retention - 1.) / (1. / d.card.desired_retention - 1.) < 0.13
        }
        MoveCardAction::Postpone => {
            (1. / d.approx_retention_after_postpone - 1.) / (1. / d.card.desired_retention - 1.)
                - 1.
                < 0.15
        }
    }
}

fn card_sort_key(d: &CardInternal, action: &MoveCardAction) -> (f64, f64) {
    match action {
        // sort by (1 - elapsed_day / scheduled_day)
        // = 1-ln(current retention)/ln(requested retention), -stability (ascending)
        MoveCardAction::Advance => (
            1. - (1. / d.current_retention - 1.) / (1. / d.card.desired_retention - 1.),
            -d.card.stability,
        ),
        // sort by (elapsed_days / scheduled_days - 1)
        // = ln(current retention)/ln(requested retention)-1, -stability (ascending)
        MoveCardAction::Postpone => (
            ((1. / d.approx_retention_after_postpone - 1.) / (1. / d.card.desired_retention - 1.)
                - 1.),
            -d.card.stability,
        ),
    }
}

async fn get_all_cards_internal<'a>(
    db: &SqlitePool,
    cards: &'a [Card],
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
    decay: f32,
) -> Result<Vec<CardInternal<'a>>, Error> {
    let mut cards_internal = Vec::new();
    for card in cards {
        // Get latest review for this card.
        // Only graded reviews carry a `scheduled_time`, and a forget marker would anchor the
        // elapsed time below on a history the user discarded.
        let review_log_res: Option<ReviewLog> = sqlx::query_as(
            r"SELECT * FROM review_log WHERE card_id = ? AND kind = ?
              ORDER BY reviewed_at DESC, id DESC LIMIT 1",
        )
        .bind(card.id)
        .bind(ReviewLogKind::Review)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        // Fix type
        let scheduled_time = review_log_res
            .as_ref()
            .and_then(|r| r.scheduled_time)
            .map(Duration::seconds);

        // Advance
        let current_elapsed_time = review_log_res
            .as_ref()
            .map_or_else(Duration::zero, |review_log| {
                requested_date - review_log.reviewed_at
            });
        // The card was last reviewed after `requested_date` (e.g. statistics for a past date), so
        // its current schedule did not exist yet at that date and it cannot be moved from there.
        if current_elapsed_time < Duration::zero() {
            continue;
        }
        // Equivalent to `current_retrievability`.
        let current_retention = retrievability(
            current_elapsed_time.num_fractional_days(),
            card.stability,
            decay,
        );
        assert!(
            !current_retention.is_nan(),
            "card {} in review has stability {}",
            card.id,
            card.stability
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
        let approx_retention_after_postpone = retrievability(
            approx_elapsed_time_after_postpone.num_fractional_days(),
            card.stability,
            decay,
        );
        cards_internal.push(CardInternal {
            card,
            // Advance
            current_elapsed_time,
            scheduled_time,
            current_retention,
            // Postpone
            approx_retention_after_postpone,
        });
    }
    cards_internal.sort_by(|a, b| {
        let (a_primary, a_secondary) = card_sort_key(a, action);
        let (b_primary, b_secondary) = card_sort_key(b, action);
        a_primary
            .total_cmp(&b_primary)
            .then(a_secondary.total_cmp(&b_secondary))
    });
    Ok(cards_internal)
}

// DB is used to:
// 1. get latest review log for each card
// 2. update card with new due date
#[expect(clippy::too_many_lines)]
pub async fn move_cards(
    db: &SqlitePool,
    count: u32,
    cards: &[Card],
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
    config: &SparesExternalConfig,
) -> Result<MoveCardsResult, Error> {
    let decay = decay(config);
    let cards_internal = get_all_cards_internal(db, cards, action, requested_date, decay).await?;
    let (mut safe_cards, not_safe_cards): (Vec<_>, Vec<_>) = cards_internal
        .into_iter()
        .partition(|x| is_card_safe(x, action));
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
            MoveCardAction::Advance => (requested_date, card_internal.current_elapsed_time),
            MoveCardAction::Postpone => {
                let delay_time = card_internal
                    .scheduled_time
                    .map_or_else(Duration::zero, |scheduled_time| {
                        card_internal.current_elapsed_time - scheduled_time
                    });
                let mut rng = rand::rng();
                let rand_float: f64 = rng.random();
                let new_scheduled_time = cmp::min(
                    cmp::max(
                        config.minimum_interval,
                        Duration::fractional_days(
                            card_internal
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
        card_changes.push(UpdateCardPayload {
            card_id: card_internal.card.id,
            order: None,
            back_type: None,
            due: Some(Transition {
                before: card_internal.card.due,
                after: new_due,
            }),
            stability: None,
            difficulty: None,
            desired_retention: None,
            special_state: None,
            state: None,
            custom_data: None,
        });

        let prev_target_retention = retrievability(
            card_internal
                .scheduled_time
                .unwrap_or_else(Duration::zero)
                .num_fractional_days(),
            card_internal.card.stability,
            decay,
        );
        prev_target_retentions.push(prev_target_retention);
        let new_target_retention = retrievability(
            new_scheduled_time.num_fractional_days(),
            card_internal.card.stability,
            decay,
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
        sqlx::query("INSERT INTO parser (name) VALUES ('markdown')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO note (data, custom_data, parser_id) VALUES ('n', '{}', 1)")
            .execute(pool)
            .await
            .unwrap();
        let card_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
               desired_retention, state, custom_data)
               VALUES (1, 1, 1, ?, 5.0, 4.0, 0.9, 2, '{}') RETURNING id"#,
        )
        .bind(newer.timestamp())
        .fetch_one(pool)
        .await
        .unwrap();
        for (reviewed_at, scheduled_time) in [(older, 86_400_i64), (newer, 172_800_i64)] {
            sqlx::query(
                r"INSERT INTO review_log
                    (card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
                     recall_duration, rate_duration, previous_state, custom_data)
                  VALUES (?, ?, 0, 3, 'fsrs', ?, 5, 2, 2, '{}')",
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
            fsrs_rs::DEFAULT_PARAMETERS[20],
        )
        .await
        .unwrap();
        assert_eq!(cards_internal.len(), 1);

        let elapsed = cards_internal[0].current_elapsed_time;
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
