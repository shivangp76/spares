use crate::config::SparesExternalConfig;
use crate::model::{Card, RatingId, ReviewLog, SpecialState};
use crate::schema::review::{Rating, RatingSubmission};
use crate::{Error, LibraryError, SchedulerErrorKind};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use itertools::Itertools;
use rand::rngs::ThreadRng;
use serde_json::Value;
use sqlx::SqlitePool;

mod fsrs;

pub fn stepped_range_inclusive(start: Duration, end: Duration, step: Duration) -> Vec<Duration> {
    let mut intervals = Vec::new();
    let mut current = start;
    while current <= end {
        intervals.push(current);
        current += step;
    }
    intervals
}

#[async_trait]
pub trait SrsScheduler: Send + Sync {
    fn get_scheduler_name(&self) -> &'static str;

    fn get_ratings(&self) -> Vec<Rating>;

    async fn get_leeches(&self, db: &SqlitePool) -> Result<Vec<Card>, Error>;

    /// Returns a rating and when it was reviewed at
    fn generate_review_history(
        &self,
        num_siblings: u32,
        num_reviews: u32,
        first_review_date: DateTime<Utc>,
        rng: &mut ThreadRng,
    ) -> Vec<Vec<(RatingSubmission, DateTime<Utc>)>>;

    /// Note that `RatingId` is used instead of a general `Rating` enum to support different schedulers having different options. For example, FSRS has 4 ratings (Again, Hard, Good, Easy), but another scheduler might just have 2, such as Pass and Fail.
    fn schedule(
        &self,
        card: &Card,
        previous_review_log: Option<ReviewLog>,
        rating: RatingId,
        reviewed_at: DateTime<Utc>,
        duration: Duration,
    ) -> Result<(Card, ReviewLog), Error>;

    /// Returns `Ok(None)` if the card should no longer be in the filtered deck.
    fn filtered_tag_schedule(
        &self,
        filtered_tag_scheduler_data: Option<&Value>,
        card: &Card,
        // previous_review_log: Option<ReviewLog>,
        rating: RatingId,
        reviewed_at: DateTime<Utc>,
        duration: Duration,
    ) -> Result<Option<Value>, Error>;

    fn bury(&self, card: &Card) -> Result<Card, Error> {
        Ok(Card {
            special_state: Some(SpecialState::UserBuried),
            ..card.clone()
        })
    }

    async fn get_advance_safe_count(
        &self,
        db: &SqlitePool,
        requested_date: DateTime<Utc>,
    ) -> Result<u32, Error>;

    async fn get_postpone_safe_count(
        &self,
        db: &SqlitePool,
        requested_date: DateTime<Utc>,
    ) -> Result<u32, Error>;

    async fn advance(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
        count: u32,
        requested_date: DateTime<Utc>,
    ) -> Result<String, Error>;

    async fn postpone(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
        count: u32,
        requested_date: DateTime<Utc>,
    ) -> Result<String, Error>;

    // - User requests Reschedule -> Compute memory state for each card -> (some combination of applying easy days and dispersing siblings to determine card's new due date) -> Update card (including due date)
    async fn reschedule(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
        mut cards_with_review_logs: Vec<(Card, Vec<ReviewLog>)>,
        at: DateTime<Utc>,
    ) -> Result<(), Error> {
        // Recompute parameters
        cards_with_review_logs.iter_mut().try_for_each(
            |(card, review_logs)| -> Result<(), Error> {
                let new_card = self.compute_memory_state(review_logs.clone())?;
                (card.stability, card.difficulty) = (new_card.stability, new_card.difficulty);
                Ok(())
            },
        )?;

        // Smart schedule
        let grouped_cards = cards_with_review_logs
            .into_iter()
            .map(|card_data| (card_data.0.note_id, card_data))
            .into_group_map();
        for (_note_id, mut siblings_with_review_logs) in grouped_cards {
            // NOTE: This currently schedules each sibling sequentially so the order of the siblings will impact how they are scheduled.
            siblings_with_review_logs.sort_by_key(|(card, _)| card.order);
            for sibling_index in 0..siblings_with_review_logs.len() {
                siblings_with_review_logs[sibling_index].0.due = self
                    .smart_schedule(
                        config,
                        &siblings_with_review_logs[sibling_index],
                        &siblings_with_review_logs,
                        at,
                    )
                    .await?;

                // Update card
                let updated_card = &siblings_with_review_logs[sibling_index].0;
                let _update_card_result = sqlx::query(
                    r"UPDATE card SET due = ?, stability = ?, difficulty = ?, state = ?, custom_data = ?, updated_at = ? WHERE id = ?",
                )
                .bind(updated_card.due.timestamp())
                .bind(updated_card.stability)
                .bind(updated_card.difficulty)
                .bind(updated_card.state)
                .bind(updated_card.custom_data.clone())
                .bind(updated_card.id)
                .bind(updated_card.updated_at.timestamp())
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            }
        }
        Ok(())
    }

    /// Schedules a card while accounting for easy days and dispersing siblings.
    ///
    /// Returns the new due date of the card.
    // Replaces `fsrs4anki-helper`'s `disperse_siblings_when_review()`.
    // Replaces `fsrs4anki-helper`'s `easy_days(did)` and `apply_easy_day_for_specific_date(self)`.
    async fn smart_schedule(
        &self,
        config: &SparesExternalConfig,
        data: &(Card, Vec<ReviewLog>),
        siblings_with_review_logs: &[(Card, Vec<ReviewLog>)],
        at: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, Error>;

    fn compute_memory_state(&self, review_logs: Vec<ReviewLog>) -> Result<Card, Error> {
        assert!(!review_logs.is_empty());
        let first_review = review_logs.first().unwrap().reviewed_at;
        let (final_card, _final_review_log) =
            review_logs
                .into_iter()
                .fold((Card::new(first_review), None), |acc, review_log| {
                    let (card, previous_review_log) = acc;
                    let duration = Duration::new(review_log.duration, 0).unwrap();
                    let (new_card, new_review_log) = self
                        .schedule(
                            &card,
                            previous_review_log,
                            review_log.rating,
                            review_log.reviewed_at,
                            duration,
                        )
                        .unwrap();
                    (new_card, Some(new_review_log))
                });
        Ok(final_card)
    }
}

fn get_all_schedulers() -> Vec<fn() -> Box<dyn SrsScheduler>> {
    // NOTE: Add scheduler here
    // Also run: `spares_cli add scheduler --name="NAME"`
    let all_schedulers: Vec<fn() -> Box<dyn SrsScheduler>> = vec![|| Box::<fsrs::FSRS>::default()];
    all_schedulers
}

pub fn get_scheduler_from_string(scheduler_str: &str) -> Result<Box<dyn SrsScheduler>, Error> {
    let all_schedulers = get_all_schedulers();
    let matching_schedulers: Vec<fn() -> Box<dyn SrsScheduler>> = all_schedulers
        .into_iter()
        .filter(|p| scheduler_str == p().get_scheduler_name())
        .collect();
    if matching_schedulers.is_empty() {
        return Err(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::NotFound(scheduler_str.to_string()),
        )));
    }
    // Not possible. See `test_schedulers_validation`
    // if matching_schedulers.len() > 1 {
    // }
    Ok(matching_schedulers[0]())
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn test_schedulers_validation() {
        let all_schedulers = get_all_schedulers();
        assert!(!all_schedulers.is_empty());
        let mut all_scheduler_names = Vec::new();
        for scheduler_fn in all_schedulers {
            let scheduler = scheduler_fn();
            all_scheduler_names.push(scheduler.get_scheduler_name());
            // assert!(validate_scheduler(scheduler.as_ref()).is_none());
        }
        assert_eq!(
            all_scheduler_names.len(),
            all_scheduler_names.iter().unique().count()
        );
    }
}
