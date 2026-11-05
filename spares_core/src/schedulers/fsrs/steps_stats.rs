//! Steps stats, as in fsrs4anki-helper: how quickly a card is forgotten after a rating in its
//! steps, and from that the learning and relearning steps that fit.
//!
//! The reviews in a step are paired with the card's next review, if it came within a day, which
//! is what steps are for. The pairs are grouped by what the step followed (see
//! [`StepsGroupKind`]), and each group gets the one stability whose forgetting curve best predicts
//! which next reviews were recalled. A step is then the time it takes recall to fall to the
//! desired retention with that stability.
//!
//! For example, if after a first Again most cards are still recalled 10 minutes later but only
//! about half after an hour, the fitted stability is a few minutes, and the first learning step
//! recommended is the minutes it takes recall to fall to 90%.
//!
//! Only reviews count, after a card's last forget and only with FSRS ratings: crams and moved due
//! dates say nothing about memory. The learning groups take cards whose history starts from new
//! in the period, the relearning group lapses in the period.

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Utc;

use super::SCHEDULER_NAME;
use super::utils::Rating;
use super::utils::State;
use super::utils::days_until_retrievability;
use super::utils::rating_to_number;
use super::utils::retrievability;
use super::utils::state_to_number;
use crate::model::NEW_CARD_STATE;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::schedulers::effective_review_logs;
use crate::schema::statistics::RecommendedSteps;
use crate::schema::statistics::StepsGroup;
use crate::schema::statistics::StepsGroupKind;

/// The fewest reviews in a group for it to count toward a recommendation, as in the helper.
pub(crate) const MIN_REVIEWS: u32 = 100;

const SECONDS_PER_DAY: i64 = 86_400;

/// The stabilities, in days, the fit chooses from: log-spaced from about a second to 100 days,
/// each half a percent above the one before.
const GRID_MIN_DAYS: f64 = 1e-5;
const GRID_MAX_DAYS: f64 = 100.0;
const GRID_POINTS_PER_DECADE: u32 = 460;

/// Every group, in the order they are reported.
const GROUP_KINDS: [StepsGroupKind; 5] = [
    StepsGroupKind::Again,
    StepsGroupKind::Hard,
    StepsGroupKind::Good,
    StepsGroupKind::AgainThenGood,
    StepsGroupKind::Relearning,
];

/// A group's pairs of reviews, counted by the seconds between them: how many next reviews were
/// recalled, and how many forgotten.
type Pairs = HashMap<i64, (u32, u32)>;

/// The groups of `review_logs` with their fitted stabilities, and the steps recommended from
/// them.
///
/// `review_logs` must be ordered by `card_id, reviewed_at ASC, id ASC`, and include forgets.
/// `since` is the start of the period, `None` for all time.
pub(crate) fn steps_stats(
    review_logs: &[ReviewLog],
    since: Option<DateTime<Utc>>,
    decay: f32,
    desired_retention: f64,
) -> (Vec<StepsGroup>, RecommendedSteps) {
    let pairs = group_reviews(review_logs, since);
    let groups = GROUP_KINDS
        .into_iter()
        .map(|kind| {
            let pairs = pairs.get(&kind);
            let (recalled, forgotten) = pairs
                .into_iter()
                .flat_map(HashMap::values)
                .fold((0, 0), |(r, f), (recalled, forgotten)| {
                    (r + recalled, f + forgotten)
                });
            let stability = pairs.and_then(|pairs| fit_stability(pairs, decay));
            StepsGroup {
                kind,
                reviews: recalled + forgotten,
                recalled,
                stability,
                interval: stability
                    .map(|stability| step_seconds(stability, desired_retention, decay)),
            }
        })
        .collect::<Vec<_>>();
    let recommended = recommend(&groups);
    (groups, recommended)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "a step of up to 100 days of stability fits in an i64, and a day in an f64"
)]
fn step_seconds(stability: f64, desired_retention: f64, decay: f32) -> i64 {
    (days_until_retrievability(stability, desired_retention, decay) * SECONDS_PER_DAY as f64)
        .round() as i64
}

fn recommend(groups: &[StepsGroup]) -> RecommendedSteps {
    let interval = |kind| {
        groups
            .iter()
            .find(|group| group.kind == kind && group.reviews >= MIN_REVIEWS)
            .and_then(|group| group.interval)
    };
    let first_learning_step = interval(StepsGroupKind::Again);
    // Hard waits halfway between the first two steps, so the second step is twice Hard's wait
    // less the first.
    let from_hard = interval(StepsGroupKind::Hard)
        .zip(first_learning_step)
        .map(|(hard, first)| 2 * hard - first);
    let second_learning_step = [
        interval(StepsGroupKind::AgainThenGood),
        interval(StepsGroupKind::Good),
        from_hard,
    ]
    .into_iter()
    .flatten()
    .min();
    RecommendedSteps {
        first_learning_step,
        second_learning_step,
        relearning_step: interval(StepsGroupKind::Relearning),
    }
}

/// The pairs of reviews in each group: a review in a step, and the card's next review if it came
/// within a day. See the module docs for which reviews count.
fn group_reviews(
    review_logs: &[ReviewLog],
    since: Option<DateTime<Utc>>,
) -> HashMap<StepsGroupKind, Pairs> {
    let mut groups: HashMap<StepsGroupKind, Pairs> = HashMap::new();
    let mut add = |kind, from: &ReviewLog, to: &ReviewLog| {
        let seconds = (to.reviewed_at - from.reviewed_at).num_seconds();
        // Two reviews in the same second say nothing about how fast memory fades.
        if seconds <= 0 || seconds >= SECONDS_PER_DAY {
            return;
        }
        let pair = groups.entry(kind).or_default().entry(seconds).or_default();
        if to.rating > Some(rating_to_number(Rating::Again)) {
            pair.0 += 1;
        } else {
            pair.1 += 1;
        }
    };
    let in_period =
        |review_log: &ReviewLog| since.is_none_or(|since| review_log.reviewed_at >= since);
    let again = Some(rating_to_number(Rating::Again));
    let hard = Some(rating_to_number(Rating::Hard));
    let good = Some(rating_to_number(Rating::Good));
    let review_state = state_to_number(State::Review);

    for card_logs in review_logs.chunk_by(|a, b| a.card_id == b.card_id) {
        if card_logs[0].card_id.is_none() {
            continue;
        }
        let reviews = effective_review_logs(card_logs)
            .iter()
            .filter(|review_log| review_log.kind == ReviewLogKind::Review)
            .collect::<Vec<_>>();
        // Ratings only mean the same thing within one scheduler.
        if reviews
            .iter()
            .any(|review_log| review_log.scheduler_name != SCHEDULER_NAME)
        {
            continue;
        }

        // The learning steps, if the card's history starts from new in the period
        if let [first, second, rest @ ..] = reviews.as_slice()
            && first.previous_state == NEW_CARD_STATE
            && in_period(first)
        {
            let kind = match first.rating {
                rating if rating == again => Some(StepsGroupKind::Again),
                rating if rating == hard => Some(StepsGroupKind::Hard),
                rating if rating == good => Some(StepsGroupKind::Good),
                // Easy graduates the card
                _ => None,
            };
            if let Some(kind) = kind {
                add(kind, first, second);
            }
            if first.rating == again
                && second.rating == good
                && (second.reviewed_at - first.reviewed_at).num_seconds() < SECONDS_PER_DAY
                && let Some(third) = rest.first()
            {
                add(StepsGroupKind::AgainThenGood, second, third);
            }
        }

        // The relearning steps, after each lapse in the period
        for pair in reviews.windows(2) {
            let [lapse, next] = pair else { unreachable!() };
            if lapse.previous_state == review_state && lapse.rating == again && in_period(lapse) {
                add(StepsGroupKind::Relearning, lapse, next);
            }
        }
    }
    groups
}

/// The stability, in days, whose forgetting curve gives `pairs` the lowest log loss. `None` if
/// there are no pairs, or the best fit is at either end of the grid, as when every next review
/// was recalled, since then the reviews do not pin it down.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "seconds within a day, and a grid of a few thousand points"
)]
fn fit_stability(pairs: &Pairs, decay: f32) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    let pairs = pairs
        .iter()
        .map(|(seconds, counts)| (*seconds as f64 / SECONDS_PER_DAY as f64, *counts))
        .collect::<Vec<_>>();
    let log_loss = |stability: f64| {
        pairs
            .iter()
            .map(|(days, (recalled, forgotten))| {
                let recall = retrievability(*days, stability, decay).clamp(1e-6, 1.0 - 1e-6);
                -(f64::from(*recalled) * recall.ln() + f64::from(*forgotten) * (1.0 - recall).ln())
            })
            .sum::<f64>()
    };
    let points = (f64::from(GRID_POINTS_PER_DECADE) * (GRID_MAX_DAYS / GRID_MIN_DAYS).log10())
        .round() as u32;
    let grid = (0..=points).map(|i| {
        GRID_MIN_DAYS * (GRID_MAX_DAYS / GRID_MIN_DAYS).powf(f64::from(i) / f64::from(points))
    });
    let (index, stability, _) = grid
        .enumerate()
        .map(|(index, stability)| (index, stability, log_loss(stability)))
        .min_by(|a, b| a.2.total_cmp(&b.2))?;
    (index > 0 && index < points as usize).then_some(stability)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::TimeZone;
    use rand::RngExt;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;
    use crate::config::SparesExternalConfig;
    use crate::model::CardId;
    use crate::schedulers::fsrs::utils::decay;

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap()
    }

    fn log(card_id: CardId, at: DateTime<Utc>, rating: u32, previous_state: State) -> ReviewLog {
        ReviewLog {
            card_id: Some(card_id),
            reviewed_at: at,
            rating: Some(rating),
            kind: ReviewLogKind::Review,
            scheduler_name: SCHEDULER_NAME.to_string(),
            previous_state: state_to_number(previous_state),
            ..ReviewLog::default()
        }
    }

    /// `cards` new cards, each rated `first` then reviewed again after a random delay of up to
    /// `max_minutes`, recalled with the probability the forgetting curve with `stability` days
    /// gives. Seeded, so the same every run.
    #[expect(clippy::cast_precision_loss, reason = "delays of minutes")]
    fn generated(
        cards: i64,
        first: u32,
        stability: f64,
        max_minutes: i64,
        seed: u64,
    ) -> Vec<ReviewLog> {
        let decay = decay(&SparesExternalConfig::default());
        let mut rng = StdRng::seed_from_u64(seed);
        let mut logs = Vec::new();
        for card_id in 0..cards {
            let at = start() + Duration::minutes(card_id);
            let seconds = rng.random_range(30..max_minutes * 60);
            let recalled = rng.random::<f64>()
                < retrievability(seconds as f64 / SECONDS_PER_DAY as f64, stability, decay);
            logs.push(log(card_id, at, first, State::New));
            logs.push(log(
                card_id,
                at + Duration::seconds(seconds),
                if recalled { 3 } else { 1 },
                State::Learning,
            ));
        }
        logs
    }

    fn group(groups: &[StepsGroup], kind: StepsGroupKind) -> &StepsGroup {
        groups.iter().find(|group| group.kind == kind).unwrap()
    }

    #[test]
    fn the_fit_finds_the_stability_reviews_were_generated_with() {
        let decay = decay(&SparesExternalConfig::default());
        // 5 minutes
        let stability = 5.0 / 1440.0;
        let logs = generated(3000, 1, stability, 60, 1);
        let (groups, recommended) = steps_stats(&logs, None, decay, 0.9);
        let again = group(&groups, StepsGroupKind::Again);
        assert_eq!(again.reviews, 3000);
        let fitted = again.stability.unwrap();
        assert!(
            (fitted / stability - 1.0).abs() < 0.15,
            "{fitted} {stability}"
        );
        // At a desired retention of 90%, the step is the stability itself
        let step = recommended.first_learning_step.unwrap();
        assert!((step - 300).abs() < 45, "{step}");
        assert_eq!(group(&groups, StepsGroupKind::Good).reviews, 0);
        assert_eq!(recommended.second_learning_step, None);
        assert_eq!(recommended.relearning_step, None);
    }

    #[test]
    fn a_small_group_is_fitted_but_recommends_nothing() {
        let decay = decay(&SparesExternalConfig::default());
        let logs = generated(i64::from(MIN_REVIEWS) - 1, 1, 0.01, 60, 2);
        let (groups, recommended) = steps_stats(&logs, None, decay, 0.9);
        assert!(group(&groups, StepsGroupKind::Again).interval.is_some());
        assert_eq!(recommended, RecommendedSteps::default());
    }

    #[test]
    fn every_review_recalled_pins_nothing_down() {
        let decay = decay(&SparesExternalConfig::default());
        let pairs = Pairs::from([(600, (50, 0)), (1200, (50, 0))]);
        assert_eq!(fit_stability(&pairs, decay), None);
        assert_eq!(fit_stability(&Pairs::new(), decay), None);
    }

    #[test]
    fn the_second_step_is_the_shortest_of_its_estimates() {
        let groups = |hard: i64, good: i64, again_then_good: i64| {
            [
                (StepsGroupKind::Again, 60),
                (StepsGroupKind::Hard, hard),
                (StepsGroupKind::Good, good),
                (StepsGroupKind::AgainThenGood, again_then_good),
            ]
            .map(|(kind, interval)| StepsGroup {
                kind,
                reviews: MIN_REVIEWS,
                recalled: 0,
                stability: Some(1.0),
                interval: Some(interval),
            })
        };
        // Hard's 300 seconds put the second step at 2 × 300 − 60
        assert_eq!(
            recommend(&groups(300, 900, 800)).second_learning_step,
            Some(540)
        );
        assert_eq!(
            recommend(&groups(600, 900, 800)).second_learning_step,
            Some(800)
        );
        assert_eq!(
            recommend(&groups(600, 700, 800)).second_learning_step,
            Some(700)
        );
        let mut groups = groups(300, 900, 800);
        groups[3].reviews = MIN_REVIEWS - 1;
        groups[1].reviews = MIN_REVIEWS - 1;
        assert_eq!(recommend(&groups).second_learning_step, Some(900));
    }

    #[test]
    fn reviews_are_grouped_by_what_the_step_followed() {
        let minutes = |m| start() + Duration::minutes(m);
        let logs = vec![
            // Again, Good 10 minutes later, then Again an hour after that
            log(1, minutes(0), 1, State::New),
            log(1, minutes(10), 3, State::Learning),
            log(1, minutes(70), 1, State::Learning),
            // Hard, then the next review two days later, too late for a step
            log(2, minutes(0), 2, State::New),
            log(2, minutes(2 * 1440), 3, State::Learning),
            // A lapse, then a relearning step 10 minutes later. Its first review was before the
            // period, so only the lapse counts.
            log(3, minutes(-30 * 1440), 3, State::New),
            log(3, minutes(0), 1, State::Review),
            log(3, minutes(10), 3, State::Relearning),
            // Easy graduates the card
            log(4, minutes(0), 4, State::New),
            log(4, minutes(10), 3, State::Review),
            // Imported: its history does not start from new
            log(5, minutes(0), 1, State::Learning),
            log(5, minutes(10), 3, State::Learning),
        ];
        let groups = group_reviews(&logs, Some(start() - Duration::days(1)));
        let counts = |kind| groups.get(&kind).cloned().unwrap_or_default();
        assert_eq!(counts(StepsGroupKind::Again), Pairs::from([(600, (1, 0))]));
        assert_eq!(
            counts(StepsGroupKind::AgainThenGood),
            Pairs::from([(3600, (0, 1))])
        );
        assert_eq!(counts(StepsGroupKind::Hard), Pairs::new());
        assert_eq!(counts(StepsGroupKind::Good), Pairs::new());
        assert_eq!(
            counts(StepsGroupKind::Relearning),
            Pairs::from([(600, (1, 0))])
        );
    }

    #[test]
    fn a_forget_starts_the_history_again_and_crams_are_left_out() {
        let minutes = |m| start() + Duration::minutes(m);
        let forget = ReviewLog {
            card_id: Some(1),
            reviewed_at: minutes(-5),
            kind: ReviewLogKind::Forget,
            rating: None,
            ..ReviewLog::default()
        };
        let cram = ReviewLog {
            kind: ReviewLogKind::Cram,
            ..log(1, minutes(5), 1, State::Learning)
        };
        let logs = vec![
            log(1, minutes(-60), 3, State::New),
            forget,
            log(1, minutes(0), 3, State::New),
            cram,
            log(1, minutes(20), 1, State::Learning),
        ];
        let groups = group_reviews(&logs, None);
        assert_eq!(groups[&StepsGroupKind::Good], Pairs::from([(1200, (0, 1))]));
    }
}
