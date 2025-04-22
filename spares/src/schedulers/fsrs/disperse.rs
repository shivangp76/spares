use crate::{
    helpers::FractionalDays,
    model::{Card, ReviewLog},
};
use chrono::{DateTime, Duration, Utc};
use itertools::Itertools;
use log::info;
use std::{cmp, ops};

use super::utils::get_fuzz_range;

// DB is used to:
// 1. get siblings of cards
// 2. get 2 latest review logs for each card
// 3. update card due date
// async fn disperse_siblings(
//     &self,
//     db: &SqlitePool,
//     config: &SparesExternalConfig,
//     note_ids: Option<Vec<NoteId>>,
// ) -> Result<(), SrsError> {
//     let cards_with_siblings: Vec<Card> = if let Some(note_ids) = note_ids {
//         let mut query = sqlx::query_as(
//             r"SELECT * FROM card
//              WHERE note_id IN (?)
//              AND state = ?
//              AND special_state IS NULL
//            ORDER BY card.due ASC",
//         );
//         for note_id in note_ids {
//             query = query.bind(note_id);
//         }
//         query = query.bind(state_to_number(State::Review));
//         query
//             .fetch_all(db)
//             .await
//             .map_err(|e| SrsError::Sqlx(e, String::new()))?
//     } else {
//         sqlx::query_as(
//             r"SELECT * FROM card
//              WHERE note_id IN (
//                 SELECT note_id FROM cards
//                 WHERE state = ?
//                   AND special_state IS NULL
//                   GROUP BY note_id
//                   HAVING COUNT(*) > 1
//              )
//              AND state = ?
//              AND special_state IS NULL
//            ORDER BY card.due ASC",
//         )
//         .bind(state_to_number(State::Review))
//         .bind(state_to_number(State::Review))
//         .fetch_all(db)
//         .await
//         .map_err(|e| SrsError::Sqlx(e, String::new()))?
//     };
//     let grouped_cards = cards_with_siblings
//         .into_iter()
//         .map(|c| (c.note_id, c))
//         .into_group_map();
//     let mut card_siblings_with_review_logs_all = Vec::new();
//     for (_note_id, card_siblings) in grouped_cards {
//         let mut card_siblings_with_review_logs = Vec::new();
//         // Get latest reviews for this card
//         for card in card_siblings {
//             let review_logs: Vec<ReviewLog> = sqlx::query_as(
//                 r"SELECT * FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC LIMIT 2",
//             )
//             .bind(card.id)
//             .fetch_all(db)
//             .await
//             .map_err(|e| SrsError::Sqlx(e, String::new()))?;
//             card_siblings_with_review_logs.push((card, review_logs));
//         }
//         card_siblings_with_review_logs_all.push(card_siblings_with_review_logs);
//     }
//     let new_due_dates = card_siblings_with_review_logs_all
//         .into_iter()
//         .map(|card_siblings_with_review_logs| {
//             disperse_siblings_distance(
//                 card_siblings_with_review_logs,
//                 config.minimum_interval,
//                 config.maximum_interval,
//             )
//         })
//         .collect::<Result<Vec<_>, _>>()?
//         .into_iter()
//         .flatten()
//         .collect::<Vec<_>>();
//     // Update cards due dates
//     for (card_id, new_due_date) in new_due_dates {
//         let _update_card_result = sqlx::query(
//             r"UPDATE card SET due = ?, updated_at = ? WHERE id = ?",
//         )
//         .bind(new_due_date.timestamp())
//         .bind(Utc::now().timestamp())
//         .bind(card_id)
//         .execute(db)
//         .await
//         .map_err(|e| SrsError::Sqlx(e, String::new()))?;
//     }
//     Ok(())
// }

/// Expects input review logs to be sorted. The last one should be the latest one.
pub fn disperse_siblings_distance(
    card_siblings: &[(Card, Vec<ReviewLog>)],
    minimum_interval: Duration,
    maximum_interval: Duration,
) -> Option<Vec<(i64, DateTime<Utc>)>> {
    let mut due_ranges = Vec::new();
    let mut reviewed_ats = Vec::new();
    let now = Utc::now();
    for (card, review_logs) in card_siblings {
        let due_range = get_due_range(card, review_logs, maximum_interval, minimum_interval, now);
        due_ranges.push((Some(card.id), due_range));
        let latest_review_log = review_logs.last();
        if let Some(review_log) = latest_review_log {
            reviewed_ats.push(review_log.reviewed_at);
        }
        info!(
            "Card Id: {}. Due Range: {} - {}",
            card.id, due_range.0, due_range.1
        );
    }
    // Add a pseudo-sibling with due date equal to the latest review date of a sibling
    let latest_review = reviewed_ats.iter().max();
    if let Some(latest_review) = latest_review {
        due_ranges.push((None, (*latest_review, *latest_review)));
    }
    let solution =
        maximize_siblings_due_gap(due_ranges.clone(), Duration::days(1), Duration::zero());

    if let Some((best_due_dates, _min_gap)) = solution {
        let new_due_dates = best_due_dates
            .into_iter()
            .filter_map(|(card_id, due_date)| card_id.map(|card_id| (card_id, due_date)))
            .collect::<Vec<_>>();
        // assert!(new_due_dates
        //     .iter()
        //     .all(|(_, d)| d.date_naive() >= now.date_naive()));
        assert_eq!(card_siblings.len(), new_due_dates.len());
        return Some(new_due_dates);
    }
    // Due dates are too close to disperse
    None
}

fn get_due_range(
    card: &Card,
    review_logs: &[ReviewLog],
    maximum_interval: Duration,
    minimum_interval: Duration,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let parameters = rs_fsrs::Parameters {
        request_retention: card.desired_retention,
        maximum_interval: maximum_interval.num_days() as i32,
        // We are manually using `get_fuzz_range()` here, so we don't want to enable fuzz when getting the next interval.
        enable_fuzz: false,
        ..Default::default()
    };
    let last_elapsed_time = review_logs
        .iter()
        .rev()
        .tuple_windows()
        .next()
        .map_or_else(Duration::zero, |(latest_rl, second_latest_rl)| {
            latest_rl.reviewed_at - second_latest_rl.reviewed_at
        });
    let next_interval = Duration::fractional_days(
        parameters.next_interval(card.stability, last_elapsed_time.num_days()),
    );
    if next_interval <= minimum_interval || review_logs.is_empty() {
        return (card.due, card.due);
    }
    let latest_review_log = review_logs.first().unwrap();

    let (mut min_ivl, mut max_ivl) = get_fuzz_range(
        next_interval,
        last_elapsed_time,
        maximum_interval,
        minimum_interval,
    );

    if card.due > latest_review_log.reviewed_at + max_ivl {
        // +2 is just a safeguard to exclude cards that go beyond the fuzz range due to rounding
        // don't reschedule the card to bring it within the fuzz range. Rather, create another fuzz range around the original due date.
        let current_ivl = card.due - latest_review_log.reviewed_at;
        // set maximum_interval = current_ivl to prevent a further increase in ivl
        (min_ivl, max_ivl) = get_fuzz_range(
            current_ivl,
            last_elapsed_time,
            current_ivl,
            minimum_interval,
        );
    }
    if card.due.date_naive() >= now.date_naive() {
        (
            (latest_review_log.reviewed_at + min_ivl).max(now),
            (latest_review_log.reviewed_at + max_ivl).max(now),
        )
    } else if latest_review_log.reviewed_at + max_ivl > now {
        (now, latest_review_log.reviewed_at + max_ivl)
    } else {
        (card.due, card.due)
    }
}

/// Finds the arrangement that maximizes the gaps between adjacent points while maintaining the maximum minimum gap.
#[allow(clippy::type_complexity)]
fn maximize_siblings_due_gap<T, A, B>(
    mut data: Vec<(T, (A, A))>,
    delta: B,
    initial_gap: B,
) -> Option<(Vec<(T, A)>, B)>
where
    T: Copy + std::fmt::Debug,
    A: Ord
        + Copy
        + ops::Add<B, Output = A>
        + ops::Sub<B, Output = A>
        + ops::Sub<A, Output = B>
        + std::fmt::Debug,
    B: Ord
        + Copy
        + ops::Add<B, Output = B>
        + ops::Sub<B, Output = B>
        + ops::Div<i32, Output = B>
        + std::fmt::Debug,
{
    // Sort the list based on the right endpoints of the intervals
    data.sort_by_key(|d| d.1.1);

    // First, find the maximum minimum gap and the arrangement that achieves it
    let intervals = data.iter().map(|x| x.1).collect::<Vec<_>>();
    let initial_solution = find_max_min_gap_and_arrangement(intervals, delta, initial_gap);

    // Initialize the optimized arrangement with the initial arrangement
    // let mut optimized_arrangement = initial_arrangement
    //     .into_iter()
    //     .zip(data.iter())
    //     .map(|(d, (card_id, _limits))| (*card_id, d))
    //     .collect::<Vec<_>>();

    // Go through each point to try to maximize the gap with its adjacent points
    // for (i, (_card_id, (_left_limit, mut right_limit))) in data.iter().enumerate() {
    //     // Set initial boundaries based on the previous and next points in the arrangement
    //     // if i > 0 {
    //     //     left_limit = cmp::max(left_limit, optimized_arrangement[i - 1] + max_min_gap);
    //     // }
    //     if i < data.len() - 1 {
    //         right_limit = cmp::min(right_limit, optimized_arrangement[i + 1].1 - max_min_gap);
    //     }
    //
    //     // Move the point as far to the right as possible within the adjusted limits
    //     optimized_arrangement[i].1 = right_limit;
    // }

    if let Some((max_min_gap, initial_arrangement)) = initial_solution {
        let last_index = data.len() - 1;
        let optimized_arrangement = data
            .into_iter()
            .zip(initial_arrangement.iter())
            .enumerate()
            .map(|(i, ((info, (_left_limit, mut right_limit)), _))| {
                if i < last_index {
                    right_limit = cmp::min(right_limit, initial_arrangement[i + 1] - max_min_gap);
                }
                (info, right_limit)
            })
            .collect::<Vec<_>>();
        return Some((optimized_arrangement, max_min_gap));
    }
    None
}

/// A greedy algorithm to check if we can place all points with a minimum gap of `min_gap`. Returns the arrangement if possible.
fn can_place_points_with_arrangement<A, B>(points: &[(A, A)], min_gap: B) -> Option<Vec<A>>
where
    A: Ord + Copy + ops::Add<B, Output = A> + std::fmt::Debug,
    B: Copy,
{
    if points.is_empty() {
        return Some(Vec::new());
    }

    // Place the first point at its leftmost position
    // SAFETY: `points` is validated to be not empty.
    let mut last_point_position = points[0].0;
    let mut temp_arrangement = vec![last_point_position];

    for (start, end) in points.iter().skip(1) {
        let next_possible_point = last_point_position + min_gap;
        // Find the rightmost position in the current point's range where it can be placed
        if next_possible_point > *end {
            // Can't place the point while maintaining the minimum gap
            return None;
        }
        last_point_position = next_possible_point.max(*start);
        temp_arrangement.push(last_point_position);
    }

    Some(temp_arrangement)
}

/// Find the maximum minimum gap between adjacent points and also return the arrangement that achieves it.
#[allow(clippy::similar_names)]
fn find_max_min_gap_and_arrangement<A, B>(
    mut points: Vec<(A, A)>,
    delta: B,
    initial_gap: B,
) -> Option<(B, Vec<A>)>
where
    A: Ord + Copy + ops::Add<B, Output = A> + ops::Sub<A, Output = B> + std::fmt::Debug,
    B: Ord
        + Copy
        + ops::Add<B, Output = B>
        + ops::Sub<B, Output = B>
        + ops::Div<i32, Output = B>
        + std::fmt::Debug,
{
    // Sort the points based on their right endpoints
    points.sort_by_key(|d| d.1);

    // Initialize binary search parameters
    let mut min_gap = initial_gap;
    let mut max_gap = points.last().unwrap().1 - points.first().unwrap().0;
    let mut best_gap = min_gap;
    let mut arrangement = None;
    while min_gap <= max_gap {
        let mid_gap = (min_gap + max_gap) / 2;

        let temp_arrangement_opt = can_place_points_with_arrangement(&points, mid_gap);
        if let Some(temp_arrangement) = temp_arrangement_opt {
            // If we can place all points with this gap, it means we can try to increase it>
            best_gap = mid_gap;
            arrangement = Some(temp_arrangement);
            min_gap = mid_gap + delta;
        } else {
            // If we can't place all points with this gap, it means we need to try a smaller gap.
            max_gap = mid_gap - delta;
        }
    }
    arrangement.map(|best_arrangement| (best_gap, best_arrangement))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::generate_review_logs;
    use crate::config::read_external_config;
    use crate::schedulers::get_scheduler_from_string;
    use chrono::{TimeZone, Utc};
    use rand::Rng;
    use std::collections::HashMap;

    #[test]
    fn test_disperse_siblings_distance() {
        let mut none_count = 0;
        let mut no_changes_count = 0;
        let total = 100;
        let mut rng = rand::thread_rng();
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        for _ in 1..=total {
            let siblings_count = rng.gen_range(1..=20);
            let card_siblings = (1..=siblings_count)
                .map(|_| {
                    let mut initial_card = Card::new(Utc::now());
                    initial_card.id = rng.gen_range(1..=999);
                    generate_review_logs(scheduler.as_ref(), initial_card, &mut rng)
                })
                .collect::<Vec<_>>();
            let config = read_external_config().unwrap();
            let result = disperse_siblings_distance(
                &card_siblings,
                config.minimum_interval,
                config.maximum_interval,
            );
            // dbg!(&result);
            if let Some(cards_and_dues) = result {
                let cards_to_original_due = card_siblings
                    .iter()
                    .map(|(card, _)| (card.id, card.due))
                    .collect::<HashMap<_, _>>();
                let changed_cards_ids = cards_and_dues
                    .iter()
                    .filter(|(card_id, new_due)| {
                        cards_to_original_due.get(card_id).unwrap() != new_due
                    })
                    .map(|(card_id, _new_due)| card_id)
                    .collect::<Vec<_>>();
                if changed_cards_ids.is_empty() {
                    no_changes_count += 1;
                }
                // dbg!(&card_siblings);
                // dbg!(&cards_and_dues);
                // dbg!(&changed_cards_ids.len());
            } else {
                none_count += 1;
                let mut dues = card_siblings
                    .iter()
                    .map(|(x, _)| x.due.clone())
                    .collect::<Vec<_>>();
                dues.sort();
                // dbg!(&dues);
            }
        }
        dbg!(&none_count);
        dbg!(&no_changes_count);
        dbg!(&total);
        // `changes_count` is around 25% of `total_count` with weights [1,3,5,3]
        let changes_count = total - no_changes_count - none_count;
        dbg!(&changes_count);
        // assert!(true);
    }

    // #[test]
    // fn test_find_gap() {
    //     let points = vec![];
    //     let delta = Duration::days(1);
    //     let initial_gap = Duration::zero();
    //     let result = find_max_min_gap_and_arrangement(points, delta, initial_gap);
    //     dbg!(&result);
    //     assert!(false);
    // }

    #[test]
    fn test_maximize_siblings_due_gap_1() {
        let data = vec![
            (
                0,
                (
                    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
                    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
                ),
            ),
            (
                1,
                (
                    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
                    Utc.with_ymd_and_hms(2024, 1, 3, 0, 0, 0).unwrap(),
                ),
            ),
            (
                2,
                (
                    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
                    Utc.with_ymd_and_hms(2024, 1, 3, 0, 0, 0).unwrap(),
                ),
            ),
        ];
        let solution = maximize_siblings_due_gap(data, Duration::days(1), Duration::zero());
        assert!(solution.is_some());
        let (siblings, min_gap) = solution.unwrap();
        dbg!(&siblings);
        dbg!(&min_gap.num_days());
        assert_eq!(min_gap.num_days(), 1);
    }

    #[test]
    fn test_maximize_siblings_due_gap_2() {
        let data = vec![
            (
                0,
                (
                    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
                    Utc.with_ymd_and_hms(2024, 1, 3, 0, 0, 0).unwrap(),
                ),
            ),
            (
                1,
                (
                    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
                    Utc.with_ymd_and_hms(2024, 1, 3, 0, 0, 0).unwrap(),
                ),
            ),
            (
                2,
                (
                    Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
                    Utc.with_ymd_and_hms(2024, 1, 4, 0, 0, 0).unwrap(),
                ),
            ),
        ];
        let solution = maximize_siblings_due_gap(data, Duration::days(1), Duration::zero());
        assert!(solution.is_some());
        let (siblings, min_gap) = solution.unwrap();
        dbg!(&siblings);
        dbg!(&min_gap.num_days());
        assert_eq!(min_gap.num_days(), 1);
    }
}
