//! The card state machine around FSRS's memory model.
//!
//! The `fsrs` crate computes memory states and day-sized intervals, but leaves learning steps to
//! the application. These follow Anki:
//!
//! | In                                | Again        | Hard                 | Good                      | Easy      |
//! |-----------------------------------|--------------|----------------------|---------------------------|-----------|
//! | New, Learning, Relearning (steps) | first step   | repeat current step* | next step, else graduate  | graduate  |
//! | Review                            | relearning** | review               | review                    | review    |
//!
//! \* On the first step, Hard waits halfway to the second step, or 1.5 times the only step.
//! \*\* Or stays in review with FSRS's interval if there are no relearning steps.
//!
//! With no steps configured, a card graduates on its first answer. Steps are not part of the
//! model: FSRS accounts for whatever same-day reviews happen.

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

#[derive(Clone, Copy, Debug)]
pub struct LearningSteps<'a> {
    pub learning: &'a [Duration],
    pub relearning: &'a [Duration],
}

#[derive(Debug, PartialEq)]
pub struct Step {
    pub state: State,
    pub memory: MemoryState,
    pub interval: Duration,
}

/// The step a card in learning is on, given the delay it was last given.
///
/// The step is not stored, but every delay a card in learning is given is at least its current
/// step and less than the next one, so it can be read back. A delay that matches no step, such as
/// from before the steps were changed, counts as the closest step below it.
fn current_step(steps: &[Duration], previous_delay: Option<Duration>) -> usize {
    previous_delay
        .and_then(|delay| steps.iter().rposition(|step| *step <= delay))
        .unwrap_or(0)
}

/// How long Hard waits on the first step.
fn first_step_hard_delay(steps: &[Duration]) -> Duration {
    match steps {
        [first, second, ..] => (*first + *second) / 2,
        [only] => (*only * 3 / 2).min(*only + Duration::days(1)),
        [] => unreachable!("only called with steps"),
    }
}

/// The state, memory and interval of a card in `state` with `memory` after being rated `rating`,
/// `elapsed_days` after its previous review, which gave it `previous_delay`.
///
/// `memory` must be `None` exactly when `state` is [`State::New`].
#[allow(clippy::too_many_arguments)]
pub fn next_step(
    model: &FSRS,
    steps: LearningSteps,
    state: State,
    memory: Option<MemoryState>,
    desired_retention: f32,
    elapsed_days: u32,
    previous_delay: Option<Duration>,
    rating: Rating,
) -> Result<Step, FSRSError> {
    let NextStates {
        again,
        hard,
        good,
        easy,
    } = model.next_states(memory, desired_retention, elapsed_days)?;
    let days =
        |interval: f32| Duration::days(interval.round().clamp(1.0, MAXIMUM_INTERVAL_DAYS) as i64);
    let graduated = |rating: Rating| {
        let interval = match rating {
            Rating::Again => days(again.interval),
            Rating::Hard => days(hard.interval).min(days(good.interval)),
            Rating::Good => days(good.interval),
            Rating::Easy => days(easy.interval).max(days(good.interval) + Duration::days(1)),
        };
        (State::Review, interval)
    };
    let in_steps = |steps: &[Duration], current: usize, learning_state: State| {
        if steps.is_empty() {
            return graduated(rating);
        }
        match rating {
            Rating::Again => (learning_state, steps[0]),
            Rating::Hard if current == 0 => (learning_state, first_step_hard_delay(steps)),
            Rating::Hard => (learning_state, steps[current]),
            Rating::Good => steps
                .get(current + 1)
                .map_or_else(|| graduated(Rating::Good), |next| (learning_state, *next)),
            Rating::Easy => graduated(Rating::Easy),
        }
    };
    let (state, interval) = match state {
        State::New => in_steps(steps.learning, 0, State::Learning),
        State::Learning => in_steps(
            steps.learning,
            current_step(steps.learning, previous_delay),
            State::Learning,
        ),
        State::Relearning => in_steps(
            steps.relearning,
            current_step(steps.relearning, previous_delay),
            State::Relearning,
        ),
        State::Review => match (rating, steps.relearning.first()) {
            (Rating::Again, Some(first)) => (State::Relearning, *first),
            (Rating::Again, None) => graduated(Rating::Again),
            _ => {
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
        },
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

    const LEARNING: [Duration; 2] = [Duration::minutes(1), Duration::minutes(10)];
    const RELEARNING: [Duration; 1] = [Duration::minutes(10)];
    const STEPS: LearningSteps = LearningSteps {
        learning: &LEARNING,
        relearning: &RELEARNING,
    };
    const NO_STEPS: LearningSteps = LearningSteps {
        learning: &[],
        relearning: &[],
    };

    fn step(
        steps: LearningSteps,
        state: State,
        memory: Option<MemoryState>,
        previous_delay: Option<Duration>,
        rating: Rating,
    ) -> Step {
        let elapsed_days = if state == State::Review { 10 } else { 0 };
        next_step(
            &FSRS::default(),
            steps,
            state,
            memory,
            0.9,
            elapsed_days,
            previous_delay,
            rating,
        )
        .unwrap()
    }

    fn reviewed() -> MemoryState {
        MemoryState {
            stability: 10.0,
            difficulty: 5.0,
        }
    }

    fn learning() -> MemoryState {
        step(STEPS, State::New, None, None, Rating::Good).memory
    }

    #[test]
    fn new_cards_start_on_the_learning_steps() {
        let delays = [Rating::Again, Rating::Hard, Rating::Good]
            .map(|rating| step(STEPS, State::New, None, None, rating))
            .map(|step| {
                assert_eq!(step.state, State::Learning);
                step.interval
            });
        assert_eq!(
            delays,
            [
                Duration::minutes(1),
                Duration::seconds(330),
                Duration::minutes(10)
            ]
        );
        let easy = step(STEPS, State::New, None, None, Rating::Easy);
        assert_eq!(easy.state, State::Review);
        assert!(easy.interval >= Duration::days(1));
    }

    #[test]
    fn learning_walks_the_steps_then_graduates() {
        let memory = Some(learning());
        let on_second_step = Some(LEARNING[1]);
        assert_eq!(
            step(
                STEPS,
                State::Learning,
                memory,
                on_second_step,
                Rating::Again
            )
            .interval,
            LEARNING[0],
            "Again goes back to the first step"
        );
        assert_eq!(
            step(STEPS, State::Learning, memory, on_second_step, Rating::Hard).interval,
            LEARNING[1],
            "Hard repeats the current step"
        );
        let on_first_step = Some(LEARNING[0]);
        let good = step(STEPS, State::Learning, memory, on_first_step, Rating::Good);
        assert_eq!((good.state, good.interval), (State::Learning, LEARNING[1]));
        let good = step(STEPS, State::Learning, memory, on_second_step, Rating::Good);
        assert_eq!(good.state, State::Review, "Good on the last step graduates");
        let easy = step(STEPS, State::Learning, memory, on_first_step, Rating::Easy);
        assert_eq!(easy.state, State::Review);
        assert!(easy.interval > good.interval);
    }

    #[test]
    fn hard_on_the_first_step_reads_back_as_the_first_step() {
        let memory = Some(learning());
        let hard_delay = step(STEPS, State::New, None, None, Rating::Hard).interval;
        let good = step(
            STEPS,
            State::Learning,
            memory,
            Some(hard_delay),
            Rating::Good,
        );
        assert_eq!(
            (good.state, good.interval),
            (State::Learning, LEARNING[1]),
            "the next step after the first, not graduation"
        );
    }

    #[test]
    fn a_single_step_hard_delay_is_one_and_a_half_steps() {
        assert_eq!(
            first_step_hard_delay(&[Duration::minutes(10)]),
            Duration::minutes(15)
        );
        assert_eq!(
            first_step_hard_delay(&[Duration::hours(20)]),
            Duration::hours(30)
        );
    }

    #[test]
    fn unknown_delays_fall_back_to_the_closest_step_below() {
        assert_eq!(current_step(&LEARNING, None), 0);
        assert_eq!(current_step(&LEARNING, Some(Duration::zero())), 0);
        assert_eq!(current_step(&LEARNING, Some(Duration::minutes(7))), 0);
        assert_eq!(current_step(&LEARNING, Some(Duration::hours(3))), 1);
    }

    #[test]
    fn lapses_go_through_the_relearning_steps() {
        let lapse = step(STEPS, State::Review, Some(reviewed()), None, Rating::Again);
        assert_eq!(
            (lapse.state, lapse.interval),
            (State::Relearning, RELEARNING[0])
        );
        assert!(lapse.memory.stability < 10.0);
        let relearned = step(
            STEPS,
            State::Relearning,
            Some(lapse.memory),
            Some(RELEARNING[0]),
            Rating::Good,
        );
        assert_eq!(relearned.state, State::Review);
    }

    #[test]
    fn without_steps_every_answer_graduates() {
        for rating in Rating::ALL {
            let first = step(NO_STEPS, State::New, None, None, rating);
            assert_eq!(first.state, State::Review, "{rating:?} on a new card");
            assert!(first.interval >= Duration::days(1));
        }
        let lapse = step(
            NO_STEPS,
            State::Review,
            Some(reviewed()),
            None,
            Rating::Again,
        );
        assert_eq!(lapse.state, State::Review);
        assert!(lapse.interval >= Duration::days(1));
    }

    #[test]
    fn better_ratings_give_strictly_longer_review_intervals() {
        for elapsed_days in [0, 1, 5, 10, 30, 365] {
            let intervals = [Rating::Hard, Rating::Good, Rating::Easy].map(|rating| {
                next_step(
                    &FSRS::default(),
                    STEPS,
                    State::Review,
                    Some(reviewed()),
                    0.9,
                    elapsed_days,
                    None,
                    rating,
                )
                .unwrap()
                .interval
            });
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
                STEPS,
                State::Review,
                Some(reviewed()),
                desired_retention,
                10,
                None,
                Rating::Good,
            )
            .unwrap()
            .interval
        };
        assert!(interval(0.8) > interval(0.9));
        assert!(interval(0.9) > interval(0.97));
    }
}
