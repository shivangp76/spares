//! Set `SPARES_REVIEW_BENCH_FIXTURE` to a new SQLite file, then reuse it to compare revisions.
#[path = "common/statistics_fixture.rs"]
mod fixture;

use spares_core::api::note::KeywordMatcher;
use spares_core::api::review::{get_review_card, submit_study_action};
use spares_core::ids::NoteId;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::review::{
    GetReviewCardFilterRequest, GetReviewCardRequest, RatingSubmission, StudyAction,
    SubmitStudyActionRequest,
};
use std::hint::black_box;
use std::time::Instant;

fn links() {
    let keywords: Vec<_> = (0..10_000)
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

/// Includes building the indexes, resolving repeated links and persisting the winners.
async fn rematch(db: &sqlx::SqlitePool) {
    sqlx::query(
        "INSERT INTO note_keyword (note_id, keyword, embedded)
         SELECT id, 'Continuity theorem ' || id, 0 FROM note WHERE id <= 10000",
    )
    .execute(db)
    .await
    .unwrap();
    for distinct in [10, 1000] {
        sqlx::query("DELETE FROM note_link")
            .execute(db)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO note_link (parent_note_id, "order", searched_keyword)
               SELECT id, 1, (CASE id % 3 WHEN 0 THEN 'Continuity' WHEN 1 THEN 'Continuty'
                              ELSE 'Missing' END) || ' theorem ' || (1 + id % ?)
               FROM note WHERE id <= 1000"#,
        )
        .bind(distinct)
        .execute(db)
        .await
        .unwrap();
        sqlx::query("DELETE FROM app_state WHERE key='linked_notes_generated'")
            .execute(db)
            .await
            .unwrap();
        let start = Instant::now();
        spares_core::api::note::match_note_links(db).await.unwrap();
        let elapsed = start.elapsed();
        let matched: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM note_link WHERE linked_note_id IS NOT NULL")
                .fetch_one(db)
                .await
                .unwrap();
        assert_eq!(matched, 667);
        println!("rematch 1000 links / {distinct} target numbers: {elapsed:?}");
    }
}

fn main() {
    // SAFETY: single threaded before the runtime starts
    unsafe { std::env::set_var("SPARES_TEST_MODE", "1") };
    links();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let source = std::env::var_os("SPARES_REVIEW_BENCH_FIXTURE").map_or_else(
            || temp.path().join("fixture.sqlite"),
            std::path::PathBuf::from,
        );
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
                let fetch_next = start.elapsed();
                println!(
                    "review {narrow} {:.3} {:.3} {:.3} {} {} {} {}",
                    fetch.as_secs_f64() * 1000.,
                    rate.as_secs_f64() * 1000.,
                    fetch_next.as_secs_f64() * 1000.,
                    card.card_id,
                    next.card_id,
                    card.cards_left_by_state.values().sum::<u32>(),
                    card.time_estimate.num_seconds()
                );
                println!(
                    "result {:?} {:?} {}",
                    card.cards_left_by_state
                        .into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>(),
                    next.cards_left_by_state
                        .into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>(),
                    next.time_estimate.num_seconds()
                );
            }
        }
        rematch(&db).await;
    });
}
