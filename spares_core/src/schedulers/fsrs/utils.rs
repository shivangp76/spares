use std::cmp;

use chrono::Duration;
use fsrs_rs::FSRS;
use fsrs_rs::FSRS6_DEFAULT_PARAMETERS;
use fsrs_rs::MemoryState;
use fsrs_rs::ModelVersion;

use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::config::SparesExternalConfig;
use crate::helpers::FractionalDays;
use crate::model::Card;
use crate::model::RatingId;
use crate::model::StateId;
use crate::schedulers::CardMemory;

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

/// The configured FSRS parameters, or FSRS-7's defaults if none are configured.
pub fn parameters(config: &SparesExternalConfig) -> &[f32] {
    if config.fsrs_parameters.is_empty() {
        &fsrs_rs::DEFAULT_PARAMETERS
    } else {
        &config.fsrs_parameters
    }
}

/// The FSRS model with the configured parameters, or the defaults if none are configured.
pub fn model(config: &SparesExternalConfig) -> Result<FSRS, Error> {
    FSRS::new(parameters(config)).map_err(|e| {
        Error::Library(LibraryError::Scheduler(SchedulerErrorKind::InvalidInput(
            format!("invalid `fsrs_parameters`: {e:?}"),
        )))
    })
}

/// The memory state FSRS remembers for `card`, which must not be new.
pub fn memory_state(card: &Card) -> MemoryState {
    fsrs_memory_state(CardMemory::from(card))
}

/// The fsrs crate's `S_MIN` and `D_MIN`..`D_MAX`, which its forgetting curves clamp a memory state
/// to.
const STABILITY_MIN: f64 = 0.0001;
const DIFFICULTY_RANGE: (f64, f64) = (1.0, 10.0);

/// The forgetting curve under the configured parameters, or the defaults' if they are invalid
/// (which validating the settings rules out).
#[derive(Clone, Debug)]
pub enum Curve {
    /// FSRS-4.5 to FSRS-6: one power curve for every card, stretched by its stability.
    Power { decay: f32 },
    /// FSRS-7: a fast and a slow curve, weighted by the card's two stabilities and difficulty.
    /// `parameters` are clipped as the model clips the ones its curve uses.
    Fsrs7 { model: FSRS, parameters: Vec<f32> },
}

impl Curve {
    pub fn new(config: &SparesExternalConfig) -> Self {
        let parameters = fsrs_rs::check_and_fill_parameters(parameters(config))
            .unwrap_or_else(|_| fsrs_rs::DEFAULT_PARAMETERS.to_vec());
        Self::from_parameters(parameters)
    }

    /// The curve steps stats fit one stability to (see the `steps_stats` module): the configured
    /// power curve, or FSRS-6's default one under FSRS-7. FSRS-7's curve for a single stability is
    /// no measure of a step: recall falls to about a quarter in that time, and to 90% almost at
    /// once.
    pub fn for_steps(config: &SparesExternalConfig) -> Self {
        match Self::new(config) {
            Self::Fsrs7 { .. } => Self::Power {
                decay: FSRS6_DEFAULT_PARAMETERS[20],
            },
            power @ Self::Power { .. } => power,
        }
    }

    /// The curve of `parameters`, as filled in by `check_and_fill_parameters`.
    fn from_parameters(mut parameters: Vec<f32>) -> Self {
        match FSRS::new(&parameters) {
            Ok(model) if model.version() == ModelVersion::Fsrs7 => {
                clip_fsrs7_curve_parameters(&mut parameters);
                Self::Fsrs7 { model, parameters }
            }
            _ => Self::Power {
                decay: parameters
                    .get(20)
                    .copied()
                    .unwrap_or(FSRS6_DEFAULT_PARAMETERS[20])
                    .clamp(0.1, 0.8),
            },
        }
    }

    /// The probability of recalling a card with `memory` after `elapsed_days`.
    pub fn retrievability(&self, memory: CardMemory, elapsed_days: f64) -> f64 {
        let state = fsrs_memory_state(memory);
        let elapsed_days = elapsed_days.max(0.0) as f32;
        f64::from(match self {
            // Not the model's, which rounds the days to whole ones.
            Self::Power { decay } => fsrs_rs::current_retrievability(state, elapsed_days, *decay),
            Self::Fsrs7 { model, .. } => model.current_retrievability(state, elapsed_days),
        })
    }

    /// The days after which a card with `memory` falls to `retention`: the inverse of
    /// [`Self::retrievability`]. Like the model's intervals, but not rounded or clamped to a day,
    /// so it also fits steps of minutes.
    pub fn days_until(&self, memory: CardMemory, retention: f64) -> f64 {
        match self {
            Self::Power { decay } => {
                let decay = f64::from(*decay);
                let factor = 0.9f64.powf(-1.0 / decay) - 1.0;
                memory.stability.max(STABILITY_MIN) / factor * (retention.powf(-1.0 / decay) - 1.0)
            }
            Self::Fsrs7 { model, .. } => f64::from(
                model.interval_at_retrievability(fsrs_memory_state(memory), retention as f32),
            ),
        }
    }

    /// [`Self::retrievability`] of the card `c` as SQL, `elapsed` being SQL for the seconds since
    /// its last review. SQLite's math functions (`pow`, `exp`, `ln`) compute the curve.
    pub fn sql(&self, elapsed: &str) -> String {
        let number = |value: f32| format!("{:?}", f64::from(value));
        let days = format!("(MAX({elapsed}, 0) / 86400.0)");
        let stability = format!("MAX(c.stability, {STABILITY_MIN:?})");
        match self {
            Self::Power { decay } => {
                let factor = 0.9f64.powf(-1.0 / f64::from(*decay)) - 1.0;
                format!(
                    "pow(1.0 + {factor:?} * {days} / {stability}, {})",
                    number(-decay)
                )
            }
            Self::Fsrs7 { parameters: w, .. } => {
                let (difficulty_min, difficulty_max) = DIFFICULTY_RANGE;
                // As in the crate's `fsrs7_forgetting_curve_scalar_for_state`
                let decay2 = f64::from(w[24].clamp(0.01, 0.95));
                let factor2 = f64::from(w[26]).powf(-1.0 / decay2) - 1.0;
                let memory = format!(
                    "SELECT {stability} AS s, \
                     MAX(c.stability_fast, {STABILITY_MIN:?}) AS sf, \
                     MIN(MAX(c.difficulty, {difficulty_min:?}), {difficulty_max:?}) AS d, \
                     {days} AS t"
                );
                let with_decay = format!(
                    "SELECT m.*, MIN(MAX({w23} * pow(m.sf, {w33} - 0.3), 0.01), 0.95) AS decay1 \
                     FROM ({memory}) m",
                    w23 = number(w[23]),
                    w33 = number(w[33]),
                );
                let terms = format!(
                    "SELECT {w27} * pow(m.sf, -{w29}) AS weight1, \
                     {w28} * pow(m.s, {w30}) * exp((m.d - 5.0) * ({w31} - 0.5)) AS weight2, \
                     pow(1.0 + (exp(MIN(ln({w25}) / -m.decay1, 60.0)) - 1.0) * m.t / m.sf, \
                         -m.decay1) AS r1, \
                     pow(1.0 + {factor2:?} * exp((m.d - 5.0) * ({w32} - 0.3)) * m.t / m.s, \
                         {neg_decay2:?}) AS r2 \
                     FROM ({with_decay}) m",
                    w25 = number(w[25]),
                    w27 = number(w[27]),
                    w28 = number(w[28]),
                    w29 = number(w[29]),
                    w30 = number(w[30]),
                    w31 = number(w[31]),
                    w32 = number(w[32]),
                    neg_decay2 = -decay2,
                );
                format!(
                    "(SELECT (k.weight1 * k.r1 + k.weight2 * k.r2) / (k.weight1 + k.weight2) \
                     * (1.0 - 2e-5) + 1e-5 FROM ({terms}) k)"
                )
            }
        }
    }
}

/// The clipping of the fsrs crate's `clip_fsrs7_parameters` for the parameters FSRS-7's
/// forgetting curve uses, 23 to 33, which the crate keeps to itself.
fn clip_fsrs7_curve_parameters(w: &mut [f32]) {
    let clamp = |value: f32, low: f32, high: f32| {
        if value.is_finite() {
            value.clamp(low, high)
        } else {
            low
        }
    };
    w[23] = clamp(w[23], 0.01, 0.25);
    w[24] = clamp(w[24], 0.01, 0.95);
    w[25] = clamp(w[25], 0.2, 0.85);
    w[26] = clamp(w[26], w[25].max(0.5), 0.99);
    w[27] = clamp(w[27], 0.01, 1.0);
    w[28] = clamp(w[28], 0.1, 1.0);
    w[29] = clamp(w[29], 0.0, 0.9);
    w[30] = clamp(w[30], 0.1, 1.1);
    w[31] = clamp(w[31], 0.0, 1.0);
    w[32] = clamp(w[32], 0.0, 0.6);
    w[33] = clamp(w[33], 0.0, 0.6);
}

/// `memory` as the fsrs crate takes it.
fn fsrs_memory_state(memory: CardMemory) -> MemoryState {
    MemoryState {
        stability: memory.stability as f32,
        difficulty: memory.difficulty as f32,
        stability_fast: memory.stability_fast as f32,
    }
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

    /// FSRS-6's and FSRS-7's defaults, and FSRS-7 parameters out of the range the model clips
    /// them to.
    pub(crate) fn curves() -> Vec<Curve> {
        let mut extreme = fsrs_rs::DEFAULT_PARAMETERS.to_vec();
        extreme[23] = 2.0;
        extreme[26] = 0.1;
        extreme[29] = -1.0;
        extreme[32] = 0.9;
        [
            FSRS6_DEFAULT_PARAMETERS.to_vec(),
            fsrs_rs::DEFAULT_PARAMETERS.to_vec(),
            extreme,
        ]
        .into_iter()
        .map(Curve::from_parameters)
        .collect()
    }

    pub(crate) fn memories() -> Vec<CardMemory> {
        [
            (0.002, 0.002, 5.0),
            (0.01, 0.02, 9.0),
            (3.0, 1.0, 1.0),
            (40.0, 25.0, 7.5),
            // Scheduled under FSRS-6
            (10.0, 10.0, 4.0),
        ]
        .into_iter()
        .map(|(stability, stability_fast, difficulty)| CardMemory {
            stability,
            stability_fast,
            difficulty,
        })
        .collect()
    }

    #[test]
    fn days_until_inverts_the_forgetting_curve() {
        for curve in curves() {
            for memory in memories() {
                for retention in [0.95, 0.9, 0.85, 0.7] {
                    let days = curve.days_until(memory, retention);
                    let recall = curve.retrievability(memory, days);
                    assert!(
                        (recall - retention).abs() < 1e-3,
                        "{curve:?} {memory:?}: {recall} after {days} days"
                    );
                }
            }
        }
    }

    #[test]
    fn the_power_curve_is_the_models_before_rounding() {
        let model = FSRS::new(&FSRS6_DEFAULT_PARAMETERS).unwrap();
        let curve = Curve::from_parameters(FSRS6_DEFAULT_PARAMETERS.to_vec());
        for memory in memories() {
            let days = curve.days_until(memory, 0.9);
            let interval = f64::from(model.next_interval(Some(memory.stability as f32), 0.9, 1));
            assert!(
                (days - interval).abs() < 1e-4 * days.max(1.0),
                "{days} {interval}"
            );
        }
    }

    #[test]
    fn the_fsrs7_curve_is_the_models() {
        let model = FSRS::new(&fsrs_rs::DEFAULT_PARAMETERS).unwrap();
        let curve = Curve::from_parameters(fsrs_rs::DEFAULT_PARAMETERS.to_vec());
        for memory in memories() {
            for days in [0.0, 0.01, 1.0, 30.0] {
                let state = fsrs_memory_state(memory);
                let expected = f64::from(model.current_retrievability(state, days as f32));
                assert!((curve.retrievability(memory, days) - expected).abs() < 1e-6);
            }
        }
    }

    #[sqlx::test(migrations = false)]
    async fn the_sql_curve_is_the_curve(pool: sqlx::SqlitePool) {
        for curve in curves() {
            for memory in memories() {
                for days in [0.0, 0.003, 0.5, 1.0, 7.5, 365.0] {
                    let sql = curve.sql("?4");
                    let recall: f64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                        "SELECT {sql} FROM (SELECT ?1 AS stability, ?2 AS stability_fast, \
                         ?3 AS difficulty) c"
                    )))
                    .bind(memory.stability)
                    .bind(memory.stability_fast)
                    .bind(memory.difficulty)
                    .bind(days * 86_400.0)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                    let expected = curve.retrievability(memory, days);
                    assert!(
                        (recall - expected).abs() < 1e-5,
                        "{curve:?} {memory:?} after {days} days: {recall}, not {expected}"
                    );
                }
            }
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
