//! The statistics page's requests over a large collection: 50,000 cards and 500,000 reviews over
//! two years, in sessions at three times of day. The target is under 300 ms for the graphs.
//!
//! The database is a file in WAL mode, as the server's is, so the graphs' queries run on several
//! connections at once.

use std::hint::black_box;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;
use spares_core::api::parser::create_parser;
use spares_core::api::statistics::get_review_history;
use spares_core::api::statistics::get_statistics;
use spares_core::api::statistics::get_statistics_graphs;
use spares_core::schema::parser::CreateParserRequest;
use spares_core::schema::review::ReviewHistoryRequest;
use spares_core::schema::review::StatisticsRequest;
use spares_core::schema::statistics::StatisticsGraphsRequest;
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqlitePoolOptions;

const CARDS: u32 = 50_000;
const REVIEWS: u32 = 500_000;
const REVIEWS_PER_DAY: u32 = 714;
/// The last day of reviews, and "now".
const NOW: i64 = 1_760_486_400; // 2025-10-15 00:00 UTC

fn database_path() -> PathBuf {
    std::env::temp_dir().join("spares-statistics-bench.sqlite")
}

/// Fills a fresh database. The triggers that keep `review_log.previous_interval` are dropped
/// while the reviews go in, and the column filled afterwards as the migration does, which is
/// much faster than a trigger per row.
#[expect(
    clippy::too_many_lines,
    reason = "mostly the SQL that fills each table"
)]
async fn fill(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let migrator = Migrator::new(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/migrations"
    )))
    .await
    .unwrap();
    migrator.run(&pool).await.unwrap();
    let parser = create_parser(
        &pool,
        CreateParserRequest {
            name: "markdown".to_string(),
        },
        false,
    )
    .await
    .unwrap();

    let triggers: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master
         WHERE type = 'trigger' AND name LIKE 'review_log_previous_interval_%'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for (name, _) in &triggers {
        sqlx::query(&format!("DROP TRIGGER {name}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    let statements = [
        format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {CARDS})
             INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id)
             SELECT i, 'Note ' || i || ': {{{{answer}}}}', {start} + i * 1200, {start}, '{{}}', {parser}
             FROM n",
            start = NOW - 730 * 86_400,
            parser = parser.id,
        ),
        // A tenth new, a tenth in (re)learning, the rest in review, due over the next 200 days.
        // One in 50 suspended, one in 77 buried.
        format!(
            "INSERT INTO card (id, note_id, \"order\", back_type, created_at, updated_at, due,
                 stability, difficulty, desired_retention, special_state, state, custom_data)
             SELECT id, id, 1, 1, created_at, created_at,
                 {NOW} + abs(random()) % (200 * 86400), 1 + abs(random()) % 100,
                 1 + abs(random()) % 9, 0.9,
                 CASE WHEN id % 50 = 0 THEN 1 WHEN id % 77 = 0 THEN 2 END,
                 CASE WHEN id % 10 = 0 THEN 0 WHEN id % 13 = 0 THEN 1 WHEN id % 17 = 0 THEN 3
                      ELSE 2 END,
                 '{{}}'
             FROM note"
        ),
        // Sessions at 08:00, 13:00 and 21:00 UTC. One in 40 a cram, one in 10 failed, three in
        // four of cards in review.
        format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < {REVIEWS} - 1)
             INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name,
                 scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
             SELECT 1 + abs(random()) % {CARDS},
                 {first} + (i / {REVIEWS_PER_DAY}) * 86400
                     + (CASE (i % {REVIEWS_PER_DAY}) % 3 WHEN 0 THEN 8 WHEN 1 THEN 13 ELSE 21 END)
                         * 3600
                     + (i % {REVIEWS_PER_DAY}) * 5,
                 CASE WHEN i % 40 = 0 THEN 16 ELSE 0 END,
                 CASE WHEN abs(random()) % 10 = 0 THEN 1 WHEN abs(random()) % 7 = 0 THEN 2
                      WHEN abs(random()) % 8 = 0 THEN 4 ELSE 3 END,
                 'fsrs',
                 CASE WHEN i % 40 = 0 THEN NULL ELSE 86400 * (1 + abs(random()) % 60) END,
                 abs(random()) % 20, abs(random()) % 5,
                 CASE WHEN abs(random()) % 4 = 0 THEN abs(random()) % 4 ELSE 2 END,
                 '{{}}'
             FROM n",
            first = NOW - i64::from(REVIEWS / REVIEWS_PER_DAY) * 86_400,
        ),
        "UPDATE review_log SET previous_interval = previous.scheduled_time
         FROM (
             SELECT id, LAG(scheduled_time) OVER (PARTITION BY card_id ORDER BY reviewed_at, id)
                 AS scheduled_time
             FROM review_log WHERE kind = 0 AND card_id IS NOT NULL
         ) AS previous
         WHERE review_log.id = previous.id AND previous.scheduled_time IS NOT NULL"
            .to_string(),
        "ANALYZE".to_string(),
    ];
    for statement in statements {
        sqlx::query(&statement).execute(&pool).await.unwrap();
    }
    for (_, sql) in &triggers {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    pool.close().await;
}

/// The server's pool, over the filled database.
async fn server_pool(path: &Path) -> SqlitePool {
    // As `spares_server` connects
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .pragma("mmap_size", "268435456");
    SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .unwrap()
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(NOW + 12 * 3600, 0).unwrap()
}

fn graphs_request(query: Option<&str>, period_days: Option<u32>) -> StatisticsGraphsRequest {
    StatisticsGraphsRequest {
        scheduler_name: "fsrs".to_string(),
        query: query.map(str::to_string),
        period_days,
        forecast_days: 30,
        date: now(),
        tz: Some("Europe/Berlin".to_string()),
    }
}

fn bench_statistics(c: &mut Criterion) {
    // Read a config of the bench's own, not the user's
    // SAFETY: no other thread is running yet.
    unsafe { std::env::set_var("SPARES_TEST_MODE", "1") };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let path = database_path();
    let pool = rt.block_on(async {
        fill(&path).await;
        server_pool(&path).await
    });

    let mut group = c.benchmark_group("statistics");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(20));

    let mut graphs = |name: &str, request: StatisticsGraphsRequest| {
        group.bench_function(name, |b| {
            b.iter(|| {
                rt.block_on(async {
                    black_box(get_statistics_graphs(&pool, request.clone()).await.unwrap())
                })
            });
        });
    };
    // The page as it opens: the last month, a 30 day forecast, every card
    graphs("graphs_month", graphs_request(None, Some(30)));
    graphs("graphs_all_time", graphs_request(None, None));
    // Half the cards
    graphs(
        "graphs_month_query",
        graphs_request(Some("c.id<25000"), Some(30)),
    );

    group.bench_function("today", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(
                    get_statistics(
                        &pool,
                        StatisticsRequest {
                            scheduler_name: "fsrs".to_string(),
                            date: now(),
                            tz: Some("Europe/Berlin".to_string()),
                        },
                    )
                    .await
                    .unwrap(),
                )
            })
        });
    });
    group.bench_function("history", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(
                    get_review_history(
                        &pool,
                        ReviewHistoryRequest {
                            query: None,
                            tz: Some("Europe/Berlin".to_string()),
                        },
                    )
                    .await
                    .unwrap(),
                )
            })
        });
    });
    group.finish();
    rt.block_on(pool.close());
    // Kept with `SPARES_BENCH_KEEP_DB=1`, to look into query plans with `sqlite3`
    if std::env::var_os("SPARES_BENCH_KEEP_DB").is_some() {
        return;
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

criterion_group!(benches, bench_statistics);
criterion_main!(benches);
