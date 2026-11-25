//! [Goals](crate::config::Goal): sets of cards to know by a date, like an exam's.
//!
//! [`goal_status`] reports how each goal is going, for `spares goal status` and the web app, and
//! [`keep_due_for_goals`] keeps a goal's cards from being due after its date with too little
//! recall left, when one is rated.

use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::Utc;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use crate::Error;
use crate::api::statistics::local_time::Zone;
use crate::config::Goal;
use crate::config::SparesExternalConfig;
use crate::model::Card;
use crate::model::CardId;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::RetrievabilityFn;
use crate::schedulers::SrsScheduler;
use crate::schedulers::all_schedulers;
use crate::schedulers::state_role;
use crate::schema::goal::GoalPartStatus;
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
    /// When it was first reviewed (crams left out), if it was
    first_review: Option<i64>,
    /// When it was last reviewed
    last_review: Option<i64>,
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
        for (index, part_name) in names.enumerate() {
            let sql = cards_sql(goal, index.checked_sub(1));
            parts.push(part_status(db, goal, part_name, &sql, zone, now, recall.as_ref()).await?);
        }
        statuses.push(GoalStatus {
            name: name.clone(),
            date: goal.date,
            learned_by: goal.learned_by(),
            retention: goal.retention,
            days_to_learned_by: (goal.learned_by() - today).num_days().max(0),
            days_to_date: (goal.date - today).num_days().max(0),
            parts,
        });
    }
    Ok(statuses)
}

/// [`GoalPartStatus`] of the cards `cards_sql` selects, named `name`, of `goal`.
async fn part_status(
    db: &SqlitePool,
    goal: &Goal,
    name: &str,
    cards_sql: &str,
    zone: Zone,
    now: DateTime<Utc>,
    recall: Option<&RetrievabilityFn>,
) -> Result<GoalPartStatus, Error> {
    let review = ReviewLogKind::Review as u8;
    let cards: Vec<GoalCard> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT c.state, c.special_state, c.stability, \
         (SELECT MIN(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = {review}) AS first_review, \
         (SELECT MAX(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = {review}) AS last_review \
         FROM card c WHERE c.id IN ({cards_sql})"
    )))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    let today = zone.date(now);
    let suspended = SpecialState::Suspended as u8;
    let mut status = GoalPartStatus {
        name: name.to_string(),
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
    };
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
            let predicted = recall(card.stability, days.max(0.0));
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
    status.weeks = weeks(db, cards_sql, &cards, zone, today).await?;
    Ok(status)
}

/// The last [`WEEKS`] weeks of `cards`, which `cards_sql` selects, ending today, oldest first.
async fn weeks(
    db: &SqlitePool,
    cards_sql: &str,
    cards: &[GoalCard],
    zone: Zone,
    today: NaiveDate,
) -> Result<Vec<GoalWeek>, Error> {
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
    let reviews: Vec<i64> = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT rl.reviewed_at FROM review_log rl WHERE rl.kind = {} AND rl.reviewed_at >= {first_start} \
         AND rl.card_id IN ({cards_sql})",
        ReviewLogKind::Review as u8
    )))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
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
        })
        .collect())
}

/// Whether card `card_id` is one of the cards `cards_sql` selects.
async fn is_among(db: &SqlitePool, card_id: CardId, cards_sql: &str) -> Result<bool, Error> {
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
    db: &SqlitePool,
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
        if recall(card.stability, days_to_day) >= retention
            || !is_among(db, card.id, &goal.query_sql).await?
        {
            continue;
        }
        // Recall falls as days pass, so the last day it is still at the retention is found by
        // halving the days to the goal's day.
        let (mut kept, mut lost) = (0.0, days_to_day);
        for _ in 0..40 {
            let middle = f64::midpoint(kept, lost);
            if recall(card.stability, middle) >= retention {
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
        let due = keep_due_for_goals(&pool, &config, &scheduler, &exam_card, now)
            .await
            .unwrap();
        assert!(due < day, "{due} should be before {day}");
        #[expect(clippy::cast_precision_loss, reason = "a test's seconds")]
        let days = (due - now).num_seconds() as f64 / 86400.0;
        assert!((recall(20.0, days) - 0.95).abs() < 0.001, "{days}");
        // A card of no goal, or of a goal without retention, keeps its due date.
        let other = card(&pool, ids[1]).await;
        assert_eq!(
            keep_due_for_goals(&pool, &config, &scheduler, &other, now)
                .await
                .unwrap(),
            other.due
        );
        let without = super::tests::config(date, "");
        assert_eq!(
            keep_due_for_goals(&pool, &without, &scheduler, &exam_card, now)
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
            keep_due_for_goals(&pool, &config, &scheduler, &strong, now)
                .await
                .unwrap(),
            strong.due
        );
        // Once the day has come, nothing changes.
        assert_eq!(
            keep_due_for_goals(&pool, &config, &scheduler, &exam_card, day)
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
        assert!(
            invalid(&format!(
                "{goal}query = 'a'\n[goals.g.parts]\nx = 'nofield=1'"
            ))
            .contains("goals.g.parts.x")
        );
    }
}
