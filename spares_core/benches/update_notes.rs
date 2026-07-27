use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;
use serde_json::Map;
use spares_core::api::note::create_notes;
use spares_core::api::note::update_notes;
use spares_core::api::parser::create_parser;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::note::CreateNoteRequest;
use spares_core::schema::note::CreateNotesRequest;
use spares_core::schema::note::NotesSelector;
use spares_core::schema::note::UpdateNotesRequest;
use spares_core::schema::note::UpdateTags;
use spares_core::schema::parser::CreateParserRequest;
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;

fn pool_and_notes(n: usize) -> (SqlitePool, Vec<i64>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("file::memory:?cache=shared")
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

        let note_data = concat!(
            "{{[o:1] First cloze }}\n",
            "{{[o:2] Second cloze }}\n",
            "{{[o:3;f:all;b:a] Third cloze }}"
        );
        let requests = (0..n)
            .map(|_| CreateNoteRequest {
                data: note_data.to_string(),
                keywords: vec![],
                tags: vec!["tag1".to_string(), "tag2".to_string()],
                is_suspended: false,
                custom_data: Map::new(),
            })
            .collect();

        let res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests,
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();

        let note_ids: Vec<i64> = res.notes.iter().map(|n| n.id).collect();
        (pool, note_ids)
    })
}

fn bench_update_notes(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (pool, note_ids) = pool_and_notes(500);

    let new_data = concat!(
        "{{[o:1] Updated first }}\n",
        "{{[o:2] Updated second }}\n",
        "{{[o:3;f:all;b:a] Updated third }}"
    );

    let mut group = c.benchmark_group("update_notes");
    group.measurement_time(Duration::from_secs(100));

    group.bench_function("data_update_500", |b| {
        b.iter(|| {
            let ids = note_ids.clone();
            rt.block_on(async {
                black_box(
                    update_notes(
                        &pool,
                        UpdateNotesRequest {
                            selector: NotesSelector::Ids(ids),
                            data: Some(new_data.to_string()),
                            parser_id: None,
                            keywords: None,
                            tags: UpdateTags::None,
                            custom_data: None,
                        },
                        Utc::now(),
                        &get_all_parsers(),
                        false,
                    )
                    .await
                    .unwrap(),
                )
            })
        })
    });

    group.bench_function("tags_only_update_500", |b| {
        b.iter(|| {
            let ids = note_ids.clone();
            rt.block_on(async {
                black_box(
                    update_notes(
                        &pool,
                        UpdateNotesRequest {
                            selector: NotesSelector::Ids(ids),
                            data: None,
                            parser_id: None,
                            keywords: None,
                            tags: UpdateTags::SetTags(vec![
                                "tag_a".to_string(),
                                "tag_b".to_string(),
                            ]),
                            custom_data: None,
                        },
                        Utc::now(),
                        &get_all_parsers(),
                        false,
                    )
                    .await
                    .unwrap(),
                )
            })
        })
    });

    group.finish();
}

criterion_group!(benches, bench_update_notes);
criterion_main!(benches);
