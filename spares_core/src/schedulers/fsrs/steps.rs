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
//!
//! FSRS-7 models memory within a day too, so its intervals can be shorter than a day. One under
//! 12 hours is given as it is, to the second, when same-day reviews are allowed (see
//! [`SameDay`]): the card then waits in learning, or relearning if it had been in review, since
//! only those states are due at a time of day rather than on a day. Its next answer is scheduled
//! as for a card in review, except that Again starts its steps again. Longer intervals, and every
//! interval of FSRS-6, are rounded to whole days, at least one.

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
    /// Whether `interval` is one FSRS gave within the day, rather than a step or whole days.
    pub same_day: bool,
}

/// Whether FSRS may bring a card back on the day it is reviewed, outside its steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SameDay {
    /// The shortest delay it may then give, or `None` if it may not, as under FSRS-6 or once the
    /// card has had as many reviews that day as allowed.
    pub minimum: Option<Duration>,
    /// Whether the card's previous review gave it such a delay, so that it is past its steps.
    pub previous: bool,
}

#[cfg(test)]
impl SameDay {
    /// Every interval in whole days.
    pub const NEVER: Self = Self {
        minimum: None,
        previous: false,
    };
}

/// The longest interval given as it is, rather than rounded to whole days.
const SAME_DAY_LIMIT: Duration = Duration::hours(12);

/// An interval FSRS gave, as the card gets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interval {
    /// Within the day, to the second.
    SameDay(Duration),
    /// A whole number of days, at least one.
    Days(i64),
}

impl Interval {
    fn duration(self) -> Duration {
        match self {
            Self::SameDay(delay) => delay,
            Self::Days(days) => Duration::days(days),
        }
    }

    fn whole_days(self) -> i64 {
        match self {
            Self::SameDay(_) => 0,
            Self::Days(days) => days,
        }
    }

    /// The shorter of `self` and `other`.
    fn at_most(self, other: Self) -> Self {
        if self.duration() <= other.duration() {
            self
        } else {
            other
        }
    }

    /// `self`, made longer than `shorter` if it is not: by a day in days, by a minute within one.
    fn after(self, shorter: Self) -> Self {
        match (self, shorter) {
            (Self::Days(days), _) => Self::Days(days.max(shorter.whole_days() + 1)),
            (Self::SameDay(delay), Self::SameDay(shorter)) => {
                Self::SameDay(delay.max(shorter + Duration::minutes(1)))
            }
            (Self::SameDay(_), Self::Days(days)) => Self::Days(days + 1),
        }
    }
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
    elapsed_days: f32,
    previous_delay: Option<Duration>,
    same_day: SameDay,
    rating: Rating,
) -> Result<Step, FSRSError> {
    let NextStates {
        again,
        hard,
        good,
        easy,
    } = model.next_states_with_elapsed_days(memory, desired_retention, elapsed_days)?;
    let days = |interval: f32| interval.round().clamp(1.0, MAXIMUM_INTERVAL_DAYS) as i64;
    let interval = |interval: f32| {
        if let Some(minimum) = same_day.minimum {
            #[expect(clippy::cast_possible_truncation, reason = "within half a day")]
            let exact = Duration::milliseconds(
                (f64::from(interval) * 86_400_000.0).clamp(0.0, 86_400_000.0) as i64,
            )
            .max(minimum);
            if exact < SAME_DAY_LIMIT {
                return Interval::SameDay(Duration::seconds(exact.num_seconds().max(1)));
            }
        }
        Interval::Days(days(interval))
    };
    // A delay within the day keeps the card in `steps_state`, as only steps are due at a time
    // of day; whole days put it in review.
    let given = |interval: Interval, steps_state: State| match interval {
        Interval::SameDay(delay) => (steps_state, delay, true),
        Interval::Days(days) => (State::Review, Duration::days(days), false),
    };
    let graduated = |rating: Rating, steps_state: State| {
        let interval = match rating {
            Rating::Again => interval(again.interval),
            Rating::Hard => interval(hard.interval).at_most(interval(good.interval)),
            Rating::Good => interval(good.interval),
            Rating::Easy => interval(easy.interval).after(interval(good.interval)),
        };
        given(interval, steps_state)
    };
    // As for a card in review, past its steps, which Again takes it back to.
    let reviewed =
        |again_steps: &[Duration], steps_state: State| match (rating, again_steps.first()) {
            (Rating::Again, Some(first)) => (steps_state, *first, false),
            (Rating::Again, None) => given(interval(again.interval), steps_state),
            _ => {
                // Better ratings never give shorter intervals.
                let hard_interval = interval(hard.interval).at_most(interval(good.interval));
                let good_interval = interval(good.interval).after(hard_interval);
                let easy_interval = interval(easy.interval).after(good_interval);
                let interval = match rating {
                    Rating::Hard => hard_interval,
                    Rating::Good => good_interval,
                    _ => easy_interval,
                };
                given(interval, steps_state)
            }
        };
    let in_steps = |steps: &[Duration], current: usize, learning_state: State| {
        if steps.is_empty() {
            return graduated(rating, learning_state);
        }
        match rating {
            Rating::Again => (learning_state, steps[0], false),
            Rating::Hard if current == 0 => (learning_state, first_step_hard_delay(steps), false),
            Rating::Hard => (learning_state, steps[current], false),
            Rating::Good => steps.get(current + 1).map_or_else(
                || graduated(Rating::Good, learning_state),
                |next| (learning_state, *next, false),
            ),
            Rating::Easy => graduated(Rating::Easy, learning_state),
        }
    };
    let (state, delay, given_same_day) = match state {
        State::New => in_steps(steps.learning, 0, State::Learning),
        State::Learning if same_day.previous => reviewed(steps.learning, State::Learning),
        State::Learning => in_steps(
            steps.learning,
            current_step(steps.learning, previous_delay),
            State::Learning,
        ),
        State::Relearning if same_day.previous => reviewed(steps.relearning, State::Relearning),
        State::Relearning => in_steps(
            steps.relearning,
            current_step(steps.relearning, previous_delay),
            State::Relearning,
        ),
        State::Review => reviewed(steps.relearning, State::Relearning),
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
        interval: delay,
        same_day: given_same_day,
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
        let elapsed_days = if state == State::Review { 10.0 } else { 0.0 };
        next_step(
            &FSRS::new(&fsrs_rs::DEFAULT_PARAMETERS).unwrap(),
            steps,
            state,
            memory,
            0.9,
            elapsed_days,
            previous_delay,
            SameDay::NEVER,
            rating,
        )
        .unwrap()
    }

    fn reviewed() -> MemoryState {
        MemoryState {
            stability: 10.0,
            difficulty: 5.0,
            stability_fast: 10.0,
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
        for elapsed_days in [0.0, 0.3, 1.0, 5.0, 10.0, 30.0, 365.0] {
            let intervals = [Rating::Hard, Rating::Good, Rating::Easy].map(|rating| {
                next_step(
                    &FSRS::new(&fsrs_rs::DEFAULT_PARAMETERS).unwrap(),
                    STEPS,
                    State::Review,
                    Some(reviewed()),
                    0.9,
                    elapsed_days,
                    None,
                    SameDay::NEVER,
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
                &FSRS::new(&fsrs_rs::DEFAULT_PARAMETERS).unwrap(),
                STEPS,
                State::Review,
                Some(reviewed()),
                desired_retention,
                10.0,
                None,
                SameDay::NEVER,
                Rating::Good,
            )
            .unwrap()
            .interval
        };
        assert!(interval(0.8) > interval(0.9));
        assert!(interval(0.9) > interval(0.97));
    }

    /// A weak memory, whose intervals FSRS-7 gives within the day.
    fn weak() -> MemoryState {
        MemoryState {
            stability: 0.05,
            difficulty: 8.0,
            stability_fast: 0.02,
        }
    }

    fn same_day_step(
        steps: LearningSteps,
        state: State,
        memory: MemoryState,
        same_day: SameDay,
        rating: Rating,
    ) -> Step {
        next_step(
            &FSRS::new(&fsrs_rs::DEFAULT_PARAMETERS).unwrap(),
            steps,
            state,
            Some(memory),
            0.9,
            0.1,
            None,
            same_day,
            rating,
        )
        .unwrap()
    }

    const ALLOWED: SameDay = SameDay {
        minimum: Some(Duration::minutes(1)),
        previous: false,
    };

    #[test]
    fn fsrs7_brings_a_weak_card_back_within_the_day() {
        let lapse = same_day_step(NO_STEPS, State::Review, weak(), ALLOWED, Rating::Again);
        assert!(lapse.same_day, "{lapse:?}");
        assert_eq!(
            lapse.state,
            State::Relearning,
            "only steps are due at a time of day"
        );
        assert!(lapse.interval >= Duration::minutes(1) && lapse.interval < SAME_DAY_LIMIT);
        // The same answer in whole days, when same-day reviews are not allowed
        let rounded = same_day_step(
            NO_STEPS,
            State::Review,
            weak(),
            SameDay::NEVER,
            Rating::Again,
        );
        assert_eq!((rounded.state, rounded.same_day), (State::Review, false));
        assert_eq!(rounded.interval, Duration::days(1));
        // A strong memory still gets days
        let strong = same_day_step(NO_STEPS, State::Review, reviewed(), ALLOWED, Rating::Good);
        assert_eq!((strong.state, strong.same_day), (State::Review, false));
    }

    #[test]
    fn the_minimum_interval_is_the_least_a_card_waits() {
        let minimum = |minimum| SameDay {
            minimum: Some(minimum),
            previous: false,
        };
        let lapse = same_day_step(
            NO_STEPS,
            State::Review,
            weak(),
            minimum(Duration::hours(3)),
            Rating::Again,
        );
        assert!(
            lapse.same_day && lapse.interval >= Duration::hours(3),
            "{lapse:?}"
        );
        // A minimum of half a day or more keeps intervals to whole days.
        let lapse = same_day_step(
            NO_STEPS,
            State::Review,
            weak(),
            minimum(Duration::hours(12)),
            Rating::Again,
        );
        assert_eq!((lapse.state, lapse.same_day), (State::Review, false));
        assert_eq!(lapse.interval, Duration::days(1));
    }

    #[test]
    fn steps_come_before_same_day_intervals() {
        let lapse = same_day_step(STEPS, State::Review, weak(), ALLOWED, Rating::Again);
        assert_eq!(
            (lapse.state, lapse.interval, lapse.same_day),
            (State::Relearning, RELEARNING[0], false)
        );
        let first = same_day_step(STEPS, State::Learning, weak(), ALLOWED, Rating::Again);
        assert_eq!((first.interval, first.same_day), (LEARNING[0], false));
    }

    /// A card FSRS-7 brought back within the day is past its steps: it is answered as in review,
    /// except that Again starts its steps again.
    #[test]
    fn a_card_brought_back_within_the_day_is_past_its_steps() {
        let previous = SameDay {
            previous: true,
            ..ALLOWED
        };
        for state in [State::Learning, State::Relearning] {
            let good = same_day_step(STEPS, state, weak(), previous, Rating::Good);
            assert!(good.same_day || good.state == State::Review, "{good:?}");
            assert_ne!(good.interval, LEARNING[1]);
            let again = same_day_step(STEPS, state, weak(), previous, Rating::Again);
            let first_step = if state == State::Learning {
                LEARNING[0]
            } else {
                RELEARNING[0]
            };
            assert_eq!((again.state, again.interval), (state, first_step));
        }
    }

    #[test]
    fn better_ratings_give_longer_intervals_within_the_day_too() {
        for memory in [weak(), learning(), reviewed()] {
            let [hard, good, easy] = [Rating::Hard, Rating::Good, Rating::Easy]
                .map(|rating| same_day_step(NO_STEPS, State::Review, memory, ALLOWED, rating));
            assert!(
                hard.interval < good.interval && good.interval < easy.interval,
                "{hard:?} {good:?} {easy:?}"
            );
        }
    }
}
