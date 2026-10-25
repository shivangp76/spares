//! The statistics graphs over a fixed collection, every section asserted exactly.

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::TimeZone;
use chrono::Utc;
use pretty_assertions::assert_eq;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use super::graphs::get_statistics_graphs_with;
use crate::api::queue::tests::create_cards;
use crate::config::SparesExternalConfig;
use crate::config::StatisticsConfig;
use crate::model::CardId;
use crate::schema::statistics::AddedDay;
use crate::schema::statistics::ForecastDay;
use crate::schema::statistics::HourStats;
use crate::schema::statistics::PassFail;
use crate::schema::statistics::RetentionRow;
use crate::schema::statistics::ReviewDay;
use crate::schema::statistics::ReviewsSummary;
use crate::schema::statistics::StatisticsGraphsRequest;
use crate::schema::statistics::StatisticsGraphsResponse;
use crate::schema::statistics::TodayStats;

const NEW: u32 = 0;
const LEARNING: u32 = 1;
const REVIEW: u32 = 2;
const REVIEW_KIND: u8 = 0;
const FORGET_KIND: u8 = 1;
const CRAM_KIND: u8 = 16;
const AGAIN: u32 = 1;
const HARD: u32 = 2;
const GOOD: u32 = 3;
const EASY: u32 = 4;
/// Seconds spent on each review and on each cram.
const REVIEW_SECONDS: i64 = 10;
const CRAM_SECONDS: i64 = 5;

/// Noon UTC on 2026-10-16, "now" in every test.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap()
}

/// `hour`:`minute` UTC, `day` days from today.
fn at(day: i64, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 16, hour, minute, 0).unwrap() + Duration::days(day)
}

fn date(day: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap() + Duration::days(day)
}

fn request(query: Option<&str>, period_days: Option<u32>) -> StatisticsGraphsRequest {
    StatisticsGraphsRequest {
        scheduler_name: "fsrs".to_string(),
        query: query.map(str::to_string),
        period_days,
        forecast_days: 7,
        date: now(),
        tz: Some("UTC".to_string()),
    }
}

fn config(mature_threshold_days: u32) -> SparesExternalConfig {
    SparesExternalConfig {
        statistics: StatisticsConfig {
            mature_threshold_days,
        },
        ..SparesExternalConfig::default()
    }
}

/// Records a row of `review_log`. Reviews and crams take [`REVIEW_SECONDS`] and
/// [`CRAM_SECONDS`]; a forget has no rating or durations.
async fn log(
    pool: &SqlitePool,
    card_id: Option<CardId>,
    at: DateTime<Utc>,
    kind: u8,
    rating: Option<u32>,
    previous_state: u32,
    scheduled_days: Option<i64>,
) {
    let seconds = match kind {
        REVIEW_KIND => Some(REVIEW_SECONDS),
        CRAM_KIND => Some(CRAM_SECONDS),
        _ => None,
    };
    sqlx::query(
        r"INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name,
          scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
          VALUES (?, ?, ?, ?, 'fsrs', ?, ?, ?, ?, '{}')",
    )
    .bind(card_id)
    .bind(at.timestamp())
    .bind(kind)
    .bind(rating)
    .bind(scheduled_days.map(|days| days * 86_400))
    .bind(seconds)
    .bind(seconds.map(|_| 0))
    .bind(previous_state)
    .execute(pool)
    .await
    .unwrap();
}

async fn review(
    pool: &SqlitePool,
    card_id: CardId,
    at: DateTime<Utc>,
    rating: u32,
    previous_state: u32,
    scheduled_days: i64,
) {
    log(
        pool,
        Some(card_id),
        at,
        REVIEW_KIND,
        Some(rating),
        previous_state,
        Some(scheduled_days),
    )
    .await;
}

async fn cram(pool: &SqlitePool, card_id: CardId, at: DateTime<Utc>, rating: u32) {
    log(
        pool,
        Some(card_id),
        at,
        CRAM_KIND,
        Some(rating),
        REVIEW,
        None,
    )
    .await;
}

async fn set_card(
    pool: &SqlitePool,
    card_id: CardId,
    state: u32,
    due: DateTime<Utc>,
    special_state: Option<u8>,
    created_at: DateTime<Utc>,
) {
    sqlx::query(
        r"UPDATE card SET state = ?, due = ?, special_state = ?, created_at = ?, stability = 10,
          difficulty = 5 WHERE id = ?",
    )
    .bind(state)
    .bind(due.timestamp())
    .bind(special_state)
    .bind(created_at.timestamp())
    .bind(card_id)
    .execute(pool)
    .await
    .unwrap();
}

/// The cards of [`collection`].
struct Cards {
    /// Reviewed through to mature, crammed, then failed. Due in 5 days, 14 days after its last
    /// review, so young as a card.
    c1: CardId,
    /// Young, then mature today, then crammed and failed tonight. Due in 40 days.
    c2: CardId,
    /// Learning, reviewed twice today and crammed tonight.
    c3: CardId,
}

/// Six cards and their reviews, among them a forget, crams, an orphaned review, a suspended card
/// and a buried card. Reviews of the same card are not inserted in order, so the triggers that
/// keep `previous_interval` have to place each between the others.
async fn collection(pool: &SqlitePool) -> Cards {
    let ids = create_cards(pool, 6, now()).await;
    let [c1, c2, c3, c4, c5, c6] = ids.try_into().unwrap();
    set_card(pool, c1, REVIEW, at(5, 9, 0), None, at(-61, 9, 0)).await;
    set_card(pool, c2, REVIEW, at(40, 8, 0), None, at(-31, 9, 0)).await;
    set_card(pool, c3, LEARNING, at(0, 13, 0), None, at(-7, 9, 0)).await;
    // Overdue but suspended
    set_card(pool, c4, REVIEW, at(-1, 11, 0), Some(1), at(-7, 9, 0)).await;
    // Overdue but buried by the user
    set_card(pool, c5, REVIEW, at(-2, 11, 0), Some(2), at(-7, 9, 0)).await;
    set_card(pool, c6, NEW, at(0, 9, 0), None, at(0, 9, 0)).await;

    // c1: 21 days going into its second review, which is mature at the threshold of 21. The
    // cram in between changes neither that nor the interval going into the last review.
    review(pool, c1, at(-9, 9, 0), AGAIN, REVIEW, 1).await;
    review(pool, c1, at(-60, 9, 0), GOOD, NEW, 21).await;
    log(pool, Some(c1), at(-61, 9, 0), FORGET_KIND, None, NEW, None).await;
    review(pool, c1, at(-39, 9, 0), GOOD, REVIEW, 30).await;
    cram(pool, c1, at(-20, 9, 0), AGAIN).await;
    // c2: 20 days going into its second review, young; 25 going into today's, mature.
    review(pool, c2, at(0, 8, 0), EASY, REVIEW, 40).await;
    review(pool, c2, at(-30, 10, 0), GOOD, NEW, 20).await;
    review(pool, c2, at(-10, 10, 0), GOOD, REVIEW, 25).await;
    cram(pool, c2, at(0, 21, 0), AGAIN).await;
    review(pool, c3, at(0, 8, 30), AGAIN, NEW, 0).await;
    review(pool, c3, at(0, 8, 40), GOOD, LEARNING, 0).await;
    cram(pool, c3, at(0, 21, 30), GOOD).await;
    review(pool, c4, at(-5, 11, 0), GOOD, NEW, 4).await;
    review(pool, c5, at(-6, 11, 0), HARD, NEW, 4).await;
    // A review of a deleted card, whose interval is unknown, so young
    log(
        pool,
        None,
        at(0, 9, 0),
        REVIEW_KIND,
        Some(GOOD),
        REVIEW,
        Some(10),
    )
    .await;
    Cards { c1, c2, c3 }
}

async fn graphs(
    pool: &SqlitePool,
    config: &SparesExternalConfig,
    request: StatisticsGraphsRequest,
) -> StatisticsGraphsResponse {
    get_statistics_graphs_with(pool, config, request)
        .await
        .unwrap()
}

fn pass_fail(passed: u32, failed: u32) -> PassFail {
    PassFail { passed, failed }
}

fn retention(label: &str, days: Option<u32>, young: PassFail, mature: PassFail) -> RetentionRow {
    RetentionRow {
        label: label.to_string(),
        days,
        young,
        mature,
    }
}

fn review_day(day: i64) -> ReviewDay {
    ReviewDay {
        date: date(day),
        ..ReviewDay::default()
    }
}

#[sqlx::test]
async fn every_section_over_all_time(pool: SqlitePool) {
    collection(&pool).await;
    let response = graphs(&pool, &config(21), request(None, None)).await;

    assert_eq!(response.mature_threshold_days, 21);
    assert_eq!(response.date, date(0));
    // c1 was created before its first review
    assert_eq!(response.period_start, date(-61));

    assert_eq!(
        response.today,
        TodayStats {
            reviews: 4,
            passed: 3,
            seconds: 4 * REVIEW_SECONDS,
            learning: 2,
            relearning: 0,
            young: 1,
            mature: 1,
            crams: 2,
            crams_passed: 1,
            cram_seconds: 2 * CRAM_SECONDS,
        }
    );

    // c3 is learning and due later today, c5 is buried so its overdue review moves to tomorrow,
    // and c1 is due in 5 days. c4 is suspended, c6 new and c2 due after the forecast.
    let mut forecast = (0..7)
        .map(|day| ForecastDay {
            date: date(day),
            learning: 0,
            review: 0,
        })
        .collect::<Vec<_>>();
    forecast[0].learning = 1;
    forecast[1].review = 1;
    forecast[5].review = 1;
    assert_eq!(response.forecast, forecast);

    assert_eq!(
        response.reviews,
        vec![
            ReviewDay {
                learning: 1,
                seconds: REVIEW_SECONDS,
                ..review_day(-60)
            },
            ReviewDay {
                mature: 1,
                seconds: REVIEW_SECONDS,
                ..review_day(-39)
            },
            ReviewDay {
                learning: 1,
                seconds: REVIEW_SECONDS,
                ..review_day(-30)
            },
            ReviewDay {
                cram: 1,
                seconds: CRAM_SECONDS,
                ..review_day(-20)
            },
            ReviewDay {
                young: 1,
                seconds: REVIEW_SECONDS,
                ..review_day(-10)
            },
            ReviewDay {
                mature: 1,
                seconds: REVIEW_SECONDS,
                ..review_day(-9)
            },
            ReviewDay {
                learning: 1,
                seconds: REVIEW_SECONDS,
                ..review_day(-6)
            },
            ReviewDay {
                learning: 1,
                seconds: REVIEW_SECONDS,
                ..review_day(-5)
            },
            ReviewDay {
                learning: 2,
                young: 1,
                mature: 1,
                cram: 2,
                seconds: 4 * REVIEW_SECONDS + 2 * CRAM_SECONDS,
                ..review_day(0)
            },
        ]
    );
    assert_eq!(
        response.reviews_summary,
        ReviewsSummary {
            reviews: 11,
            crams: 3,
            seconds: 11 * REVIEW_SECONDS + 3 * CRAM_SECONDS,
            days_studied: 9,
            days: 62,
            daily_average: 14.0 / 62.0,
        }
    );

    let today = (pass_fail(1, 0), pass_fail(1, 0));
    let month = (pass_fail(2, 0), pass_fail(1, 1));
    let all = (pass_fail(2, 0), pass_fail(2, 1));
    assert_eq!(
        response.true_retention,
        vec![
            retention("Today", Some(1), today.0, today.1),
            retention("Yesterday", Some(1), pass_fail(0, 0), pass_fail(0, 0)),
            retention("Last 7 days", Some(7), today.0, today.1),
            retention("Last 30 days", Some(30), month.0, month.1),
            retention("Last year", Some(365), all.0, all.1),
            retention("All time", None, all.0, all.1),
        ]
    );

    let buttons = &response.answer_buttons;
    assert_eq!(
        buttons
            .ratings
            .iter()
            .map(|rating| rating.description.as_str())
            .collect::<Vec<_>>(),
        ["Again", "Hard", "Good", "Easy"]
    );
    assert_eq!(buttons.learning.counts, [(1, 1), (2, 1), (3, 4), (4, 0)]);
    assert_eq!((buttons.learning.passed, buttons.learning.total), (5, 6));
    assert_eq!(buttons.young.counts, [(1, 0), (2, 0), (3, 2), (4, 0)]);
    assert_eq!((buttons.young.passed, buttons.young.total), (2, 2));
    assert_eq!(buttons.mature.counts, [(1, 1), (2, 0), (3, 1), (4, 1)]);
    assert_eq!((buttons.mature.passed, buttons.mature.total), (2, 3));

    let mut hourly = [HourStats::default(); 24];
    hourly[8] = HourStats {
        reviews: 3,
        passed: 2,
        ..HourStats::default()
    };
    hourly[9] = HourStats {
        reviews: 4,
        passed: 3,
        crams: 1,
        crams_passed: 0,
    };
    hourly[10] = HourStats {
        reviews: 2,
        passed: 2,
        ..HourStats::default()
    };
    hourly[11] = HourStats {
        reviews: 2,
        passed: 2,
        ..HourStats::default()
    };
    hourly[21] = HourStats {
        crams: 2,
        crams_passed: 1,
        ..HourStats::default()
    };
    assert_eq!(response.hourly, hourly);

    let counts = &response.card_counts;
    assert_eq!(
        counts.by_state,
        HashMap::from([(NEW, 1), (LEARNING, 1), (REVIEW, 2)])
    );
    assert_eq!(
        (
            counts.young,
            counts.mature,
            counts.suspended,
            counts.buried,
            counts.total
        ),
        (1, 1, 1, 1, 6)
    );

    assert_eq!(
        response.added,
        vec![
            AddedDay {
                date: date(-61),
                cards: 1
            },
            AddedDay {
                date: date(-31),
                cards: 1
            },
            AddedDay {
                date: date(-7),
                cards: 3
            },
            AddedDay {
                date: date(0),
                cards: 1
            },
        ]
    );

    // The five cards that are not new: intervals of 14 and 40 days, 4 hours 20 minutes for c3, and
    // 4 days each for c4 and c5.
    let distributions = &response.distributions;
    let interval = &distributions.interval;
    assert_eq!(interval.counts.iter().sum::<u32>(), 5);
    let mean = (14.0 + 40.0 + 260.0 / 1440.0 + 4.0 + 4.0) / 5.0;
    assert!((interval.mean.unwrap() - mean).abs() < 1e-9, "{interval:?}");
    assert_eq!(interval.edges.len(), interval.counts.len() + 1);
    assert!(interval.edges[0].abs() < f64::EPSILON);
    assert!(*interval.edges.last().unwrap() >= 40.0);
    assert_eq!(distributions.stability.counts.iter().sum::<u32>(), 5);
    assert_eq!(distributions.stability.mean, Some(10.0));
    assert_eq!(distributions.difficulty.mean, Some(5.0));
    let retrievability = distributions.retrievability.as_ref().unwrap();
    assert_eq!(retrievability.counts.iter().sum::<u32>(), 5);
    assert_eq!(retrievability.edges.len(), 21);
    let knowledge = distributions.total_knowledge.unwrap();
    assert!(
        (knowledge - retrievability.mean.unwrap() * 5.0).abs() < 1e-9,
        "{knowledge}"
    );
}

#[sqlx::test]
async fn a_lower_mature_threshold_counts_more_as_mature(pool: SqlitePool) {
    collection(&pool).await;
    let response = graphs(&pool, &config(7), request(None, None)).await;
    assert_eq!(response.mature_threshold_days, 7);
    // Every review of a card in review but the orphaned one had an interval of 20 days or more
    // going into it.
    assert_eq!(
        response.true_retention[5],
        retention("All time", None, pass_fail(1, 0), pass_fail(3, 1))
    );
    assert_eq!(response.today.mature, 1);
    // c1's interval of 14 days is mature too
    assert_eq!(
        (response.card_counts.young, response.card_counts.mature),
        (0, 2)
    );
}

#[sqlx::test]
async fn the_mature_boundary_is_inclusive(pool: SqlitePool) {
    collection(&pool).await;
    // c1's second review had exactly 21 days going into it, c2's second 20.
    let at_21 = graphs(&pool, &config(21), request(None, None)).await;
    let at_22 = graphs(&pool, &config(22), request(None, None)).await;
    let day = |response: &StatisticsGraphsResponse, day: i64| {
        response
            .reviews
            .iter()
            .find(|review| review.date == date(day))
            .copied()
            .unwrap()
    };
    assert_eq!((day(&at_21, -39).young, day(&at_21, -39).mature), (0, 1));
    assert_eq!((day(&at_22, -39).young, day(&at_22, -39).mature), (1, 0));
    assert_eq!((day(&at_21, -10).young, day(&at_21, -10).mature), (1, 0));
}

#[sqlx::test]
async fn a_query_narrows_every_section(pool: SqlitePool) {
    let cards = collection(&pool).await;
    let query = format!("c.id={}", cards.c2);
    let response = graphs(&pool, &config(21), request(Some(&query), None)).await;
    // Today's mature review, without the orphaned one, and tonight's failed cram
    assert_eq!(
        response.today,
        TodayStats {
            reviews: 1,
            passed: 1,
            seconds: REVIEW_SECONDS,
            mature: 1,
            crams: 1,
            crams_passed: 0,
            cram_seconds: CRAM_SECONDS,
            ..TodayStats::default()
        }
    );
    assert_eq!(response.period_start, date(-31));
    assert_eq!(response.reviews_summary.reviews, 3);
    assert_eq!(response.reviews_summary.crams, 1);
    assert_eq!(
        response.true_retention[5],
        retention("All time", None, pass_fail(1, 0), pass_fail(1, 0))
    );
    assert_eq!(response.card_counts.total, 1);
    assert_eq!(response.card_counts.mature, 1);
    assert!(
        response
            .forecast
            .iter()
            .all(|day| day.learning + day.review == 0)
    );
    assert_eq!(
        response.added,
        vec![AddedDay {
            date: date(-31),
            cards: 1
        }]
    );
}

#[sqlx::test]
async fn a_query_matching_nothing_gives_zeros(pool: SqlitePool) {
    collection(&pool).await;
    let response = graphs(&pool, &config(21), request(Some("c.id=999999"), None)).await;
    assert_eq!(response.today, TodayStats::default());
    assert_eq!(response.period_start, date(0));
    assert_eq!(response.reviews, []);
    assert_eq!(response.reviews_summary.days, 1);
    assert!(
        response
            .true_retention
            .iter()
            .all(|row| row.young == pass_fail(0, 0) && row.mature == pass_fail(0, 0))
    );
    assert_eq!(response.answer_buttons.learning.total, 0);
    assert_eq!(response.hourly, [HourStats::default(); 24]);
    assert_eq!(response.card_counts.total, 0);
    assert_eq!(response.added, []);
    assert_eq!(response.distributions.interval.counts, Vec::<u32>::new());
    let retrievability = response.distributions.retrievability.unwrap();
    assert_eq!(retrievability.counts.iter().sum::<u32>(), 0);
    assert_eq!(retrievability.mean, None);
    assert_eq!(response.distributions.total_knowledge, Some(0.0));
}

#[sqlx::test]
async fn a_period_starting_mid_history(pool: SqlitePool) {
    collection(&pool).await;
    let response = graphs(&pool, &config(21), request(None, Some(10))).await;
    assert_eq!(response.period_start, date(-9));
    assert_eq!(
        response
            .reviews
            .iter()
            .map(|review| review.date)
            .collect::<Vec<_>>(),
        [date(-9), date(-6), date(-5), date(0)]
    );
    assert_eq!(
        response.reviews_summary,
        ReviewsSummary {
            reviews: 7,
            crams: 2,
            seconds: 7 * REVIEW_SECONDS + 2 * CRAM_SECONDS,
            days_studied: 4,
            days: 10,
            daily_average: 0.9,
        }
    );
    let buttons = &response.answer_buttons;
    assert_eq!(
        (
            buttons.learning.total,
            buttons.young.total,
            buttons.mature.total
        ),
        (4, 1, 2)
    );
    assert_eq!(response.hourly[9].crams, 0);
    assert_eq!(
        response.added,
        vec![
            AddedDay {
                date: date(-7),
                cards: 3
            },
            AddedDay {
                date: date(0),
                cards: 1
            },
        ]
    );
    // True retention always covers its own spans
    assert_eq!(
        response.true_retention[5],
        retention("All time", None, pass_fail(2, 0), pass_fail(2, 1))
    );
}

#[sqlx::test]
async fn a_failed_cram_lowers_only_the_cram_percent_correct(pool: SqlitePool) {
    let cards = collection(&pool).await;
    let before = graphs(&pool, &config(21), request(None, None)).await;
    cram(&pool, cards.c3, at(0, 9, 30), AGAIN).await;
    let after = graphs(&pool, &config(21), request(None, None)).await;
    assert_eq!(
        (after.today.crams, after.today.crams_passed),
        (before.today.crams + 1, before.today.crams_passed)
    );
    assert_eq!(
        (after.today.reviews, after.today.passed),
        (before.today.reviews, before.today.passed)
    );
    assert_eq!(after.hourly[9].crams, before.hourly[9].crams + 1);
    assert_eq!(after.hourly[9].crams_passed, before.hourly[9].crams_passed);
    assert_eq!(after.hourly[9].passed, before.hourly[9].passed);
    assert_eq!(after.true_retention, before.true_retention);
    assert_eq!(after.answer_buttons, before.answer_buttons);
}

#[sqlx::test]
async fn a_cram_after_the_last_review_is_not_the_last_review(pool: SqlitePool) {
    let cards = collection(&pool).await;
    // A cram a day after c1's last review: c1's interval is still measured from that review, 14
    // days, so it is young as a card, and its retrievability is unchanged.
    let before = graphs(&pool, &config(21), request(None, None)).await;
    cram(&pool, cards.c1, at(-8, 9, 0), GOOD).await;
    let after = graphs(&pool, &config(21), request(None, None)).await;
    assert_eq!(after.card_counts, before.card_counts);
    assert_eq!(after.distributions, before.distributions);
}

#[sqlx::test]
async fn days_and_hours_follow_the_time_zone(pool: SqlitePool) {
    collection(&pool).await;
    let tokyo = graphs(
        &pool,
        &config(21),
        StatisticsGraphsRequest {
            tz: Some("Asia/Tokyo".to_string()),
            ..request(None, None)
        },
    )
    .await;
    // Noon UTC is 21:00 in Tokyo, the same date. Tonight's crams, at 21:00 and 21:30 UTC, are
    // tomorrow morning there, so after the day statistics are for.
    assert_eq!(tokyo.date, date(0));
    assert_eq!((tokyo.today.reviews, tokyo.today.crams), (4, 0));
    let today = tokyo.reviews.last().unwrap();
    assert_eq!((today.date, today.cram), (date(0), 0));
    // 09:00 UTC is 18:00 in Tokyo, and 08:00 UTC 17:00.
    assert_eq!(tokyo.hourly[18].reviews, 4);
    assert_eq!(tokyo.hourly[17].reviews, 3);
    assert_eq!(tokyo.hourly[9].reviews, 0);

    // In Los Angeles, at UTC-7, noon UTC is 05:00 the same day, and 08:00 UTC is 01:00. So today
    // there began at 07:00 UTC, and holds the same reviews.
    let los_angeles = graphs(
        &pool,
        &config(21),
        StatisticsGraphsRequest {
            tz: Some("America/Los_Angeles".to_string()),
            ..request(None, None)
        },
    )
    .await;
    assert_eq!(los_angeles.today.reviews, 4);
    assert_eq!(los_angeles.hourly[1].reviews, 3);
    assert_eq!(los_angeles.hourly[2].reviews, 4);
}

#[sqlx::test]
async fn an_unknown_time_zone_is_an_error(pool: SqlitePool) {
    let result = get_statistics_graphs_with(
        &pool,
        &config(21),
        StatisticsGraphsRequest {
            tz: Some("Mars/Olympus".to_string()),
            ..request(None, None)
        },
    )
    .await;
    assert!(result.is_err());
}

/// `previous_interval` as the definition has it, computed from scratch.
async fn assert_previous_intervals_match(pool: &SqlitePool) {
    let mismatched: Vec<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        r"SELECT r.id, r.previous_interval, expected.previous FROM review_log r
          LEFT JOIN (
              SELECT id, LAG(scheduled_time) OVER (PARTITION BY card_id ORDER BY reviewed_at, id)
                  AS previous
              FROM review_log WHERE kind = 0 AND card_id IS NOT NULL
          ) AS expected ON expected.id = r.id
          WHERE r.previous_interval IS NOT expected.previous",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(mismatched, []);
}

#[sqlx::test]
async fn previous_intervals_follow_every_change(pool: SqlitePool) {
    let cards = collection(&pool).await;
    assert_previous_intervals_match(&pool).await;

    // Undoing a review in the middle of c2's history
    sqlx::query("DELETE FROM review_log WHERE card_id = ? AND reviewed_at = ?")
        .bind(cards.c2)
        .bind(at(-10, 10, 0).timestamp())
        .execute(&pool)
        .await
        .unwrap();
    assert_previous_intervals_match(&pool).await;

    // Moving a review earlier, past another
    sqlx::query("UPDATE review_log SET reviewed_at = ? WHERE card_id = ? AND reviewed_at = ?")
        .bind(at(-70, 9, 0).timestamp())
        .bind(cards.c1)
        .bind(at(-39, 9, 0).timestamp())
        .execute(&pool)
        .await
        .unwrap();
    assert_previous_intervals_match(&pool).await;

    // Two reviews in the same second are ordered by id
    review(&pool, cards.c3, at(0, 8, 40), GOOD, LEARNING, 3).await;
    assert_previous_intervals_match(&pool).await;

    // Deleting a card orphans its reviews, which then have no previous interval
    sqlx::query("DELETE FROM card WHERE id = ?")
        .bind(cards.c1)
        .execute(&pool)
        .await
        .unwrap();
    let orphaned: Vec<(Option<i64>,)> = sqlx::query_as(
        "SELECT previous_interval FROM review_log WHERE card_id IS NULL AND kind = 0",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(orphaned.len(), 4);
    assert!(orphaned.iter().all(|(interval,)| interval.is_none()));
    assert_previous_intervals_match(&pool).await;
}

#[sqlx::test]
async fn the_heatmap_follows_the_query_and_time_zone(pool: SqlitePool) {
    use crate::api::statistics::get_review_history;
    use crate::schema::review::ReviewHistoryRequest;

    let cards = collection(&pool).await;
    let history = |query: Option<String>, tz: &str| {
        get_review_history(
            &pool,
            ReviewHistoryRequest {
                query,
                tz: Some(tz.to_string()),
            },
        )
    };
    let all = history(None, "UTC").await.unwrap();
    // Today's three reviews of c2 and c3, and the orphaned one
    assert_eq!(all.review_count_by_date[&date(0)], 4);
    assert_eq!(all.cram_count_by_date[&date(0)], 2);
    assert_eq!(all.review_count_by_date.values().sum::<u32>(), 11);

    let c2 = history(Some(format!("c.id={}", cards.c2)), "UTC")
        .await
        .unwrap();
    assert_eq!(c2.review_count_by_date[&date(0)], 1);
    assert_eq!(c2.review_count_by_date.values().sum::<u32>(), 3);

    // Tonight's crams are tomorrow in Tokyo
    let tokyo = history(None, "Asia/Tokyo").await.unwrap();
    assert_eq!(tokyo.cram_count_by_date[&date(1)], 2);
    assert!(!tokyo.cram_count_by_date.contains_key(&date(0)));
}

/// The plans the statistics' speed rests on, from `EXPLAIN QUERY PLAN`.
#[sqlx::test]
async fn statistics_queries_use_the_indexes(pool: SqlitePool) {
    let plan = async |sql: &str| {
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
                .fetch_all(&pool)
                .await
                .unwrap();
        rows.into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>()
            .join("\n")
    };
    // A period's reviews
    let period = plan(
        "SELECT COUNT(*) FROM review_log WHERE reviewed_at >= 1 AND reviewed_at < 2 AND kind IN (0, 16)",
    )
    .await;
    assert!(period.contains("idx_review_log_reviewed_at"), "{period}");
    // A card's last review, for its interval and retrievability
    let last = plan("SELECT MAX(reviewed_at) FROM review_log WHERE card_id = 1 AND kind = 0").await;
    assert!(
        last.contains("COVERING INDEX idx_review_log_card_id_kind_reviewed_at"),
        "{last}"
    );
    // The previous Review the triggers look up
    let previous = plan(
        "SELECT p.scheduled_time FROM review_log p
         WHERE p.card_id = 1 AND p.kind = 0 AND p.reviewed_at <= 5 AND (p.reviewed_at < 5 OR p.id < 3)
         ORDER BY p.reviewed_at DESC, p.id DESC LIMIT 1",
    )
    .await;
    assert!(
        previous.contains(
            "idx_review_log_card_id_kind_reviewed_at (card_id=? AND kind=? AND reviewed_at<?)"
        ),
        "{previous}"
    );
}

#[sqlx::test]
async fn todays_statistics_agree_with_the_forecast(pool: SqlitePool) {
    use crate::api::statistics::get_statistics;
    use crate::schema::review::StatisticsRequest;

    collection(&pool).await;
    let statistics = get_statistics(
        &pool,
        StatisticsRequest {
            scheduler_name: "fsrs".to_string(),
            date: now(),
            tz: Some("UTC".to_string()),
        },
    )
    .await
    .unwrap();
    // c5, buried, tomorrow; c1 in 5 days; c2 in 40. c4 is suspended, so never due.
    assert_eq!(
        statistics.due_count_by_date,
        HashMap::from([(date(1), 1), (date(5), 1), (date(40), 1)])
    );
    let forecast = graphs(&pool, &config(21), request(None, None))
        .await
        .forecast;
    for day in &forecast[1..] {
        assert_eq!(
            statistics
                .due_count_by_date
                .get(&day.date)
                .copied()
                .unwrap_or_default(),
            day.learning + day.review,
            "{}",
            day.date
        );
    }
    // c6 is new and c3 learning. Nothing in review is due today: c4 is suspended and c5 buried.
    assert_eq!(
        statistics.due_count_by_state,
        HashMap::from([(NEW, 1), (LEARNING, 1)])
    );
    // c2 and c3 were reviewed today, the orphan does not count as a card studied
    assert_eq!(statistics.cards_studied_count, 2);
    assert_eq!(
        statistics.recall_duration,
        Duration::seconds(3 * REVIEW_SECONDS)
    );
    assert_eq!(statistics.cram_count, 2);
}
