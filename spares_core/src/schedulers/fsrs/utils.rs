use std::cmp;

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use rs_fsrs::State;
use serde_json::Map;

use crate::helpers::FractionalDays;
use crate::model::Card;
use crate::model::RatingId;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::model::StateId;

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
    recall_duration: &Duration,
    rate_duration: &Duration,
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
        card_id: Some(original_card.id),
        reviewed_at: *fsrs_reviewed_date,
        kind: ReviewLogKind::Review,
        rating: Some(rating_to_number(*fsrs_rating)),
        // The scheduler has no notion of filtered tags; `rate_card` stamps this on the row it
        // actually inserts.
        tag_id: None,
        scheduler_name: scheduler_name.to_string(),
        scheduled_time: Some(Duration::days(*fsrs_scheduled_days).num_seconds()),
        recall_duration: Some(recall_duration.num_seconds()),
        rate_duration: Some(rate_duration.num_seconds()),
        previous_state: state_to_number(*fsrs_revlog_state),
        custom_data: serde_json::Value::Object(Map::new()),
    };
    (card, review_log)
}

// NOTE: rs-fsrs has the same function, but it is private.
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
    // Clamp first: fuzz is a fraction of the interval the card actually gets. Measured on an
    // interval far beyond the maximum, it would span most of the way down to zero.
    let interval = interval.min(maximum_interval);
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
    let mut min_ivl = cmp::max(minimum_interval, interval - delta);
    let max_ivl = cmp::min(maximum_interval, interval + delta);
    if interval > elapsed_time {
        min_ivl = min_ivl.max(elapsed_time + Duration::days(1));
    }
    min_ivl = min_ivl.min(max_ivl);
    (min_ivl, max_ivl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzz_range_is_measured_on_the_clamped_interval() {
        let maximum = Duration::days(180);
        let (min_ivl, max_ivl) = get_fuzz_range(
            Duration::days(3568),
            Duration::days(500),
            maximum,
            Duration::days(2),
        );
        assert_eq!(max_ivl, maximum);
        assert_eq!(
            (min_ivl, max_ivl),
            get_fuzz_range(maximum, Duration::days(500), maximum, Duration::days(2)),
            "an interval beyond the maximum is fuzzed like the maximum itself"
        );
    }
}
