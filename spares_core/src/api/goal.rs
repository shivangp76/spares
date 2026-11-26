//! [Goals](crate::config::Goal): sets of cards to know by a date, like an exam's.
//!
//! [`goal_status`] reports how each goal is going, for `spares goal status` and the web app, and
//! [`keep_due_for_goals`] keeps a goal's cards from being due after its date with too little
//! recall left, when one is rated.

pub(crate) mod projection;

use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use itertools::Itertools;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use crate::Error;
use crate::api::statistics::local_time::Zone;
use crate::api::study_pace::capped_answer_seconds;
use crate::config::Goal;
use crate::config::SparesExternalConfig;
use crate::model::Card;
use crate::model::CardId;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::CardMemory;
use crate::schedulers::RetrievabilityFn;
use crate::schedulers::SrsScheduler;
use crate::schedulers::all_schedulers;
use crate::schedulers::state_role;
use crate::schema::goal::GoalMilestoneStatus;
use crate::schema::goal::GoalPartProjection;
use crate::schema::goal::GoalPartStatus;
use crate::schema::goal::GoalPresetUse;
use crate::schema::goal::GoalSearch;
use crate::schema::goal::GoalStatus;
use crate::schema::goal::GoalWeek;
use crate::schema::review::StateRole;

/// The days the pace of [`GoalPartStatus::learned_recently`] is measured over.
pub const PACE_DAYS: i64 = 14;

/// The weeks [`GoalPartStatus::weeks`] goes back.
const WEEKS: i64 = 8;

/// The recall [`GoalPartStatus::below_retention_on_date`] counts against when a goal sets none.
const DEFAULT_RETENTION: f64 = 0.9;

/// The SQL of the ids of the cards of `goal`, or of its part at `part` (an index into
/// [`Goal::parts`]).
fn cards_sql(goal: &Goal, part: Option<usize>) -> String {
    match part.and_then(|index| goal.part_sqls.get(index)) {
        Some(part_sql) => format!(
            "SELECT id FROM card WHERE id IN ({}) AND id IN ({part_sql})",
            goal.query_sql
        ),
        None => goal.query_sql.clone(),
    }
}

/// One card of a goal, for [`part_status`].
#[derive(sqlx::FromRow)]
struct GoalCard {
    state: StateId,
    special_state: Option<u8>,
    stability: f64,
    stability_fast: f64,
    difficulty: f64,
    /// When it was first reviewed (crams left out), if it was
    first_review: Option<i64>,
    /// When it was last reviewed
    last_review: Option<i64>,
    /// When its note was added. A card's own `created_at` is when its row was inserted, which a
    /// sync or an import makes later.
    created_at: i64,
}

/// When a card that isn't suspended was added and first studied, to tell whether it was new on a
/// past day.
#[derive(Clone, Copy)]
struct CardHistory {
    created_at: i64,
    first_review: Option<i64>,
}

impl CardHistory {
    /// Whether the card was new when the day starting at `day_start` started.
    fn new_at(self, day_start: i64) -> bool {
        self.created_at < day_start && self.first_review.is_none_or(|first| first >= day_start)
    }
}

/// The searches of the goals of `config` and of their parts, in the order of `config.toml`.
pub fn goal_searches(config: &SparesExternalConfig) -> Vec<GoalSearch> {
    config
        .goals
        .iter()
        .map(|(name, goal)| GoalSearch {
            name: name.clone(),
            query: goal.query.clone(),
            parts: goal
                .parts
                .keys()
                .map(|part| (part.clone(), goal.part_query(Some(part))))
                .collect(),
        })
        .collect()
}

/// How every goal of `config` is going, as of `now`, in the order of `config.toml`.
pub async fn goal_status(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    now: DateTime<Utc>,
) -> Result<Vec<GoalStatus>, Error> {
    let zone = config.zone();
    let today = zone.date(now);
    // Cards are scheduled by the default scheduler unless rated otherwise, and only it has a
    // forgetting curve today.
    let recall = all_schedulers()
        .first()
        .and_then(|scheduler| scheduler.retrievability(config));
    let mut statuses = Vec::new();
    for (name, goal) in &config.goals {
        let names = std::iter::once(name).chain(goal.parts.keys());
        let mut parts = Vec::new();
        let mut histories = Vec::new();
        for (index, part_name) in names.enumerate() {
            let sql = cards_sql(goal, index.checked_sub(1));
            let part = (index > 0).then_some(part_name.as_str());
            let (part, history) = part_status(
                db,
                config,
                goal,
                part_name,
                part,
                &sql,
                now,
                recall.as_ref(),
            )
            .await?;
            parts.push(part);
            histories.push(history);
        }
        add_milestones(goal, &mut parts, &histories, zone, today);
        let (presets, minutes_per_day) = add_projection(db, config, goal, &mut parts, now).await?;
        statuses.push(GoalStatus {
            name: name.clone(),
            date: goal.date,
            learned_by: goal.learned_by(),
            retention: goal.retention,
            days_to_learned_by: (goal.learned_by() - today).num_days().max(0),
            days_to_date: (goal.date - today).num_days().max(0),
            parts,
            presets,
            minutes_per_day,
        });
    }
    Ok(statuses)
}

/// [`GoalPartStatus`] of the cards `cards_sql` selects, named `name`, of `goal` or of its part
/// `part`, without its milestones, and the history of those of its cards that aren't suspended.
#[expect(
    clippy::too_many_arguments,
    reason = "the part and what it is measured against"
)]
async fn part_status(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    goal: &Goal,
    name: &str,
    part: Option<&str>,
    cards_sql: &str,
    now: DateTime<Utc>,
    recall: Option<&RetrievabilityFn>,
) -> Result<(GoalPartStatus, Vec<CardHistory>), Error> {
    let review = ReviewLogKind::Review as u8;
    let cards: Vec<GoalCard> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT c.state, c.special_state, c.stability, c.stability_fast, c.difficulty, \
         (SELECT n.created_at FROM note n WHERE n.id = c.note_id) AS created_at, \
         (SELECT MIN(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = {review}) AS first_review, \
         (SELECT MAX(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = {review}) AS last_review \
         FROM card c WHERE c.id IN ({cards_sql})"
    )))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    let zone = config.zone();
    let today = zone.date(now);
    let suspended = SpecialState::Suspended as u8;
    let mut status = GoalPartStatus {
        name: name.to_string(),
        query: goal.part_query(part),
        cards: 0,
        new: 0,
        learning: 0,
        review: 0,
        suspended: 0,
        learned_recently: 0,
        new_per_day_needed: None,
        projected_learned_by: None,
        mean_recall_on_date: None,
        below_retention_on_date: 0,
        weeks: Vec::new(),
        milestones: Vec::new(),
        projection: None,
    };
    let mut history = Vec::new();
    let pace_start = (now - Duration::days(PACE_DAYS)).timestamp();
    let on_date = zone.start_of(goal.date).timestamp();
    let retention = goal.retention.unwrap_or(DEFAULT_RETENTION);
    let mut recall_sum = 0.0;
    let mut recall_count = 0_u32;
    for card in &cards {
        status.cards += 1;
        if card.first_review.is_some_and(|first| first >= pace_start) {
            status.learned_recently += 1;
        }
        if card.special_state == Some(suspended) {
            status.suspended += 1;
            continue;
        }
        history.push(CardHistory {
            created_at: card.created_at,
            first_review: card.first_review,
        });
        match state_role(card.state) {
            Some(StateRole::New) | None => status.new += 1,
            Some(StateRole::Learning | StateRole::Relearning) => status.learning += 1,
            Some(StateRole::Review) => status.review += 1,
        }
        if let (Some(recall), Some(last_review)) = (recall, card.last_review)
            && !matches!(state_role(card.state), Some(StateRole::New) | None)
        {
            #[expect(
                clippy::cast_precision_loss,
                reason = "seconds between reviews fit in f64"
            )]
            let days = (on_date - last_review) as f64 / 86400.0;
            let memory = CardMemory {
                stability: card.stability,
                stability_fast: card.stability_fast,
                difficulty: card.difficulty,
            };
            let predicted = recall(memory, days.max(0.0));
            recall_sum += predicted;
            recall_count += 1;
            if predicted < retention {
                status.below_retention_on_date += 1;
            }
        }
    }
    if recall_count > 0 {
        status.mean_recall_on_date = Some(recall_sum / f64::from(recall_count));
    }
    let days_left = (goal.learned_by() - today).num_days();
    if days_left > 0 {
        #[expect(clippy::cast_precision_loss, reason = "a number of days")]
        let per_day = f64::from(status.new) / days_left as f64;
        status.new_per_day_needed = Some(per_day);
    }
    if status.new > 0 && status.learned_recently > 0 {
        #[expect(clippy::cast_precision_loss, reason = "a number of days")]
        let pace = f64::from(status.learned_recently) / PACE_DAYS as f64;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "days, rounded up, of at most the number of cards over the pace"
        )]
        let days = (f64::from(status.new) / pace).ceil() as i64;
        status.projected_learned_by = Some(today + Duration::days(days));
    }
    status.weeks = weeks(db, config, cards_sql, &cards, today).await?;
    Ok((status, history))
}

/// Fills in [`GoalPartStatus::milestones`] of `parts`, the whole goal first, whose cards that
/// aren't suspended are `histories`, in the same order.
fn add_milestones(
    goal: &Goal,
    parts: &mut [GoalPartStatus],
    histories: &[Vec<CardHistory>],
    zone: Zone,
    today: NaiveDate,
) {
    let goal_new_now = parts.first().map_or(0, |goal| goal.new);
    for milestone in &goal.milestones {
        let day_start = zone.start_of(milestone.date).timestamp();
        let new_on_day = |history: &[CardHistory]| {
            let new = history.iter().filter(|card| card.new_at(day_start)).count();
            u32::try_from(new).unwrap_or(u32::MAX)
        };
        let passed = milestone.date <= today;
        let goal_new_on_day = histories.first().map_or(0, |history| new_on_day(history));
        for (part, history) in parts.iter_mut().zip(histories) {
            let status = if passed {
                let new = new_on_day(history);
                let target = share(milestone.new_left, new, goal_new_on_day);
                GoalMilestoneStatus {
                    date: milestone.date,
                    new_left: target,
                    days_left: 0,
                    new_per_day_needed: None,
                    on_track: None,
                    new_on_day: Some(new),
                    met: Some(new <= target),
                    projected_new_left: None,
                }
            } else {
                let target = share(milestone.new_left, part.new, goal_new_now);
                let days_left = (milestone.date - today).num_days();
                let to_learn = f64::from(part.new.saturating_sub(target));
                #[expect(clippy::cast_precision_loss, reason = "a number of days")]
                let (days, pace_days) = (days_left as f64, PACE_DAYS as f64);
                let pace = f64::from(part.learned_recently) / pace_days;
                GoalMilestoneStatus {
                    date: milestone.date,
                    new_left: target,
                    days_left,
                    new_per_day_needed: Some(to_learn / days),
                    on_track: Some(pace * days >= to_learn),
                    new_on_day: None,
                    met: None,
                    projected_new_left: None,
                }
            };
            part.milestones.push(status);
        }
    }
}

/// Projects `goal` (see [`projection`]) and fills in [`GoalPartStatus::projection`] and the
/// milestones' [`projected_new_left`](GoalMilestoneStatus::projected_new_left) of `parts`, the
/// whole goal first. Returns [`GoalStatus::presets`] and [`GoalStatus::minutes_per_day`].
async fn add_projection(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    goal: &Goal,
    parts: &mut [GoalPartStatus],
    now: DateTime<Utc>,
) -> Result<(Vec<GoalPresetUse>, Option<f64>), Error> {
    let today = config.zone().date(now);
    let days_until = |date: NaiveDate| u32::try_from((date - today).num_days()).unwrap_or(0);
    let input =
        projection::load(db, config, &goal.query_sql, days_until(goal.learned_by())).await?;
    let output = projection::simulate(&input);
    for (index, part) in parts.iter_mut().enumerate() {
        let sql = cards_sql(goal, index.checked_sub(1));
        let ids: std::collections::HashSet<CardId> = sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
            .into_iter()
            .collect();
        // The part's new cards, by index in the projection
        let new = input
            .cards
            .iter()
            .enumerate()
            .filter(|(_, card)| card.start == projection::Start::New && ids.contains(&card.id))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        // In each run, the cards still new when day `day` starts
        let new_left = |day: u32| {
            let counts = output.studied_on.iter().map(|studied| {
                let left = new
                    .iter()
                    .filter(|&&card| studied[card].is_none_or(|on| on >= day))
                    .count();
                u32::try_from(left).unwrap_or(u32::MAX)
            });
            let (fewest, most) = counts.minmax().into_option().unwrap_or((0, 0));
            [fewest, most]
        };
        // In each run, the day the last new card is studied, if all are
        let learned_on = output
            .studied_on
            .iter()
            .map(|studied| {
                new.iter()
                    .map(|&card| studied[card])
                    .collect::<Option<Vec<_>>>()
                    .and_then(|days| days.into_iter().max())
            })
            .collect::<Vec<_>>();
        let date = |day: Option<u32>| day.map(|day| today + Duration::days(i64::from(day)));
        let learned_latest = if learned_on.iter().all(Option::is_some) {
            date(learned_on.iter().flatten().max().copied())
        } else {
            None
        };
        part.projection = Some(GoalPartProjection {
            learned_earliest: date(learned_on.iter().flatten().min().copied()),
            learned_latest,
            new_left_at_learned_by: new_left(days_until(goal.learned_by())),
        });
        for milestone in &mut part.milestones {
            if milestone.new_on_day.is_none() {
                milestone.projected_new_left = Some(new_left(days_until(milestone.date)));
            }
        }
    }
    let minutes = estimate_minutes(db, config, &goal.query_sql, &input, &output, today).await?;
    let presets = input
        .presets
        .iter()
        .zip(&output.presets)
        .enumerate()
        .filter_map(|(index, (preset, used))| {
            let new_limit = preset.limit?;
            let new = input
                .cards
                .iter()
                .filter(|card| card.preset == Some(index) && card.start == projection::Start::New)
                .count();
            if new == 0 {
                return None;
            }
            #[expect(clippy::cast_precision_loss, reason = "counts of cards")]
            let lost_to_links = if used.slots == 0 {
                0.0
            } else {
                // Both are summed over the runs, so this is their average.
                used.lost_to_links as f64 / used.slots as f64
            };
            Some(GoalPresetUse {
                name: preset.name.clone(),
                new_limit,
                new: u32::try_from(new).unwrap_or(u32::MAX),
                lost_to_links,
            })
        })
        .collect();
    Ok((presets, minutes))
}

/// The days [`GoalStatus::minutes_per_day`] averages over.
const ESTIMATE_DAYS: u32 = 7;

/// [`GoalStatus::minutes_per_day`] of the goal whose cards `cards_sql` selects, projected as
/// `output` from `input`.
async fn estimate_minutes(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    cards_sql: &str,
    input: &projection::SimInput,
    output: &projection::SimOutput,
    today: NaiveDate,
) -> Result<Option<f64>, Error> {
    let error = |source| Error::Sqlx { source };
    let reviewed: bool = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM review_log WHERE kind = {})",
        ReviewLogKind::Review as u8
    )))
    .fetch_one(db)
    .await
    .map_err(error)?;
    if !reviewed {
        return Ok(None);
    }
    let zone = config.zone();
    let presets = &config.queue.presets;
    let cards: Vec<(CardId, Option<i64>, StateId, i64)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT c.id, {} AS preset, c.state, c.due FROM card c WHERE c.id IN ({cards_sql}) \
         AND (c.special_state IS NULL OR c.special_state IN ({}, {}))",
        crate::api::preset::preset_sql(presets),
        SpecialState::SchedulerBuried as u8,
        SpecialState::BuriedUntilLaterToday as u8,
    )))
    .fetch_all(db)
    .await
    .map_err(error)?;
    let week_end = zone
        .start_of(today + Duration::days(i64::from(ESTIMATE_DAYS)))
        .timestamp();
    let index = input
        .cards
        .iter()
        .enumerate()
        .map(|(index, card)| (card.id, index))
        .collect::<std::collections::HashMap<_, _>>();
    #[expect(clippy::cast_precision_loss, reason = "a few runs")]
    let runs = output.studied_on.len().max(1) as f64;
    // Each card studied in the week, and how many of the runs study it
    let mut studied = Vec::new();
    for (id, preset, state, due) in cards {
        let preset = crate::api::preset::preset_index(preset);
        let weight = match state_role(state) {
            Some(StateRole::New) | None => {
                let Some(&index) = index.get(&id) else {
                    continue;
                };
                let runs_studying = output
                    .studied_on
                    .iter()
                    .filter(|run| run[index].is_some_and(|day| day < ESTIMATE_DAYS))
                    .count();
                #[expect(clippy::cast_precision_loss, reason = "a few runs")]
                let share = runs_studying as f64 / runs;
                share
            }
            Some(_) if due < week_end => 1.0,
            Some(_) => continue,
        };
        if weight > 0.0 {
            studied.push((preset, state, weight));
        }
    }
    let needed = studied
        .iter()
        .map(|(preset, state, _)| (*preset, *state))
        .collect();
    let paces = crate::api::study_pace::StudyPaces::load(
        db,
        &crate::api::preset::cards_with_preset(presets),
        zone,
        config.maximum_answer_seconds(),
        &needed,
    )
    .await?;
    let seconds: f64 = studied
        .iter()
        .map(|(preset, state, weight)| {
            let pace = paces.of(*preset, *state);
            pace.seconds_per_review * pace.reviews_per_day * weight
        })
        .sum();
    Ok(Some(seconds / f64::from(ESTIMATE_DAYS) / 60.0))
}

/// A part's share of a target of `new_left` new cards for the whole goal, in proportion to its
/// `part_new` of the goal's `goal_new` new cards. The whole goal's is `new_left`.
fn share(new_left: u32, part_new: u32, goal_new: u32) -> u32 {
    if goal_new == 0 || part_new >= goal_new {
        return new_left;
    }
    let share = f64::from(new_left) * f64::from(part_new) / f64::from(goal_new);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "at most new_left"
    )]
    let share = share.round() as u32;
    share
}

/// The last [`WEEKS`] weeks of `cards`, which `cards_sql` selects, ending today, oldest first.
async fn weeks(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    cards_sql: &str,
    cards: &[GoalCard],
    today: NaiveDate,
) -> Result<Vec<GoalWeek>, Error> {
    let zone = config.zone();
    let starts = (0..WEEKS)
        .rev()
        .map(|weeks_ago| today - Duration::days(7 * weeks_ago + 6))
        .collect::<Vec<_>>();
    let bounds = starts
        .iter()
        .map(|start| {
            (
                zone.start_of(*start).timestamp(),
                zone.start_of(*start + Duration::days(7)).timestamp(),
            )
        })
        .collect::<Vec<_>>();
    let first_start = bounds.first().map_or(0, |(start, _)| *start);
    // Crams take time too, so count toward the minutes, but aren't reviews.
    let logs: Vec<(i64, u8, i64)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT rl.reviewed_at, rl.kind, {seconds} FROM review_log rl \
         WHERE rl.kind IN ({review}, {cram}) AND rl.reviewed_at >= {first_start} \
         AND rl.card_id IN ({cards_sql})",
        seconds = capped_answer_seconds("rl", config.maximum_answer_seconds()),
        review = ReviewLogKind::Review as u8,
        cram = ReviewLogKind::Cram as u8,
    )))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let reviews = logs
        .iter()
        .filter(|(_, kind, _)| *kind == ReviewLogKind::Review as u8)
        .map(|(at, _, _)| *at)
        .collect::<Vec<_>>();
    let count =
        |values: &mut dyn Iterator<Item = i64>| u32::try_from(values.count()).unwrap_or(u32::MAX);
    Ok(starts
        .into_iter()
        .zip(bounds)
        .map(|(start, (from, to))| GoalWeek {
            start,
            learned: count(
                &mut cards
                    .iter()
                    .filter_map(|card| card.first_review)
                    .filter(|first| *first < to),
            ),
            reviews: count(&mut reviews.iter().copied().filter(|at| (from..to).contains(at))),
            #[expect(clippy::cast_precision_loss, reason = "seconds of study")]
            minutes: logs
                .iter()
                .filter(|(at, _, _)| (from..to).contains(at))
                // From 0, as an empty `sum` of floats is -0.
                .fold(0.0, |minutes, (_, _, seconds)| {
                    minutes + *seconds as f64 / 60.0
                }),
        })
        .collect())
}

/// Whether card `card_id` is one of the cards `cards_sql` selects.
async fn is_among(
    db: &mut sqlx::SqliteConnection,
    card_id: CardId,
    cards_sql: &str,
) -> Result<bool, Error> {
    sqlx::query_scalar(AssertSqlSafe(format!("SELECT ? IN ({cards_sql})")))
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

/// The due date of `card`, just rated at `at` by `scheduler` and placed at `card.due`, kept within
/// the goals it belongs to, as [`Goal::retention`] describes.
///
/// For each goal with a retention whose day is still to come, if `card.due` is after the day and
/// the card's recall on the day would be below the retention, it is instead due when its recall
/// falls to the retention, though at least a day after `at`. Several goals give the earliest of
/// their due dates.
pub(crate) async fn keep_due_for_goals(
    db: &mut sqlx::SqliteConnection,
    config: &SparesExternalConfig,
    scheduler: &dyn SrsScheduler,
    card: &Card,
    at: DateTime<Utc>,
) -> Result<DateTime<Utc>, Error> {
    let mut due = card.due;
    if config.goals.values().all(|goal| goal.retention.is_none()) {
        return Ok(due);
    }
    let Some(recall) = scheduler.retrievability(config) else {
        return Ok(due);
    };
    let zone = config.zone();
    for goal in config.goals.values() {
        let Some(retention) = goal.retention else {
            continue;
        };
        let day = zone.start_of(goal.date);
        if at >= day || card.due <= day {
            continue;
        }
        #[expect(clippy::cast_precision_loss, reason = "seconds until a date")]
        let days_to_day = (day - at).num_seconds() as f64 / 86400.0;
        if recall(CardMemory::from(card), days_to_day) >= retention
            || !is_among(&mut *db, card.id, &goal.query_sql).await?
        {
            continue;
        }
        // Recall falls as days pass, so the last day it is still at the retention is found by
        // halving the days to the goal's day.
        let (mut kept, mut lost) = (0.0, days_to_day);
        for _ in 0..40 {
            let middle = f64::midpoint(kept, lost);
            if recall(CardMemory::from(card), middle) >= retention {
                kept = middle;
            } else {
                lost = middle;
            }
        }
        #[expect(clippy::cast_possible_truncation, reason = "seconds before a date")]
        let latest = at + Duration::seconds((kept * 86400.0) as i64);
        due = due.min(latest.max(at + Duration::days(1)).min(day));
    }
    Ok(due)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::Utc;
    use sqlx::SqlitePool;

    use super::*;
    use crate::api::queue::preset_tests::parse;
    use crate::api::queue::preset_tests::tagged_cards;
    use crate::api::queue::tests::NEW;
    use crate::api::queue::tests::REVIEW;
    use crate::api::queue::tests::add_review;
    use crate::api::queue::tests::set_card;
    use crate::schedulers::fsrs::Fsrs;

    fn config(date: NaiveDate, retention: &str) -> SparesExternalConfig {
        parse(&format!(
            "[goals.exam]\ndate = \"{date}\"\nquery = 'tag=exam'\n{retention}\n\
             [goals.exam.parts]\nprob = 'tag=\"exam:prob\"'\nspdes = 'tag=\"exam:spdes\"'\n"
        ))
    }

    async fn card(pool: &SqlitePool, id: CardId) -> Card {
        sqlx::query_as("SELECT * FROM card WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The counts, pace and weeks of a goal and its parts.
    #[sqlx::test]
    async fn reports_each_part(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[
                &["exam:prob"],
                &["exam:prob"],
                &["exam:prob"],
                &["exam:spdes"],
                &["exam:spdes"],
                &["other"],
            ],
            now - Duration::days(60),
        )
        .await;
        // Prob's first card was learned 30 days ago, its second 3 days ago, and one SPDE card
        // yesterday. The rest are new.
        for (card, days_ago) in [(ids[0], 30), (ids[1], 3), (ids[3], 1), (ids[5], 2)] {
            add_review(&pool, card, now - Duration::days(days_ago), NEW).await;
            set_card(&pool, card, REVIEW, now + Duration::days(5), 10.0, 5.0).await;
        }
        let date = SparesExternalConfig::default().zone().date(now) + Duration::days(20);
        let statuses = goal_status(&pool, &config(date, ""), now).await.unwrap();
        let [status] = statuses.as_slice() else {
            panic!("one goal: {statuses:?}")
        };
        assert_eq!(status.days_to_date, 20);
        assert_eq!(status.learned_by, date);
        let summary = status
            .parts
            .iter()
            .map(|part| {
                (
                    part.name.as_str(),
                    part.cards,
                    part.new,
                    part.review,
                    part.learned_recently,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            [
                ("exam", 5, 2, 3, 2),
                ("prob", 3, 1, 2, 1),
                ("spdes", 2, 1, 1, 1)
            ]
        );
        let all = &status.parts[0];
        assert_eq!(all.new_per_day_needed, Some(2.0 / 20.0));
        // 2 cards in 14 days is a card a week: the 2 left take 14 days.
        assert_eq!(
            all.projected_learned_by,
            Some(SparesExternalConfig::default().zone().date(now) + Duration::days(14))
        );
        let recall = all.mean_recall_on_date.unwrap();
        assert!(recall > 0.0 && recall < 1.0, "{recall}");
        assert_eq!(all.weeks.len(), 8);
        let learned = all
            .weeks
            .iter()
            .map(|week| week.learned)
            .collect::<Vec<_>>();
        assert_eq!(learned, [0, 0, 0, 1, 1, 1, 1, 3]);
        let reviews = all.weeks.iter().map(|week| week.reviews).sum::<u32>();
        assert_eq!(reviews, 3);
    }

    /// Milestones: the pace each needs and whether the recent pace reaches it, each part's share of
    /// the target, and whether the past ones were met.
    #[sqlx::test]
    async fn reports_milestones(pool: SqlitePool) {
        let now = Utc::now();
        let tags: [&[&str]; 10] = [
            &["exam:prob"],
            &["exam:prob"],
            &["exam:prob"],
            &["exam:prob"],
            &["exam:spdes"],
            &["exam:spdes"],
            &["exam:spdes"],
            &["exam:spdes"],
            &["exam:spdes"],
            &["exam:spdes"],
        ];
        let ids = tagged_cards(&pool, &tags, now - Duration::days(60)).await;
        // A prob card learned 20 days ago, two SPDE cards 3 days ago: 3 prob and 4 SPDE cards
        // are new, and the pace is 2 cards in 14 days.
        for (card, days_ago) in [(ids[0], 20), (ids[4], 3), (ids[5], 3)] {
            add_review(&pool, card, now - Duration::days(days_ago), NEW).await;
            set_card(&pool, card, REVIEW, now + Duration::days(5), 10.0, 5.0).await;
        }
        let today = SparesExternalConfig::default().zone().date(now);
        let day = |days: i64| today + Duration::days(days);
        let milestones = format!(
            "milestones = [{{ date = {}, new_left = 9 }}, {{ date = {}, new_left = 6 }}, \
             {{ date = {}, new_left = 2 }}]",
            day(-10),
            day(-1),
            day(10)
        );
        let statuses = goal_status(&pool, &config(day(60), &milestones), now)
            .await
            .unwrap();
        let summary = |part: &GoalPartStatus| {
            part.milestones
                .iter()
                .map(|m| (m.new_left, m.days_left, m.new_on_day, m.met, m.on_track))
                .collect::<Vec<_>>()
        };
        let [exam, prob, spdes] = statuses[0].parts.as_slice() else {
            panic!("the goal and 2 parts: {statuses:?}")
        };
        assert_eq!(exam.query, "tag=exam");
        let searches = goal_searches(&config(day(60), ""));
        assert_eq!(searches[0].query, "tag=exam");
        assert_eq!(searches[0].parts[1].0, "spdes");
        assert_eq!(searches[0].parts[1].1, spdes.query);
        assert_eq!(prob.query, "(tag=exam) (tag=\"exam:prob\")");
        // 10 days ago, 9 were new (the prob card was learned before): met. Yesterday 7 were new,
        // more than 6: missed. 7 new now to 2 in 10 days needs 0.5 a day, and 2 a fortnight
        // learns only 1.4 in 10 days.
        assert_eq!(
            summary(exam),
            [
                (9, 0, Some(9), Some(true), None),
                (6, 0, Some(7), Some(false), None),
                (2, 10, None, None, Some(false)),
            ]
        );
        assert_eq!(exam.milestones[2].new_per_day_needed, Some(0.5));
        assert_eq!(exam.milestones[0].new_per_day_needed, None);
        // Each part gets a share of the target in proportion to its new cards: on the day for past
        // milestones (3 of 9, then 3 of 7, rounded), now for the next one (3 and 4 of 7).
        assert_eq!(
            summary(prob),
            [
                (3, 0, Some(3), Some(true), None),
                (3, 0, Some(3), Some(true), None),
                (1, 10, None, None, Some(false)),
            ]
        );
        assert_eq!(
            summary(spdes),
            [
                (6, 0, Some(6), Some(true), None),
                (3, 0, Some(4), Some(false), None),
                (1, 10, None, None, Some(false)),
            ]
        );
        assert_eq!(prob.milestones[2].new_per_day_needed, Some(0.2));
        assert_eq!(spdes.milestones[2].new_per_day_needed, Some(0.3));

        // 1 card in 30 days is within the pace, and a target already reached needs nothing.
        let statuses = goal_status(
            &pool,
            &config(
                day(60),
                &format!("milestones = [{{ date = {}, new_left = 6 }}]", day(30)),
            ),
            now,
        )
        .await
        .unwrap();
        let [exam, prob, _] = statuses[0].parts.as_slice() else {
            panic!("the goal and 2 parts: {statuses:?}")
        };
        assert_eq!(exam.milestones[0].on_track, Some(true));
        assert_eq!(prob.milestones[0].new_left, 3);
        assert_eq!(prob.milestones[0].new_per_day_needed, Some(0.0));
        assert_eq!(prob.milestones[0].on_track, Some(true));
    }

    /// Minutes by week, crams included, and the estimate of the minutes a day, which needs reviews
    /// to go on.
    #[sqlx::test]
    async fn reports_minutes(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[&["exam:prob"], &["exam:prob"], &["exam:spdes"]],
            now - Duration::days(30),
        )
        .await;
        let today = SparesExternalConfig::default().zone().date(now);
        let config = config(today + Duration::days(30), "");
        let statuses = goal_status(&pool, &config, now).await.unwrap();
        assert_eq!(statuses[0].minutes_per_day, None, "no reviews to go on");
        assert!(
            statuses[0].parts[0]
                .weeks
                .iter()
                .all(|week| week.minutes == 0.0)
        );

        // 60 and 120 seconds of reviews and a 30-second cram 2 days ago, and 6 minutes 17 days ago
        let log = |card: CardId, days_ago: i64, kind: ReviewLogKind, recall: i64, rate: i64| {
            sqlx::query(
                r"INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name,
                scheduled_time, recall_duration, rate_duration, previous_state, custom_data, usn)
                VALUES (?, ?, ?, 3, 'fsrs', 0, ?, ?, ?, '{}', -1)",
            )
            .bind(card)
            .bind((now - Duration::days(days_ago)).timestamp())
            .bind(kind as u8)
            .bind(recall)
            .bind(rate)
            .bind(NEW)
            .execute(&pool)
        };
        log(ids[0], 2, ReviewLogKind::Review, 50, 10).await.unwrap();
        log(ids[1], 2, ReviewLogKind::Review, 120, 0).await.unwrap();
        log(ids[0], 2, ReviewLogKind::Cram, 30, 0).await.unwrap();
        log(ids[2], 17, ReviewLogKind::Review, 300, 60)
            .await
            .unwrap();
        for id in &ids {
            set_card(&pool, *id, REVIEW, now + Duration::days(3), 10.0, 5.0).await;
        }
        let statuses = goal_status(&pool, &config, now).await.unwrap();
        let minutes = |part: &GoalPartStatus| {
            part.weeks
                .iter()
                .map(|week| (week.minutes * 10.0).round() / 10.0)
                .collect::<Vec<_>>()
        };
        let [exam, prob, spdes] = statuses[0].parts.as_slice() else {
            panic!("the goal and 2 parts")
        };
        assert_eq!(minutes(exam), [0.0, 0.0, 0.0, 0.0, 0.0, 6.0, 0.0, 3.5]);
        assert_eq!(minutes(prob), [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 3.5]);
        assert_eq!(minutes(spdes), [0.0, 0.0, 0.0, 0.0, 0.0, 6.0, 0.0, 0.0]);
        // The crams aren't reviews.
        assert_eq!(exam.weeks[7].reviews, 2);
        // Each card is due this week, at the pace of reviews so far.
        let estimate = statuses[0].minutes_per_day.unwrap();
        assert!(estimate > 0.0, "{estimate}");
    }

    /// The projection: a link between the goal's notes holds the newer one back a day, and a link
    /// to a note outside the goal holds nothing back.
    #[sqlx::test]
    async fn projects_with_links_inside_the_goal_only(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[&["other"], &["exam:prob"], &["exam:prob"]],
            now - Duration::days(1),
        )
        .await;
        let link = |from: CardId, to: CardId| {
            sqlx::query(
                r#"INSERT INTO note_link
                    (parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score, relation)
                VALUES ((SELECT note_id FROM card WHERE id = ?), (SELECT note_id FROM card WHERE id = ?),
                    0, 'keyword', 'keyword', 0, 0)"#,
            )
            .bind(from)
            .bind(to)
            .execute(&pool)
        };
        // The first exam note links to the note outside the goal, which is new.
        link(ids[1], ids[0]).await.unwrap();
        let today = SparesExternalConfig::default().zone().date(now);
        let date = today + Duration::days(30);
        let config = |milestone: &str| {
            parse(&format!(
                "[queue]\nnew_after_linked_notes = true\n\
                 [queue.presets.exam]\ntags = [\"exam\"]\nnew_cards_daily_limit = 5\n\
                 [goals.exam]\ndate = \"{date}\"\nquery = 'tag=exam'\n{milestone}"
            ))
        };
        let projection = |statuses: &[GoalStatus]| statuses[0].parts[0].projection.clone().unwrap();
        let statuses = goal_status(&pool, &config(""), now).await.unwrap();
        assert_eq!(projection(&statuses).learned_latest, Some(today));
        assert_eq!(projection(&statuses).new_left_at_learned_by, [0, 0]);
        let [preset] = statuses[0].presets.as_slice() else {
            panic!("the exam preset: {:?}", statuses[0].presets)
        };
        assert_eq!(
            (preset.name.as_str(), preset.new_limit, preset.new),
            ("exam", 5, 2)
        );
        assert!(preset.lost_to_links.abs() < f64::EPSILON);

        // The second exam note links to the first, so waits a day.
        link(ids[2], ids[1]).await.unwrap();
        let milestone = format!(
            "milestones = [{{ date = {}, new_left = 0 }}]",
            today + Duration::days(1)
        );
        let statuses = goal_status(&pool, &config(&milestone), now).await.unwrap();
        let projected = projection(&statuses);
        assert_eq!(projected.learned_earliest, Some(today + Duration::days(1)));
        assert_eq!(projected.learned_latest, Some(today + Duration::days(1)));
        // When tomorrow starts, the second card is still new.
        assert_eq!(
            statuses[0].parts[0].milestones[0].projected_new_left,
            Some([1, 1])
        );
        // Of 10 slots over the 2 days with new cards, today's 4 free ones lose the card waiting on
        // its link.
        let lost = statuses[0].presets[0].lost_to_links;
        assert!((lost - 0.1).abs() < 1e-9, "{lost}");
    }

    /// A card whose next review is after the goal's day, with too little recall left on the day,
    /// is due when its recall falls to the retention instead.
    #[sqlx::test]
    async fn keeps_goal_cards_due_before_the_day(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(&pool, &[&["exam:prob"], &["other"]], now).await;
        let config_now = SparesExternalConfig::default();
        let date = config_now.zone().date(now) + Duration::days(30);
        let config = config(date, "retention = 0.95");
        let day = config.zone().start_of(date);
        let scheduler = Fsrs;
        let recall = scheduler.retrievability(&config).unwrap();
        // Due 60 days on with a stability of 20, recall on the day would be under 0.95.
        for id in &ids {
            set_card(&pool, *id, REVIEW, now + Duration::days(60), 20.0, 5.0).await;
        }
        let exam_card = card(&pool, ids[0]).await;
        let due = keep_due_for_goals(
            &mut pool.acquire().await.unwrap(),
            &config,
            &scheduler,
            &exam_card,
            now,
        )
        .await
        .unwrap();
        assert!(due < day, "{due} should be before {day}");
        #[expect(clippy::cast_precision_loss, reason = "a test's seconds")]
        let days = (due - now).num_seconds() as f64 / 86400.0;
        let memory = CardMemory::from(&card(&pool, ids[0]).await);
        assert!((recall(memory, days) - 0.95).abs() < 0.001, "{days}");
        // A card of no goal, or of a goal without retention, keeps its due date.
        let other = card(&pool, ids[1]).await;
        assert_eq!(
            keep_due_for_goals(
                &mut pool.acquire().await.unwrap(),
                &config,
                &scheduler,
                &other,
                now
            )
            .await
            .unwrap(),
            other.due
        );
        let without = super::tests::config(date, "");
        assert_eq!(
            keep_due_for_goals(
                &mut pool.acquire().await.unwrap(),
                &without,
                &scheduler,
                &exam_card,
                now
            )
            .await
            .unwrap(),
            exam_card.due
        );
        // Strong enough to keep 0.95 for 30 days: left alone.
        set_card(
            &pool,
            ids[0],
            REVIEW,
            now + Duration::days(400),
            3000.0,
            5.0,
        )
        .await;
        let strong = card(&pool, ids[0]).await;
        assert_eq!(
            keep_due_for_goals(
                &mut pool.acquire().await.unwrap(),
                &config,
                &scheduler,
                &strong,
                now
            )
            .await
            .unwrap(),
            strong.due
        );
        // Once the day has come, nothing changes.
        assert_eq!(
            keep_due_for_goals(
                &mut pool.acquire().await.unwrap(),
                &config,
                &scheduler,
                &exam_card,
                day
            )
            .await
            .unwrap(),
            exam_card.due
        );
    }

    /// A TOML date works as well as a quoted one.
    #[test]
    fn dates_can_be_toml_dates() {
        let config = parse("[goals.g]\ndate = 2027-05-01\nlearned_by = 2027-04-01\nquery = 'a'\n");
        assert_eq!(
            config.goals["g"].date,
            NaiveDate::from_ymd_opt(2027, 5, 1).unwrap()
        );
        assert_eq!(
            config.goals["g"].learned_by(),
            NaiveDate::from_ymd_opt(2027, 4, 1).unwrap()
        );
    }

    #[test]
    fn invalid_goals_are_rejected() {
        let invalid = |toml: &str| {
            let mut config: SparesExternalConfig = toml_edit::de::from_str(toml).unwrap();
            config.validate().unwrap_err()
        };
        let goal = "[goals.g]\ndate = \"2027-05-01\"\n";
        assert!(invalid(&format!("{goal}query = ''")).contains("goals.g.query"));
        assert!(invalid(&format!("{goal}query = 'tag=a limit=2'")).contains("goals.g.query"));
        assert!(
            invalid(&format!("{goal}query = 'a'\nlearned_by = \"2027-06-01\""))
                .contains("learned_by")
        );
        assert!(invalid(&format!("{goal}query = 'a'\nretention = 1.5")).contains("retention"));
        let milestones = |milestones: &str| {
            invalid(&format!(
                "{goal}query = 'a'\nlearned_by = 2027-04-01\nmilestones = [{milestones}]"
            ))
        };
        let error = milestones("{ date = 2027-04-01, new_left = 5 }");
        assert!(
            error.contains("goals.g.milestones[0]") && error.contains("before"),
            "{error}"
        );
        let error =
            milestones("{ date = 2027-02-01, new_left = 5 }, { date = 2027-01-01, new_left = 3 }");
        assert!(
            error.contains("goals.g.milestones[1]") && error.contains("after"),
            "{error}"
        );
        let error =
            milestones("{ date = 2027-01-01, new_left = 5 }, { date = 2027-02-01, new_left = 8 }");
        assert!(
            error.contains("goals.g.milestones[1]") && error.contains("easier"),
            "{error}"
        );
        let mut config: SparesExternalConfig = toml_edit::de::from_str(&format!(
            "{goal}query = 'a'\nmilestones = [{{ date = 2027-01-01, new_left = 5 }}, \
             {{ date = \"2027-02-01\", new_left = 5 }}]"
        ))
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.goals["g"].milestones[1].new_left, 5);
        assert!(
            invalid(&format!(
                "{goal}query = 'a'\n[goals.g.parts]\nx = 'nofield=1'"
            ))
            .contains("goals.g.parts.x")
        );
    }
}
