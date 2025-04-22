// use super::stepped_range_inclusive;
// use super::utils::{get_next_interval, state_to_number};
// use crate::helpers::get_start_end_local_date;
// use crate::schedulers::fsrs::utils::get_fuzz_range;
// use crate::{
//     config::{EasyDaysConfig, SparesExternalConfig},
//     helpers::FractionalDays,
//     model::{Card, ReviewLog},
//     Error,
// };
// use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc};
// use fsrs::State;
// use itertools::Itertools;
// use log::info;
// use rand::{distributions::Distribution, thread_rng};
// use sqlx::SqlitePool;
// use std::collections::{HashMap, HashSet};
//
// // Called `check_review_distribution` in `fsrs4anki-helper`.
// fn get_review_count_adjustments(workload_percentage_and_review_count: &[(f64, i64)]) -> Vec<f64> {
//     let (workload_percentages_sum, total_reviews_count) = workload_percentage_and_review_count
//         .iter()
//         .fold((0., 0.), |(sum_f64, sum_i64), &(f, i)| {
//             (sum_f64 + f, sum_i64 + i as f64)
//         });
//     // Each day cannot have no workload. If this is the case, reset all to the normal workload.
//     if workload_percentages_sum.abs() < f64::EPSILON {
//         return vec![1.; workload_percentage_and_review_count.len()];
//     }
//     workload_percentage_and_review_count
//         .iter()
//         .map(|(workload_percentage, review_count)| {
//             let expected_review_count =
//                 total_reviews_count * (workload_percentage / workload_percentages_sum);
//             (expected_review_count - (*review_count as f64)).max(0.)
//         })
//         .collect::<Vec<_>>()
// }
//
// fn load_balance(
//     interval_and_review_count: Vec<(Duration, i64)>,
//     easy_days_config: &EasyDaysConfig,
//     last_review: DateTime<Utc>,
//     now: DateTime<Utc>,
// ) -> Duration {
//     if easy_days_config.specific_dates.is_empty()
//         && easy_days_config
//             .days_to_workload_percentage
//             .values()
//             .all(|v| (v - 1.).abs() < f64::EPSILON)
//     {
//         return interval_and_review_count
//             .iter()
//             .min_by_key(|(_, review_count)| review_count)
//             .unwrap()
//             .0;
//     }
//     let weights = interval_and_review_count
//         .iter()
//         .map(|(interval, review_count)| {
//             if *review_count == 0 {
//                 1.
//             } else {
//                 // <https://github.com/open-spaced-repetition/load-balance-simulator/issues/1#issuecomment-2361674400>
//                 (1. / ((*review_count as f64).powf(2.))) * (1. / interval.num_fractional_days())
//             }
//         })
//         .collect::<Vec<_>>();
//     let workload_percentage_and_review_count = interval_and_review_count
//         .iter()
//         .map(|(interval, review_count)| {
//             let weekday = (now + *interval).weekday();
//             (
//                 *easy_days_config
//                     .days_to_workload_percentage
//                     .get(&weekday)
//                     .unwrap(),
//                 *review_count,
//             )
//         })
//         .collect::<Vec<_>>();
//     let mut mask = get_review_count_adjustments(&workload_percentage_and_review_count);
//     let (possible_intervals, _): (Vec<_>, Vec<_>) = interval_and_review_count.into_iter().unzip();
//     for (i, interval) in possible_intervals.iter().enumerate() {
//         if easy_days_config
//             .specific_dates
//             .contains(&(last_review + *interval).date_naive())
//         {
//             mask[i] = 0.;
//         }
//     }
//     let final_weights = weights
//         .iter()
//         .zip(mask.iter())
//         .map(|(&w, &m)| w * m)
//         .collect::<Vec<_>>();
//     let mut rng = thread_rng();
//     let mut chosen_weights = if final_weights.iter().sum::<f64>() > 0.0 {
//         final_weights
//     } else {
//         weights
//     };
//     // Decay weights, so today's failed reviews cause the future reviews to be balanced.
//     // See <https://github.com/open-spaced-repetition/fsrs4anki-helper/issues/372>
//     let decay_factor: f64 = 0.9;
//     for (i, weight) in chosen_weights.iter_mut().enumerate() {
//         *weight *= decay_factor.powi(i as i32);
//     }
//     let dist = rand::distributions::WeightedIndex::new(&chosen_weights).unwrap();
//     possible_intervals[dist.sample(&mut rng)]
// }
//
// pub fn apply_fuzz(
//     interval: Duration,
//     card: &Card,
//     review_log: &ReviewLog,
//     config: &SparesExternalConfig,
//     date_to_due_count: &HashMap<NaiveDate, i64>,
//     workload_today: i64,
// ) -> Duration {
//     if interval < config.minimum_interval {
//         return interval;
//     }
//     let now = Utc::now();
//     let easy_days_config = &config.easy_days_config;
//     let (mut min_ivl, mut max_ivl) = get_fuzz_range(
//         interval,
//         Duration::zero(),
//         config.maximum_interval,
//         config.minimum_interval,
//     );
//     if config.easy_days_config.enabled {
//         if card.due > review_log.reviewed_at + max_ivl + config.minimum_interval {
//             let current_ivl = card.due.signed_duration_since(review_log.reviewed_at);
//             (min_ivl, max_ivl) = get_fuzz_range(
//                 current_ivl,
//                 Duration::zero(),
//                 current_ivl,
//                 config.minimum_interval,
//             );
//         }
//         if review_log.reviewed_at + max_ivl < now {
//             // If the latest possible due date is in the past, skip load balance
//             return interval.min(max_ivl);
//         }
//         // Don't schedule the card in the past
//         min_ivl = min_ivl.max(now.signed_duration_since(review_log.reviewed_at));
//
//         let possible_intervals = (min_ivl.num_days()..=max_ivl.num_days())
//             .map(Duration::days)
//             .collect::<Vec<_>>();
//         let interval_and_review_count = possible_intervals
//             .into_iter()
//             .map(|check_ivl| {
//                 let check_due = review_log.reviewed_at + check_ivl;
//                 let review_count = if check_due > now {
//                     // If the due date is in the future, the workload is the number of cards due on that day
//                     *date_to_due_count.get(&check_due.date_naive()).unwrap_or(&0)
//                 } else {
//                     // If the due date is today, the workload is the number of cards due today plus the number of cards learned today
//                     // &(due_today + reviewed_today)
//                     workload_today
//                 };
//                 (check_ivl, review_count)
//             })
//             .collect::<Vec<_>>();
//         load_balance(
//             interval_and_review_count,
//             easy_days_config,
//             review_log.reviewed_at,
//             now,
//         )
//     } else {
//         let fuzz_factor: f64 = rand::random();
//         // NOTE: The following is the updated code for newer Anki versions. Translate it and replace.
//         // Refer to `anki/rslib/src/scheduler/states/load_balancer.rs`.
//         // return ivl + mw.col.fuzz_delta(self.card.id, ivl)
//         Duration::fractional_days(
//             fuzz_factor * ((max_ivl - min_ivl + Duration::days(1)) + min_ivl).num_fractional_days(),
//         )
//     }
// }
//
// async fn reschedule(
//     db: &SqlitePool,
//     config: &SparesExternalConfig,
//     mut cards_with_review_logs: Vec<(Card, Vec<ReviewLog>)>,
// ) -> Result<(), Error> {
//     let all_review_cards: Vec<Card> =
//         sqlx::query_as(r"SELECT * FROM card WHERE state = ? AND special_state IS NULL")
//             .bind(state_to_number(State::Review))
//             .fetch_all(db)
//             .await
//             .map_err(|e| Error::Sqlx { source: e })?;
//     let mut date_to_due_count = all_review_cards
//         .into_iter()
//         .map(|card| card.due.date_naive())
//         .fold(HashMap::new(), |mut acc, date| {
//             *acc.entry(date).or_insert(0) += 1;
//             acc
//         });
//     let now = Utc::now();
//     let today = now.date_naive();
//     let mut due_today: i64 = date_to_due_count
//         .iter()
//         .filter(|(&date, _)| date <= today)
//         .map(|(_, &count)| count)
//         .sum();
//     let (lower_limit, _) = get_start_end_local_date(&now);
//     let studied_today: i64 = sqlx::query_scalar(
//         r"SELECT COUNT(DISTINCT card_id) FROM review_log WHERE reviewed_at >= ?",
//     )
//     .bind(lower_limit.timestamp())
//     .fetch_one(db)
//     .await
//     .map_err(|e| Error::Sqlx { source: e })?;
//
//     // Recompute parameters
//     // cards_with_review_logs.iter_mut().try_for_each(
//     //     |(card, review_logs)| -> Result<(), SrsError> {
//     //         (card.stability, card.difficulty) =
//     //             compute_memory_state(self, card, review_logs.to_vec()).map_err(SrsError::Other)?;
//     //         Ok(())
//     //     },
//     // )?;
//
//     // Get latest review for review cards
//     let cards_to_modify = cards_with_review_logs
//         .iter_mut()
//         .filter(|(card, _)| card.state == state_to_number(State::Review))
//         .filter_map(|(card, review_logs)| {
//             review_logs
//                 .iter()
//                 .max_by_key(|rl| rl.reviewed_at)
//                 .map(|r| (card, r))
//         })
//         .collect::<Vec<_>>();
//
//     // NOTE: This must be done sequentially since it uses the previous cards and their scheduled due date to find the optimal workload for the next card. This ensures the workload is balanced, if `config.load_balance`.
//     for (card, latest_review_log) in cards_to_modify {
//         let mut new_interval = get_next_interval(card.stability, card.desired_retention);
//         // This only requires access to *read* the `date_to_due_count` `HashMap`, NOT write.
//         new_interval = apply_fuzz(
//             new_interval,
//             card,
//             latest_review_log,
//             config,
//             &date_to_due_count,
//             due_today + studied_today,
//         );
//         let due_before = card.due;
//         card.due += new_interval;
//         if config.easy_days_config.enabled {
//             date_to_due_count
//                 .entry(due_before.date_naive())
//                 .and_modify(|x| *x -= 1)
//                 .or_insert(0);
//             date_to_due_count
//                 .entry(card.due.date_naive())
//                 .and_modify(|x| *x += 1)
//                 .or_insert(0);
//             if due_before <= now && card.due > now {
//                 due_today -= 1;
//             }
//             if due_before > now && card.due <= now {
//                 due_today += 1;
//             }
//         }
//     }
//     let note_ids = cards_with_review_logs
//         .into_iter()
//         .map(|(card, _)| card.note_id)
//         .unique()
//         .collect::<Vec<_>>();
//     // disperse_siblings(db, config, Some(note_ids)).await?;
//     Ok(())
// }
//
// async fn apply_easy_days(db: &SqlitePool, config: &SparesExternalConfig) -> Result<(), Error> {
//     // if !config.easy_days_config.enabled {
//     //     return Err(SrsError::Other(
//     //         "Enable config value for easy days.".to_string(),
//     //     ));
//     // }
//     let now = Utc::now();
//     let today = now.date_naive();
//     let past_dates = config
//         .easy_days_config
//         .specific_dates
//         .iter()
//         .filter(|date| date < &&today)
//         .collect::<Vec<_>>();
//     if !past_dates.is_empty() {
//         info!(
//             "Skipping applying easy dates to {} since they are in the past.",
//             past_dates.into_iter().join(", ")
//         );
//     }
//     let reschedule_range = Duration::days(7);
//
//     // Convert `config.easy_days_config` to dates.
//     let mut dates = config
//         .easy_days_config
//         .specific_dates
//         .clone()
//         .into_iter()
//         .filter(|date| date >= &today)
//         .collect::<Vec<_>>();
//     let easy_dates = stepped_range_inclusive(Duration::zero(), reschedule_range, Duration::days(1))
//         .into_iter()
//         .map(|offset| now + offset)
//         .filter_map(|current_date| {
//             config
//                 .easy_days_config
//                 .days_to_workload_percentage
//                 .get(&current_date.with_timezone(&Local).weekday())
//                 .map(|day_review_ratio| (current_date, day_review_ratio))
//         })
//         .filter_map(|(current_date, day_review_ratio)| {
//             (day_review_ratio <= &1.).then(|| current_date)
//         })
//         .map(|date| date.date_naive())
//         .collect::<HashSet<_>>();
//     dates.extend(easy_dates);
//
//     // Get cards due on these dates and reschedule them
//     let mut cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card")
//         .fetch_all(db)
//         .await
//         .map_err(|e| Error::Sqlx { source: e })?;
//     cards = cards
//         .into_iter()
//         .filter(|card| dates.contains(&card.due.date_naive()))
//         .collect::<Vec<_>>();
//
//     // Get all review logs for cards
//     let mut query = sqlx::query_as(r"SELECT * FROM review_log WHERE card_id IN (?)");
//     for card in &cards {
//         query = query.bind(card.id);
//     }
//     let all_review_logs: Vec<ReviewLog> = query
//         .fetch_all(db)
//         .await
//         .map_err(|e| Error::Sqlx { source: e })?;
//     let grouped_review_logs = all_review_logs
//         .into_iter()
//         .map(|rl| (rl.card_id, rl))
//         .into_group_map();
//     let cards_with_review_logs = cards
//         .into_iter()
//         .map(|card| {
//             (
//                 card.clone(),
//                 grouped_review_logs.get(&card.id).unwrap().clone(),
//             )
//         })
//         .collect::<Vec<_>>();
//
//     reschedule(db, config, cards_with_review_logs).await?;
//     Ok(())
// }
