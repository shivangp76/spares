//! The FSRS scheduler, backed by the `fsrs` crate with its default (FSRS-6) parameters.
//!
//! ## `fsrs4anki-helper` features
//! - Reschedule: Supported. Reschedules every card from its full review history. `Reschedule
//!   recent` is not supported.
//! - Postpone and Advance: Supported, see the `reposition` module.
//! - Load balance, easy days and disperse siblings: Supported, as one step that runs whenever a
//!   card in review is rated or rescheduled. See [`crate::schedulers::placement`].
//! - Flatten: Unsupported. See <https://github.com/open-spaced-repetition/fsrs4anki-helper/issues/439#issuecomment-2268740000> for reasoning. Load balancing is close enough to this feature.
//! - Remedy hard misuse: Unsupported.
mod optimize;
mod reposition;
mod steps;
mod utils;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use fsrs_rs::MemoryState;
use itertools::Itertools;
use rand::RngExt;
use rand::distr::Distribution;
use rand::distr::weighted::WeightedIndex;
use rand::rngs::ThreadRng;
use reposition::MoveCardAction;
use reposition::get_safe_cards;
use reposition::move_cards;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use sqlx::SqlitePool;
use steps::LearningSteps;
use steps::next_step;
use utils::Rating as FsrsRating;
use utils::State;
use utils::decay;
use utils::get_fuzz_range;
use utils::model;
use utils::number_to_rating;
use utils::number_to_state;
use utils::rating_to_number;
use utils::state_to_number;

use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::helpers::FractionalDays;
use crate::helpers::get_start_end_local_date;
use crate::model::Card;
use crate::model::RatingId;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::schedulers::MoveCardsResult;
use crate::schedulers::SrsScheduler;
use crate::schedulers::effective_review_logs;
use crate::schema::review::OptimizeResponse;
use crate::schema::review::Rating;
use crate::schema::review::RatingSubmission;
use crate::search::evaluator::Evaluator;

/// The interval, in days, after which a card with `stability` is expected to fall to
/// `desired_retention`. Not rounded, clamped or fuzzed.
#[cfg(test)]
pub(crate) fn optimal_interval_days(stability: f64, desired_retention: f64) -> f64 {
    f64::from(fsrs_rs::FSRS::default().next_interval(
        Some(stability as f32),
        desired_retention as f32,
        1,
    ))
}

/// Stateless: the model is built from the config's `fsrs_parameters` whenever it is needed, so a
/// change to them applies without restarting the server.
#[derive(Debug, Default)]
pub struct Fsrs;

// NOTE: Make sure to pass time data as a `Duration` instead of an integer representing days.
#[async_trait]
impl SrsScheduler for Fsrs {
    fn get_scheduler_name(&self) -> &'static str {
        "fsrs"
    }

    fn get_ratings(&self) -> Vec<Rating> {
        use std::sync::OnceLock;
        static RATINGS: OnceLock<Vec<Rating>> = OnceLock::new();
        RATINGS
            .get_or_init(|| {
                FsrsRating::ALL
                    .iter()
                    .map(|fsrs_rating| Rating {
                        id: rating_to_number(*fsrs_rating),
                        description: format!("{:?}", fsrs_rating),
                    })
                    .collect::<Vec<_>>()
            })
            .clone()
    }

    async fn get_leeches(&self, db: &SqlitePool) -> Result<Vec<Card>, Error> {
        let config = read_external_config()?;
        // A lapse is forgetting a card that had graduated to review, so `Again` during learning
        // or relearning steps does not count.
        let cards = sqlx::query_as(
            r"SELECT * FROM card
              WHERE special_state IS NULL
                AND id IN (
                  SELECT card_id FROM review_log
                  WHERE kind = ? AND rating = ? AND previous_state = ?
                  GROUP BY card_id
                  HAVING COUNT(*) > ?
                )
              ORDER BY due ASC",
        )
        .bind(ReviewLogKind::Review)
        .bind(rating_to_number(FsrsRating::Again))
        .bind(state_to_number(State::Review))
        .bind(config.leech.lapses_threshold)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        Ok(cards)
    }

    // Note that this can be optimized to account for the correlation between retention amongst siblings. The ratings can also be chosen better to better reflect human tendencies.
    fn generate_review_history(
        &self,
        num_siblings: u32,
        num_reviews: u32,
        first_review_date: DateTime<Utc>,
        rng: &mut ThreadRng,
    ) -> Vec<Vec<(RatingSubmission, DateTime<Utc>)>> {
        // Again = 1, Hard = 2, Good = 3, Easy = 4,
        // let weights = [0, 0, 0, 3]; // Weights favoring 3
        let weights = [1, 3, 5, 3]; // Weights favoring 3
        let dist = WeightedIndex::new(weights).unwrap();
        let ratings: Vec<u32> = (1..=num_reviews)
            .map(|_| (dist.sample(rng) + 1) as u32) // Add 1 to map to 1..=4
            .collect::<Vec<_>>();

        let mut all_review_histories = Vec::new();
        for card_id in 1..=num_siblings {
            let (_card, review_logs) =
                ratings
                    .iter()
                    .fold((Card::new(first_review_date), Vec::new()), |acc, rating| {
                        let (card, mut review_logs): (_, Vec<ReviewLog>) = acc;
                        let recall_duration = Duration::new(rng.random_range(5..=60), 0).unwrap();
                        let rate_duration = Duration::new(rng.random_range(5..=60), 0).unwrap();
                        let previous_review_log = review_logs.last();
                        let reviewed_at = previous_review_log.map_or_else(
                            || first_review_date,
                            |rl| {
                                let scheduled_time = rl
                                    .scheduled_time
                                    .expect("a scheduler-produced review log has a scheduled time");
                                rl.reviewed_at + Duration::new(scheduled_time, 0).unwrap()
                            },
                        );
                        let (new_card, new_review_log) = self
                            .schedule(
                                &SparesExternalConfig::default(),
                                &card,
                                previous_review_log.cloned(),
                                *rating,
                                reviewed_at,
                                recall_duration,
                                rate_duration,
                            )
                            .unwrap();
                        assert_eq!(new_review_log.reviewed_at, reviewed_at);
                        review_logs.push(new_review_log);
                        (new_card, review_logs)
                    });
            let card_review_history = review_logs
                .into_iter()
                .map(|review_log| {
                    (
                        RatingSubmission {
                            card_id: i64::from(card_id),
                            // These are all produced by `schedule` just above, so they are always
                            // a review and always fully populated.
                            rating: review_log
                                .rating
                                .expect("a scheduler-produced review log has a rating"),
                            recall_duration: Duration::seconds(
                                review_log.recall_duration.unwrap_or(0),
                            ),
                            rate_duration: Duration::seconds(review_log.rate_duration.unwrap_or(0)),
                            tag_id: None,
                        },
                        review_log.reviewed_at,
                    )
                })
                .collect::<Vec<_>>();
            all_review_histories.push(card_review_history);
        }
        all_review_histories
    }

    fn schedule(
        &self,
        config: &SparesExternalConfig,
        card: &Card,
        previous_review_log: Option<ReviewLog>,
        rating: RatingId,
        reviewed_at: DateTime<Utc>,
        recall_duration: Duration,
        rate_duration: Duration,
    ) -> Result<(Card, ReviewLog), Error> {
        let state = number_to_state(card.state).ok_or(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::InvalidState(card.state),
        )))?;
        let fsrs_rating = number_to_rating(rating).ok_or(Error::Library(
            LibraryError::Scheduler(SchedulerErrorKind::InvalidRating(rating)),
        ))?;
        // Without a previous review there is nothing to measure from. That only happens for a new
        // card, whose first step ignores the elapsed time anyway.
        let elapsed_days = previous_review_log.as_ref().map_or(0, |previous| {
            u32::try_from((reviewed_at - previous.reviewed_at).num_days().max(0))
                .unwrap_or(u32::MAX)
        });
        let memory = (state != State::New).then_some(MemoryState {
            stability: card.stability as f32,
            difficulty: card.difficulty as f32,
        });
        let previous_delay = previous_review_log
            .as_ref()
            .and_then(|previous| previous.scheduled_time)
            .map(Duration::seconds);
        let steps = LearningSteps {
            learning: &config.learning_steps,
            relearning: &config.relearning_steps,
        };
        let model = model(config)?;
        let step = next_step(
            &model,
            steps,
            state,
            memory,
            card.desired_retention as f32,
            elapsed_days,
            previous_delay,
            fsrs_rating,
        )
        .map_err(|e| {
            Error::Library(LibraryError::Scheduler(SchedulerErrorKind::InvalidInput(
                format!("FSRS could not schedule card {}: {e}", card.id),
            )))
        })?;
        let new_card = Card {
            updated_at: reviewed_at,
            due: reviewed_at + step.interval,
            stability: f64::from(step.memory.stability),
            difficulty: f64::from(step.memory.difficulty),
            state: state_to_number(step.state),
            ..card.clone()
        };
        let new_review_log = ReviewLog {
            id: 1,
            card_id: Some(card.id),
            reviewed_at,
            kind: ReviewLogKind::Review,
            rating: Some(rating),
            // The scheduler has no notion of filtered tags; `rate_card` stamps this on the row it
            // actually inserts.
            tag_id: None,
            scheduler_name: self.get_scheduler_name().to_string(),
            scheduled_time: Some(step.interval.num_seconds()),
            recall_duration: Some(recall_duration.num_seconds()),
            rate_duration: Some(rate_duration.num_seconds()),
            previous_state: card.state,
            custom_data: Value::Object(Map::new()),
        };
        Ok((new_card, new_review_log))
    }

    fn filtered_tag_schedule(
        &self,
        filtered_tag_scheduler_data: Option<&Value>,
        _card: &Card,
        rating: RatingId,
        _reviewed_at: DateTime<Utc>,
        _recall_duration: Duration,
    ) -> Result<Option<Value>, Error> {
        // NOTE: This might not work because a state of Review takes too long to achieve for a new card
        // Show at least once or until state is Review
        // if card.state == state_to_number(State::Review) {
        //     return Ok(None);
        // }
        // return Ok(Some(Value::Null));
        //
        // 3 Good ratings or 1 Easy rating
        let good_key = "good";
        let good_threshold = 2;
        let rating = number_to_rating(rating).ok_or(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::InvalidRating(rating),
        )))?;
        match rating {
            FsrsRating::Again | FsrsRating::Hard => Ok(Some(Value::Object(Map::new()))),
            FsrsRating::Good => {
                let current_good_count = filtered_tag_scheduler_data
                    .and_then(|data| data.get(good_key))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if current_good_count + 1 >= good_threshold {
                    return Ok(None);
                }
                let map_iter = vec![(
                    good_key.to_string(),
                    Value::Number(Number::from(current_good_count + 1)),
                )];
                Ok(Some(Value::Object(Map::from_iter(map_iter))))
            }
            FsrsRating::Easy => Ok(None),
        }
    }

    async fn get_advance_safe_count(
        &self,
        db: &SqlitePool,
        requested_date: DateTime<Utc>,
    ) -> Result<u32, Error> {
        let (_, card_due_limit) = get_start_end_local_date(&requested_date);
        let cards: Vec<Card> = sqlx::query_as(
            r"SELECT * FROM card
           WHERE due > ?
             AND state = ?
             AND special_state IS NULL
           ORDER BY card.due ASC",
        )
        .bind(card_due_limit.timestamp())
        .bind(state_to_number(State::Review))
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

        let config = read_external_config()?;
        let cards_internal = get_safe_cards(
            db,
            &cards,
            &MoveCardAction::Advance,
            card_due_limit,
            decay(&config),
        )
        .await?;
        Ok(cards_internal.len() as u32)
    }

    async fn get_postpone_safe_count(
        &self,
        db: &SqlitePool,
        requested_date: DateTime<Utc>,
    ) -> Result<u32, Error> {
        let (_, card_due_limit) = get_start_end_local_date(&requested_date);
        let cards: Vec<Card> = sqlx::query_as(
            r"SELECT * FROM card
           WHERE due <= ?
             AND state = ?
             AND special_state IS NULL
           ORDER BY card.due ASC",
        )
        .bind(card_due_limit.timestamp())
        .bind(state_to_number(State::Review))
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

        let config = read_external_config()?;
        let cards_internal = get_safe_cards(
            db,
            &cards,
            &MoveCardAction::Postpone,
            card_due_limit,
            decay(&config),
        )
        .await?;
        Ok(cards_internal.len() as u32)
    }

    async fn advance(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
        count: u32,
        query: Option<String>,
        requested_date: DateTime<Utc>,
    ) -> Result<MoveCardsResult, Error> {
        let (_, card_due_limit) = get_start_end_local_date(&requested_date);
        let card_id_query_str = if let Some(query_str) = query {
            let evaluator = Evaluator::new(&query_str);
            let card_ids_str = evaluator.get_card_ids(db).await?.into_iter().join(", ");
            format!("AND id IN ({})", card_ids_str)
        } else {
            String::new()
        };
        let query_str = format!(
            r"SELECT * FROM card
           WHERE due > ?
             AND state = ?
             AND special_state IS NULL
             {}
           ORDER BY card.due ASC",
            card_id_query_str
        );
        let mut query = sqlx::query_as(&query_str);
        query = query.bind(card_due_limit.timestamp());
        query = query.bind(state_to_number(State::Review));
        let cards: Vec<Card> = query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        move_cards(
            db,
            count,
            &cards,
            &MoveCardAction::Advance,
            card_due_limit,
            config,
        )
        .await
    }

    async fn postpone(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
        count: u32,
        query: Option<String>,
        requested_date: DateTime<Utc>,
    ) -> Result<MoveCardsResult, Error> {
        let (_, card_due_limit) = get_start_end_local_date(&requested_date);
        let card_id_query_str = if let Some(query_str) = query {
            let evaluator = Evaluator::new(&query_str);
            let card_ids_str = evaluator.get_card_ids(db).await?.into_iter().join(", ");
            format!("AND id IN ({})", card_ids_str)
        } else {
            String::new()
        };
        let query_str = format!(
            r"SELECT * FROM card
           WHERE due <= ?
             AND state = ?
             AND special_state IS NULL
             {}
           ORDER BY card.due ASC",
            card_id_query_str
        );
        let mut query = sqlx::query_as(&query_str);
        query = query.bind(card_due_limit.timestamp());
        query = query.bind(state_to_number(State::Review));
        let cards: Vec<Card> = query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        move_cards(
            db,
            count,
            &cards,
            &MoveCardAction::Postpone,
            card_due_limit,
            config,
        )
        .await
    }

    async fn optimize(
        &self,
        db: &SqlitePool,
        config: &SparesExternalConfig,
    ) -> Result<OptimizeResponse, Error> {
        // Forget markers are kept: the training set starts each card's history after its last one.
        let review_logs: Vec<ReviewLog> = sqlx::query_as(
            r"SELECT * FROM review_log WHERE card_id IS NOT NULL
              ORDER BY card_id, reviewed_at ASC, id ASC",
        )
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        let training_set = optimize::training_set(&review_logs);
        let current_parameters = config.fsrs_parameters.clone();
        let num_relearning_steps = config.relearning_steps.len();
        // Training is CPU-bound, so it runs on the rayon pool rather than blocking the executor.
        let (sender, receiver) = futures::channel::oneshot::channel();
        rayon::spawn(move || {
            let _ = sender.send(optimize::optimize(
                training_set,
                &current_parameters,
                num_relearning_steps,
            ));
        });
        receiver.await.map_err(|_| {
            Error::Library(LibraryError::Scheduler(SchedulerErrorKind::Custom {
                scheduler_name: self.get_scheduler_name().to_string(),
                error: "the optimizer stopped unexpectedly".to_string(),
            }))
        })?
    }

    fn due_candidates(
        &self,
        config: &SparesExternalConfig,
        card: &Card,
        review_logs: &[ReviewLog],
        at: DateTime<Utc>,
    ) -> Vec<DateTime<Utc>> {
        // Learning and relearning steps are short, deliberate intervals. Moving them by
        // review-sized amounts would skip the steps entirely.
        if card.state != state_to_number(State::Review) {
            return Vec::new();
        }
        // Intervals are measured from the log, so a forget must hide everything before it.
        let review_logs = effective_review_logs(review_logs);
        let Some(last_review) = review_logs.last() else {
            return Vec::new();
        };
        let interval = card.due - last_review.reviewed_at;
        // Too short to move without noticeably changing the card's retention.
        if interval < config.minimum_interval {
            return Vec::new();
        }
        let elapsed = review_logs
            .iter()
            .rev()
            .tuple_windows()
            .next()
            .map_or_else(Duration::zero, |(latest, previous)| {
                latest.reviewed_at - previous.reviewed_at
            });
        let (min_ivl, max_ivl) = get_fuzz_range(
            interval,
            elapsed,
            config.maximum_interval,
            config.minimum_interval,
        );
        // Whole days only, so the card keeps the time of day it was reviewed at.
        let min_days = min_ivl.num_fractional_days().ceil() as i64;
        let max_days = (max_ivl.num_fractional_days().floor() as i64).max(min_days);
        let today_start = get_start_end_local_date(&at).0;
        let candidates = (min_days..=max_days)
            .map(|days| last_review.reviewed_at + Duration::days(days))
            .collect::<Vec<_>>();
        let upcoming = candidates
            .iter()
            .copied()
            .filter(|due| *due >= today_start)
            .collect::<Vec<_>>();
        if upcoming.is_empty() {
            // The whole range has passed, so the card is overdue. It is due at the end of the
            // range, not at `card.due`, which is not clamped to the maximum interval.
            return candidates.last().copied().into_iter().collect();
        }
        upcoming
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::generate_review_logs;

    /// Random histories, each ending with the review that produced the card.
    fn random_cards(count: usize) -> Vec<(Card, Vec<ReviewLog>)> {
        let mut rng = rand::rng();
        let scheduler = Fsrs;
        (0..count)
            .map(|_| {
                let created_at = Utc::now() - Duration::days(rng.random_range(0..400));
                generate_review_logs(&scheduler, Card::new(created_at), &mut rng)
            })
            .collect()
    }

    #[test]
    fn configured_parameters_are_used() {
        let schedule_good = |config: &SparesExternalConfig| {
            let created_at = Utc::now();
            Fsrs.schedule(
                config,
                &Card::new(created_at),
                None,
                rating_to_number(FsrsRating::Good),
                created_at,
                Duration::seconds(5),
                Duration::seconds(2),
            )
            .unwrap()
            .0
        };
        // Graduate on the first answer, so the interval comes from the initial stability.
        let defaults = SparesExternalConfig {
            learning_steps: Vec::new(),
            ..SparesExternalConfig::default()
        };
        let mut parameters = fsrs_rs::DEFAULT_PARAMETERS.to_vec();
        // The initial stability after Good.
        parameters[2] *= 4.0;
        let custom = SparesExternalConfig {
            fsrs_parameters: parameters,
            learning_steps: Vec::new(),
            ..SparesExternalConfig::default()
        };
        let default_card = schedule_good(&defaults);
        let custom_card = schedule_good(&custom);
        assert!(custom_card.stability > default_card.stability * 3.0);
        assert!(custom_card.due > default_card.due);
    }

    #[test]
    fn due_candidates_stay_within_the_fuzz_range() {
        let config = SparesExternalConfig::default();
        let scheduler = Fsrs;
        for (card, review_logs) in random_cards(300) {
            let last_review = review_logs.last().unwrap().reviewed_at;
            let candidates = scheduler.due_candidates(&config, &card, &review_logs, last_review);
            let interval = card.due - last_review;
            if card.state != state_to_number(State::Review) || interval < config.minimum_interval {
                assert!(
                    candidates.is_empty(),
                    "card {card:?} must keep its due date"
                );
                continue;
            }
            assert_ne!(
                candidates,
                Vec::<DateTime<Utc>>::new(),
                "card {card:?} can be moved"
            );
            assert!(candidates.is_sorted(), "candidates are earliest first");
            for candidate in candidates {
                let offset = candidate - last_review;
                assert_eq!(offset.num_seconds() % 86_400, 0, "the time of day is kept");
                assert!(offset >= config.minimum_interval && offset <= config.maximum_interval);
                // Fuzz never strays further than this for an interval of this size.
                let target = interval.min(config.maximum_interval);
                let slack = 1.0 + 0.15 * target.num_fractional_days();
                assert!(
                    (offset - target).num_fractional_days().abs() <= slack,
                    "{offset} is too far from the proposed {target}"
                );
            }
        }
    }

    #[test]
    fn due_candidates_are_never_in_the_past() {
        let config = SparesExternalConfig::default();
        let scheduler = Fsrs;
        for (card, review_logs) in random_cards(300) {
            let last_review = review_logs.last().unwrap().reviewed_at;
            // Partway through the card's interval, so part of its range may have passed.
            let at = last_review + (card.due - last_review) / 2;
            let today_start = get_start_end_local_date(&at).0;
            let candidates = scheduler.due_candidates(&config, &card, &review_logs, at);
            if candidates.iter().any(|candidate| *candidate < today_start) {
                assert_eq!(
                    candidates.len(),
                    1,
                    "a past date is only offered alone, for an overdue card"
                );
            }
        }
    }

    #[test]
    fn an_overdue_card_is_due_at_the_end_of_its_range() {
        let config = SparesExternalConfig::default();
        let scheduler = Fsrs;
        let mut checked = 0;
        for (card, review_logs) in random_cards(300) {
            let last_review = review_logs.last().unwrap().reviewed_at;
            if card.state != state_to_number(State::Review)
                || card.due - last_review < config.minimum_interval
            {
                continue;
            }
            // Long after the whole range has passed, even for an interval past the maximum.
            let at = last_review + config.maximum_interval + Duration::days(60);
            let candidates = scheduler.due_candidates(&config, &card, &review_logs, at);
            assert_eq!(candidates.len(), 1, "{candidates:?}");
            assert!(candidates[0] < at, "the card is overdue");
            assert!(candidates[0] - last_review <= config.maximum_interval);
            checked += 1;
        }
        assert!(checked > 0, "precondition: some random cards are in review");
    }
}
