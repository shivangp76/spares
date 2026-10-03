//! Fitting the FSRS parameters to the review log.
//!
//! The review log is turned into the training data the `fsrs` crate expects the way Anki does it:
//! one item per review, holding the card's reviews up to and including it, with the time between
//! reviews counted in days.
use chrono::Local;
use chrono::NaiveDate;
use fsrs_rs::ComputeParametersInput;
use fsrs_rs::FSRS;
use fsrs_rs::FSRSError;
use fsrs_rs::FSRSItem;
use fsrs_rs::FSRSReview;
use fsrs_rs::check_and_fill_parameters;
use fsrs_rs::compute_parameters;
use itertools::Itertools;

use super::utils::number_to_rating;
use super::utils::rating_to_number;
use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::model::CardId;
use crate::model::NEW_CARD_STATE;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::schedulers::effective_review_logs;
use crate::schema::review::OptimizeResponse;
use crate::schema::review::ParametersEvaluation;

const SCHEDULER_NAME: &str = "fsrs";

#[derive(Debug, Default)]
pub struct TrainingSet {
    items: Vec<FSRSItem>,
    /// The card each item belongs to, aligned with `items`.
    card_ids: Vec<CardId>,
    card_count: usize,
}

/// Builds the training set from every card's review log.
///
/// `review_logs` must be ordered by `card_id, reviewed_at ASC, id ASC`, and include every
/// [`ReviewLogKind`], so that forgets are seen.
pub fn training_set(review_logs: &[ReviewLog]) -> TrainingSet {
    let mut training_set = TrainingSet::default();
    for (card_id, card_review_logs) in &review_logs.iter().chunk_by(|review_log| review_log.card_id)
    {
        let Some(card_id) = card_id else {
            continue;
        };
        let card_review_logs = card_review_logs.cloned().collect::<Vec<_>>();
        let items = card_items(&card_review_logs);
        if items.is_empty() {
            continue;
        }
        training_set.card_count += 1;
        training_set
            .card_ids
            .extend(std::iter::repeat_n(card_id, items.len()));
        training_set.items.extend(items);
    }
    training_set
}

/// The training items for one card, one per review that has an earlier review on a previous day.
fn card_items(review_logs: &[ReviewLog]) -> Vec<FSRSItem> {
    // Reviews before a forget were discarded by the user and say nothing about the memory the card
    // has now.
    let reviews = effective_review_logs(review_logs)
        .iter()
        .filter(|review_log| review_log.kind == ReviewLogKind::Review)
        .collect::<Vec<_>>();
    // Ratings only mean the same thing within one scheduler.
    if reviews
        .iter()
        .any(|review_log| review_log.scheduler_name != SCHEDULER_NAME)
    {
        return Vec::new();
    }
    // A history that does not start from a new card is missing its beginning, for example because
    // it was imported, so the model would see the wrong first review.
    if reviews
        .first()
        .is_none_or(|first| first.previous_state != NEW_CARD_STATE)
    {
        return Vec::new();
    }

    let mut items = Vec::new();
    let mut history = Vec::new();
    let mut previous_day: Option<NaiveDate> = None;
    for review_log in reviews {
        let Some(rating) = review_log
            .rating
            .and_then(number_to_rating)
            .map(rating_to_number)
        else {
            // A review without a valid rating cannot be learned from, and dropping it would
            // misstate the time between the reviews around it.
            return Vec::new();
        };
        // Days as the user lives them, so that a review late in the evening and one the next
        // morning are a day apart.
        let day = review_log.reviewed_at.with_timezone(&Local).date_naive();
        let delta_t = previous_day.map_or(0, |previous_day| {
            u32::try_from((day - previous_day).num_days()).unwrap_or(0)
        });
        previous_day = Some(day);
        history.push(FSRSReview { rating, delta_t });
        let item = FSRSItem {
            reviews: history.clone(),
        };
        // An item needs a review on a later day to say anything about long-term memory.
        if item.long_term_review_cnt() > 0 {
            items.push(item);
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

fn evaluate(parameters: &[f32], items: Vec<FSRSItem>) -> Result<ParametersEvaluation, Error> {
    let evaluation = FSRS::new(parameters)
        .map_err(|e| fsrs_error(&e))?
        .evaluate(items, |_| true)
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
    } = training_set;
    if items.is_empty() {
        return Err(Error::Library(LibraryError::Scheduler(
            SchedulerErrorKind::InvalidInput(
                "There are no reviews to optimize on yet. Only reviews on a later day than the \
                 card's previous review count."
                    .to_string(),
            ),
        )));
    }
    let current_parameters =
        check_and_fill_parameters(current_parameters).map_err(|e| fsrs_error(&e))?;
    let optimized_parameters = compute_parameters(ComputeParametersInput {
        train_set: items.clone(),
        card_ids: Some(card_ids),
        // Same-day reviews from learning steps are in the log, so fit the short-term parameters too.
        enable_short_term: true,
        num_relearning_steps: Some(num_relearning_steps),
        ..ComputeParametersInput::default()
    })
    .map_err(|e| fsrs_error(&e))?;
    let current = evaluate(&current_parameters, items.clone())?;
    let optimized = evaluate(&optimized_parameters, items.clone())?;
    Ok(OptimizeResponse {
        card_count,
        review_count: items.len(),
        current_parameters,
        current,
        optimized_parameters,
        optimized,
        applied: false,
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use chrono::Duration;
    use chrono::TimeZone;
    use chrono::Utc;
    use serde_json::Map;
    use serde_json::Value;

    use super::*;
    use crate::model::RatingId;
    use crate::model::StateId;

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
            .map(|review| (review.rating, review.delta_t))
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
        let set = training_set(&logs);
        assert_eq!(set.card_count, 1);
        assert_eq!(set.card_ids, vec![1, 1, 1]);
        assert_eq!(
            set.items.iter().map(deltas).collect::<Vec<_>>(),
            vec![
                vec![(3, 0), (3, 1)],
                vec![(3, 0), (3, 1), (1, 3)],
                vec![(3, 0), (3, 1), (1, 3), (4, 6)],
            ]
        );
    }

    #[test]
    fn delta_t_counts_calendar_days() {
        let mut late = review(1, 0, 3, 0);
        late.reviewed_at = at(0, 23);
        let mut early = review(1, 1, 3, 1);
        early.reviewed_at = at(1, 1);
        let set = training_set(&[late, early]);
        assert_eq!(
            deltas(&set.items[0]),
            vec![(3, 0), (3, 1)],
            "two hours apart, but on consecutive days"
        );
    }

    #[test]
    fn same_day_reviews_alone_give_no_items() {
        let mut second = review(1, 0, 3, 1);
        second.id = 2;
        second.reviewed_at = at(0, 13);
        let set = training_set(&[review(1, 0, 1, 0), second]);
        assert_eq!(set.items, Vec::new());
        assert_eq!(set.card_count, 0);
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
        let set = training_set(&logs);
        assert_eq!(
            set.items.iter().map(deltas).collect::<Vec<_>>(),
            vec![vec![(3, 0), (2, 2)]]
        );
    }

    #[test]
    fn a_history_without_its_first_review_is_skipped() {
        let set = training_set(&[review(1, 0, 3, 2), review(1, 5, 3, 2)]);
        assert_eq!(set.items, Vec::new());
    }

    #[test]
    fn other_schedulers_reviews_are_skipped() {
        let mut other = review(1, 5, 3, 2);
        other.scheduler_name = "other".to_string();
        let set = training_set(&[review(1, 0, 3, 0), other]);
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
        let set = training_set(&logs);
        assert_eq!(set.card_count, 2);
        assert_eq!(set.card_ids, vec![1, 2]);
        assert_eq!(deltas(&set.items[1]), vec![(3, 0), (3, 3)]);
    }

    #[test]
    fn optimizing_nothing_is_an_error() {
        assert!(optimize(TrainingSet::default(), &[], 1).is_err());
    }
}
