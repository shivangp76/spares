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
            r#"INSERT INTO note_link (parent_note_id, "order", searched_keyword, relation)
               SELECT id, 1, (CASE id % 3 WHEN 0 THEN 'Continuity' WHEN 1 THEN 'Continuty'
                              ELSE 'Missing' END) || ' theorem ' || (1 + id % ?), 0
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

/// Times the public APIs, including SQLite row decoding and response construction.
async fn browsing(db: &sqlx::SqlitePool, root: &std::path::Path) {
    use spares_core::api::note::{search_keyword, search_page};
    use spares_core::collection::{Collection, CollectionPaths};
    use spares_core::schema::note::{SearchKeywordRequest, SearchPageItems, SearchPageRequest};
    use spares_core::search::QueryReturnItemType;
    use spares_core::sync::media::{ChangedFilesRequest, changed_rendered_files};
    let paths = CollectionPaths::in_dir(root);
    paths.create().unwrap();
    let note = ("word αβγ ".repeat(120) + "{{ answer }} ").repeat(8);
    sqlx::query("UPDATE note SET data=? WHERE id<=100")
        .bind(&note)
        .execute(db)
        .await
        .unwrap();
    sqlx::query(r#"UPDATE card SET note_id=1+(id-1)/8, "order"=1+(id-1)%8 WHERE id<=800"#)
        .execute(db)
        .await
        .unwrap();
    sqlx::query("UPDATE note_keyword SET keyword='Continuity theorem' WHERE note_id % 2 = 0")
        .execute(db)
        .await
        .unwrap();
    for i in 0..2000 {
        std::fs::write(paths.files.join(format!("card-{i}.html")), b"front").unwrap();
    }
    let collection = std::sync::Arc::new(Collection::new("bench", db.clone(), paths));
    collection
        .scope(async {
            let mut request = ChangedFilesRequest::default();
            for i in 0..12 {
                let start = Instant::now();
                let changed = changed_rendered_files(&request).unwrap();
                let elapsed = start.elapsed();
                assert_eq!(changed.files.len(), if i == 0 { 2000 } else { 0 });
                request.buckets = changed.buckets;
                println!("manifest {i} {:.3}", elapsed.as_secs_f64() * 1000.);
            }
            for i in 0..12 {
                let start = Instant::now();
                let page = search_page(
                    db,
                    SearchPageRequest {
                        query: "c.id<=800".into(),
                        page: 1,
                        limit: 800,
                        sort: None,
                        output_type: QueryReturnItemType::Cards,
                    },
                )
                .await
                .unwrap();
                let elapsed = start.elapsed();
                let SearchPageItems::Cards(rows) = page.items else {
                    unreachable!()
                };
                let expected: String = note.chars().take(200).collect();
                assert_eq!(rows.len(), 800);
                assert!(rows.iter().all(|row| row.note_preview == expected));
                println!("preview {i} {:.3}", elapsed.as_secs_f64() * 1000.);
            }
            for i in 0..12 {
                for (label, term, count) in [
                    ("numbered", "Continuty theorem 101", 1),
                    ("numberless", "Continuty theorem", 5000),
                ] {
                    let start = Instant::now();
                    let matches = search_keyword(
                        db,
                        SearchKeywordRequest {
                            keyword: term.into(),
                        },
                    )
                    .await
                    .unwrap();
                    let elapsed = start.elapsed();
                    assert_eq!(matches.len(), count);
                    println!("{label} {i} {:.3}", elapsed.as_secs_f64() * 1000.);
                }
            }
        })
        .await;
}

/// Measures the default note list and graph against the same linked fixture.
async fn note_views(db: &sqlx::SqlitePool) {
    use spares_core::api::note::{get_note_graph, list_notes};
    use spares_core::schema::{FilterOptions, note::NoteGraphRequest};

    for sample in 0..12 {
        let start = Instant::now();
        let notes = list_notes(
            db,
            FilterOptions {
                page: Some(1),
                limit: Some(100),
            },
        )
        .await
        .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(notes.len(), 100);
        for (id, note) in (1_i64..).zip(&notes) {
            assert_eq!(note.id.packed(), id);
            assert_eq!(note.linked_notes.as_ref().unwrap().len(), 1);
        }
        let hash = blake3::hash(&serde_json::to_vec(&notes).unwrap());
        println!(
            "note_page {sample} {:.3} {hash}",
            elapsed.as_secs_f64() * 1000.
        );

        let start = Instant::now();
        let graph = get_note_graph(
            db,
            NoteGraphRequest {
                query: String::new(),
                max_score: Some(5.0),
                max_nodes: 2000,
                ancestor_depth: None,
                descendant_depth: Some(0),
                groups: Vec::new(),
            },
        )
        .await
        .unwrap();
        let elapsed = start.elapsed();
        assert!(graph.truncated);
        assert_eq!(graph.nodes.len(), 2000);
        // Compare the complete ordered response between runs.
        let hash = blake3::hash(&serde_json::to_vec(&graph).unwrap());
        println!(
            "note_graph {sample} {:.3} {hash}",
            elapsed.as_secs_f64() * 1000.
        );
    }
}

fn main() {
    // SAFETY: single threaded before the runtime starts
    unsafe { std::env::set_var("SPARES_TEST_MODE", "1") };
    env_logger::init();
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
        spares_core::collection::migrator().run(&db).await.unwrap();
        // Ensure both filters have enough due cards even after repeated ratings.
        sqlx::query("UPDATE card SET due = ?, state = 2 WHERE id < 201")
            .bind(fixture::now().timestamp())
            .execute(&db)
            .await
            .unwrap();
        // Reopen a checkpointed copy. Schema upgrades and fixture preparation can otherwise
        // prime the review-log pages and statement cache before the first measured fetch.
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&db)
            .await
            .unwrap();
        db.close().await;
        let reopened = temp.path().join("reopened.sqlite");
        std::fs::copy(&path, &reopened).unwrap();
        println!(
            "database_bytes {}",
            std::fs::metadata(&reopened).unwrap().len()
        );
        let db = fixture::server_pool(&reopened).await;
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
                        action_id: Some(spares_core::api::review::new_action_id()),
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
        note_views(&db).await;
        browsing(&db, temp.path()).await;
    });
}
