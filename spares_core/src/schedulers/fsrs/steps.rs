//! The card state machine around FSRS's memory model.
//!
//! The `fsrs` crate computes memory states and day-sized intervals, but leaves learning steps to
//! the application, as Anki does. These are the steps `rs-fsrs`, which spares used before, had
//! built in:
//!
//! | From                  | Again             | Hard              | Good              | Easy   |
//! |-----------------------|-------------------|-------------------|-------------------|--------|
//! | New                   | Learning, 1 min   | Learning, 5 min   | Learning, 10 min  | Review |
//! | Learning, Relearning  | unchanged, 5 min  | unchanged, 10 min | Review            | Review |
//! | Review                | Relearning, 5 min | Review            | Review            | Review |

use chrono::Duration;
use fsrs_rs::FSRS;
use fsrs_rs::FSRSError;
use fsrs_rs::MemoryState;
use fsrs_rs::NextStates;

use super::utils::Rating;
use super::utils::State;

/// The longest interval FSRS may propose, so that a due date always fits in a timestamp. The
/// configured maximum interval is applied later, when the due date is placed.
const MAXIMUM_INTERVAL_DAYS: f32 = 36_500.0;

#[derive(Debug, PartialEq)]
pub struct Step {
    pub state: State,
    pub memory: MemoryState,
    pub interval: Duration,
}

/// The state, memory and interval of a card in `state` with `memory` after being rated `rating`,
/// `elapsed_days` after its previous review.
///
/// `memory` must be `None` exactly when `state` is [`State::New`].
pub fn next_step(
    model: &FSRS,
    state: State,
    memory: Option<MemoryState>,
    desired_retention: f32,
    elapsed_days: u32,
    rating: Rating,
) -> Result<Step, FSRSError> {
    let next_states = model.next_states(memory, desired_retention, elapsed_days)?;
    let NextStates {
        again,
        hard,
        good,
        easy,
    } = next_states;
    let days =
        |interval: f32| Duration::days(interval.round().clamp(1.0, MAXIMUM_INTERVAL_DAYS) as i64);
    let (state, interval) = match (state, rating) {
        (State::New, Rating::Again) => (State::Learning, Duration::minutes(1)),
        (State::New, Rating::Hard) => (State::Learning, Duration::minutes(5)),
        (State::New, Rating::Good) => (State::Learning, Duration::minutes(10)),
        (State::New, Rating::Easy) => (State::Review, days(easy.interval)),

        (State::Learning | State::Relearning, Rating::Again) => (state, Duration::minutes(5)),
        (State::Learning | State::Relearning, Rating::Hard) => (state, Duration::minutes(10)),
        (State::Learning | State::Relearning, Rating::Good) => (State::Review, days(good.interval)),
        (State::Learning | State::Relearning, Rating::Easy) => (
            State::Review,
            days(easy.interval).max(days(good.interval) + Duration::days(1)),
        ),

        (State::Review, Rating::Again) => (State::Relearning, Duration::minutes(5)),
        (State::Review, _) => {
            // Better ratings never give shorter intervals.
            let hard_interval = days(hard.interval).min(days(good.interval));
            let good_interval = days(good.interval).max(hard_interval + Duration::days(1));
            let easy_interval = days(easy.interval).max(good_interval + Duration::days(1));
            let interval = match rating {
                Rating::Hard => hard_interval,
                Rating::Good => good_interval,
                _ => easy_interval,
            };
            (State::Review, interval)
        }
    };
    let memory = match rating {
        Rating::Again => again.memory,
        Rating::Hard => hard.memory,
        Rating::Good => good.memory,
        Rating::Easy => easy.memory,
    };
    Ok(Step {
        state,
        memory,
        interval,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(state: State, memory: Option<MemoryState>, elapsed_days: u32, rating: Rating) -> Step {
        next_step(&FSRS::default(), state, memory, 0.9, elapsed_days, rating).unwrap()
    }

    fn reviewed() -> MemoryState {
        MemoryState {
            stability: 10.0,
            difficulty: 5.0,
        }
    }

    #[test]
    fn new_cards_enter_learning_steps_unless_easy() {
        for (rating, minutes) in [(Rating::Again, 1), (Rating::Hard, 5), (Rating::Good, 10)] {
            let step = step(State::New, None, 0, rating);
            assert_eq!(step.state, State::Learning);
            assert_eq!(step.interval, Duration::minutes(minutes));
        }
        let easy = step(State::New, None, 0, Rating::Easy);
        assert_eq!(easy.state, State::Review);
        assert!(easy.interval >= Duration::days(1));
    }

    #[test]
    fn learning_cards_graduate_on_good_or_easy() {
        let learning = step(State::New, None, 0, Rating::Good).memory;
        for state in [State::Learning, State::Relearning] {
            assert_eq!(step(state, Some(learning), 0, Rating::Again).state, state);
            assert_eq!(
                step(state, Some(learning), 0, Rating::Hard).interval,
                Duration::minutes(10)
            );
            let good = step(state, Some(learning), 0, Rating::Good);
            let easy = step(state, Some(learning), 0, Rating::Easy);
            assert_eq!((good.state, easy.state), (State::Review, State::Review));
            assert!(easy.interval > good.interval);
        }
    }

    #[test]
    fn lapses_enter_relearning() {
        let lapse = step(State::Review, Some(reviewed()), 10, Rating::Again);
        assert_eq!(lapse.state, State::Relearning);
        assert_eq!(lapse.interval, Duration::minutes(5));
        assert!(lapse.memory.stability < 10.0);
    }

    #[test]
    fn better_ratings_give_strictly_longer_review_intervals() {
        for elapsed_days in [0, 1, 5, 10, 30, 365] {
            let intervals = [Rating::Hard, Rating::Good, Rating::Easy]
                .map(|rating| step(State::Review, Some(reviewed()), elapsed_days, rating).interval);
            assert!(
                intervals[0] < intervals[1] && intervals[1] < intervals[2],
                "{intervals:?} after {elapsed_days} days"
            );
        }
    }

    #[test]
    fn intervals_follow_desired_retention() {
        let interval = |desired_retention| {
            next_step(
                &FSRS::default(),
                State::Review,
                Some(reviewed()),
                desired_retention,
                10,
                Rating::Good,
            )
            .unwrap()
            .interval
        };
        assert!(interval(0.8) > interval(0.9));
        assert!(interval(0.9) > interval(0.97));
    }
}
