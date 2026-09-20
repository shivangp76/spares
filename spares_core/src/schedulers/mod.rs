use async_trait::async_trait;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use itertools::Itertools;
use rand::rngs::ThreadRng;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::config::SparesExternalConfig;
use crate::model::Card;
use crate::model::RatingId;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::schema::review::Rating;
use crate::schema::review::RatingSubmission;

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

#[derive(Debug)]
pub struct MoveCardsResult {
    pub card_payloads: Vec<UpdateCardPayload>,
    pub message: String,
}

#[async_trait]
pub trait SrsScheduler: Send + Sync {
    fn get_scheduler_name(&self) -> &'static str;

    fn get_ratings(&self) -> Vec<Rating>;

    /// Map a `[0, 1]` score to a [`Rating`] using continuous linear
    /// interpolation across the ratings sorted by id (lowest id = worst,
    /// highest id = best).
    ///
    /// Returns an error if the scheduler has no ratings or if `score` is
    /// not a finite value in `[0, 1]`.
    fn rating_from_score(&self, score: f64) -> Result<Rating, Error> {
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(Error::Library(LibraryError::Scheduler(
                SchedulerErrorKind::InvalidInput(format!(
                    "score {score} is not a finite number in [0, 1]"
                )),
            )));
        }
        let mut ratings = self.get_ratings();
        if ratings.is_empty() {
            return Err(Error::Library(LibraryError::Scheduler(
                SchedulerErrorKind::InvalidInput("scheduler returned no ratings".to_string()),
            )));
        }
        ratings.sort_by_key(|r| r.id);
        let last = ratings.len() - 1;
        // `ratings.len()` is nowhere near 2^52, so this conversion never loses
        // meaningful precision — silencing pedantic's blanket concern here.
        #[allow(clippy::cast_precision_loss)]
        let last_f64 = last as f64;
        let position = (score * last_f64).round();
        // `score` is guaranteed in [0, 1] so `position` is always in
        // `[0.0, last_f64]` — never negative, never larger than `last`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let idx = (position as usize).min(last);
        Ok(ratings[idx].clone())
    }

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
        recall_duration: Duration,
        rate_duration: Duration,
    ) -> Result<(Card, ReviewLog), Error>;

    /// Returns `Ok(None)` if the card should no longer be in the filtered deck.
    fn filtered_tag_schedule(
        &self,
        filtered_tag_scheduler_data: Option<&Value>,
        card: &Card,
        // previous_review_log: Option<ReviewLog>,
        rating: RatingId,
        reviewed_at: DateTime<Utc>,
        recall_duration: Duration,
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
        query: Option<String>,
        requested_date: DateTime<Utc>,
    ) -> Result<MoveCardsResult, Error>;

    async fn postpone(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
        count: u32,
        query: Option<String>,
        requested_date: DateTime<Utc>,
    ) -> Result<MoveCardsResult, Error>;

    // - User requests Reschedule -> Compute memory state for each card -> (some combination of applying easy days and dispersing siblings to determine card's new due date) -> Update card (including due date)
    async fn reschedule(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
        mut cards_with_review_logs: Vec<(Card, Vec<ReviewLog>)>,
        at: DateTime<Utc>,
    ) -> Result<(), Error> {
        // A card with no log at all has nothing to replay, and `compute_memory_state` asserts a
        // non-empty log.
        cards_with_review_logs.retain(|(_card, review_logs)| !review_logs.is_empty());

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
                .bind(updated_card.updated_at.timestamp())
                .bind(updated_card.id)
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

    /// Replays a card's log to reconstruct the memory state it should currently have.
    ///
    /// `review_logs` must be ordered by `reviewed_at ASC, id ASC` and must include every
    /// [`ReviewLogKind`], not just reviews: a [`ReviewLogKind::Forget`] row is what tells the
    /// replay to start over, and dropping it would reconstruct the state the card would have had
    /// if it had never been forgotten.
    fn compute_memory_state(&self, review_logs: Vec<ReviewLog>) -> Result<Card, Error> {
        assert!(!review_logs.is_empty());
        // Seeded from the first entry whatever its kind, so that a card forgotten before it was
        // ever reviewed replays to a card created at that instant rather than at the epoch.
        let mut card = Card::new(review_logs.first().unwrap().reviewed_at);
        let mut previous_review_log: Option<ReviewLog> = None;
        for review_log in review_logs {
            match review_log.kind {
                ReviewLogKind::Forget => {
                    // Mirrors `api::card::forget_card`, which resets stability, difficulty, state
                    // and due but leaves `desired_retention` and `custom_data` alone.
                    card = Card {
                        desired_retention: card.desired_retention,
                        custom_data: card.custom_data,
                        ..Card::new(review_log.reviewed_at)
                    };
                    // A forget also severs the elapsed-time chain: the next review is a first
                    // review, not a lapse continuation, so it must not see a review the user has
                    // explicitly discarded.
                    //
                    // This is currently defensive rather than observable: FSRS reaches
                    // `card_to_fsrs_card` with `state = New` here and its new-card branch ignores
                    // `last_review` entirely. It is kept because it is what the log actually
                    // means, and a scheduler that does consult the previous review would
                    // otherwise silently compute intervals across the forget.
                    previous_review_log = None;
                }
                ReviewLogKind::Review => {
                    let rating = review_log.rating.ok_or_else(|| {
                        Error::Library(LibraryError::Scheduler(SchedulerErrorKind::InvalidInput(
                            format!(
                                "review log {} has kind = Review but no rating",
                                review_log.id
                            ),
                        )))
                    })?;
                    let recall_duration =
                        Duration::new(review_log.recall_duration.unwrap_or(0), 0).unwrap();
                    let rate_duration =
                        Duration::new(review_log.rate_duration.unwrap_or(0), 0).unwrap();
                    let (new_card, new_review_log) = self.schedule(
                        &card,
                        previous_review_log,
                        rating,
                        review_log.reviewed_at,
                        recall_duration,
                        rate_duration,
                    )?;
                    card = new_card;
                    previous_review_log = Some(new_review_log);
                }
            }
        }
        Ok(card)
    }
}

/// The review logs that still bear on a card's current memory state: everything after the most
/// recent [`ReviewLogKind::Forget`] marker, or the whole slice if the card was never forgotten.
///
/// `review_logs` must be ordered by `reviewed_at ASC, id ASC`. The `id` tie-break matters because
/// a forget and a review can land in the same second.
///
/// Use this wherever an interval is derived from the log (elapsed time, the anchor for a new due
/// date, the `previous_review_log` handed to [`SrsScheduler::schedule`]). Do *not* use it when
/// feeding [`SrsScheduler::compute_memory_state`], which needs the forget markers themselves.
pub fn effective_review_logs(review_logs: &[ReviewLog]) -> &[ReviewLog] {
    match review_logs
        .iter()
        .rposition(|review_log| review_log.kind == ReviewLogKind::Forget)
    {
        Some(index) => &review_logs[index + 1..],
        None => review_logs,
    }
}

fn get_all_schedulers() -> Vec<fn() -> Box<dyn SrsScheduler>> {
    // NOTE: Add scheduler here
    // Also run: `spares scheduler add --name="NAME"`
    // Schedulers are selected by name via the `--scheduler-name` CLI flag.
    let all_schedulers: Vec<fn() -> Box<dyn SrsScheduler>> = vec![|| Box::<fsrs::FSRS>::default()];
    all_schedulers
}

/// The scheduler to attribute an action to when no review log names one, such as forgetting a
/// card that has never been reviewed.
pub fn get_default_scheduler_name() -> &'static str {
    get_all_schedulers()[0]().get_scheduler_name()
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
    use serde_json::Map;

    use super::*;
    use crate::model::NEW_CARD_STATE;

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

    fn scheduler() -> Box<dyn SrsScheduler> {
        get_scheduler_from_string("fsrs").unwrap()
    }

    fn at(day: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + day * 86_400, 0).unwrap()
    }

    fn review_at(day: i64, rating: RatingId) -> ReviewLog {
        ReviewLog {
            id: day,
            card_id: Some(1),
            reviewed_at: at(day),
            kind: ReviewLogKind::Review,
            rating: Some(rating),
            tag_id: None,
            scheduler_name: "fsrs".to_string(),
            scheduled_time: Some(86_400),
            recall_duration: Some(5),
            rate_duration: Some(2),
            previous_state: NEW_CARD_STATE,
            custom_data: Value::Object(Map::new()),
        }
    }

    fn forget_at(day: i64) -> ReviewLog {
        ReviewLog {
            id: day,
            card_id: Some(1),
            reviewed_at: at(day),
            kind: ReviewLogKind::Forget,
            rating: None,
            tag_id: None,
            scheduler_name: "fsrs".to_string(),
            scheduled_time: None,
            recall_duration: None,
            rate_duration: None,
            previous_state: 2,
            custom_data: Value::Object(Map::new()),
        }
    }

    #[test]
    fn compute_memory_state_forget_resets_memory() {
        let scheduler = scheduler();
        let reviewed = scheduler
            .compute_memory_state(vec![review_at(0, 3), review_at(1, 3), review_at(2, 4)])
            .unwrap();
        assert!(reviewed.stability > 0.0, "precondition: card was learned");

        let forgotten = scheduler
            .compute_memory_state(vec![
                review_at(0, 3),
                review_at(1, 3),
                review_at(2, 4),
                forget_at(3),
            ])
            .unwrap();
        assert_eq!(forgotten.stability, 0.0);
        assert_eq!(forgotten.difficulty, 0.0);
        assert_eq!(forgotten.state, NEW_CARD_STATE);
        assert_eq!(forgotten.due, at(3));
    }

    #[test]
    fn compute_memory_state_review_after_forget_starts_fresh() {
        let scheduler = scheduler();
        // A card with history, forgotten, then reviewed once.
        let after_forget = scheduler
            .compute_memory_state(vec![
                review_at(0, 3),
                review_at(1, 3),
                review_at(2, 4),
                forget_at(3),
                review_at(4, 3),
            ])
            .unwrap();
        // The same single review against a card that never had any history.
        let from_scratch = scheduler
            .compute_memory_state(vec![review_at(4, 3)])
            .unwrap();
        assert_eq!(after_forget.stability, from_scratch.stability);
        assert_eq!(after_forget.difficulty, from_scratch.difficulty);
        assert_eq!(after_forget.state, from_scratch.state);
        assert_eq!(after_forget.due, from_scratch.due);
    }

    #[test]
    fn compute_memory_state_forget_ignores_gap_to_discarded_reviews() {
        let scheduler = scheduler();
        // The forget and the next review are 37 days apart. Replay must not treat that as a long
        // interval on the pre-forget history.
        //
        // NOTE: this does not by itself pin `previous_review_log = None` in the fold. FSRS reaches
        // its new-card branch here, which ignores `last_review`, so that reset is not observable
        // through this scheduler. What this does pin is that the pre-forget reviews contribute
        // nothing to the resulting interval.
        let after_forget = scheduler
            .compute_memory_state(vec![
                review_at(0, 3),
                review_at(1, 3),
                review_at(2, 4),
                forget_at(3),
                review_at(40, 3),
            ])
            .unwrap();
        let from_scratch = scheduler
            .compute_memory_state(vec![review_at(40, 3)])
            .unwrap();
        assert_eq!(
            after_forget.due, from_scratch.due,
            "a review after a forget must be scheduled as a first review"
        );
    }

    #[test]
    fn compute_memory_state_forget_only() {
        let scheduler = scheduler();
        // A card forgotten before it was ever reviewed: the fold is seeded from the marker, so the
        // card is created at that instant rather than at the epoch.
        let card = scheduler.compute_memory_state(vec![forget_at(7)]).unwrap();
        assert_eq!(card.stability, 0.0);
        assert_eq!(card.state, NEW_CARD_STATE);
        assert_eq!(card.due, at(7));
    }

    #[test]
    fn compute_memory_state_rejects_review_without_rating() {
        let scheduler = scheduler();
        let mut broken = review_at(0, 3);
        broken.rating = None;
        assert!(scheduler.compute_memory_state(vec![broken]).is_err());
    }

    #[test]
    fn effective_review_logs_without_forget_is_everything() {
        let logs = vec![review_at(0, 3), review_at(1, 3)];
        assert_eq!(effective_review_logs(&logs).len(), 2);
        assert!(effective_review_logs(&[]).is_empty());
    }

    #[test]
    fn effective_review_logs_drops_everything_up_to_the_last_forget() {
        let logs = vec![
            review_at(0, 3),
            forget_at(1),
            review_at(2, 3),
            forget_at(3),
            review_at(4, 3),
            review_at(5, 3),
        ];
        let effective = effective_review_logs(&logs);
        assert_eq!(effective.len(), 2, "only the reviews after the last forget");
        assert_eq!(effective[0].reviewed_at, at(4));

        // A trailing forget leaves nothing effective at all.
        let logs = vec![review_at(0, 3), forget_at(1)];
        assert!(effective_review_logs(&logs).is_empty());

        // Consecutive forgets behave like one.
        let logs = vec![review_at(0, 3), forget_at(1), forget_at(2)];
        assert!(effective_review_logs(&logs).is_empty());
    }
}
