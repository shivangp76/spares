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
mod disperse;
mod easy_days;
mod reposition;
mod utils;

use crate::{
    Error, LibraryError, SchedulerErrorKind,
    config::{SparesExternalConfig, read_external_config},
    helpers::{FractionalDays, get_start_end_local_date},
    model::{Card, RatingId, ReviewLog},
    schedulers::{SrsScheduler, stepped_range_inclusive},
    schema::review::{Rating, RatingSubmission},
};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, Local, Utc};
use disperse::disperse_siblings_distance;
use indexmap::IndexMap;
use itertools::Itertools;
use log::info;
use rand::{
    Rng,
    distributions::{Distribution, WeightedIndex},
    rngs::ThreadRng,
};
use reposition::{MoveCardAction, get_safe_cards, move_cards};
use rs_fsrs::State;
use sqlx::SqlitePool;
use std::collections::HashMap;
use utils::{
    card_to_fsrs_card, fsrs_card_to_card, get_fuzz_range, number_to_rating, number_to_state,
    rating_to_number, state_to_number,
};

pub use rs_fsrs::FSRS;
use serde_json::{Map, Number, Value};

// NOTE: Make sure to pass time data as a `Duration` instead of an integer representing days.
#[async_trait]
impl SrsScheduler for FSRS {
    fn get_scheduler_name(&self) -> &'static str {
        "fsrs"
    }

    fn get_ratings(&self) -> Vec<Rating> {
        rs_fsrs::Rating::iter()
            .map(|fsrs_rating| Rating {
                id: rating_to_number(*fsrs_rating),
                description: format!("{:?}", fsrs_rating),
            })
            .collect::<Vec<_>>()
    }

    async fn get_leeches(&self, db: &SqlitePool) -> Result<Vec<Card>, Error> {
        let cards_lapses: Vec<(i64, u32)> = sqlx::query_as(
            r"SELECT card_id, COUNT(*)
            FROM review_log
            WHERE state = ? AND rating = ?
            GROUP BY card_id",
        )
        .bind(state_to_number(State::Review))
        .bind(rating_to_number(rs_fsrs::Rating::Again))
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        let config = read_external_config()?;
        let cards_leeches = cards_lapses
            .into_iter()
            .filter(|(_card_id, lapses)| *lapses > config.leech.lapses_threshold)
            .collect::<Vec<_>>();
        let mut query = sqlx::query_as(
            r"SELECT * FROM card
             WHERE note_id IN (?)
             AND state = ?
             AND special_state IS NULL
           ORDER BY card.due ASC",
        );
        for (card_id, _) in cards_leeches {
            query = query.bind(card_id);
        }
        let cards = query
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
        mut rng: &mut ThreadRng,
    ) -> Vec<Vec<(RatingSubmission, DateTime<Utc>)>> {
        // Again = 1, Hard = 2, Good = 3, Easy = 4,
        // let weights = [0, 0, 0, 3]; // Weights favoring 3
        let weights = [1, 3, 5, 3]; // Weights favoring 3
        let dist = WeightedIndex::new(weights).unwrap();
        let ratings: Vec<u32> = (1..=num_reviews)
            .map(|_| (dist.sample(&mut rng) + 1) as u32) // Add 1 to map to 1..=4
            .collect::<Vec<_>>();

        let mut all_review_histories = Vec::new();
        for card_id in 1..=num_siblings {
            let (_card, review_logs) =
                ratings
                    .iter()
                    .fold((Card::new(first_review_date), Vec::new()), |acc, rating| {
                        let (card, mut review_logs): (_, Vec<ReviewLog>) = acc;
                        let duration = Duration::new(rng.gen_range(5..=60), 0).unwrap();
                        let previous_review_log = review_logs.last();
                        let reviewed_at = previous_review_log.map_or_else(
                            || first_review_date,
                            |rl| rl.reviewed_at + Duration::new(rl.scheduled_time, 0).unwrap(),
                        );
                        let (new_card, new_review_log) = <FSRS as SrsScheduler>::schedule(
                            self,
                            &card,
                            previous_review_log.cloned(),
                            *rating,
                            reviewed_at,
                            duration,
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
                            rating: review_log.rating,
                            duration: Duration::seconds(review_log.duration),
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
        duration: Duration,
    ) -> Result<(Card, ReviewLog), Error> {
        let state = number_to_state(card.state).ok_or(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::InvalidState(card.state),
        )))?;
        let last_review = previous_review_log.map_or(DateTime::<Utc>::MIN_UTC, |r| r.reviewed_at);
        let card_fsrs = card_to_fsrs_card(card, state, last_review);
        // This returns 4 versions of the card, from which we select one depending on the rating chosen by the user.
        let record_log_fsrs = self.repeat(card_fsrs, reviewed_at);
        let rating = number_to_rating(rating).ok_or(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::InvalidRating(rating),
        )))?;
        let scheduling_info_fsrs = record_log_fsrs.get(&rating).unwrap();
        let (new_card, new_review_log) = fsrs_card_to_card(
            &scheduling_info_fsrs.card,
            &scheduling_info_fsrs.review_log,
            card,
            self.get_scheduler_name(),
            &duration,
        );
        Ok((new_card, new_review_log))
    }

    fn filtered_tag_schedule(
        &self,
        filtered_tag_scheduler_data: Option<&Value>,
        _card: &Card,
        rating: RatingId,
        _reviewed_at: DateTime<Utc>,
        _duration: Duration,
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
        requested_date: DateTime<Utc>,
    ) -> Result<String, Error> {
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
        .bind(count)
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
        requested_date: DateTime<Utc>,
    ) -> Result<String, Error> {
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

    /// PROPOSAL:
    /// - If no easy days and yes siblings, then old disperse siblings
    /// - If yes easy days and yes siblings, then:
    ///   - For each sibling, determine fuzz range. Determine weights for fuzz range by looking at easy days. If fuzz range < 7 days, then scale each weight in fuzz range to increase by other easy days percentages not in fuzz range. If > 7 days, then the same but to decrease weights.
    ///   - In parallel, for each sibling use current disperse siblings to determine new date. If no siblings need to be changed, then just sample from current weights and we are done. If some siblings change, then look at those new siblings' due dates. Skew the weights in the direction of the new siblings' due dates. Then, sample from the distribution and we are done.
    #[expect(clippy::comparison_chain)]
    #[allow(clippy::too_many_lines, reason = "still a work in progress")]
    async fn smart_schedule(
        &self,
        config: &SparesExternalConfig,
        data: &(Card, Vec<ReviewLog>),
        siblings_with_review_logs: &[(Card, Vec<ReviewLog>)],
        _at: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, Error> {
        let (main_card, main_review_logs) = data;
        if main_review_logs.is_empty() {
            return Ok(main_card.due);
        }

        let mut all_cards = siblings_with_review_logs.to_vec();
        all_cards.push(data.clone());
        let dispersed_siblings = disperse_siblings_distance(
            &all_cards,
            config.minimum_interval,
            config.maximum_interval,
        );
        let easy_days_disabled = !config.easy_days.enabled
            || (config.easy_days.specific_dates.is_empty()
                && config
                    .easy_days
                    .days_to_workload_percentage
                    .values()
                    .all(|v| (v - 1. / 7.).abs() < f64::EPSILON));
        if easy_days_disabled {
            if let Some(cards_and_dues) = dispersed_siblings {
                let main_card_new_due = cards_and_dues
                    .iter()
                    .find(|(card_id, _new_due)| *card_id == main_card.id)
                    .unwrap()
                    .1;
                return Ok(main_card_new_due);
            }
            return Ok(main_card.due);
        }

        // Determine weights
        let parameters = rs_fsrs::Parameters {
            request_retention: main_card.desired_retention,
            maximum_interval: config.maximum_interval.num_days() as i32,
            // We are manually using `get_fuzz_range()` here, so we don't want to enable fuzz when getting the next interval.
            enable_fuzz: false,
            ..Default::default()
        };
        let last_elapsed_time = main_review_logs
            .iter()
            .rev()
            .tuple_windows()
            .next()
            .map_or_else(Duration::zero, |(latest_rl, second_latest_rl)| {
                latest_rl.reviewed_at - second_latest_rl.reviewed_at
            });
        let next_interval = Duration::fractional_days(
            parameters.next_interval(main_card.stability, last_elapsed_time.num_days()),
        );
        let (min_ivl, max_ivl) = get_fuzz_range(
            next_interval,
            last_elapsed_time,
            config.maximum_interval,
            config.minimum_interval,
        );
        let possible_intervals = stepped_range_inclusive(min_ivl, max_ivl, Duration::days(1))
            .into_iter()
            .filter(|duration| *duration > Duration::zero())
            .collect::<Vec<_>>();
        if possible_intervals.len() == 1 {
            let chosen_interval = possible_intervals[0];
            let chosen_due_date = main_review_logs.last().unwrap().reviewed_at + chosen_interval;
            return Ok(chosen_due_date);
        }
        // TODO: Maybe weights should initially be a bell curve and then we can convolve it with the days_to_workload_percentage to get a new distribution.
        let mut weights = possible_intervals
            .iter()
            .map(|duration| main_review_logs.last().unwrap().reviewed_at + *duration)
            .map(|date| {
                let day = date.with_timezone(&Local).weekday();
                *config
                    .easy_days
                    .days_to_workload_percentage
                    .get(&day)
                    .unwrap()
            })
            .collect::<Vec<_>>();

        // Normalize weights to ensure they sum up to 1
        let sum: f64 = weights.iter().sum();
        for weight in &mut weights {
            *weight /= sum;
        }

        // if dispersed_siblings.is_some() {
        //     println!("BEFORE");
        //     dbg!(&weights);
        // }

        if let Some(ref new_dues) = dispersed_siblings {
            // Skew weights in the direction of the new due date
            let original_dues_map = all_cards
                .iter()
                .map(|(card, _)| (card.id, card.due))
                .collect::<HashMap<_, _>>();
            for (card_id, new_due) in new_dues {
                let original_due_opt = original_dues_map.get(card_id);
                assert!(original_due_opt.is_some());
                let original_due = original_due_opt.unwrap();
                let diff = *new_due - *original_due;
                if diff.num_seconds() < 1 {
                    continue;
                }
                let weights_len = weights.len();

                // Exponentially skew weights based on the time difference.
                // let diff_days = diff.num_fractional_days();
                // TODO: More diff_days should mean there is a stronger difference
                // dbg!(&diff_days);
                for (i, weight) in weights.iter_mut().enumerate() {
                    let skewed_index = if diff > Duration::zero() {
                        // New due date is later, so increase weight for later indices
                        weights_len - i
                    } else {
                        // New due date is earlier, so increase weight for earlier indices
                        i
                    };
                    // let factor = (-0.5 * diff_days * (j as f64 / weights_len as f64)).exp();
                    let factor = if skewed_index > weights_len / 2 {
                        // let factor = 1.2 * diff_days.abs();
                        0.1
                    } else if skewed_index < weights_len / 2 {
                        -0.1
                    } else {
                        0.
                    };
                    *weight = (*weight + factor).max(0.);
                }
            }

            // Normalize weights to ensure they sum up to 1
            let sum: f64 = weights.iter().sum();
            for weight in &mut weights {
                *weight /= sum;
            }
        }

        // Sample from weights
        let mut rng = rand::thread_rng();
        // if dispersed_siblings.is_some() {
        //     println!("AFTER");
        //     dbg!(&weights);
        // }
        let dist = rand::distributions::WeightedIndex::new(&weights).unwrap();
        let chosen_interval = possible_intervals[dist.sample(&mut rng)];
        let chosen_due_date = main_review_logs.last().unwrap().reviewed_at + chosen_interval;

        if chosen_due_date != main_card.due {
            info!(
                "[Card {}, Siblings: {}] Due date changed by smart schedule from {} to {}",
                main_card.id,
                siblings_with_review_logs.len(),
                main_card.due,
                chosen_due_date
            );
            let options_with_weights = possible_intervals
                .into_iter()
                .zip(weights)
                .map(|(interval, weight)| {
                    (
                        main_review_logs.last().unwrap().reviewed_at + interval,
                        weight,
                    )
                })
                .collect::<IndexMap<_, _>>();
            dbg!(&options_with_weights);
        }

        Ok(chosen_due_date)
    }
}
