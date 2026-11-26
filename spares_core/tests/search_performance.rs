//! Run with SPARES_TEST_MODE=1 cargo test --release -p spares_core --test search_performance -- --ignored --nocapture
use std::future::Future;
use std::time::Instant;

use chrono::DateTime;
use spares_core::api::note::{search_notes, search_page};
use spares_core::api::queue_position::queue_positions;
use spares_core::schema::note::{SearchNotesRequest, SearchNotesResponse, SearchPageRequest};
use spares_core::schema::review::QueuePositionRequest;
use spares_core::search::QueryReturnItemType;
use sqlx::sqlite::SqlitePoolOptions;

async fn measure<F: Future<Output = usize>>(
    name: &str,
    expected: usize,
    mut run: impl FnMut() -> F,
) {
    assert_eq!(run().await, expected);
    let mut times = Vec::new();
    for _ in 0..11 {
        let start = Instant::now();
        assert_eq!(run().await, expected);
        times.push(start.elapsed());
    }
    times.sort_unstable();
    println!("{name} median {:.3} ms", times[5].as_secs_f64() * 1000.0);
}

#[tokio::test]
#[ignore = "performance measurement"]
async fn search_and_queue() {
    assert_eq!(std::env::var("SPARES_TEST_MODE").as_deref(), Ok("1"));
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!().run(&db).await.unwrap();
    let now = DateTime::from_timestamp(1760486400, 0).unwrap();
    let data = ("word αβγ ".repeat(120) + "{{ answer }} ").repeat(8);
    sqlx::query("INSERT INTO parser (id,name, usn) VALUES (1,'markdown', -1)")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<1000)
      INSERT INTO note(id, data, created_at, updated_at, custom_data, parser_id, usn) SELECT i,CASE WHEN i<=100 THEN ? ELSE 'short' END,0,0,'{}',1,-1 FROM n")
        .bind(data).execute(&db).await.unwrap();
    sqlx::raw_sql(r#"WITH RECURSIVE o(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM o WHERE i<8)
      INSERT INTO card(id,note_id,"order",back_type,created_at,updated_at,due,stability,stability_fast,difficulty,desired_retention,state,custom_data,usn)
      SELECT (n.id-1)*8+i,n.id,i,1,0,0,1760486300,10,10,5,0.9,2,'{}',-1 FROM note n CROSS JOIN o WHERE n.id<=100 OR i=1;
      INSERT INTO tag (id,name,description,auto_delete, created_at, updated_at, usn, reschedule) VALUES(1,'sample','',0, strftime('%s', 'now'), strftime('%s', 'now'), -1, 1);
      INSERT INTO note_tag(note_id,tag_id) SELECT id,1 FROM note;
      INSERT INTO note_keyword(note_id,keyword,embedded) SELECT id,'key',0 FROM note;
      INSERT INTO note_link(parent_note_id,linked_note_id,"order",searched_keyword,relation) SELECT id,id,1,'key',0 FROM note;
      WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM r WHERE i<80)
      INSERT INTO review_log(card_id,reviewed_at,kind,rating,scheduler_name,scheduled_time,recall_duration,rate_duration,previous_state,custom_data,usn)
      SELECT id,1750000000+i,0,3,'fsrs',86400,5,2,2,'{}',-1 FROM card CROSS JOIN r WHERE note_id>100;
      ANALYZE;"#).execute(&db).await.unwrap();
    for links in ["false", "true"] {
        sqlx::query(
            "INSERT OR REPLACE INTO app_state(key,value) VALUES('linked_notes_generated',?)",
        )
        .bind(links)
        .execute(&db)
        .await
        .unwrap();
        for size in [10, 100, 1000] {
            measure(&format!("notes {size} links {links}"), size, || async {
                let SearchNotesResponse::Notes(rows) = search_notes(
                    &db,
                    SearchNotesRequest {
                        query: format!("limit={size}"),
                        output_type: QueryReturnItemType::Notes,
                    },
                )
                .await
                .unwrap() else {
                    unreachable!()
                };
                rows.len()
            })
            .await;
        }
    }
    for (label, query, total, limit) in [
        ("cloze", "c.id<=800 c.cloze~answer", 800, 20),
        ("card page", "c.id>800", 900, 900),
    ] {
        measure(label, total, || async {
            search_page(
                &db,
                SearchPageRequest {
                    query: query.into(),
                    output_type: QueryReturnItemType::Cards,
                    page: 1,
                    limit,
                    sort: None,
                },
            )
            .await
            .unwrap()
            .total
        })
        .await;
    }
    measure("queue", 800, || async {
        queue_positions(
            &db,
            QueuePositionRequest {
                match_query: Some("c.id<=800".into()),
                ..Default::default()
            },
            now,
        )
        .await
        .unwrap()
        .positions
        .len()
    })
    .await;
}
