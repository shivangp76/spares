use chrono::DateTime;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::Error;
use crate::api::review::pinned_card_ids;
use crate::api::review::reschedule_all_cards;
use crate::config::read_external_config;
use crate::config::write_fsrs_parameters;
use crate::schedulers::get_scheduler_from_string;
use crate::schema::review::CardState;
use crate::schema::review::OptimizeRequest;
use crate::schema::review::OptimizeResponse;
use crate::schema::review::Rating;
use crate::schema::review::ReschedulePreviewResponse;
use crate::schema::review::RescheduleResponse;

pub fn get_scheduler_ratings(scheduler_name: &str) -> Result<Vec<Rating>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    Ok(scheduler.get_ratings())
}

/// The states the scheduler puts cards in. See
/// [`SrsScheduler::get_states`](crate::schedulers::SrsScheduler::get_states).
pub fn get_scheduler_states(scheduler_name: &str) -> Result<Vec<CardState>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    Ok(scheduler.get_states())
}

pub fn resolve_rating_from_score(scheduler_name: &str, score: f64) -> Result<Rating, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    scheduler.rating_from_score(score)
}

/// Recomputes every card's memory state and due date from its review history. Run after changing
/// the scheduler, its parameters, learning steps or easy days. Cards moved since their last
/// review keep their due date, unless
/// [`reschedule_moved_cards`](crate::config::SparesExternalConfig::reschedule_moved_cards) is set.
// There is deliberately no query to reschedule only some cards. Cards are placed one at a time
// against the due dates already given to the others (load balancing, easy days, sibling
// dispersal), so rescheduling a subset would leave the load spread across days unbalanced. The
// settings that call for a reschedule also apply to every card.
pub async fn reschedule_scheduler(
    db: &SqlitePool,
    scheduler_name: &str,
    at: DateTime<Utc>,
) -> Result<RescheduleResponse, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let event_id = reschedule_all_cards(db, scheduler.as_ref(), &config, at).await?;
    Ok(RescheduleResponse { event_id })
}

/// What [`reschedule_scheduler`] would do with the cards moved since their last review, for the
/// confirmation before it.
pub async fn reschedule_preview(
    db: &SqlitePool,
    scheduler_name: &str,
) -> Result<ReschedulePreviewResponse, Error> {
    // Checks the name, as the reschedule would.
    get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    Ok(ReschedulePreviewResponse {
        moved_count: pinned_card_ids(db, None).await?.len(),
        moved_cards_kept: !config.reschedule_moved_cards,
    })
}

/// Fits the scheduler's parameters to the review history. With `apply`, saves them if they
/// predict it better than the current ones and reschedules every card with them.
pub async fn optimize_scheduler(
    db: &SqlitePool,
    scheduler_name: &str,
    OptimizeRequest { apply }: OptimizeRequest,
    at: DateTime<Utc>,
) -> Result<OptimizeResponse, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let mut response = scheduler.optimize(db, &config).await?;
    if apply && response.optimized.log_loss < response.current.log_loss {
        write_fsrs_parameters(&response.optimized_parameters)?;
        // Read back rather than patched in memory, so the reschedule sees exactly what was saved.
        let config = read_external_config()?;
        response.event_id = reschedule_all_cards(db, scheduler.as_ref(), &config, at).await?;
        response.applied = true;
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    /// `card_count` cards, each reviewed on days 0, 1, 3, 7, 15 and 30, and forgotten far more
    /// often than the default parameters expect.
    async fn seed_review_history(pool: &SqlitePool, card_count: i64, start: DateTime<Utc>) {
        sqlx::query("INSERT INTO parser (name, usn) VALUES ('markdown', -1)")
            .execute(pool)
            .await
            .unwrap();
        let note_id = crate::ids::insert_test_note(pool, "n", 1).await;
        for order in 1..=card_count {
            let card_id: i64 = sqlx::query_scalar(
                r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
                   desired_retention, state, custom_data, created_at, updated_at, usn, stability_fast)
                   VALUES (?, ?, 1, ?, 5.0, 4.0, 0.9, 2, '{}', strftime('%s', 'now'), strftime('%s', 'now'), -1, 5.0) RETURNING id"#,
            )
            .bind(note_id)
            .bind(order)
            .bind(start.timestamp())
            .fetch_one(pool)
            .await
            .unwrap();
            for (index, day) in (0_i64..).zip([0_i64, 1, 3, 7, 15, 30]) {
                let rating = if (order + index) % 3 == 0 { 3 } else { 1 };
                let previous_state = if index == 0 { 0 } else { 2 };
                sqlx::query(
                    r"INSERT INTO review_log
                        (card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
                         recall_duration, rate_duration, previous_state, custom_data, usn)
                      VALUES (?, ?, 0, ?, 'fsrs', 86400, 5, 2, ?, '{}', -1)",
                )
                .bind(card_id)
                .bind((start + Duration::days(day)).timestamp())
                .bind(rating)
                .bind(previous_state)
                .execute(pool)
                .await
                .unwrap();
            }
        }
    }

    #[sqlx::test]
    async fn optimize_fits_the_review_history(pool: SqlitePool) {
        let start = Utc::now() - Duration::days(60);
        seed_review_history(&pool, 40, start).await;

        let response =
            optimize_scheduler(&pool, "fsrs", OptimizeRequest { apply: false }, Utc::now())
                .await
                .unwrap();
        assert!(!response.applied, "nothing is saved without `apply`");
        assert_eq!(response.card_count, 40);
        assert_eq!(
            response.review_count,
            40 * 5,
            "every review after the first day"
        );
        assert_eq!(response.current_parameters.len(), 34, "FSRS-7's defaults");
        assert_eq!(response.optimized_parameters.len(), 34);
        assert!(response.optimized_parameters.iter().all(|w| w.is_finite()));
        assert!(
            response.optimized.log_loss < response.current.log_loss,
            "parameters fitted to a history this forgetful predict it better: {response:?}"
        );
    }

    #[sqlx::test]
    async fn optimize_without_history_is_an_error(pool: SqlitePool) {
        assert!(
            optimize_scheduler(&pool, "fsrs", OptimizeRequest::default(), Utc::now())
                .await
                .is_err()
        );
    }
}
