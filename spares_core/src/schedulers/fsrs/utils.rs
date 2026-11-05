use std::cmp;

use chrono::Duration;
use fsrs_rs::DEFAULT_PARAMETERS;
use fsrs_rs::FSRS;
use fsrs_rs::MemoryState;

use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::config::SparesExternalConfig;
use crate::helpers::FractionalDays;
use crate::model::RatingId;
use crate::model::StateId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rating {
    Again,
    Hard,
    Good,
    Easy,
}

impl Rating {
    pub const ALL: [Self; 4] = [Self::Again, Self::Hard, Self::Good, Self::Easy];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    New,
    Learning,
    Review,
    Relearning,
}

impl State {
    pub const ALL: [Self; 4] = [Self::New, Self::Learning, Self::Review, Self::Relearning];
}

pub fn number_to_rating(num: RatingId) -> Option<Rating> {
    match num {
        1 => Some(Rating::Again),
        2 => Some(Rating::Hard),
        3 => Some(Rating::Good),
        4 => Some(Rating::Easy),
        _ => None,
    }
}

pub fn rating_to_number(rating: Rating) -> RatingId {
    match rating {
        Rating::Again => 1,
        Rating::Hard => 2,
        Rating::Good => 3,
        Rating::Easy => 4,
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

/// The FSRS model with the configured parameters, or the defaults if none are configured.
pub fn model(config: &SparesExternalConfig) -> Result<FSRS, Error> {
    FSRS::new(&config.fsrs_parameters).map_err(|e| {
        Error::Library(LibraryError::Scheduler(SchedulerErrorKind::InvalidInput(
            format!("invalid `fsrs_parameters`: {e:?}"),
        )))
    })
}

/// The forgetting curve's decay under the configured parameters, clipped like the model clips
/// it.
pub fn decay(config: &SparesExternalConfig) -> f32 {
    fsrs_rs::check_and_fill_parameters(&config.fsrs_parameters)
        .map_or(DEFAULT_PARAMETERS[20], |parameters| parameters[20])
        .clamp(0.1, 0.8)
}

/// The probability of recalling a card with `stability` after `elapsed_days`, on the forgetting
/// curve with `decay`.
pub fn retrievability(elapsed_days: f64, stability: f64, decay: f32) -> f64 {
    let memory_state = MemoryState {
        stability: stability as f32,
        // Not used by the forgetting curve.
        difficulty: 1.0,
    };
    f64::from(fsrs_rs::current_retrievability(
        memory_state,
        elapsed_days as f32,
        decay,
    ))
}

/// The days after which a card with `stability` falls to `retention` on the forgetting curve with
/// `decay`: the inverse of [`retrievability`]. Like the model's intervals, but not rounded or
/// clamped to a day, so it also fits steps of minutes.
pub fn days_until_retrievability(stability: f64, retention: f64, decay: f32) -> f64 {
    let decay = f64::from(decay);
    let factor = 0.9f64.powf(-1.0 / decay) - 1.0;
    stability / factor * (retention.powf(-1.0 / decay) - 1.0)
}

/// The range of intervals around `interval` that FSRS considers interchangeable, as in Anki and
/// `fsrs4anki-helper`.
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
    fn days_until_retrievability_inverts_the_forgetting_curve() {
        let decay = decay(&SparesExternalConfig::default());
        for (stability, retention) in [(0.002, 0.9), (0.01, 0.85), (3.0, 0.95), (40.0, 0.7)] {
            let days = days_until_retrievability(stability, retention, decay);
            assert!((retrievability(days, stability, decay) - retention).abs() < 1e-4);
            // The same as the model's interval, before it is rounded
            let model = f64::from(FSRS::default().next_interval(
                Some(stability as f32),
                retention as f32,
                1,
            ));
            assert!(
                (days - model).abs() < 1e-4 * days.max(1.0),
                "{days} {model}"
            );
        }
    }

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
