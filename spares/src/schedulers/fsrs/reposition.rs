use crate::{
    Error,
    helpers::{FractionalDays, mean},
    model::{Card, ReviewLog},
};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sqlx::SqlitePool;
use std::cmp;

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
) -> Result<Vec<CardInternal<'a>>, Error> {
    let cards_internal = get_all_cards_internal(db, cards, action, requested_date).await?;
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
    let result = match action {
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
    };
    if result.0.is_nan() || result.1.is_nan() {
        match action {
            MoveCardAction::Advance => {
                dbg!(&d.current_retention);
                dbg!(&d.card.desired_retention);
                dbg!(&d.card.stability);
            }
            MoveCardAction::Postpone => {
                dbg!(&d.approx_retention_after_postpone);
                dbg!(&d.card.desired_retention);
                dbg!(&d.card.stability);
            }
        }
    }
    result
}

async fn get_all_cards_internal<'a>(
    db: &SqlitePool,
    cards: &'a [Card],
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
) -> Result<Vec<CardInternal<'a>>, Error> {
    let mut cards_internal = Vec::new();
    for card in cards {
        // Get latest review for this card
        let review_log_res: Option<ReviewLog> =
            sqlx::query_as(r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC")
                .bind(card.id)
                .fetch_optional(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        // Fix type
        let scheduled_time = review_log_res
            .as_ref()
            .map(|r| Duration::seconds(r.scheduled_time));

        // Advance
        let current_elapsed_time = review_log_res
            .as_ref()
            .map_or_else(Duration::zero, |review_log| {
                requested_date - review_log.reviewed_at
            });
        if current_elapsed_time < Duration::zero() {
            dbg!(&requested_date);
            dbg!(&review_log_res.map(|x| x.reviewed_at));
            dbg!(&current_elapsed_time);
        }
        assert!(current_elapsed_time >= Duration::zero());
        // Equivalent to `current_retrievability`.
        let current_retention = rs_fsrs::Parameters::forgetting_curve(
            current_elapsed_time.num_fractional_days(),
            card.stability,
        );
        if current_retention.is_nan() {
            dbg!(&current_elapsed_time);
            dbg!(&card.stability);
        }
        assert!(!current_retention.is_nan());

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
        let approx_retention_after_postpone = rs_fsrs::Parameters::forgetting_curve(
            approx_elapsed_time_after_postpone.num_fractional_days(),
            card.stability,
        );
        if approx_retention_after_postpone.is_nan() {
            dbg!(&card);
            dbg!(&requested_date);
            dbg!(&current_elapsed_time.num_fractional_days());
            dbg!(&scheduled_time.unwrap().num_fractional_days());
            dbg!(&approx_elapsed_time_after_postpone);
            dbg!(&approx_elapsed_time_after_postpone.num_fractional_days());
            dbg!(&card.stability);
        }
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
        let key_a = card_sort_key(a, action);
        let key_b = card_sort_key(b, action);
        key_a.partial_cmp(&key_b).unwrap()
    });
    Ok(cards_internal)
}

// DB is used to:
// 1. get latest review log for each card
// 2. update card with new due date
pub async fn move_cards(
    db: &SqlitePool,
    count: u32,
    cards: &[Card],
    action: &MoveCardAction,
    requested_date: DateTime<Utc>,
    minimum_interval: Duration,
    maximum_interval: Duration,
) -> Result<String, Error> {
    let cards_internal = get_all_cards_internal(db, cards, action, requested_date).await?;
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
                let mut rng = rand::thread_rng();
                let rand_float: f64 = rng.r#gen();
                let new_scheduled_time = cmp::min(
                    cmp::max(
                        minimum_interval,
                        Duration::fractional_days(
                            card_internal
                                .scheduled_time
                                .unwrap_or_else(Duration::zero)
                                .num_fractional_days()
                                * (1.05 + 0.05 * rand_float),
                        ) + delay_time,
                    ),
                    maximum_interval,
                );
                (requested_date + new_scheduled_time, new_scheduled_time)
            }
        };
        // dbg!(&action);
        // dbg!(&requested_date);
        // dbg!(&new_due);
        let _update_card_result =
            sqlx::query(r"UPDATE card SET due = ?, updated_at = ? WHERE id = ?")
                .bind(new_due.timestamp())
                .bind(requested_date.timestamp())
                .bind(card_internal.card.id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

        let prev_target_retention = rs_fsrs::Parameters::forgetting_curve(
            card_internal
                .scheduled_time
                .unwrap_or_else(Duration::zero)
                .num_fractional_days(),
            card_internal.card.stability,
        );
        prev_target_retentions.push(prev_target_retention);
        let new_target_retention = rs_fsrs::Parameters::forgetting_curve(
            new_scheduled_time.num_fractional_days(),
            card_internal.card.stability,
        );
        new_target_retentions.push(new_target_retention);
    }

    let result_text = if !prev_target_retentions.is_empty() && !new_target_retentions.is_empty() {
        format!(
            "Mean target retention of moved cards: {:?} -> {:?}",
            mean(&prev_target_retentions).unwrap(),
            mean(&new_target_retentions).unwrap()
        )
    } else {
        String::new()
    };
    Ok(result_text)
}
