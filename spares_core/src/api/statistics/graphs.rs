//! The statistics page's graphs. See [`crate::schema::statistics`] for what each figure means.
//!
//! Everything is aggregated in SQL, each section in a query of its own, and the queries run
//! together on the pool's connections. The only per-card work in Rust is the distributions, which
//! need the scheduler's forgetting curve. Young and mature reviews are told apart by
//! `review_log.previous_interval`, which triggers keep up to date, so no query has to pair each
//! review with the one before it.

use std::collections::BTreeMap;

use chrono::DateTime;
use chrono::Days;
use chrono::NaiveDate;
use chrono::Utc;
use itertools::Itertools;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use super::local_time::LocalClock;
use super::local_time::Zone;
use super::local_time::date_of_day_number;
use super::local_time::day_number;
use crate::Error;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::model::CardId;
use crate::model::RatingId;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::RetrievabilityFn;
use crate::schedulers::SrsScheduler;
use crate::schedulers::get_scheduler_from_string;
use crate::schedulers::state_ids_sql;
use crate::schedulers::state_role;
use crate::schedulers::step_states;
use crate::schema::review::StateRole;
use crate::schema::statistics::AddedDay;
use crate::schema::statistics::AnswerButtons;
use crate::schema::statistics::AnswerGroup;
use crate::schema::statistics::CardCounts;
use crate::schema::statistics::Distributions;
use crate::schema::statistics::ForecastDay;
use crate::schema::statistics::Histogram;
use crate::schema::statistics::HourStats;
use crate::schema::statistics::MAX_FORECAST_DAYS;
use crate::schema::statistics::PassFail;
use crate::schema::statistics::RetentionRow;
use crate::schema::statistics::ReviewDay;
use crate::schema::statistics::ReviewsSummary;
use crate::schema::statistics::StatisticsGraphsRequest;
use crate::schema::statistics::StatisticsGraphsResponse;
use crate::schema::statistics::TodayStats;
use crate::search::evaluator::Evaluator;

const SECONDS_PER_DAY: i64 = 86_400;
const REVIEW: u8 = ReviewLogKind::Review as u8;
const CRAM: u8 = ReviewLogKind::Cram as u8;

/// How a review is classified by the card's state before it. The SQL [`Context::class`] gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Learning,
    Relearning,
    Young,
    Mature,
}

impl Class {
    fn from_sql(class: u8) -> Self {
        match class {
            1 => Self::Relearning,
            2 => Self::Young,
            3 => Self::Mature,
            _ => Self::Learning,
        }
    }
}

/// The cards statistics are narrowed to.
pub(crate) enum CardFilter {
    All,
    /// The ids of the cards matching a query, as a JSON array. Queries bind it as `?1` and read it
    /// with `json_each`, so thousands of ids are neither formatted into nor parsed with each
    /// query.
    Ids(String),
}

impl CardFilter {
    pub(crate) async fn new(db: &SqlitePool, query: Option<&str>) -> Result<Self, Error> {
        let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
            return Ok(Self::All);
        };
        let ids: Vec<CardId> = Evaluator::new(query).get_card_ids(db).await?;
        Ok(Self::Ids(format!("[{}]", ids.iter().join(","))))
    }

    /// `AND {column} IN (the cards)`, or nothing without a query.
    pub(crate) fn sql(&self, column: &str) -> String {
        match self {
            Self::All => String::new(),
            Self::Ids(_) => format!(" AND {column} IN (SELECT value FROM json_each(?1))"),
        }
    }

    /// Binds the ids for [`Self::sql`].
    pub(crate) fn bind<'q, O>(
        &'q self,
        query: sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments>,
    ) -> sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments> {
        match self {
            Self::All => query,
            Self::Ids(ids) => query.bind(ids.as_str()),
        }
    }

    /// `NOT INDEXED`, to scan the whole table in order, when that beats seeking each row through
    /// an index: a long range of `reviewed_at` without a query. Through the index, each row is a
    /// lookup of its own, which takes about twice as long once it is most of the table.
    fn scan_hint(&self, long_range: bool) -> &'static str {
        match self {
            Self::All if long_range => " NOT INDEXED",
            _ => "",
        }
    }
}

/// What every section needs.
struct Context<'a> {
    db: &'a SqlitePool,
    scheduler: &'a dyn SrsScheduler,
    cards: &'a CardFilter,
    zone: Zone,
    now: DateTime<Utc>,
    today: NaiveDate,
    /// The start of today and of tomorrow, as timestamps.
    today_start: i64,
    tomorrow_start: i64,
    period_first_day: NaiveDate,
    period_start: i64,
    mature_seconds: i64,
}

impl Context<'_> {
    /// SQL that is 1 for a passing rating, else 0.
    fn passed(&self) -> String {
        let failing = self.scheduler.failing_ratings().into_iter().join(", ");
        if failing.is_empty() {
            "(rating IS NOT NULL)".to_string()
        } else {
            format!("(rating NOT IN ({failing}))")
        }
    }

    /// SQL for a review's [`Class`]. Crams get one too, which is ignored.
    fn class(&self) -> String {
        format!(
            "(CASE WHEN previous_state IN ({review}) THEN 2 + (COALESCE(previous_interval, 0) >= {mature}) \
             WHEN previous_state IN ({relearning}) THEN 1 ELSE 0 END)",
            review = state_ids_sql(&[StateRole::Review]),
            relearning = state_ids_sql(&[StateRole::Relearning]),
            mature = self.mature_seconds,
        )
    }

    /// A clock exact over the period.
    fn period_clock(&self) -> LocalClock {
        self.zone.clock(self.period_start, self.tomorrow_start)
    }

    async fn fetch_all<O>(&self, sql: &str) -> Result<Vec<O>, Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
    {
        self.cards
            .bind(sqlx::query_as(AssertSqlSafe(sql)))
            .fetch_all(self.db)
            .await
            .map_err(|e| Error::Sqlx { source: e })
    }
}

pub async fn get_statistics_graphs(
    db: &SqlitePool,
    request: StatisticsGraphsRequest,
) -> Result<StatisticsGraphsResponse, Error> {
    let config = read_external_config()?;
    get_statistics_graphs_with(db, &config, request).await
}

pub(crate) async fn get_statistics_graphs_with(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    request: StatisticsGraphsRequest,
) -> Result<StatisticsGraphsResponse, Error> {
    let scheduler = get_scheduler_from_string(&request.scheduler_name)?;
    let zone = Zone::parse(request.tz.as_deref())?;
    let cards = CardFilter::new(db, request.query.as_deref()).await?;
    let now = request.date;
    let today = zone.date(now);
    let tomorrow = today + Days::new(1);
    let period_first_day = match request.period_days {
        Some(days) => today - Days::new(u64::from(days.max(1) - 1)),
        None => earliest_day(db, zone, &cards)
            .await?
            .unwrap_or(today)
            .min(today),
    };
    let mature_threshold_days = config.statistics.mature_threshold_days;
    let context = Context {
        db,
        scheduler: scheduler.as_ref(),
        cards: &cards,
        zone,
        now,
        today,
        today_start: zone.start_of(today).timestamp(),
        tomorrow_start: zone.start_of(tomorrow).timestamp(),
        period_first_day,
        period_start: zone.start_of(period_first_day).timestamp(),
        mature_seconds: i64::from(mature_threshold_days) * SECONDS_PER_DAY,
    };
    let forecast_days = request.forecast_days.clamp(1, MAX_FORECAST_DAYS);
    let (
        today_stats,
        forecast,
        (reviews, reviews_summary),
        true_retention,
        answer_buttons,
        hourly,
        (card_counts, distributions),
        added,
    ) = futures::try_join!(
        today_stats(&context),
        forecast(&context, forecast_days),
        reviews(&context),
        true_retention(&context),
        answer_buttons(&context),
        hourly(&context),
        cards_and_distributions(&context, config),
        added(&context),
    )?;
    Ok(StatisticsGraphsResponse {
        mature_threshold_days,
        date: today,
        period_start: period_first_day,
        today: today_stats,
        forecast,
        reviews,
        reviews_summary,
        true_retention,
        answer_buttons,
        hourly,
        card_counts,
        added,
        distributions,
    })
}

/// The day of the earliest review or card matching `cards`, for an all-time period.
async fn earliest_day(
    db: &SqlitePool,
    zone: Zone,
    cards: &CardFilter,
) -> Result<Option<NaiveDate>, Error> {
    let (reviewed, created): (Option<i64>, Option<i64>) = cards
        .bind(sqlx::query_as(AssertSqlSafe(format!(
            "SELECT (SELECT MIN(reviewed_at) FROM review_log WHERE kind IN ({REVIEW}, {CRAM}){}),
                    (SELECT MIN(created_at) FROM card WHERE 1{})",
            cards.sql("card_id"),
            cards.sql("id"),
        ))))
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(reviewed
        .into_iter()
        .chain(created)
        .min()
        .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
        .map(|at| zone.date(at)))
}

async fn today_stats(context: &Context<'_>) -> Result<TodayStats, Error> {
    let rows: Vec<(u8, u8, bool, u32, i64)> = context
        .fetch_all(&format!(
            "SELECT kind, {class} AS class, {passed} AS passed, COUNT(*),
                    COALESCE(SUM(recall_duration + rate_duration), 0)
             FROM review_log
             WHERE reviewed_at >= {start} AND reviewed_at < {end} AND kind IN ({REVIEW}, {CRAM}){cards}
             GROUP BY kind, class, passed",
            class = context.class(),
            passed = context.passed(),
            start = context.today_start,
            end = context.tomorrow_start,
            cards = context.cards.sql("card_id"),
        ))
        .await?;
    let mut today = TodayStats::default();
    for (kind, class, passed, count, seconds) in rows {
        if kind == CRAM {
            today.crams += count;
            today.cram_seconds += seconds;
            if passed {
                today.crams_passed += count;
            }
            continue;
        }
        today.reviews += count;
        today.seconds += seconds;
        if passed {
            today.passed += count;
        }
        *match Class::from_sql(class) {
            Class::Learning => &mut today.learning,
            Class::Relearning => &mut today.relearning,
            Class::Young => &mut today.young,
            Class::Mature => &mut today.mature,
        } += count;
    }
    Ok(today)
}

/// Learning and review cards due per local day from `today`, by day number. Overdue cards count
/// as due today, and cards buried by the user or the scheduler that are due by today as due
/// tomorrow. New and suspended cards are left out. Not capped by the daily limits, so it is the
/// whole backlog. `horizon` is the first day number left out, `None` for every day.
pub(crate) async fn due_by_day(
    db: &SqlitePool,
    zone: Zone,
    cards: &CardFilter,
    today: NaiveDate,
    horizon: Option<i64>,
) -> Result<BTreeMap<i64, (u32, u32)>, Error> {
    let today_number = day_number(today);
    let today_start = zone.start_of(today).timestamp();
    let clock = zone.clock(
        today_start,
        today_start + horizon.map_or(366, |h| h - today_number + 1) * SECONDS_PER_DAY,
    );
    let due_day = clock.day("due");
    let learning = step_states().iter().join(", ");
    let having = horizon.map_or(String::new(), |h| format!(" HAVING day < {h}"));
    let rows: Vec<(i64, u32, u32)> = cards
        .bind(sqlx::query_as(AssertSqlSafe(format!(
            "SELECT MAX({due_day}, {today_number})
                    + (IFNULL(special_state, 0) IN ({user}, {scheduler}) AND {due_day} <= {today_number}) AS day,
                    SUM(state IN ({learning})), SUM(state NOT IN ({learning}))
             FROM card
             WHERE state NOT IN ({new})
               AND (special_state IS NULL OR special_state != {suspended}){cards}
             GROUP BY day{having}",
            new = state_ids_sql(&[StateRole::New]),
            user = SpecialState::UserBuried as u8,
            scheduler = SpecialState::SchedulerBuried as u8,
            suspended = SpecialState::Suspended as u8,
            cards = cards.sql("id"),
        ))))
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(rows
        .into_iter()
        .map(|(day, learning, review)| (day, (learning, review)))
        .collect())
}

async fn forecast(context: &Context<'_>, days: u32) -> Result<Vec<ForecastDay>, Error> {
    let today = day_number(context.today);
    let due = due_by_day(
        context.db,
        context.zone,
        context.cards,
        context.today,
        Some(today + i64::from(days)),
    )
    .await?;
    Ok((today..today + i64::from(days))
        .map(|day| {
            let (learning, review) = due.get(&day).copied().unwrap_or_default();
            ForecastDay {
                date: date_of_day_number(day),
                learning,
                review,
            }
        })
        .collect())
}

#[expect(
    clippy::cast_sign_loss,
    reason = "the period's days and days studied are far below 2^31"
)]
async fn reviews(context: &Context<'_>) -> Result<(Vec<ReviewDay>, ReviewsSummary), Error> {
    let clock = context.period_clock();
    let rows: Vec<(i64, u8, u8, u32, i64)> = context
        .fetch_all(&format!(
            "SELECT {day} AS day, kind, {class} AS class, COUNT(*),
                    COALESCE(SUM(recall_duration + rate_duration), 0)
             FROM review_log{hint}
             WHERE reviewed_at >= {start} AND reviewed_at < {end} AND kind IN ({REVIEW}, {CRAM}){cards}
             GROUP BY day, kind, class",
            day = clock.day("reviewed_at"),
            class = context.class(),
            hint = context.cards.scan_hint(context.period_first_day < context.today - Days::new(180)),
            start = context.period_start,
            end = context.tomorrow_start,
            cards = context.cards.sql("card_id"),
        ))
        .await?;
    let mut days = BTreeMap::<i64, ReviewDay>::new();
    let mut summary = ReviewsSummary::default();
    for (day, kind, class, count, seconds) in rows {
        let entry = days.entry(day).or_insert_with(|| ReviewDay {
            date: date_of_day_number(day),
            ..ReviewDay::default()
        });
        entry.seconds += seconds;
        summary.seconds += seconds;
        if kind == CRAM {
            entry.cram += count;
            summary.crams += count;
            continue;
        }
        summary.reviews += count;
        *match Class::from_sql(class) {
            Class::Learning => &mut entry.learning,
            Class::Relearning => &mut entry.relearning,
            Class::Young => &mut entry.young,
            Class::Mature => &mut entry.mature,
        } += count;
    }
    summary.days_studied = days.len() as u32;
    summary.days = (context.today - context.period_first_day).num_days().max(0) as u32 + 1;
    summary.daily_average =
        f64::from(summary.reviews + summary.crams) / f64::from(summary.days.max(1));
    Ok((days.into_values().collect(), summary))
}

async fn true_retention(context: &Context<'_>) -> Result<Vec<RetentionRow>, Error> {
    let start_of_days_ago = |days: u64| {
        context
            .zone
            .start_of(context.today - Days::new(days))
            .timestamp()
    };
    // (label, days, start, end)
    let rows = [
        (
            "Today",
            Some(1),
            context.today_start,
            context.tomorrow_start,
        ),
        (
            "Yesterday",
            Some(1),
            start_of_days_ago(1),
            context.today_start,
        ),
        (
            "Last 7 days",
            Some(7),
            start_of_days_ago(6),
            context.tomorrow_start,
        ),
        (
            "Last 30 days",
            Some(30),
            start_of_days_ago(29),
            context.tomorrow_start,
        ),
        (
            "Last year",
            Some(365),
            start_of_days_ago(364),
            context.tomorrow_start,
        ),
        ("All time", None, i64::MIN, context.tomorrow_start),
    ];
    let passed = context.passed();
    let counts = futures::future::try_join_all(rows.iter().map(|(_, days, start, end)| {
        // Sums rather than a `GROUP BY`, which would sort every review of the year first.
        let sql = format!(
            "SELECT COALESCE(SUM(young AND passed), 0), COALESCE(SUM(young AND NOT passed), 0),
                    COALESCE(SUM(NOT young AND passed), 0), COALESCE(SUM(NOT young AND NOT passed), 0)
             FROM (SELECT COALESCE(previous_interval, 0) < {mature} AS young, {passed} AS passed
                   FROM review_log{hint}
                   WHERE kind = {REVIEW} AND previous_state IN ({review})
                     AND reviewed_at >= {start} AND reviewed_at < {end}{cards})",
            mature = context.mature_seconds,
            hint = context.cards.scan_hint(days.is_none_or(|days| days > 30)),
            review = state_ids_sql(&[StateRole::Review]),
            cards = context.cards.sql("card_id"),
        );
        async move {
            let [counts]: [(u32, u32, u32, u32); 1] = context
                .fetch_all(&sql)
                .await?
                .try_into()
                .map_err(|_| Error::Sqlx {
                    source: sqlx::Error::RowNotFound,
                })?;
            Ok::<_, Error>(counts)
        }
    }))
    .await?;
    Ok(rows
        .iter()
        .zip(counts)
        .map(
            |((label, days, _, _), (young_passed, young_failed, mature_passed, mature_failed))| {
                RetentionRow {
                    label: (*label).to_string(),
                    days: *days,
                    young: PassFail {
                        passed: young_passed,
                        failed: young_failed,
                    },
                    mature: PassFail {
                        passed: mature_passed,
                        failed: mature_failed,
                    },
                }
            },
        )
        .collect())
}

async fn answer_buttons(context: &Context<'_>) -> Result<AnswerButtons, Error> {
    let rows: Vec<(u8, RatingId, u32)> = context
        .fetch_all(&format!(
            "SELECT {class} AS class, rating, COUNT(*)
             FROM review_log{hint}
             WHERE reviewed_at >= {start} AND reviewed_at < {end} AND kind = {REVIEW}{cards}
             GROUP BY class, rating",
            class = context.class(),
            hint = context
                .cards
                .scan_hint(context.period_first_day < context.today - Days::new(180)),
            start = context.period_start,
            end = context.tomorrow_start,
            cards = context.cards.sql("card_id"),
        ))
        .await?;
    let mut ratings = context.scheduler.get_ratings();
    ratings.sort_by_key(|rating| rating.id);
    let group = |classes: &[Class]| {
        let counts = ratings
            .iter()
            .map(|rating| {
                let count = rows
                    .iter()
                    .filter(|(class, id, _)| {
                        *id == rating.id && classes.contains(&Class::from_sql(*class))
                    })
                    .map(|(_, _, count)| count)
                    .sum::<u32>();
                (rating.id, count)
            })
            .collect::<Vec<_>>();
        AnswerGroup {
            passed: counts
                .iter()
                .filter(|(id, _)| context.scheduler.is_pass(*id))
                .map(|(_, count)| count)
                .sum(),
            total: counts.iter().map(|(_, count)| count).sum(),
            counts,
        }
    };
    Ok(AnswerButtons {
        learning: group(&[Class::Learning, Class::Relearning]),
        young: group(&[Class::Young]),
        mature: group(&[Class::Mature]),
        ratings,
    })
}

async fn hourly(context: &Context<'_>) -> Result<[HourStats; 24], Error> {
    let clock = context.period_clock();
    let rows: Vec<(u32, u8, bool, u32)> = context
        .fetch_all(&format!(
            "SELECT {hour} AS hour, kind, {passed} AS passed, COUNT(*)
             FROM review_log{hint}
             WHERE reviewed_at >= {start} AND reviewed_at < {end} AND kind IN ({REVIEW}, {CRAM}){cards}
             GROUP BY hour, kind, passed",
            hour = clock.hour("reviewed_at"),
            passed = context.passed(),
            hint = context.cards.scan_hint(context.period_first_day < context.today - Days::new(180)),
            start = context.period_start,
            end = context.tomorrow_start,
            cards = context.cards.sql("card_id"),
        ))
        .await?;
    let mut hours = [HourStats::default(); 24];
    for (hour, kind, passed, count) in rows {
        let Some(stats) = hours.get_mut(hour as usize) else {
            continue;
        };
        if kind == CRAM {
            stats.crams += count;
            stats.crams_passed += if passed { count } else { 0 };
        } else {
            stats.reviews += count;
            stats.passed += if passed { count } else { 0 };
        }
    }
    Ok(hours)
}

async fn added(context: &Context<'_>) -> Result<Vec<AddedDay>, Error> {
    let clock = context.period_clock();
    let rows: Vec<(i64, u32)> = context
        .fetch_all(&format!(
            "SELECT {day} AS day, COUNT(*) FROM card
             WHERE created_at >= {start} AND created_at < {end}{cards}
             GROUP BY day ORDER BY day",
            day = clock.day("created_at"),
            start = context.period_start,
            end = context.tomorrow_start,
            cards = context.cards.sql("id"),
        ))
        .await?;
    Ok(rows
        .into_iter()
        .map(|(day, cards)| AddedDay {
            date: date_of_day_number(day),
            cards,
        })
        .collect())
}

/// Card counts and the distributions, which share a pass over the cards that are not new.
async fn cards_and_distributions(
    context: &Context<'_>,
    config: &SparesExternalConfig,
) -> Result<(CardCounts, Distributions), Error> {
    let cards = context.cards.sql("c.id");
    let counts_sql = format!(
        "SELECT state, special_state, COUNT(*) FROM card c WHERE 1{cards} GROUP BY state, special_state"
    );
    // Crams are left out of the last review, as for review order (see `LAST_REVIEW`). Forgets are
    // too: a card that is not new was reviewed after its last forget.
    //
    // The cards come back as one JSON array rather than a row each, which is several times faster
    // to read with tens of thousands of cards.
    let cards_sql = format!(
        "SELECT json_group_array(json_array(c.state, c.special_state, c.due, c.stability,
                    c.difficulty,
                    (SELECT MAX(rl.reviewed_at) FROM review_log rl
                     WHERE rl.card_id = c.id AND rl.kind = {REVIEW})))
         FROM card c WHERE c.state NOT IN ({new}){cards}",
        new = state_ids_sql(&[StateRole::New]),
    );
    let (counts, rows): (Vec<StateCount>, Vec<(String,)>) = futures::try_join!(
        context.fetch_all(&counts_sql),
        context.fetch_all(&cards_sql)
    )?;
    let rows: Vec<CardRow> = match rows.first() {
        Some((json,)) => serde_json::from_str(json).map_err(|e| Error::Sqlx {
            source: sqlx::Error::Decode(Box::new(e)),
        })?,
        None => Vec::new(),
    };
    Ok(summarize_cards(
        counts,
        rows,
        context.scheduler.retrievability(config),
        context.now,
        context.mature_seconds,
    ))
}

/// The cards in a state and special state.
type StateCount = (StateId, Option<u8>, u32);

/// A card that is not new: its state, special state, due date, stability, difficulty and last
/// review.
type CardRow = (StateId, Option<u8>, i64, f64, f64, Option<i64>);

#[expect(
    clippy::cast_precision_loss,
    reason = "timestamp differences in seconds are far below 2^52"
)]
fn summarize_cards(
    counts: Vec<StateCount>,
    rows: Vec<CardRow>,
    retrievability: Option<RetrievabilityFn>,
    now: DateTime<Utc>,
    mature_seconds: i64,
) -> (CardCounts, Distributions) {
    let mut card_counts = CardCounts::default();
    for (state, special_state, count) in counts {
        card_counts.total += count;
        match special_state {
            Some(s) if s == SpecialState::Suspended as u8 => card_counts.suspended += count,
            Some(_) => card_counts.buried += count,
            None => *card_counts.by_state.entry(state).or_default() += count,
        }
    }

    let now = now.timestamp();
    let mut intervals = Vec::with_capacity(rows.len());
    let mut stabilities = Vec::with_capacity(rows.len());
    let mut difficulties = Vec::with_capacity(rows.len());
    let mut recalls = Vec::with_capacity(rows.len());
    for (state, special_state, due, stability, difficulty, last_review) in rows {
        let interval = last_review.map(|last| due - last);
        if state_role(state) == Some(StateRole::Review) && special_state.is_none() {
            if interval.is_some_and(|interval| interval >= mature_seconds) {
                card_counts.mature += 1;
            } else {
                card_counts.young += 1;
            }
        }
        intervals.extend(interval.map(|interval| interval.max(0) as f64 / SECONDS_PER_DAY as f64));
        stabilities.push(stability);
        difficulties.push(difficulty);
        if let (Some(retrievability), Some(last)) = (&retrievability, last_review) {
            recalls.push(retrievability(
                stability,
                (now - last) as f64 / SECONDS_PER_DAY as f64,
            ));
        }
    }
    let distributions = Distributions {
        interval: histogram(intervals, Some(0.0)),
        stability: histogram(stabilities, Some(0.0)),
        difficulty: histogram(difficulties, None),
        total_knowledge: retrievability.as_ref().map(|_| recalls.iter().sum()),
        retrievability: retrievability.map(|_| fixed_histogram(&recalls, 0.0, 1.0, 20)),
    };
    (card_counts, distributions)
}

/// The most buckets [`histogram`] makes.
const MAX_BUCKETS: usize = 30;
/// [`histogram`] folds the values above this quantile into its last bucket, so a few cards with
/// intervals of years do not squeeze everyone else into the first bucket.
const TAIL_QUANTILE: f64 = 0.95;

/// Values in buckets of a round width (1, 2 or 5 times a power of ten), at most
/// [`MAX_BUCKETS`] of them, starting from `lower` or else from the smallest value rounded down.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "bucket counts and indices are small"
)]
fn histogram(mut values: Vec<f64>, lower: Option<f64>) -> Histogram {
    values.retain(|value| value.is_finite());
    if values.is_empty() {
        return Histogram::default();
    }
    values.sort_unstable_by(f64::total_cmp);
    let min = values[0];
    let max = values[values.len() - 1];
    let tail = values[((values.len() - 1) as f64 * TAIL_QUANTILE).round() as usize];
    let mut width = round_width((tail - lower.unwrap_or(min)).max(0.0) / MAX_BUCKETS as f64);
    let start = lower.unwrap_or_else(|| (min / width).floor() * width);
    let mut buckets = (((tail - start) / width).floor() as usize + 1).min(MAX_BUCKETS);
    // A few distinct values, e.g. every interval 0, still get a bucket of their own.
    if buckets == 0 {
        buckets = 1;
        width = width.max(1.0);
    }
    let mut counts = vec![0u32; buckets];
    for value in &values {
        let index = (((value - start) / width).floor().max(0.0) as usize).min(buckets - 1);
        counts[index] += 1;
    }
    let mut edges = (0..=buckets)
        .map(|i| start + i as f64 * width)
        .collect::<Vec<_>>();
    if let Some(last) = edges.last_mut() {
        *last = last.max(max);
    }
    Histogram {
        edges,
        counts,
        mean: Some(values.iter().sum::<f64>() / values.len() as f64),
    }
}

/// 1, 2 or 5 times a power of ten, at least `raw`.
fn round_width(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|step| step * magnitude)
        .find(|width| *width >= raw)
        .unwrap_or(10.0 * magnitude)
}

/// `values` in `buckets` buckets of the same width from `lower` to `upper`.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "bucket counts and indices are small"
)]
fn fixed_histogram(values: &[f64], lower: f64, upper: f64, buckets: usize) -> Histogram {
    let width = (upper - lower) / buckets as f64;
    let mut counts = vec![0u32; buckets];
    for value in values.iter().filter(|value| value.is_finite()) {
        let index = (((value - lower) / width).floor().max(0.0) as usize).min(buckets - 1);
        counts[index] += 1;
    }
    Histogram {
        edges: (0..=buckets).map(|i| lower + i as f64 * width).collect(),
        counts,
        mean: (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedulers::review_state;

    #[test]
    fn a_scheduler_without_a_forgetting_curve_leaves_retrievability_out() {
        let review = review_state();
        let rows = vec![(review, None, 30 * SECONDS_PER_DAY, 20.0, 5.0, Some(0))];
        let (counts, distributions) = summarize_cards(
            vec![(review, None, 1)],
            rows,
            None,
            DateTime::from_timestamp(SECONDS_PER_DAY, 0).unwrap(),
            21 * SECONDS_PER_DAY,
        );
        assert_eq!((counts.young, counts.mature), (0, 1));
        assert_eq!(distributions.retrievability, None);
        assert_eq!(distributions.total_knowledge, None);
        assert_eq!(distributions.interval.mean, Some(30.0));
    }

    #[test]
    fn histograms_have_round_buckets() {
        let histogram = histogram((0..100).map(f64::from).collect(), Some(0.0));
        // 95 / 30 rounds up to buckets of 5
        assert_eq!(histogram.edges[..3], [0.0, 5.0, 10.0]);
        assert_eq!(histogram.counts[0], 5);
        assert_eq!(histogram.counts.iter().sum::<u32>(), 100);
        assert_eq!(histogram.edges.len(), histogram.counts.len() + 1);
        assert_eq!(histogram.mean, Some(49.5));
    }

    #[test]
    fn a_long_tail_folds_into_the_last_bucket() {
        // Intervals of 0 to 29 days, a hundred cards each, and one card at ten years
        let mut values = (0..3000).map(|i| f64::from(i % 30)).collect::<Vec<_>>();
        values.push(3650.0);
        let histogram = histogram(values, Some(0.0));
        assert_eq!(histogram.counts.len(), 29);
        assert_eq!(histogram.counts[0], 100);
        assert_eq!(histogram.counts[28], 201);
        assert_eq!(histogram.edges[28], 28.0);
        assert_eq!(histogram.edges[29], 3650.0);
    }

    #[test]
    fn equal_values_get_one_bucket() {
        let histogram = histogram(vec![5.0, 5.0], None);
        assert_eq!(histogram.counts, [2]);
        assert_eq!(histogram.edges, [5.0, 6.0]);
        assert_eq!(super::histogram(Vec::new(), None), Histogram::default());
    }

    #[test]
    fn round_widths() {
        assert_eq!(round_width(0.3), 0.5);
        assert_eq!(round_width(1.0), 1.0);
        assert_eq!(round_width(1.3), 2.0);
        assert_eq!(round_width(30.0), 50.0);
        assert_eq!(round_width(0.0), 1.0);
    }
}
