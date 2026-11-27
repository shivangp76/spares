//! A year in review, for the Statistics page: how much was reviewed in a year, for how long, how
//! many cards were learned, the longest streak of days studied, the retention reached, and the
//! tags studied most. Built from the review log alone, with days of study in the collection's time
//! zone (see [`Zone`]), as the rest of the Statistics page counts them.
use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Datelike;
use chrono::NaiveDate;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use super::local_time::Zone;
use crate::Error;
use crate::api::study_pace::capped_answer_seconds;
use crate::config::read_external_config;
use crate::model::ReviewLogKind;
use crate::schedulers::state_role;
use crate::schema::review::StateRole;

/// How many of the tags studied most a recap lists.
const TOP_TAGS: usize = 5;

/// One year's review history, summed up.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct YearRecap {
    pub year: i32,
    /// The years with reviews, newest first, to choose from
    pub years: Vec<i32>,
    pub reviews: u32,
    /// Time answering, capped as `maximum_answer_time` caps it
    pub minutes: u32,
    /// Cards reviewed for the first time
    pub cards_learned: u32,
    /// Days with a review
    pub days_studied: u32,
    /// The most days in a row with a review, and the first of them
    pub longest_streak: u32,
    pub longest_streak_start: Option<NaiveDate>,
    /// Of the reviews of cards in review (not in their steps), the share recalled (not Again)
    pub retention: Option<f64>,
    /// The tags of the cards reviewed most, with their reviews, most first. A card's note's tags
    /// and its own count, but not filtered tags, nor the tags that say how a card went rather
    /// than what it is about: the flagged, leech and buried tags of `config.toml`.
    pub top_tags: Vec<(String, u32)>,
}

/// A review: when, its rating, its card's state before it, and the seconds it took.
type Row = (i64, Option<i64>, i64, i64);

/// The recap of `year` (or, if `None`, the latest year with reviews, else this one).
pub async fn year_recap(
    db: &SqlitePool,
    year: Option<i32>,
    now: DateTime<Utc>,
) -> Result<YearRecap, Error> {
    let config = read_external_config()?;
    let zone = Zone::from_config(&config);
    let sql_error = |e| Error::Sqlx { source: e };
    let review = ReviewLogKind::Review as u8;

    let (first, last): (Option<i64>, Option<i64>) = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT MIN(reviewed_at), MAX(reviewed_at) FROM review_log WHERE kind = {review}"
    )))
    .fetch_one(db)
    .await
    .map_err(sql_error)?;
    let year_of = |at: Option<i64>| {
        at.and_then(|at| DateTime::from_timestamp(at, 0))
            .map(|at| zone.date(at).year())
    };
    let years: Vec<i32> = match (year_of(first), year_of(last)) {
        (Some(first), Some(last)) => (first..=last).rev().collect(),
        _ => Vec::new(),
    };
    let year = year
        .or_else(|| years.first().copied())
        .unwrap_or_else(|| zone.date(now).year());
    let start_of = |year: i32| {
        NaiveDate::from_ymd_opt(year, 1, 1).map_or(0, |date| zone.start_of(date).timestamp())
    };
    let (from, to) = (start_of(year), start_of(year + 1));

    let seconds = capped_answer_seconds("rl", config.maximum_answer_time.map(|d| d.num_seconds()));
    let rows: Vec<Row> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT rl.reviewed_at, rl.rating, rl.previous_state, {seconds}
         FROM review_log rl
         WHERE rl.kind = {review} AND rl.reviewed_at >= ? AND rl.reviewed_at < ?"
    )))
    .bind(from)
    .bind(to)
    .fetch_all(db)
    .await
    .map_err(sql_error)?;
    let tags: Vec<(String, u32)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT name, COUNT(*) AS n FROM (
             SELECT rl.id, t.name FROM review_log rl JOIN card c ON c.id = rl.card_id
             JOIN note_tag nt ON nt.note_id = c.note_id JOIN tag t ON t.id = nt.tag_id
             WHERE rl.kind = {review} AND rl.reviewed_at >= ?1 AND rl.reviewed_at < ?2
               AND t.query IS NULL
             UNION
             SELECT rl.id, t.name FROM review_log rl JOIN card_tag ct ON ct.card_id = rl.card_id
             JOIN tag t ON t.id = ct.tag_id
             WHERE rl.kind = {review} AND rl.reviewed_at >= ?1 AND rl.reviewed_at < ?2
               AND t.query IS NULL)
         WHERE name NOT IN (?3, ?4, ?5)
         GROUP BY name ORDER BY n DESC, name LIMIT {TOP_TAGS}"
    )))
    .bind(from)
    .bind(to)
    .bind(&config.flagged_tag_name)
    .bind(&config.leech.tag_name)
    .bind(&config.queue.tag_buried.tag)
    .fetch_all(db)
    .await
    .map_err(sql_error)?;

    let mut recap = summarize(&rows, zone);
    recap.year = year;
    recap.years = years;
    recap.top_tags = tags;
    Ok(recap)
}

/// The recap of `rows`, the reviews of one year, but its year and tags.
fn summarize(rows: &[Row], zone: Zone) -> YearRecap {
    let mut recap = YearRecap::default();
    let mut days = BTreeSet::new();
    let mut seconds = 0i64;
    let (mut in_review, mut recalled) = (0u32, 0u32);
    for &(at, rating, previous_state, answer_seconds) in rows {
        recap.reviews += 1;
        seconds += answer_seconds.max(0);
        let role = u32::try_from(previous_state).ok().and_then(state_role);
        if role == Some(StateRole::New) {
            recap.cards_learned += 1;
        }
        if role == Some(StateRole::Review) {
            in_review += 1;
            if rating.is_some_and(|r| r > 1) {
                recalled += 1;
            }
        }
        if let Some(at) = DateTime::from_timestamp(at, 0) {
            days.insert(zone.date(at));
        }
    }
    recap.minutes = u32::try_from(seconds / 60).unwrap_or(u32::MAX);
    recap.days_studied = u32::try_from(days.len()).unwrap_or(u32::MAX);
    recap.retention = (in_review > 0).then(|| f64::from(recalled) / f64::from(in_review));
    let mut run: Option<(NaiveDate, NaiveDate, u32)> = None;
    for day in days {
        run = Some(match run {
            Some((start, previous, length)) if previous.succ_opt() == Some(day) => {
                (start, day, length + 1)
            }
            _ => (day, day, 1),
        });
        if let Some((start, _, length)) = run
            && length > recap.longest_streak
        {
            recap.longest_streak = length;
            recap.longest_streak_start = Some(start);
        }
    }
    recap
}

/// Each tag's reviews, for tests.
#[cfg(test)]
fn by_name(tags: &[(String, u32)]) -> std::collections::HashMap<&str, u32> {
    tags.iter().map(|(name, n)| (name.as_str(), *n)).collect()
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::TimeZone;

    use super::*;

    fn at(date: &str, hour: u32) -> i64 {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        Utc.from_utc_datetime(&date.and_hms_opt(hour, 0, 0).unwrap())
            .timestamp()
    }

    #[test]
    fn streaks_retention_and_cards_learned() {
        let zone = Zone::parse(Some("UTC"), 4).unwrap();
        let rows = [
            // A new card learned, then reviewed on three days in a row, then after a gap
            (at("2026-03-01", 12), Some(3), 0, 30),
            (at("2026-03-02", 12), Some(3), 2, 30),
            (at("2026-03-03", 12), Some(1), 2, 30),
            // Before 4 AM, so still the 3rd's day of study
            (at("2026-03-04", 3), Some(3), 3, 30),
            (at("2026-03-10", 12), Some(4), 2, 60),
        ];
        let recap = summarize(&rows, zone);
        assert_eq!(recap.reviews, 5);
        assert_eq!(recap.minutes, 3);
        assert_eq!(recap.cards_learned, 1);
        assert_eq!(recap.days_studied, 4);
        assert_eq!(recap.longest_streak, 3);
        assert_eq!(
            recap.longest_streak_start,
            NaiveDate::from_ymd_opt(2026, 3, 1)
        );
        assert_eq!(
            recap.retention,
            Some(2.0 / 3.0),
            "Again on one of three in review"
        );
    }

    #[sqlx::test]
    async fn a_year_of_the_review_log(pool: SqlitePool) {
        let now = Utc::now();
        crate::api::scheduler::tests::seed_review_history(&pool, 4, now - Duration::days(40)).await;
        // `leech` says how the card went, not what it is about, so it isn't one of the tags
        sqlx::query(
            "INSERT INTO tag (name, description, query, auto_delete, reschedule, created_at, updated_at, usn)
             VALUES ('geo', '', NULL, 0, 0, 0, 0, -1), ('leech', '', NULL, 0, 0, 0, 0, -1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO note_tag (note_id, tag_id) SELECT (SELECT note_id FROM card LIMIT 1), id FROM tag")
            .execute(&pool)
            .await
            .unwrap();
        let recap = year_recap(&pool, None, now).await.unwrap();
        let reviewed: u32 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM review_log WHERE reviewed_at >= ? AND reviewed_at < ?",
        )
        .bind(
            Utc.with_ymd_and_hms(recap.year, 1, 1, 0, 0, 0)
                .unwrap()
                .timestamp()
                - 86_400,
        )
        .bind(
            Utc.with_ymd_and_hms(recap.year + 1, 1, 1, 0, 0, 0)
                .unwrap()
                .timestamp()
                + 86_400,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(recap.reviews > 0 && recap.reviews <= reviewed);
        assert_ne!(recap.years, Vec::<i32>::new());
        assert_eq!(by_name(&recap.top_tags).get("geo"), Some(&recap.reviews));
        assert_eq!(recap.top_tags.len(), 1);
        let empty = year_recap(&pool, Some(1999), now).await.unwrap();
        assert_eq!(
            (empty.reviews, empty.longest_streak, empty.retention),
            (0, 0, None)
        );
    }
}
