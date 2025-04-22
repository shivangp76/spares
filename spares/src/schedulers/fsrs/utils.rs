use crate::{
    helpers::FractionalDays,
    model::{Card, RatingId, ReviewLog, StateId},
};
use chrono::{DateTime, Duration, Utc};
use rs_fsrs::State;
use serde_json::Map;
use std::cmp;

pub fn number_to_rating(num: RatingId) -> Option<rs_fsrs::Rating> {
    match num {
        1 => Some(rs_fsrs::Rating::Again),
        2 => Some(rs_fsrs::Rating::Hard),
        3 => Some(rs_fsrs::Rating::Good),
        4 => Some(rs_fsrs::Rating::Easy),
        _ => None,
    }
}

pub fn rating_to_number(rating: rs_fsrs::Rating) -> RatingId {
    match rating {
        rs_fsrs::Rating::Again => 1,
        rs_fsrs::Rating::Hard => 2,
        rs_fsrs::Rating::Good => 3,
        rs_fsrs::Rating::Easy => 4,
    }
}

pub fn number_to_state(num: StateId) -> Option<State> {
    match num {
        0 => Some(State::New),
        1 => Some(State::Learning),
        2 => Some(State::Review),
        3 => Some(State::Relearning),
        _ => None,
    }
}

pub fn state_to_number(state: State) -> StateId {
    match state {
        State::New => 0,
        State::Learning => 1,
        State::Review => 2,
        State::Relearning => 3,
    }
}

pub fn card_to_fsrs_card(
    card: &Card,
    state: rs_fsrs::State,
    last_review: DateTime<Utc>,
) -> rs_fsrs::Card {
    rs_fsrs::Card {
        due: card.due,
        stability: card.stability,
        difficulty: card.difficulty,
        // This value is only used as an output for FSRS, not an input.
        elapsed_days: 0,
        // This value is only used as an output for FSRS, not an input.
        scheduled_days: 0,
        // This value is not used by FSRS for scheduling.
        reps: 0,
        // This value is not used by FSRS for scheduling.
        lapses: 0,
        state,
        last_review,
    }
}

pub fn fsrs_card_to_card(
    card_fsrs: &rs_fsrs::Card,
    review_log_fsrs: &rs_fsrs::ReviewLog,
    original_card: &Card,
    scheduler_name: &str,
    duration: &Duration,
) -> (Card, ReviewLog) {
    let rs_fsrs::Card {
        due,
        stability: fsrs_stability,
        difficulty: fsrs_difficulty,
        elapsed_days: _,
        scheduled_days: fsrs_scheduled_days,
        reps: _,
        lapses: _,
        state: fsrs_state,
        last_review: _,
    } = card_fsrs;
    let rs_fsrs::ReviewLog {
        rating: fsrs_rating,
        elapsed_days: _,
        // `scheduled_days = 0` for some reason. Card's scheduled days look fine, so using that
        // instead
        scheduled_days: _,
        state: fsrs_revlog_state,
        reviewed_date: fsrs_reviewed_date,
    } = review_log_fsrs;
    let card = Card {
        id: original_card.id,
        note_id: original_card.note_id,
        order: original_card.order,
        back_type: original_card.back_type,
        created_at: original_card.created_at,
        updated_at: *fsrs_reviewed_date,
        due: *due,
        stability: *fsrs_stability,
        difficulty: *fsrs_difficulty,
        desired_retention: original_card.desired_retention,
        special_state: original_card.special_state,
        state: state_to_number(*fsrs_state),
        custom_data: original_card.custom_data.clone(),
    };
    let review_log = ReviewLog {
        id: 1,
        card_id: original_card.id,
        reviewed_at: *fsrs_reviewed_date,
        rating: rating_to_number(*fsrs_rating),
        scheduler_name: scheduler_name.to_string(),
        scheduled_time: Duration::days(*fsrs_scheduled_days).num_seconds(),
        duration: duration.num_seconds(),
        previous_state: state_to_number(*fsrs_revlog_state),
        custom_data: serde_json::Value::Object(Map::new()),
    };
    (card, review_log)
}

// NOTE: A similar function exists in FSRS, but it is private. If native smart schedule is implemented in FSRS, this won't be needed.
pub fn get_fuzz_range(
    interval: Duration,
    elapsed_time: Duration,
    maximum_interval: Duration,
    minimum_interval: Duration,
) -> (Duration, Duration) {
    // Describes a range of days for which a certain amount of fuzz is applied to the new interval.
    struct FuzzRange {
        start: Duration,
        end: Duration,
        factor: f64,
    }
    let fuzz_ranges: &[FuzzRange] = &[
        FuzzRange {
            start: Duration::fractional_days(2.5),
            end: Duration::days(7),
            factor: 0.15,
        },
        FuzzRange {
            start: Duration::days(7),
            end: Duration::days(20),
            factor: 0.1,
        },
        FuzzRange {
            start: Duration::days(20),
            end: Duration::MAX,
            factor: 0.05,
        },
    ];
    let mut delta = Duration::days(1);
    for range in fuzz_ranges {
        delta += Duration::fractional_days(
            range.factor
                * cmp::max(
                    cmp::min(interval, range.end) - range.start,
                    Duration::zero(),
                )
                .num_fractional_days(),
        );
    }
    let interval = interval.min(maximum_interval);
    let mut min_ivl = cmp::max(minimum_interval, interval - delta);
    let max_ivl = cmp::min(maximum_interval, interval + delta);
    if interval > elapsed_time {
        min_ivl = min_ivl.max(elapsed_time + Duration::days(1));
    }
    min_ivl = min_ivl.min(max_ivl);
    (min_ivl, max_ivl)
}

// let (obey_easy_days, obey_specific_due_dates) = if easy_days_config.review_ratio == 0.0 {
//     (true, true)
// } else {
//     let mut rng = rand::thread_rng();
//     let (sample_1, sample_2): (f64, f64) = rng.gen();
//     (
//         sample_1 < p_obey_easy_days_val,
//         sample_2 < p_obey_specific_due_dates_val,
//     )
// };
// let step = Duration::fractional_days(
//     (1. + (max_ivl - min_ivl).num_fractional_days() / 100.).floor(),
// );
// let range = stepped_range(min_ivl, max_ivl, step)
//     .into_iter()
//     .rev()
//     .filter(|check_ivl| {
//         let check_due = review_log.reviewed_at + *check_ivl;
//         // If the due date is on an easy day, skip
//         !(obey_specific_due_dates
//             && easy_days_config
//                 .specific_dates
//                 .contains(&check_due.date_naive()))
//             || (obey_easy_days
//                 && easy_days_config
//                     .days_to_review_ratio
//                     .contains_key(&check_due.weekday()))
//     })
//     .collect::<Vec<_>>();
// let workloads = range
//     .into_iter()
//     .map(|check_ivl| {
//         let check_due = review_log.reviewed_at + check_ivl;
//         let workload = if check_due > now {
//             // If the due date is in the future, the workload is the number of cards due on that day
//             date_to_due_count.get(&check_due.date_naive()).unwrap_or(&0)
//         } else {
//             // If the due date is today, the workload is the number of cards due today plus the number of cards learned today
//             // &(due_today + reviewed_today)
//             &workload_today
//         };
//         (check_ivl, workload)
//     })
//     .collect::<Vec<_>>();
// let (best_ivl, _min_workload) = workloads
//     .into_iter()
//     .min_by_key(|(_ivl, workload)| **workload)
//     .unwrap_or((max_ivl, &i64::MAX));
// best_ivl

// /// Calculate the probability of obeying easy days to ensure the review ratio.
// ///
// /// # Parameters
// /// - `num_of_easy_days`: The number of easy days.
// /// - `easy_days_review_ratio`: The ratio of reviews on easy days.
// ///
// /// # Math
// /// - A week has 7 days, `n` easy days, and `7 - n` non-easy days.
// /// - Assume we have `y` reviews per non-easy day, the number of reviews per easy day is `a * y`.
// /// - The total number of reviews in a week is `y * (7 - n) + a * y * n`.
// /// - The probability of a review on an easy day is the number of reviews on easy days divided by the total number of reviews:
// ///   `(a * y * n) / (y * (7 - n) + a * y * n) = (a * n) / (a * n + 7 - n)`.
// /// - The probability of skipping a review on an easy day is:
// ///   `1 - (a * n) / (a * n + 7 - n) = (7 - n) / (a * n + 7 - n)`.
// pub fn p_obey_easy_days(num_of_easy_days: f64, easy_days_review_ratio: f64) -> f64 {
//     (7.0 - num_of_easy_days) / (easy_days_review_ratio * num_of_easy_days + 7.0 - num_of_easy_days)
// }
//
// /// Calculate the probability of obeying specific due dates to ensure the review ratio.
// ///
// /// # Parameters
// /// - `num_of_specific_due_dates`: The number of specific due dates.
// /// - `easy_days_review_ratio`: The ratio of reviews on easy days.
// /// - `fuzz_range`: The number of days after (or before) the specific due date that will be rescheduled. For example, if this is 2, then 2 days before, the specific due date, and 2 days after will be rescheduled, totaling 5 days.
// ///
// /// # Math
// /// - When we have `n` specific due dates, the number of days to reschedule is `(fuzz_range * 2) + n`.
// /// - Assume we have `y` reviews per non-easy day, the number of reviews per easy day is `a * y`.
// /// - The total number of reviews in the days to reschedule is `y * (fuzz_range * 2) + a * y * n`.
// /// - The probability of a review on a specific due date is the number of reviews on specific due dates divided by the total number of reviews:
// ///   `(a * y * n) / (y * (fuzz_range * 2) + a * y * n) = (a * n) / (a * n + (fuzz_range * 2))`.
// /// - The probability of skipping a review on a specific due date is:
// ///   `1 - (a * n) / (a * n + (fuzz_range * 2)) = (fuzz_range * 2) / (a * n + (fuzz_range * 2))`.
// pub fn p_obey_specific_due_dates(
//     num_of_specific_due_dates: f64,
//     easy_days_review_ratio: f64,
//     fuzz_range: f64,
// ) -> f64 {
//     (fuzz_range * 2.0) / (easy_days_review_ratio * num_of_specific_due_dates + (fuzz_range * 2.0))
// }
