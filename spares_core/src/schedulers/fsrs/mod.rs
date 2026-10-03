//! # `fsrs4anki-helper`
//! - Unmigrated changes: <https://github.com/open-spaced-repetition/fsrs4anki-helper/compare/ee308f9c5f723ebe6eceb6da24c1c8fcb2d50c6a..main>.
//!
//! ## Feature status
//! 1. Reschedule: Implemented differently. This will still reschedule all cards, but uses `Smart Schedule` (see below). Note that `Reschedule recent` is not supported.
//! 2. Postpone: Implemented.
//! 3. Advance: Implemented.
//! 4. (Load) Balance: See `Smart Schedule` below.
//! 5. Easy days: See `Smart Schedule` below.
//! 6. Disperse siblings: Always enabled. See `Smart Schedule` below.
//! 7. Flatten: Unsupported. See <https://github.com/open-spaced-repetition/fsrs4anki-helper/issues/439#issuecomment-2268740000> for reasoning. Load balancing is close enough to this feature.
//! 8. Remedy hard misuse: Unsupported
//!
//! ### Smart Schedule
//! - This is a combination of `Easy days` and `Disperse siblings`.
mod easy_days;
mod reposition;
mod utils;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use itertools::Itertools;
use rand::RngExt;
use rand::distr::Distribution;
use rand::distr::weighted::WeightedIndex;
use rand::rngs::ThreadRng;
use reposition::MoveCardAction;
use reposition::get_safe_cards;
use reposition::move_cards;
pub(crate) use rs_fsrs::FSRS;
use rs_fsrs::State;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use sqlx::SqlitePool;
use utils::card_to_fsrs_card;
use utils::fsrs_card_to_card;
use utils::get_fuzz_range;
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
use crate::schema::review::Rating;
use crate::schema::review::RatingSubmission;
use crate::search::evaluator::Evaluator;

/// The interval, in days, after which a card with `stability` is expected to fall to
/// `desired_retention`. Not rounded, clamped or fuzzed.
#[cfg(test)]
pub(crate) fn optimal_interval_days(stability: f64, desired_retention: f64) -> f64 {
    stability / rs_fsrs::Parameters::FACTOR
        * (desired_retention.powf(1.0 / rs_fsrs::Parameters::DECAY) - 1.0)
}

// NOTE: Make sure to pass time data as a `Duration` instead of an integer representing days.
#[async_trait]
impl SrsScheduler for FSRS {
    fn get_scheduler_name(&self) -> &'static str {
        "fsrs"
    }

    fn get_ratings(&self) -> Vec<Rating> {
        use std::sync::OnceLock;
        static RATINGS: OnceLock<Vec<Rating>> = OnceLock::new();
        RATINGS
            .get_or_init(|| {
                rs_fsrs::Rating::iter()
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
        .bind(rating_to_number(rs_fsrs::Rating::Again))
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
                        let (new_card, new_review_log) = <FSRS as SrsScheduler>::schedule(
                            self,
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
        let last_review = previous_review_log.map_or(DateTime::<Utc>::MIN_UTC, |r| r.reviewed_at);
        let card_fsrs = card_to_fsrs_card(card, state, last_review);
        // `self` carries the default parameters, whose `request_retention` is fixed at 0.9. The
        // interval must instead target the card's own desired retention. The maximum interval is
        // left at FSRS's default here and enforced by `smart_schedule`, which has the config.
        let fsrs = FSRS::new(rs_fsrs::Parameters {
            request_retention: card.desired_retention,
            ..Default::default()
        });
        // This returns 4 versions of the card, from which we select one depending on the rating chosen by the user.
        let record_log_fsrs = fsrs.repeat(card_fsrs, reviewed_at);
        let rating = number_to_rating(rating).ok_or(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::InvalidRating(rating),
        )))?;
        let scheduling_info_fsrs = record_log_fsrs.get(&rating).unwrap();
        let (new_card, new_review_log) = fsrs_card_to_card(
            &scheduling_info_fsrs.card,
            &scheduling_info_fsrs.review_log,
            card,
            self.get_scheduler_name(),
            &recall_duration,
            &rate_duration,
        );
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
            rs_fsrs::Rating::Again | rs_fsrs::Rating::Hard => Ok(Some(Value::Object(Map::new()))),
            rs_fsrs::Rating::Good => {
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
            rs_fsrs::Rating::Easy => Ok(None),
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

        let cards_internal =
            get_safe_cards(db, &cards, &MoveCardAction::Advance, card_due_limit).await?;
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

        let cards_internal =
            get_safe_cards(db, &cards, &MoveCardAction::Postpone, card_due_limit).await?;
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
            config.minimum_interval,
            config.maximum_interval,
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
            config.minimum_interval,
            config.maximum_interval,
        )
        .await
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
        (min_days..=max_days)
            .map(|days| last_review.reviewed_at + Duration::days(days))
            .filter(|due| *due >= today_start)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::generate_review_logs;

    /// Random histories, each ending with the review that produced the card.
    fn random_cards(count: usize) -> Vec<(Card, Vec<ReviewLog>)> {
        let mut rng = rand::rng();
        let scheduler = FSRS::default();
        (0..count)
            .map(|_| {
                let created_at = Utc::now() - Duration::days(rng.random_range(0..400));
                generate_review_logs(&scheduler, Card::new(created_at), &mut rng)
            })
            .collect()
    }

    #[test]
    fn due_candidates_stay_within_the_fuzz_range() {
        let config = SparesExternalConfig::default();
        let scheduler = FSRS::default();
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
        let scheduler = FSRS::default();
        for (card, review_logs) in random_cards(100) {
            // Reviewed long enough ago that the whole window has passed.
            let at = card.due + Duration::days(60);
            assert_eq!(
                scheduler.due_candidates(&config, &card, &review_logs, at),
                Vec::<DateTime<Utc>>::new()
            );
        }
    }
}
