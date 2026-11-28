//! The statistics page's requests over a large collection: 50,000 cards and 500,000 reviews over
//! two years, in sessions at three times of day. The target is under 300 ms for the graphs.
//!
//! The database is a file in WAL mode, as the server's is, so the graphs' queries run on several
//! connections at once.

use std::hint::black_box;
use std::time::Duration;

use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;
use spares_core::api::statistics::get_review_history;
use spares_core::api::statistics::get_statistics;
use spares_core::api::statistics::get_statistics_graphs;
use spares_core::schema::review::ReviewHistoryRequest;
use spares_core::schema::review::StatisticsRequest;
use spares_core::schema::statistics::StatisticsGraphsRequest;

#[path = "common/statistics_fixture.rs"]
mod fixture;
use fixture::fill;
use fixture::now;
use fixture::server_pool;

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
    let path = std::env::temp_dir().join("spares-statistics-bench.sqlite");
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
