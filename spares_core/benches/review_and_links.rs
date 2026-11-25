//! Set SPARES_REVIEW_BENCH_FIXTURE to a new SQLite file, then reuse it to compare revisions.
#[path = "common/statistics_fixture.rs"]
mod fixture;

use spares_core::api::note::KeywordMatcher;
use spares_core::api::review::{get_review_card, submit_study_action};
use spares_core::ids::NoteId;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::review::*;
use std::hint::black_box;
use std::time::Instant;

fn links() {
    let keywords: Vec<_> = (0..10000)
        .map(|i| {
            (
                NoteId::from_packed(i + 1),
                format!("Continuity theorem {i}"),
            )
        })
        .collect();
    let matcher = KeywordMatcher::new(&keywords);
    for word in ["Continuity", "Continuty", "Missing"] {
        for distinct in [10, 100] {
            let start = Instant::now();
            for i in 0..100 {
                black_box(matcher.best_match(&format!("{word} theorem {}", i % distinct)));
            }
            println!("links {word} {distinct} distinct: {:?}", start.elapsed());
        }
    }
}

fn main() {
    // SAFETY: single threaded before the runtime starts
    unsafe { std::env::set_var("SPARES_TEST_MODE", "1") };
    links();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let source = std::env::var_os("SPARES_REVIEW_BENCH_FIXTURE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| temp.path().join("fixture.sqlite"));
        if !source.exists() {
            fixture::fill(&source).await;
        }
        let path = temp.path().join("review.sqlite");
        std::fs::copy(source, &path).unwrap();
        let db = fixture::server_pool(&path).await;
        // Ensure both filters have enough due cards even after repeated ratings.
        sqlx::query("UPDATE card SET due = ?, state = 2 WHERE id < 201")
            .bind(fixture::now().timestamp())
            .execute(&db)
            .await
            .unwrap();
        let parsers = get_all_parsers();
        for narrow in [false, true] {
            for _ in 0..10 {
                let next = || {
                    get_review_card(
                        &db,
                        GetReviewCardRequest {
                            filter: narrow
                                .then(|| GetReviewCardFilterRequest::Query("c.id<201".into())),
                        },
                        fixture::now(),
                        &parsers,
                    )
                };
                let start = Instant::now();
                let card = next().await.unwrap().unwrap();
                let fetch = start.elapsed();
                let start = Instant::now();
                submit_study_action(
                    &db,
                    SubmitStudyActionRequest {
                        scheduler_name: "fsrs".into(),
                        action: StudyAction::Rate(RatingSubmission {
                            card_id: card.card_id,
                            rating: 3,
                            recall_duration: chrono::Duration::seconds(5),
                            rate_duration: chrono::Duration::seconds(1),
                            tag_id: None,
                        }),
                    },
                    fixture::now(),
                )
                .await
                .unwrap();
                let rate = start.elapsed();
                let start = Instant::now();
                let next = next().await.unwrap().unwrap();
                println!(
                    "review {narrow} {:.3} {:.3} {:.3} {} {} {} {}",
                    fetch.as_secs_f64() * 1000.,
                    rate.as_secs_f64() * 1000.,
                    start.elapsed().as_secs_f64() * 1000.,
                    card.card_id,
                    next.card_id,
                    card.cards_left_by_state.values().sum::<u32>(),
                    card.time_estimate.num_seconds()
                );
            }
        }
    });
}
