//! `mv:NOTE_ID/ORDER`: moving a card, with its review history, into another note

use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::api::note::create_notes;
use crate::api::note::delete_notes;
use crate::api::note::update_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::review::submit_study_action;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::model::Card;
use crate::model::CardId;
use crate::model::NoteId;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::review::RatingSubmission;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::UndoEventRequest;

async fn create_note(pool: &SqlitePool, parser_id: i64, data: &str, log: bool) -> NoteId {
    create_notes(
        pool,
        CreateNotesRequest {
            parser_id,
            requests: vec![CreateNoteRequest {
                data: data.to_string(),
                keywords: vec![],
                tags: vec![],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        log,
    )
    .await
    .unwrap()
    .notes[0]
        .id
}

async fn update_note(
    pool: &SqlitePool,
    note_id: NoteId,
    data: &str,
) -> Result<crate::schema::note::UpdateNotesResponse, crate::Error> {
    update_notes(
        pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            parser_id: None,
            data: Some(data.to_string()),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
}

async fn card_at(pool: &SqlitePool, note_id: NoteId, order: u32) -> Option<Card> {
    sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? AND "order" = ?"#)
        .bind(note_id)
        .bind(order)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn rate(pool: &SqlitePool, card_id: CardId) {
    submit_study_action(
        pool,
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
}

async fn review_count(pool: &SqlitePool, card_id: CardId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM review_log WHERE card_id = ?")
        .bind(card_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count(pool: &SqlitePool, query: &'static str) -> i64 {
    sqlx::query_scalar(query).fetch_one(pool).await.unwrap()
}

async fn undo_group(pool: &SqlitePool) {
    undo_event(
        pool,
        UndoEventRequest {
            event_id: None,
            undo_group: true,
        },
    )
    .await
    .unwrap();
}

async fn redo_group(pool: &SqlitePool) {
    redo_event(
        pool,
        RedoEventRequest {
            event_id: None,
            redo_group: true,
        },
    )
    .await
    .unwrap();
}

/// A note created with `mv:` takes the source card itself: same id, schedule and reviews, with
/// nothing copied. Undo puts it back, and redo moves it again.
#[sqlx::test]
async fn e2e_move_card_into_created_note(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    let src = create_note(&pool, parser.id, "{{ a }} {{ b }}", true).await;
    let card = card_at(&pool, src, 1).await.unwrap();
    rate(&pool, card.id).await;
    rate(&pool, card.id).await;
    let reviewed = card_at(&pool, src, 1).await.unwrap();
    let total_reviews = count(&pool, "SELECT COUNT(*) FROM review_log").await;

    let dst = create_note(&pool, parser.id, &format!("{{{{[mv:{src}/1] a }}}}"), true).await;

    let moved = card_at(&pool, dst, 1).await.unwrap();
    assert_eq!(moved.id, card.id, "the source card itself moves");
    assert_eq!(moved.stability, reviewed.stability);
    assert_eq!(moved.due, reviewed.due);
    assert_eq!(review_count(&pool, card.id).await, 2);
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM review_log").await,
        total_reviews,
        "no review is copied"
    );
    assert!(card_at(&pool, src, 1).await.is_none());
    assert!(
        card_at(&pool, src, 2).await.is_some(),
        "the other card stays"
    );
    let data: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
        .bind(dst)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!data.contains("mv:"), "`mv:` is removed once used: {data}");

    undo_group(&pool).await;
    assert_eq!(card_at(&pool, src, 1).await.unwrap().id, card.id);
    assert_eq!(review_count(&pool, card.id).await, 2);
    let dst_notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note WHERE id = ?")
        .bind(dst)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dst_notes, 0, "undo removes the created note");

    redo_group(&pool).await;
    assert_eq!(card_at(&pool, dst, 1).await.unwrap().id, card.id);
    assert!(card_at(&pool, src, 1).await.is_none());
    assert_eq!(review_count(&pool, card.id).await, 2);
}

/// Merging: an updated note takes a card with `mv:`, then the emptied source note is deleted
/// without leaving any review behind.
#[sqlx::test]
async fn e2e_move_card_into_updated_note_then_delete_source(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    let src = create_note(&pool, parser.id, "{{ a }}", true).await;
    let dst = create_note(&pool, parser.id, "{{ x }}", true).await;
    let card = card_at(&pool, src, 1).await.unwrap();
    let kept = card_at(&pool, dst, 1).await.unwrap();
    rate(&pool, card.id).await;

    update_note(
        &pool,
        dst,
        &format!("{{{{[o:1] x }}}} {{{{[mv:{src}/1] a }}}}"),
    )
    .await
    .unwrap();
    assert_eq!(card_at(&pool, dst, 1).await.unwrap().id, kept.id);
    assert_eq!(card_at(&pool, dst, 2).await.unwrap().id, card.id);
    assert_eq!(review_count(&pool, card.id).await, 1);

    // Undo and redo the update with its move
    undo_group(&pool).await;
    assert_eq!(card_at(&pool, src, 1).await.unwrap().id, card.id);
    assert!(card_at(&pool, dst, 2).await.is_none());
    redo_group(&pool).await;
    assert_eq!(card_at(&pool, dst, 2).await.unwrap().id, card.id);

    delete_notes(
        &pool,
        DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![src]),
        },
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM review_log WHERE card_id IS NULL"
        )
        .await,
        0,
        "deleting the source note leaves no review without a card"
    );
    assert_eq!(review_count(&pool, card.id).await, 1);
}

/// A `mv:` that cannot be carried out fails before the note changes.
#[sqlx::test]
async fn e2e_move_card_errors(pool: SqlitePool) {
    let parser = create_parser_helper(&pool, "markdown").await;
    let src = create_note(&pool, parser.id, "{{ a }}", true).await;
    let dst = create_note(&pool, parser.id, "{{ x }}", true).await;
    let before: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
        .bind(dst)
        .fetch_one(&pool)
        .await
        .unwrap();

    for (data, expected) in [
        (
            format!("{{{{[o:1] x }}}} {{{{[mv:{src}/1] a }}}} {{{{[mv:{src}/1] b }}}}"),
            "more than one cloze",
        ),
        (
            format!("{{{{[o:1] x }}}} {{{{[mv:{dst}/1] a }}}}"),
            "within its own note",
        ),
        (
            format!("{{{{[o:1] x }}}} {{{{[mv:{src}/9] a }}}}"),
            "source card not found",
        ),
        (format!("{{{{[o:1;mv:{src}/1] x }}}}"), "already exists"),
    ] {
        let err = update_note(&pool, dst, &data)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(expected), "{data}: {err}");
        let after: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
            .bind(dst)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(after, before, "{data}: the note must be unchanged");
        assert_eq!(card_at(&pool, src, 1).await.unwrap().note_id, src);
    }

    let created = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: format!("{{{{[mv:{src}/2] a }}}}"),
                keywords: vec![],
                tags: vec![],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await;
    assert!(
        created
            .unwrap_err()
            .to_string()
            .contains("source card not found")
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM note").await, 2);
}
