//! Tests of the change-tracking triggers in migration `20261014130000_sync` (and
//! `20261017140000_saved_search_sync` for saved searches).

use chrono::Utc;
use serde_json::Map;
use serde_json::Value;
use sqlx::SqlitePool;

use super::set_applying;
use crate::api::note::create_notes;
use crate::api::note::delete_notes;
use crate::api::note::update_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::api::tag::delete_tag;
use crate::model::NoteId;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;

async fn create_note(pool: &SqlitePool, data: &str, tags: &[&str]) -> NoteId {
    let parser_id: i64 = match sqlx::query_scalar("SELECT id FROM parser WHERE name = 'markdown'")
        .fetch_optional(pool)
        .await
        .unwrap()
    {
        Some(id) => id,
        None => create_parser_helper(pool, "markdown").await.id,
    };
    let response = create_notes(
        pool,
        CreateNotesRequest {
            parser_id,
            requests: vec![CreateNoteRequest {
                data: data.to_string(),
                keywords: vec![],
                tags: tags.iter().map(|t| (*t).to_string()).collect(),
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    response.notes[0].id
}

/// Marks everything as synced at `usn`, as a sync would.
async fn stamp_all(pool: &SqlitePool, usn: i64) {
    let mut tx = pool.begin().await.unwrap();
    set_applying(&mut tx, true).await.unwrap();
    for table in super::USN_TABLES {
        sqlx::query(&format!("UPDATE {table} SET usn = ?"))
            .bind(usn)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    for table in ["grave", "sync_base"] {
        sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    set_applying(&mut tx, false).await.unwrap();
    tx.commit().await.unwrap();
}

async fn usn(pool: &SqlitePool, table: &str, id: i64) -> i64 {
    sqlx::query_scalar(&format!("SELECT usn FROM {table} WHERE id = ?"))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn base(pool: &SqlitePool, kind: &str, id: i64) -> Option<Value> {
    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT snapshot FROM sync_base WHERE kind = ? AND oid = ?")
            .bind(kind)
            .bind(id)
            .fetch_optional(pool)
            .await
            .unwrap();
    snapshot.map(|s| serde_json::from_str(&s).unwrap())
}

async fn graves(pool: &SqlitePool) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT kind, oid FROM grave ORDER BY kind, oid")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn tag_id(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM tag WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn update_data(note_id: NoteId, data: &str) -> UpdateNotesRequest {
    UpdateNotesRequest {
        selector: NotesSelector::Ids(vec![note_id]),
        parser_id: None,
        data: Some(data.to_string()),
        keywords: None,
        tags: UpdateTags::ModifyTags {
            tags_to_remove: None,
            tags_to_add: None,
        },
        custom_data: None,
    }
}

#[sqlx::test]
async fn new_rows_are_dirty(pool: SqlitePool) {
    let note_id = create_note(&pool, "Hello {{ world }}", &["math"]).await;
    assert_eq!(usn(&pool, "note", note_id.packed()).await, -1);
    let card_id: i64 = sqlx::query_scalar("SELECT id FROM card WHERE note_id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(usn(&pool, "card", card_id).await, -1);
    assert_eq!(usn(&pool, "tag", tag_id(&pool, "math").await).await, -1);
    // Nothing was synced yet, so there is no base to keep.
    let bases: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_base")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(bases, 0);
}

#[sqlx::test]
async fn first_edit_after_sync_keeps_the_synced_version(pool: SqlitePool) {
    let note_id = create_note(&pool, "Hello {{ world }}", &["math"]).await;
    stamp_all(&pool, 3).await;

    update_notes(
        &pool,
        update_data(note_id, "Hello {{ there }}"),
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    assert_eq!(usn(&pool, "note", note_id.packed()).await, -1);
    let snapshot = base(&pool, "note", note_id.packed()).await.unwrap();
    assert!(
        snapshot["data"].as_str().unwrap().contains("world"),
        "{snapshot}"
    );
    assert_eq!(
        snapshot["tags"],
        Value::from(vec![tag_id(&pool, "math").await])
    );
    assert_eq!(snapshot["keywords"], Value::Array(vec![]));

    // A second edit keeps the base from the sync.
    update_notes(
        &pool,
        update_data(note_id, "Hello {{ again }}"),
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    let snapshot = base(&pool, "note", note_id.packed()).await.unwrap();
    assert!(snapshot["data"].as_str().unwrap().contains("world"));
}

#[sqlx::test]
async fn tag_changes_keep_the_old_tags_as_base(pool: SqlitePool) {
    let note_id = create_note(&pool, "Hello {{ world }}", &["math"]).await;
    let math = tag_id(&pool, "math").await;
    stamp_all(&pool, 3).await;

    update_notes(
        &pool,
        UpdateNotesRequest {
            tags: UpdateTags::SetTags(vec!["physics".to_string()]),
            ..update_data(note_id, "Hello {{ world }}")
        },
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    assert_eq!(usn(&pool, "note", note_id.packed()).await, -1);
    let snapshot = base(&pool, "note", note_id.packed()).await.unwrap();
    assert_eq!(snapshot["tags"], Value::from(vec![math]));
}

#[sqlx::test]
async fn dirty_count_counts_changed_rows_and_graves(pool: SqlitePool) {
    stamp_all(&pool, 3).await;
    assert_eq!(super::dirty_count(&pool).await.unwrap(), 0);
    let note_id = create_note(&pool, "Hello {{ world }}", &["math"]).await;
    // The parser, the note, its card and the tag
    assert_eq!(super::dirty_count(&pool).await.unwrap(), 4);
    stamp_all(&pool, 3).await;
    sqlx::query("DELETE FROM note WHERE id = ?")
        .bind(note_id)
        .execute(&pool)
        .await
        .unwrap();
    // The note's and its card's graves
    assert_eq!(super::dirty_count(&pool).await.unwrap(), 2);
}

#[sqlx::test]
async fn unchanged_columns_do_not_mark_rows(pool: SqlitePool) {
    let note_id = create_note(&pool, "Hello {{ world }}", &[]).await;
    stamp_all(&pool, 3).await;
    sqlx::query("UPDATE note SET updated_at = updated_at + 10 WHERE id = ?")
        .bind(note_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE card SET updated_at = updated_at + 10")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(usn(&pool, "note", note_id.packed()).await, 3);
    let dirty_cards: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE usn = -1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dirty_cards, 0);
    // Embedded keywords are derived from the text, so they are not tracked either.
    sqlx::query("INSERT INTO note_keyword (note_id, keyword, embedded) VALUES (?, 'k', 1)")
        .bind(note_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(usn(&pool, "note", note_id.packed()).await, 3);
}

#[sqlx::test]
async fn card_changes_are_tracked(pool: SqlitePool) {
    let note_id = create_note(&pool, "Hello {{ world }}", &[]).await;
    stamp_all(&pool, 3).await;
    let (card_id, due): (i64, i64) = sqlx::query_as("SELECT id, due FROM card WHERE note_id = ?")
        .bind(note_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE card SET due = due + 86400 WHERE id = ?")
        .bind(card_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(usn(&pool, "card", card_id).await, -1);
    let snapshot = base(&pool, "card", card_id).await.unwrap();
    assert_eq!(snapshot["due"], Value::from(due));
    // The note itself did not change.
    assert_eq!(usn(&pool, "note", note_id.packed()).await, 3);
}

#[sqlx::test]
async fn deletes_leave_graves_without_marking_cascaded_parents(pool: SqlitePool) {
    let kept = create_note(&pool, "Kept {{ note }}", &["shared"]).await;
    let deleted = create_note(&pool, "Deleted {{ note }}", &["shared"]).await;
    let deleted_card: i64 = sqlx::query_scalar("SELECT id FROM card WHERE note_id = ?")
        .bind(deleted)
        .fetch_one(&pool)
        .await
        .unwrap();
    let shared = tag_id(&pool, "shared").await;
    stamp_all(&pool, 3).await;

    delete_notes(
        &pool,
        DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![deleted]),
        },
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        graves(&pool).await,
        [
            ("card".to_string(), deleted_card),
            ("note".to_string(), deleted.packed())
        ]
    );
    // The deleted rows keep their synced version, with the note's tags.
    let bases: Vec<(String, i64)> =
        sqlx::query_as("SELECT kind, oid FROM sync_base ORDER BY kind, oid")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        bases,
        [
            ("card".to_string(), deleted_card),
            ("note".to_string(), deleted.packed())
        ]
    );
    let snapshot = base(&pool, "note", deleted.packed()).await.unwrap();
    assert_eq!(snapshot["tags"], Value::from(vec![shared]));

    // Deleting a tag does not mark every note that had it.
    delete_tag(&pool, shared, false).await.unwrap();
    assert!(graves(&pool).await.contains(&("tag".to_string(), shared)));
    assert_eq!(usn(&pool, "note", kept.packed()).await, 3);
}

#[sqlx::test]
async fn reinserting_a_row_removes_its_grave(pool: SqlitePool) {
    let note_id = create_note(&pool, "Hello", &[]).await;
    let row: (String, String, i64) =
        sqlx::query_as("SELECT data, custom_data, parser_id FROM note WHERE id = ?")
            .bind(note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("DELETE FROM note WHERE id = ?")
        .bind(note_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(graves(&pool).await.len(), 1);
    sqlx::query("INSERT INTO note (id, data, custom_data, parser_id) VALUES (?, ?, ?, ?)")
        .bind(note_id)
        .bind(&row.0)
        .bind(&row.1)
        .bind(row.2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(graves(&pool).await, []);
}

#[sqlx::test]
async fn saved_search_changes_are_tracked(pool: SqlitePool) {
    let save = async |query: &str| {
        crate::api::saved_search::save_search(
            &pool,
            crate::schema::saved_search::SavedSearch {
                name: "due".to_string(),
                query: query.to_string(),
            },
        )
        .await
        .unwrap();
    };
    save("c.due<=today").await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM saved_search")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(usn(&pool, "saved_search", id).await, -1);
    stamp_all(&pool, 4).await;
    // Saving the same query again changes nothing that is synced.
    save("c.due<=today").await;
    assert_eq!(usn(&pool, "saved_search", id).await, 4);
    save("c.due<=today+1d").await;
    assert_eq!(usn(&pool, "saved_search", id).await, -1);
    assert_eq!(
        base(&pool, "saved_search", id).await.unwrap()["query"],
        "c.due<=today"
    );
    crate::api::saved_search::delete_saved_search(&pool, "due")
        .await
        .unwrap();
    assert_eq!(graves(&pool).await, [("saved_search".to_string(), id)]);
}

#[sqlx::test]
async fn applying_turns_tracking_off(pool: SqlitePool) {
    let note_id = create_note(&pool, "Hello {{ world }}", &["math"]).await;
    stamp_all(&pool, 3).await;
    let mut tx = pool.begin().await.unwrap();
    set_applying(&mut tx, true).await.unwrap();
    sqlx::query("UPDATE note SET data = 'changed' WHERE id = ?")
        .bind(note_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM note_tag")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM tag")
        .execute(&mut *tx)
        .await
        .unwrap();
    set_applying(&mut tx, false).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(usn(&pool, "note", note_id.packed()).await, 3);
    assert_eq!(graves(&pool).await, []);
    assert!(base(&pool, "note", note_id.packed()).await.is_none());

    // The flag is gone once the transaction ends.
    sqlx::query("UPDATE note SET data = 'changed again' WHERE id = ?")
        .bind(note_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(usn(&pool, "note", note_id.packed()).await, -1);
}
