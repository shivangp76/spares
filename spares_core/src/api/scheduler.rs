use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::api::review::pinned_card_ids;
use crate::api::review::reschedule_all_cards;
use crate::api::review::reschedule_all_cards_unlogged;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::OptimizeSchedulerPayload;
use crate::api::undo::payloads::SchedulerParameters;
use crate::api::undo::payloads::Transition;
use crate::config::FittedAt;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::config::write_fsrs_parameters;
use crate::helpers::study_date;
use crate::model::Card;
use crate::model::EventType;
use crate::model::ReviewLog;
use crate::schedulers::SrsScheduler;
use crate::schedulers::get_scheduler_from_string;
use crate::schema::review::CardState;
use crate::schema::review::IntervalExample;
use crate::schema::review::IntervalPreviewRequest;
use crate::schema::review::IntervalPreviewResponse;
use crate::schema::review::OptimizeProposal;
use crate::schema::review::OptimizeRequest;
use crate::schema::review::OptimizeResponse;
use crate::schema::review::OptimizeStatusResponse;
use crate::schema::review::Rating;
use crate::schema::review::RatingInterval;
use crate::schema::review::ReschedulePreviewResponse;
use crate::schema::review::RescheduleResponse;

pub fn get_scheduler_ratings(scheduler_name: &str) -> Result<Vec<Rating>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    Ok(scheduler.get_ratings())
}

/// The states the scheduler puts cards in. See
/// [`SrsScheduler::get_states`](crate::schedulers::SrsScheduler::get_states).
pub fn get_scheduler_states(scheduler_name: &str) -> Result<Vec<CardState>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    Ok(scheduler.get_states())
}

pub fn resolve_rating_from_score(scheduler_name: &str, score: f64) -> Result<Rating, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    scheduler.rating_from_score(score)
}

/// Recomputes every card's memory state and due date from its review history. Run after changing
/// the scheduler, its parameters, learning steps or easy days. Cards moved since their last
/// review keep their due date, unless
/// [`reschedule_moved_cards`](crate::config::SparesExternalConfig::reschedule_moved_cards) is set.
// There is deliberately no query to reschedule only some cards. Cards are placed one at a time
// against the due dates already given to the others (load balancing, easy days, sibling
// dispersal), so rescheduling a subset would leave the load spread across days unbalanced. The
// settings that call for a reschedule also apply to every card.
pub async fn reschedule_scheduler(
    db: &SqlitePool,
    scheduler_name: &str,
    at: DateTime<Utc>,
) -> Result<RescheduleResponse, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let event_id = reschedule_all_cards(db, scheduler.as_ref(), &config, at).await?;
    Ok(RescheduleResponse { event_id })
}

/// What [`reschedule_scheduler`] would do with the cards moved since their last review, for the
/// confirmation before it.
pub async fn reschedule_preview(
    db: &SqlitePool,
    scheduler_name: &str,
) -> Result<ReschedulePreviewResponse, Error> {
    // Checks the name, as the reschedule would.
    get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    Ok(ReschedulePreviewResponse {
        moved_count: pinned_card_ids(db, None).await?.len(),
        moved_cards_kept: !config.reschedule_moved_cards,
    })
}

/// Fits the scheduler's parameters to the review history. With `apply`, saves them if they
/// predict it better than the current ones, with the day and the number of reviews they were fitted
/// to, and reschedules every card with them, as one
/// [`OptimizeScheduler`](EventType::OptimizeScheduler) event that undo reverts.
pub async fn optimize_scheduler(
    db: &SqlitePool,
    scheduler_name: &str,
    OptimizeRequest { apply }: OptimizeRequest,
    at: DateTime<Utc>,
) -> Result<OptimizeResponse, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let mut response = scheduler.optimize(db, &config).await?;
    if apply && response.optimized.log_loss < response.current.log_loss {
        let fitted = FittedAt {
            date: study_date(at, config.zone()),
            review_count: response.review_count,
        };
        response.event_id = Some(
            apply_parameters(
                db,
                scheduler.as_ref(),
                SchedulerParameters {
                    fsrs_parameters: response.optimized_parameters.clone(),
                    fitted: Some(fitted),
                },
                at,
            )
            .await?,
        );
        response.applied = true;
    }
    Ok(response)
}

/// The example cards [`preview_intervals`] shows: a new card, and one reviewed this many times.
const PREVIEW_REVIEWS: [usize; 2] = [0, 3];

/// The next interval of each rating for example cards (a new card, and one rated Good three times,
/// each when it was due), as the scheduler gives them with the collection's settings, or the
/// request's in their place. It is the scheduler's own `schedule`, kept within the minimum and
/// maximum interval as placing a card does, but before load balancing or easy days move it by a
/// day or so (see `place_due`), so it shows what the settings' numbers mean.
pub fn preview_intervals(
    scheduler_name: &str,
    request: IntervalPreviewRequest,
    at: DateTime<Utc>,
) -> Result<IntervalPreviewResponse, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let mut config = read_external_config()?;
    let invalid = |message: String| Error::Library(LibraryError::InvalidConfig(message));
    let steps = |seconds: Vec<i64>, name: &str| {
        if seconds.iter().any(|s| *s <= 0 || *s >= 86_400) || !seconds.is_sorted() {
            return Err(invalid(format!(
                "{name} must be increasing, each longer than 0 and shorter than a day"
            )));
        }
        Ok(seconds
            .into_iter()
            .map(Duration::seconds)
            .collect::<Vec<_>>())
    };
    if let Some(seconds) = request.learning_steps {
        config.learning_steps = steps(seconds, "Learning steps")?;
    }
    if let Some(seconds) = request.relearning_steps {
        config.relearning_steps = steps(seconds, "Relearning steps")?;
    }
    if let Some(seconds) = request.maximum_interval {
        if seconds < 86_400 {
            return Err(invalid(
                "The maximum interval must be a day or more".to_string(),
            ));
        }
        config.maximum_interval = Duration::seconds(seconds);
    }
    let desired_retention = request
        .desired_retention
        .unwrap_or(crate::model::DEFAULT_DESIRED_RETENTION);
    if !(0.7..=0.99).contains(&desired_retention) {
        return Err(invalid(format!(
            "Desired retention must be from 0.7 to 0.99, not {desired_retention}"
        )));
    }
    let ratings = scheduler.get_ratings();
    let good = ratings
        .iter()
        .find(|r| r.description.eq_ignore_ascii_case("good"))
        .or_else(|| ratings.get(ratings.len().saturating_sub(2)))
        .ok_or_else(|| invalid("The scheduler has no ratings".to_string()))?
        .id;
    let examples = PREVIEW_REVIEWS
        .iter()
        .map(|&reviews| {
            let (card, logs, now) = example_card(
                scheduler.as_ref(),
                &config,
                desired_retention,
                good,
                reviews,
                at,
            )?;
            let intervals = ratings
                .iter()
                .map(|rating| {
                    let (next, _) = scheduler.schedule(
                        &config,
                        &card,
                        &logs,
                        rating.id,
                        now,
                        Duration::seconds(5),
                        Duration::seconds(2),
                    )?;
                    Ok(RatingInterval {
                        rating: rating.clone(),
                        seconds: (placed_due(&config, &next, now) - now).num_seconds(),
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok(IntervalExample {
                label: if reviews == 0 {
                    "New card".to_string()
                } else {
                    format!("Seen {reviews} times")
                },
                reviews,
                intervals,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let seconds = |steps: &[Duration]| steps.iter().map(Duration::num_seconds).collect();
    Ok(IntervalPreviewResponse {
        desired_retention,
        learning_steps: seconds(&config.learning_steps),
        relearning_steps: seconds(&config.relearning_steps),
        maximum_interval: config.maximum_interval.num_seconds(),
        examples,
    })
}

/// `card`'s due date after a review at `at`, kept within the minimum and maximum interval once it
/// is in review, as placing it does (`place_due`), leaving out the day or so load balancing moves
/// it.
fn placed_due(config: &SparesExternalConfig, card: &Card, at: DateTime<Utc>) -> DateTime<Utc> {
    if crate::schedulers::state_role(card.state) != Some(crate::schema::review::StateRole::Review) {
        return card.due;
    }
    at + (card.due - at).clamp(config.minimum_interval, config.maximum_interval)
}

/// A card rated `rating` `reviews` times, each when it was due, from a new card, with its review
/// logs and when it is next due. Its reviews start far enough back to end before `at`.
fn example_card(
    scheduler: &dyn SrsScheduler,
    config: &SparesExternalConfig,
    desired_retention: f64,
    rating: crate::model::RatingId,
    reviews: usize,
    at: DateTime<Utc>,
) -> Result<(Card, Vec<ReviewLog>, DateTime<Utc>), Error> {
    let mut card = Card::new(at);
    card.desired_retention = desired_retention;
    let mut logs = Vec::new();
    let mut now = at;
    for _ in 0..reviews {
        let (next, log) = scheduler.schedule(
            config,
            &card,
            &logs,
            rating,
            now,
            Duration::seconds(5),
            Duration::seconds(2),
        )?;
        now = placed_due(config, &next, now);
        card = next;
        card.due = now;
        logs.push(log);
    }
    Ok((card, logs, now))
}

/// Suggest optimizing again after this many reviews since the parameters were fitted, ...
pub const OPTIMIZE_AFTER_REVIEWS: usize = 500;
/// ... or this many days of study, whichever comes first.
pub const OPTIMIZE_AFTER_DAYS: i64 = 30;
/// How much lower, as a fraction of the current one, the log loss of parameters fitted by
/// `auto_optimize` must be for them to be offered, so that a tiny gain changes nothing.
pub const AUTO_OPTIMIZE_MIN_IMPROVEMENT: f32 = 0.01;
/// `app_state` key of the [`OptimizeProposal`] `auto_optimize` fitted, as JSON.
const PROPOSAL_KEY: &str = "fsrs_parameters_proposal";
/// `app_state` key of when `auto_optimize` last fitted parameters, as a [`FittedAt`] in JSON, so
/// that parameters it found no better are not fitted again until as many reviews or days have
/// passed since.
const AUTO_OPTIMIZED_KEY: &str = "fsrs_auto_optimized";

/// The parameters in the settings, and when they were fitted.
pub(crate) fn current_parameters() -> Result<SchedulerParameters, Error> {
    Ok(parameters_of(&read_external_config()?))
}

fn parameters_of(config: &SparesExternalConfig) -> SchedulerParameters {
    SchedulerParameters {
        fsrs_parameters: config.fsrs_parameters.clone(),
        fitted: fitted_of(config),
    }
}

fn fitted_of(config: &SparesExternalConfig) -> Option<FittedAt> {
    Some(FittedAt {
        date: config.fsrs_parameters_fitted_at?,
        review_count: config.fsrs_parameters_review_count?,
    })
}

/// Saves `parameters` to the settings and reschedules every card with them, logged as one
/// [`OptimizeScheduler`](EventType::OptimizeScheduler) event, whose id it returns. Any proposal is
/// dropped, as it was scored against the parameters replaced.
async fn apply_parameters(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    parameters: SchedulerParameters,
    at: DateTime<Utc>,
) -> Result<i64, Error> {
    let before = current_parameters()?;
    write_fsrs_parameters(db, &parameters.fsrs_parameters, parameters.fitted).await?;
    // Read back rather than patched in memory, so the reschedule sees exactly what was saved.
    let config = read_external_config()?;
    let cards = reschedule_all_cards_unlogged(db, scheduler, &config, at).await?;
    let payload = OptimizeSchedulerPayload {
        parameters: Transition {
            before,
            after: parameters,
        },
        cards,
    };
    let event_ids = insert_events(
        db,
        &[(
            EventType::OptimizeScheduler,
            serde_json::to_value(&payload).unwrap(),
        )],
        at,
        None,
    )
    .await?;
    delete_app_state(db, PROPOSAL_KEY).await?;
    Ok(event_ids[0])
}

/// Whether as many reviews or days have passed since `since` as call for optimizing again. Never
/// optimized, it is once there are as many reviews as optimizing again would wait for.
fn optimize_due(since: Option<FittedAt>, review_count: usize, today: NaiveDate) -> bool {
    match since {
        None => review_count >= OPTIMIZE_AFTER_REVIEWS,
        Some(since) => {
            let reviews = review_count.saturating_sub(since.review_count);
            // Days alone don't call for it when nothing was reviewed since
            reviews >= OPTIMIZE_AFTER_REVIEWS
                || (reviews > 0 && (today - since.date).num_days() >= OPTIMIZE_AFTER_DAYS)
        }
    }
}

/// Whether this host optimizes by itself: `auto_optimize` is on, and it is the root of its sync
/// chain, so that two hosts never do at once.
fn optimizes_by_itself(config: &SparesExternalConfig) -> bool {
    config.auto_optimize && config.sync.url.is_none()
}

/// How many reviews and days have passed since the parameters were last fitted, whether that
/// calls for optimizing again, and any parameters `auto_optimize` fitted and is waiting to have
/// applied. Ask [`auto_optimize_due`] whether to fit them by itself now.
pub async fn optimize_status(
    db: &SqlitePool,
    scheduler_name: &str,
    at: DateTime<Utc>,
) -> Result<OptimizeStatusResponse, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let mut status = OptimizeStatusResponse {
        after_reviews: OPTIMIZE_AFTER_REVIEWS,
        after_days: OPTIMIZE_AFTER_DAYS,
        ..OptimizeStatusResponse::default()
    };
    let Some(review_count) = scheduler.optimizable_review_count(db, &config).await? else {
        return Ok(status);
    };
    let today = study_date(at, config.zone());
    let fitted = fitted_of(&config);
    status.optimizable = true;
    status.review_count = review_count;
    status.fitted_at = fitted.map(|f| f.date);
    status.reviews_since = review_count.saturating_sub(fitted.map_or(0, |f| f.review_count));
    status.days_since = fitted.map(|f| (today - f.date).num_days());
    status.auto_optimize = optimizes_by_itself(&config);
    status.proposal = proposal(db, &config).await?;
    // Optimizing by itself, it suggests nothing it already found no better
    let since = if status.auto_optimize {
        later(fitted, auto_optimized(db).await?)
    } else {
        fitted
    };
    status.suggested = status.proposal.is_none() && optimize_due(since, review_count, today);
    Ok(status)
}

fn later(a: Option<FittedAt>, b: Option<FittedAt>) -> Option<FittedAt> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.date > a.date { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Whether `auto_optimize` should fit the parameters now: this host optimizes by itself, nothing
/// is waiting to be applied, and enough reviews or days have passed since they were last fitted,
/// here or by itself.
pub async fn auto_optimize_due(
    db: &SqlitePool,
    scheduler_name: &str,
    at: DateTime<Utc>,
) -> Result<bool, Error> {
    let config = read_external_config()?;
    if !optimizes_by_itself(&config) {
        return Ok(false);
    }
    let status = optimize_status(db, scheduler_name, at).await?;
    Ok(status.optimizable && status.suggested)
}

/// Fits the parameters by itself, for `auto_optimize`, and keeps them as a proposal if they predict
/// the review history at least [`AUTO_OPTIMIZE_MIN_IMPROVEMENT`] better. Nothing in the settings
/// or the cards changes until the proposal is applied ([`apply_optimize_proposal`]). Returns it.
pub async fn auto_optimize(
    db: &SqlitePool,
    scheduler_name: &str,
    at: DateTime<Utc>,
) -> Result<Option<OptimizeProposal>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let response = scheduler.optimize(db, &config).await?;
    let fitted_at = study_date(at, config.zone());
    set_app_state(
        db,
        AUTO_OPTIMIZED_KEY,
        &serde_json::to_string(&FittedAt {
            date: fitted_at,
            review_count: response.review_count,
        })
        .unwrap(),
    )
    .await?;
    let gain = response.current.log_loss - response.optimized.log_loss;
    if gain < AUTO_OPTIMIZE_MIN_IMPROVEMENT * response.current.log_loss {
        return Ok(None);
    }
    let proposal = OptimizeProposal {
        parameters: response.optimized_parameters,
        current_parameters: config.fsrs_parameters.clone(),
        current: response.current,
        optimized: response.optimized,
        review_count: response.review_count,
        card_count: response.card_count,
        fitted_at,
    };
    set_app_state(db, PROPOSAL_KEY, &serde_json::to_string(&proposal).unwrap()).await?;
    Ok(Some(proposal))
}

/// Saves the parameters `auto_optimize` proposed and reschedules every card with them, as
/// `optimize --apply` would have.
pub async fn apply_optimize_proposal(
    db: &SqlitePool,
    scheduler_name: &str,
    at: DateTime<Utc>,
) -> Result<OptimizeResponse, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let Some(proposal) = proposal(db, &config).await? else {
        return Err(Error::Library(LibraryError::InvalidConfig(
            "There are no proposed parameters to apply: they were applied or discarded, or \
             `fsrs_parameters` changed since they were fitted."
                .to_string(),
        )));
    };
    let event_id = apply_parameters(
        db,
        scheduler.as_ref(),
        SchedulerParameters {
            fsrs_parameters: proposal.parameters.clone(),
            fitted: Some(FittedAt {
                date: proposal.fitted_at,
                review_count: proposal.review_count,
            }),
        },
        at,
    )
    .await?;
    Ok(OptimizeResponse {
        card_count: proposal.card_count,
        review_count: proposal.review_count,
        current_parameters: proposal.current_parameters,
        current: proposal.current,
        optimized_parameters: proposal.parameters,
        optimized: proposal.optimized,
        applied: true,
        event_id: Some(event_id),
    })
}

/// Discards the parameters `auto_optimize` proposed. It fits them again only once as many reviews
/// or days have passed again.
pub async fn discard_optimize_proposal(db: &SqlitePool) -> Result<(), Error> {
    delete_app_state(db, PROPOSAL_KEY).await
}

/// The proposal waiting to be applied, if it was scored against the parameters `config` has.
async fn proposal(
    db: &SqlitePool,
    config: &SparesExternalConfig,
) -> Result<Option<OptimizeProposal>, Error> {
    let Some(value) = app_state(db, PROPOSAL_KEY).await? else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<OptimizeProposal>(&value)
        .ok()
        .filter(|p| p.current_parameters == config.fsrs_parameters))
}

async fn auto_optimized(db: &SqlitePool) -> Result<Option<FittedAt>, Error> {
    Ok(app_state(db, AUTO_OPTIMIZED_KEY)
        .await?
        .and_then(|value| serde_json::from_str(&value).ok()))
}

async fn app_state(db: &SqlitePool, key: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT value FROM app_state WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

async fn set_app_state(db: &SqlitePool, key: &str, value: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO app_state (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}

async fn delete_app_state(db: &SqlitePool, key: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM app_state WHERE key = ?")
        .bind(key)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::Duration;

    use super::*;

    /// `card_count` cards, each reviewed on days 0, 1, 3, 7, 15 and 30, and forgotten far more
    /// often than the default parameters expect.
    pub(crate) async fn seed_review_history(
        pool: &SqlitePool,
        card_count: i64,
        start: DateTime<Utc>,
    ) {
        // Seeding more cards later reuses the parser
        sqlx::query("INSERT OR IGNORE INTO parser (name, usn) VALUES ('markdown', -1)")
            .execute(pool)
            .await
            .unwrap();
        let note_id = crate::ids::insert_test_note(pool, "n", 1).await;
        for order in 1..=card_count {
            let card_id: i64 = sqlx::query_scalar(
                r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
                   desired_retention, state, custom_data, created_at, updated_at, usn, stability_fast)
                   VALUES (?, ?, 1, ?, 5.0, 4.0, 0.9, 2, '{}', strftime('%s', 'now'), strftime('%s', 'now'), -1, 5.0) RETURNING id"#,
            )
            .bind(note_id)
            .bind(order)
            .bind(start.timestamp())
            .fetch_one(pool)
            .await
            .unwrap();
            for (index, day) in (0_i64..).zip([0_i64, 1, 3, 7, 15, 30]) {
                let rating = if (order + index) % 3 == 0 { 3 } else { 1 };
                let previous_state = if index == 0 { 0 } else { 2 };
                sqlx::query(
                    r"INSERT INTO review_log
                        (card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
                         recall_duration, rate_duration, previous_state, custom_data, usn)
                      VALUES (?, ?, 0, ?, 'fsrs', 86400, 5, 2, ?, '{}', -1)",
                )
                .bind(card_id)
                .bind((start + Duration::days(day)).timestamp())
                .bind(rating)
                .bind(previous_state)
                .execute(pool)
                .await
                .unwrap();
            }
        }
    }

    #[sqlx::test]
    async fn optimize_fits_the_review_history(pool: SqlitePool) {
        let start = Utc::now() - Duration::days(60);
        seed_review_history(&pool, 40, start).await;

        let response =
            optimize_scheduler(&pool, "fsrs", OptimizeRequest { apply: false }, Utc::now())
                .await
                .unwrap();
        assert!(!response.applied, "nothing is saved without `apply`");
        assert_eq!(response.card_count, 40);
        assert_eq!(
            response.review_count,
            40 * 5,
            "every review after the first day"
        );
        assert_eq!(response.current_parameters.len(), 34, "FSRS-7's defaults");
        assert_eq!(response.optimized_parameters.len(), 34);
        assert!(response.optimized_parameters.iter().all(|w| w.is_finite()));
        assert!(
            response.optimized.log_loss < response.current.log_loss,
            "parameters fitted to a history this forgetful predict it better: {response:?}"
        );
    }

    #[test]
    fn optimizing_again_is_due_after_500_reviews_or_30_days() {
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        let fitted = Some(FittedAt {
            date: day(1),
            review_count: 1000,
        });
        // Never optimized: once there are as many reviews as it would wait for
        assert!(!optimize_due(None, 499, day(1)));
        assert!(optimize_due(None, 500, day(1)));
        // 500 reviews since, whichever day
        assert!(!optimize_due(fitted, 1499, day(2)));
        assert!(optimize_due(fitted, 1500, day(2)));
        // 30 days since, with a review since
        assert!(!optimize_due(fitted, 1001, day(30)));
        assert!(optimize_due(fitted, 1001, day(1) + chrono::Days::new(30)));
        // ... but not without one
        assert!(!optimize_due(fitted, 1000, day(1) + chrono::Days::new(90)));
        // Fewer reviews than then, e.g. after deleting cards, is none since
        assert!(!optimize_due(fitted, 10, day(2)));
    }

    /// Runs `test` in a collection of its own whose `config.toml` is `config`.
    fn with_config<T>(config: &str, test: impl FnOnce() -> T) -> T {
        let root = tempfile::tempdir().unwrap();
        let paths = crate::collection::CollectionPaths::in_dir(root.path());
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config_file(), config).unwrap();
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let collection =
            std::sync::Arc::new(crate::collection::Collection::new("preview", pool, paths));
        let _guard = collection.enter();
        test()
    }

    fn seconds(response: &IntervalPreviewResponse, example: usize) -> Vec<i64> {
        response.examples[example]
            .intervals
            .iter()
            .map(|i| i.seconds)
            .collect()
    }

    #[tokio::test]
    async fn the_preview_is_what_the_scheduler_does() {
        with_config("learning_steps = [60, 600]\n", || {
            let at = Utc::now();
            let preview = preview_intervals("fsrs", IntervalPreviewRequest::default(), at).unwrap();
            assert_eq!(preview.desired_retention, 0.9);
            assert_eq!(preview.learning_steps, [60, 600]);
            assert_eq!(preview.examples.len(), 2);
            let new = seconds(&preview, 0);
            assert_eq!(new[0], 60, "Again on a new card is its first step");
            assert!(new[3] >= 86_400, "Easy graduates it: {new:?}");
            let seen = seconds(&preview, 1);
            assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");

            // The same as scheduling the card itself
            let config = read_external_config().unwrap();
            let scheduler = get_scheduler_from_string("fsrs").unwrap();
            let (card, logs, now) =
                example_card(scheduler.as_ref(), &config, 0.9, 3, 3, at).unwrap();
            let (next, _) = scheduler
                .schedule(
                    &config,
                    &card,
                    &logs,
                    3,
                    now,
                    Duration::seconds(5),
                    Duration::seconds(2),
                )
                .unwrap();
            assert_eq!(
                seen[2],
                (placed_due(&config, &next, now) - now).num_seconds()
            );
        });
    }

    #[tokio::test]
    async fn the_preview_follows_the_settings_tried() {
        with_config("", || {
            let at = Utc::now();
            let preview = |request| preview_intervals("fsrs", request, at).unwrap();
            let usual = preview(IntervalPreviewRequest::default());
            let strong = preview(IntervalPreviewRequest {
                desired_retention: Some(0.97),
                ..IntervalPreviewRequest::default()
            });
            assert!(
                seconds(&strong, 1)[2] < seconds(&usual, 1)[2],
                "a higher retention reviews sooner"
            );
            let steps = preview(IntervalPreviewRequest {
                learning_steps: Some(vec![300]),
                ..IntervalPreviewRequest::default()
            });
            assert_eq!(seconds(&steps, 0)[0], 300);
            let capped = preview(IntervalPreviewRequest {
                maximum_interval: Some(2 * 86_400),
                ..IntervalPreviewRequest::default()
            });
            assert!(
                seconds(&capped, 1).iter().all(|s| *s <= 2 * 86_400),
                "{capped:?}"
            );
            for bad in [
                IntervalPreviewRequest {
                    desired_retention: Some(0.5),
                    ..Default::default()
                },
                IntervalPreviewRequest {
                    learning_steps: Some(vec![600, 60]),
                    ..Default::default()
                },
                IntervalPreviewRequest {
                    maximum_interval: Some(60),
                    ..Default::default()
                },
            ] {
                assert!(preview_intervals("fsrs", bad, at).is_err());
            }
        });
    }

    #[sqlx::test]
    async fn optimize_without_history_is_an_error(pool: SqlitePool) {
        assert!(
            optimize_scheduler(&pool, "fsrs", OptimizeRequest::default(), Utc::now())
                .await
                .is_err()
        );
    }
}
