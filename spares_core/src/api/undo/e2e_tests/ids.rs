//! Every row of a table that sync carries gets its id from `ids::next_ids`, including rows
//! created by redo.

use chrono::Utc;
use serde_json::Map;

use super::create_card_helper;
use crate::api::card::forget_card;
use crate::api::note::create_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::review::submit_study_action;
use crate::api::tag::create_tag;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::ids::ID_STEP;
use crate::ids::IdTable;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::review::RatingSubmission;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;
use crate::schema::tag::CreateTagRequest;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::UndoEventRequest;

#[sqlx::test]
async fn e2e_synced_tables_only_get_helper_ids(
    pool_options: sqlx::sqlite::SqlitePoolOptions,
    options: sqlx::sqlite::SqliteConnectOptions,
) {
    // Its filtered tag searches text, which needs `REGEXP`.
    let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
    let card_id = create_card_helper(&pool).await;
    submit_study_action(
        &pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating: 3,
                recall_duration: chrono::Duration::seconds(5),
                rate_duration: chrono::Duration::seconds(2),
                tag_id: None,
            }),
        },
        Utc::now(),
    )
    .await
    .unwrap();
    forget_card(&pool, card_id, Utc::now(), true).await.unwrap();
    let forget_log_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM review_log WHERE card_id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    // Undo and redo the forget: the redone marker keeps its id.
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .unwrap();
    redo_event(
        &pool,
        RedoEventRequest {
            event_id: None,
            redo_group: false,
        },
    )
    .await
    .unwrap();
    let restored: i64 = sqlx::query_scalar("SELECT MAX(id) FROM review_log WHERE card_id = ?")
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        restored, forget_log_id,
        "redo must reuse the review log's id"
    );

    // A note that inherits the card's history (copies its review logs), with a new tag.
    create_parser_helper(&pool, "typst").await;
    let src_note_id: crate::model::NoteId =
        sqlx::query_scalar("SELECT note_id FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let markdown_id: i64 = sqlx::query_scalar("SELECT id FROM parser WHERE name = 'markdown'")
        .fetch_one(&pool)
        .await
        .unwrap();
    create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: markdown_id,
            requests: vec![CreateNoteRequest {
                data: format!("Hi {{{{[inh:{}/1] there }}}}", src_note_id.letters()),
                keywords: vec![],
                tags: vec!["fresh".to_string()],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    create_tag(
        &pool,
        CreateTagRequest {
            name: "filtered".to_string(),
            description: String::new(),
            query: Some("c.new".to_string()),
            auto_delete: false,
            reschedule: true,
        },
        true,
    )
    .await
    .unwrap();

    for table in IdTable::ALL {
        let ids: Vec<i64> = sqlx::query_scalar(&format!("SELECT id FROM {}", table.name()))
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(!ids.is_empty(), "{} has no rows to check", table.name());
        for id in ids {
            assert_eq!(
                id % ID_STEP,
                0,
                "{} row {id} did not get its id from `next_ids`",
                table.name()
            );
        }
    }
    let inherited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_log WHERE card_id != ? AND card_id IS NOT NULL",
    )
    .bind(card_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(inherited > 0, "the inheriting card must have copied logs");
}
