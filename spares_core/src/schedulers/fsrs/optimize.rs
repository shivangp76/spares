//! Fitting the FSRS parameters to the review log.
//!
//! The review log is turned into the training data the `fsrs` crate expects the way Anki does it:
//! one item per review, holding the card's reviews up to and including it, with the time between
//! reviews counted in days.
//!
//! The parameters fitted are FSRS-7's, which learn from every review after a card's first, same-day
//! ones included, with the time between reviews as it was. Both the current parameters and the
//! fitted ones are scored on the reviews on a later day of study than the card's previous review,
//! each model seeing the history as it counts time, so FSRS-6 parameters can be compared too:
//! FSRS-6 counts whole days of study.
use chrono::NaiveDate;
use fsrs_rs::ComputeParametersInput;
use fsrs_rs::ComputeParametersVersion;
use fsrs_rs::FSRS;
use fsrs_rs::FSRSError;
use fsrs_rs::FSRSItem;
use fsrs_rs::FSRSReview;
use fsrs_rs::ModelVersion;
use fsrs_rs::check_and_fill_parameters;
use fsrs_rs::compute_parameters;
use itertools::Itertools;

use super::SCHEDULER_NAME;
use super::utils::number_to_rating;
use super::utils::rating_to_number;
use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::api::statistics::local_time::Zone;
use crate::helpers::FractionalDays;
use crate::helpers::study_date;
use crate::model::CardId;
use crate::model::NEW_CARD_STATE;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::schedulers::effective_review_logs;
use crate::schema::review::OptimizeResponse;
use crate::schema::review::ParametersEvaluation;

#[derive(Debug, Default)]
pub struct TrainingSet {
    /// What FSRS-7 is fitted to: an item per review after a card's first.
    items: Vec<FSRSItem>,
    /// The card each item belongs to, aligned with `items`.
    card_ids: Vec<CardId>,
    card_count: usize,
    /// What parameters are scored on: an item per review on a later day of study than the card's
    /// previous review, with the time between reviews as FSRS-7 counts it, ...
    scored_by_time: Vec<FSRSItem>,
    /// ... and the same items, with the days of study FSRS-6 counts.
    scored_by_days: Vec<FSRSItem>,
}

/// One card's part of a [`TrainingSet`].
#[derive(Debug, Default, PartialEq)]
struct CardItems {
    items: Vec<FSRSItem>,
    scored_by_time: Vec<FSRSItem>,
    scored_by_days: Vec<FSRSItem>,
}

/// Builds the training set from every card's review log.
///
/// `review_logs` must be ordered by `card_id, reviewed_at ASC, id ASC`, and include every
/// [`ReviewLogKind`], so that forgets are seen. Reviews are told apart by the day of study they
/// count for in `zone` (see [`Zone::from_config`]). Reviews of days before `ignore_before` are
/// left out (see
/// [`SparesExternalConfig::fsrs_optimizer_ignore_reviews_before`](crate::config::SparesExternalConfig::fsrs_optimizer_ignore_reviews_before)).
pub fn training_set(
    review_logs: &[ReviewLog],
    zone: Zone,
    ignore_before: Option<NaiveDate>,
) -> TrainingSet {
    let mut training_set = TrainingSet::default();
    for (card_id, card_review_logs) in &review_logs.iter().chunk_by(|review_log| review_log.card_id)
    {
        let Some(card_id) = card_id else {
            continue;
        };
        let card_review_logs = card_review_logs.cloned().collect::<Vec<_>>();
        let items = card_items(&card_review_logs, zone, ignore_before);
        if items.items.is_empty() {
            continue;
        }
        training_set.card_count += 1;
        training_set
            .card_ids
            .extend(std::iter::repeat_n(card_id, items.items.len()));
        training_set.items.extend(items.items);
        training_set.scored_by_time.extend(items.scored_by_time);
        training_set.scored_by_days.extend(items.scored_by_days);
    }
    training_set
}

/// The items of one card.
fn card_items(
    review_logs: &[ReviewLog],
    zone: Zone,
    ignore_before: Option<NaiveDate>,
) -> CardItems {
    // Reviews before a forget were discarded by the user and say nothing about the memory the card
    // has now.
    let reviews = effective_review_logs(review_logs)
        .iter()
        .filter(|review_log| review_log.kind == ReviewLogKind::Review)
        .filter(|review_log| {
            ignore_before.is_none_or(|first| study_date(review_log.reviewed_at, zone) >= first)
        })
        .collect::<Vec<_>>();
    // Ratings only mean the same thing within one scheduler.
    if reviews
        .iter()
        .any(|review_log| review_log.scheduler_name != SCHEDULER_NAME)
    {
        return CardItems::default();
    }
    // A history that does not start from a new card is missing its beginning, for example because
    // it was imported or its first reviews were ignored, so the model would see the wrong first
    // review.
    if reviews
        .first()
        .is_none_or(|first| first.previous_state != NEW_CARD_STATE)
    {
        return CardItems::default();
    }

    let mut items = CardItems::default();
    let mut by_time = Vec::new();
    let mut by_days = Vec::new();
    let mut previous: Option<&ReviewLog> = None;
    for review_log in reviews {
        let Some(rating) = review_log
            .rating
            .and_then(number_to_rating)
            .map(rating_to_number)
        else {
            // A review without a valid rating cannot be learned from, and dropping it would
            // misstate the time between the reviews around it.
            return CardItems::default();
        };
        // Days as the user lives them, so that a review late in the evening and one the next
        // morning are a day apart, while one after midnight still counts for the evening before.
        let days = previous.map_or(0, |previous| {
            u32::try_from(
                (study_date(review_log.reviewed_at, zone) - study_date(previous.reviewed_at, zone))
                    .num_days(),
            )
            .unwrap_or(0)
        });
        let elapsed = previous.map_or(0.0, |previous| {
            (review_log.reviewed_at - previous.reviewed_at)
                .num_fractional_days()
                .max(0.0) as f32
        });
        let first = previous.is_none();
        previous = Some(review_log);
        by_time.push(FSRSReview {
            rating,
            delta_t: elapsed,
        });
        by_days.push(FSRSReview {
            rating,
            delta_t: days as f32,
        });
        if !first {
            items.items.push(FSRSItem {
                reviews: by_time.clone(),
            });
        }
        // Only a review on a later day than the previous one says anything about long-term
        // memory to FSRS-6. A same-day review, such as a relearning step after a lapse, stays in
        // the history of the items after it, but `FSRS::evaluate` cannot score it under FSRS-6 as
        // an item of its own.
        if days > 0 {
            items.scored_by_time.push(FSRSItem {
                reviews: by_time.clone(),
            });
            items.scored_by_days.push(FSRSItem {
                reviews: by_days.clone(),
            });
        }
    }
    items
}

fn fsrs_error(error: &FSRSError) -> Error {
    Error::Library(LibraryError::Scheduler(SchedulerErrorKind::Custom {
        scheduler_name: SCHEDULER_NAME.to_string(),
        error: format!("{error:?}"),
    }))
}

/// Scores `parameters` on the items of their version: `by_time` for FSRS-7, `by_days` before.
fn evaluate(
    parameters: &[f32],
    by_time: &[FSRSItem],
    by_days: &[FSRSItem],
) -> Result<ParametersEvaluation, Error> {
    let model = FSRS::new(parameters).map_err(|e| fsrs_error(&e))?;
    let items = if model.version() == ModelVersion::Fsrs7 {
        by_time
    } else {
        by_days
    };
    let evaluation = model
        .evaluate(items.to_vec(), |_| true)
        .map_err(|e| fsrs_error(&e))?;
    Ok(ParametersEvaluation {
        log_loss: evaluation.log_loss,
        rmse_bins: evaluation.rmse_bins,
    })
}

/// Fits the parameters to `training_set` and scores them against `current_parameters`.
///
/// This is CPU-bound and can take a while on a large collection, so call it off the async
/// executor.
pub fn optimize(
    training_set: TrainingSet,
    current_parameters: &[f32],
    num_relearning_steps: usize,
) -> Result<OptimizeResponse, Error> {
    let TrainingSet {
        items,
        card_ids,
        card_count,
        scored_by_time,
        scored_by_days,
    } = training_set;
    // Without a review on a later day, there would be nothing to score the parameters on.
    if scored_by_time.is_empty() {
        return Err(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::InvalidInput(
                "There are no reviews to optimize on yet. A card needs a review on a later day \
                 than its first."
                    .to_string(),
            ),
        )));
    }
    let current_parameters =
        check_and_fill_parameters(current_parameters).map_err(|e| fsrs_error(&e))?;
    let review_count = items.len();
    let optimized_parameters = compute_parameters(ComputeParametersInput {
        train_set: items,
        card_ids: Some(card_ids),
        num_relearning_steps: Some(num_relearning_steps),
        model_version: ComputeParametersVersion::Fsrs7,
        ..ComputeParametersInput::default()
    })
    .map_err(|e| fsrs_error(&e))?;
    let current = evaluate(&current_parameters, &scored_by_time, &scored_by_days)?;
    let optimized = evaluate(&optimized_parameters, &scored_by_time, &scored_by_days)?;
    Ok(OptimizeResponse {
        card_count,
        review_count,
        current_parameters,
        current,
        optimized_parameters,
        optimized,
        applied: false,
        event_id: None,
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use chrono::Duration;
    use chrono::Local;
    use chrono::TimeZone;
    use chrono::Utc;
    use serde_json::Map;
    use serde_json::Value;

    use super::*;
    use crate::model::RatingId;
    use crate::model::StateId;

    /// The server's zone, with days starting at `day_starts_at`.
    fn hours(day_starts_at: u8) -> Zone {
        Zone::parse(None, day_starts_at).unwrap()
    }

    /// Local noon `day` days after a fixed date, so that reviews on different days are always a
    /// whole number of local days apart.
    fn at(day: i64, hour: u32) -> DateTime<Utc> {
        let base = Local
            .with_ymd_and_hms(2026, 1, 1, hour, 0, 0)
            .single()
            .unwrap();
        (base + Duration::days(day)).to_utc()
    }

    fn review(card_id: CardId, day: i64, rating: RatingId, previous_state: StateId) -> ReviewLog {
        ReviewLog {
            id: day,
            card_id: Some(card_id),
            reviewed_at: at(day, 12),
            kind: ReviewLogKind::Review,
            rating: Some(rating),
            tag_id: None,
            scheduler_name: SCHEDULER_NAME.to_string(),
            scheduled_time: Some(86_400),
            recall_duration: Some(5),
            rate_duration: Some(2),
            previous_state,
            custom_data: Value::Object(Map::new()),
        }
    }

    fn forget(card_id: CardId, day: i64) -> ReviewLog {
        ReviewLog {
            kind: ReviewLogKind::Forget,
            rating: None,
            scheduled_time: None,
            recall_duration: None,
            rate_duration: None,
            ..review(card_id, day, 1, 2)
        }
    }

    fn deltas(item: &FSRSItem) -> Vec<(u32, u32)> {
        item.reviews
            .iter()
            .map(|review| (review.rating, review.delta_t as u32))
            .collect()
    }

    #[test]
    fn one_item_per_review_on_a_later_day() {
        let logs = vec![
            review(1, 0, 3, 0),
            review(1, 1, 3, 1),
            review(1, 4, 1, 2),
            review(1, 10, 4, 3),
        ];
        let set = training_set(&logs, hours(4), None);
        assert_eq!(set.card_count, 1);
        assert_eq!(set.card_ids, vec![1, 1, 1]);
        assert_eq!(
            set.scored_by_days.iter().map(deltas).collect::<Vec<_>>(),
            vec![
                vec![(3, 0), (3, 1)],
                vec![(3, 0), (3, 1), (1, 3)],
                vec![(3, 0), (3, 1), (1, 3), (4, 6)],
            ]
        );
    }

    /// Reviews before the day ignored are left out, and with them a card first studied before
    /// it, whose history then misses its beginning.
    #[test]
    fn reviews_before_the_day_ignored_are_left_out() {
        let logs = vec![
            review(1, 0, 3, 0),
            review(1, 4, 3, 2),
            review(1, 10, 3, 2),
            review(2, 5, 3, 0),
            review(2, 8, 3, 1),
        ];
        let day = |day| hours(4).date(at(day, 12));
        let set = training_set(&logs, hours(4), Some(day(5)));
        assert_eq!(set.card_count, 1);
        assert_eq!(set.card_ids, vec![2]);
        assert_eq!(
            set.scored_by_days.iter().map(deltas).collect::<Vec<_>>(),
            vec![vec![(3, 0), (3, 3)]]
        );
        assert_eq!(training_set(&logs, hours(4), Some(day(0))).card_count, 2);
    }

    #[test]
    fn a_same_day_review_after_a_later_day_gives_no_item() {
        let mut relearn = review(1, 5, 4, 3);
        relearn.id = 6;
        relearn.reviewed_at = at(5, 13);
        let logs = vec![
            review(1, 0, 1, 0),
            review(1, 5, 1, 2),
            relearn,
            review(1, 8, 3, 2),
        ];
        let set = training_set(&logs, hours(4), None);
        assert_eq!(
            set.scored_by_days.iter().map(deltas).collect::<Vec<_>>(),
            vec![vec![(1, 0), (1, 5)], vec![(1, 0), (1, 5), (4, 0), (3, 3)],],
            "the relearning step is kept in the history but is not an item"
        );
        // `FSRS::evaluate` refuses the whole set if any item ends on a same-day review.
        let fsrs6 = fsrs_rs::FSRS6_DEFAULT_PARAMETERS;
        assert!(evaluate(&fsrs6, &set.scored_by_time, &set.scored_by_days).is_ok());
        assert!(evaluate(&[], &set.scored_by_time, &set.scored_by_days).is_ok());
    }

    /// FSRS-7 learns from every review after the first, with the time between reviews as it was.
    #[test]
    fn fsrs7_learns_from_same_day_reviews_too() {
        let mut relearn = review(1, 5, 4, 3);
        relearn.id = 6;
        relearn.reviewed_at = at(5, 18);
        let logs = vec![review(1, 0, 1, 0), review(1, 5, 1, 2), relearn];
        let set = training_set(&logs, hours(4), None);
        let times = |item: &FSRSItem| {
            item.reviews
                .iter()
                .map(|review| (review.rating, review.delta_t))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            set.items.iter().map(times).collect::<Vec<_>>(),
            vec![
                vec![(1, 0.0), (1, 5.0)],
                vec![(1, 0.0), (1, 5.0), (4, 0.25)],
            ]
        );
        assert_eq!(set.card_ids, vec![1, 1]);
        assert_eq!(set.scored_by_time.len(), 1);
    }

    #[test]
    fn delta_t_counts_days_of_study() {
        let mut late = review(1, 0, 3, 0);
        late.reviewed_at = at(0, 23);
        let mut early = review(1, 1, 3, 1);
        early.reviewed_at = at(1, 5);
        let set = training_set(&[late, early], hours(4), None);
        assert_eq!(
            deltas(&set.scored_by_days[0]),
            vec![(3, 0), (3, 1)],
            "six hours apart, but on consecutive days"
        );
    }

    #[test]
    fn a_review_before_the_day_starts_counts_for_the_day_before() {
        let mut late = review(1, 0, 3, 0);
        late.reviewed_at = at(0, 23);
        let mut after_midnight = review(1, 1, 3, 1);
        after_midnight.reviewed_at = at(1, 1);
        assert_eq!(
            training_set(&[late.clone(), after_midnight.clone()], hours(4), None).scored_by_days,
            Vec::new(),
            "the same day of study"
        );
        let set = training_set(&[late, after_midnight], hours(0), None);
        assert_eq!(deltas(&set.scored_by_days[0]), vec![(3, 0), (3, 1)]);
    }

    /// FSRS-7 learns from them, but there is nothing to score parameters on.
    #[test]
    fn same_day_reviews_alone_are_not_scored() {
        let mut second = review(1, 0, 3, 1);
        second.id = 2;
        second.reviewed_at = at(0, 13);
        let set = training_set(&[review(1, 0, 1, 0), second], hours(4), None);
        assert_eq!(set.items.len(), 1);
        assert_eq!(set.card_count, 1);
        assert_eq!(set.scored_by_time, Vec::new());
        assert!(optimize(set, &[], 1).is_err());
    }

    #[test]
    fn a_forget_starts_the_history_over() {
        let logs = vec![
            review(1, 0, 3, 0),
            review(1, 5, 3, 2),
            forget(1, 6),
            review(1, 7, 3, 0),
            review(1, 9, 2, 2),
        ];
        let set = training_set(&logs, hours(4), None);
        assert_eq!(
            set.scored_by_days.iter().map(deltas).collect::<Vec<_>>(),
            vec![vec![(3, 0), (2, 2)]]
        );
    }

    #[test]
    fn a_history_without_its_first_review_is_skipped() {
        let set = training_set(&[review(1, 0, 3, 2), review(1, 5, 3, 2)], hours(4), None);
        assert_eq!(set.items, Vec::new());
    }

    #[test]
    fn other_schedulers_reviews_are_skipped() {
        let mut other = review(1, 5, 3, 2);
        other.scheduler_name = "other".to_string();
        let set = training_set(&[review(1, 0, 3, 0), other], hours(4), None);
        assert_eq!(set.items, Vec::new());
    }

    #[test]
    fn cards_are_kept_apart() {
        let logs = vec![
            review(1, 0, 3, 0),
            review(1, 2, 3, 2),
            review(2, 1, 3, 0),
            review(2, 4, 3, 2),
        ];
        let set = training_set(&logs, hours(4), None);
        assert_eq!(set.card_count, 2);
        assert_eq!(set.card_ids, vec![1, 2]);
        assert_eq!(deltas(&set.items[1]), vec![(3, 0), (3, 3)]);
    }

    #[test]
    fn optimizing_nothing_is_an_error() {
        assert!(optimize(TrainingSet::default(), &[], 1).is_err());
    }
}
