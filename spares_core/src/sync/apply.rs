//! Writing records from the other replica. Everything here runs with the triggers off
//! ([`super::set_applying`]), so it stamps `usn` itself and writes graves itself where it needs
//! them.
//!
//! Rows are written parents first and deleted children first, so foreign keys hold after every
//! statement. Upserts use `ON CONFLICT DO UPDATE`, never `INSERT OR REPLACE`, which deletes the
//! old row first and so cascades to its children.
//!
//! Incoming notes' `live_block_order` is cleared before notes are written, so two live notes
//! swapping block orders in one sync don't trip the unique index.

use std::collections::BTreeSet;

use sqlx::SqliteConnection;

use super::SyncKind;
use super::records::CardRecord;
use super::records::Changes;
use super::records::Grave;
use super::records::NoteRecord;
use super::records::ParserRecord;
use super::records::ReviewLogRecord;
use super::records::TagRecord;
use crate::Error;
use crate::api::note::update::links::update_note_links;
use crate::model::NoteId;
use crate::parsers::extract_and_combine_keywords;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

pub(crate) fn sync_error(message: impl Into<String>) -> Error {
    Error::Sync(message.into())
}

fn id_list(ids: impl IntoIterator<Item = i64>) -> String {
    ids.into_iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) async fn upsert_parser(
    conn: &mut SqliteConnection,
    record: &ParserRecord,
    usn: i64,
) -> Result<(), Error> {
    let clash: Option<i64> = sqlx::query_scalar("SELECT id FROM parser WHERE name = ? AND id != ?")
        .bind(&record.name)
        .bind(record.id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if let Some(clash) = clash {
        return Err(sync_error(format!(
            "both replicas have a parser named `{}` (ids {clash} and {}). Rename or delete one \
             of them on one side and sync again.",
            record.name, record.id
        )));
    }
    sqlx::query(
        "INSERT INTO parser (id, name, usn) VALUES (?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET name = excluded.name, usn = excluded.usn",
    )
    .bind(record.id)
    .bind(&record.name)
    .bind(usn)
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

pub(crate) async fn upsert_tag(
    conn: &mut SqliteConnection,
    record: &TagRecord,
    usn: i64,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO tag (id, name, description, query, auto_delete, reschedule, created_at,
             updated_at, usn)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET name = excluded.name,
             description = excluded.description, query = excluded.query,
             auto_delete = excluded.auto_delete, reschedule = excluded.reschedule,
             created_at = excluded.created_at, updated_at = excluded.updated_at,
             usn = excluded.usn",
    )
    .bind(record.id)
    .bind(&record.name)
    .bind(&record.description)
    .bind(&record.query)
    .bind(record.auto_delete)
    .bind(record.reschedule)
    .bind(record.created_at)
    .bind(record.updated_at)
    .bind(usn)
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

/// Writes a note and its tags and keywords. When its text changed (`rebuild`), also rebuilds what
/// is derived from the text: embedded keywords and links. Tag ids that do not exist here are
/// dropped.
pub(crate) async fn upsert_note(
    conn: &mut SqliteConnection,
    record: &NoteRecord,
    usn: i64,
    rebuild: bool,
) -> Result<(), Error> {
    // A live note's `(live_sync_name, live_block_order)` is unique, and another note written in
    // the same sync may still hold this one's: callers first call `release_live_keys` on all of
    // them.
    sqlx::query(
        "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id, usn)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET data = excluded.data, created_at = excluded.created_at,
             updated_at = excluded.updated_at, custom_data = excluded.custom_data,
             parser_id = excluded.parser_id, usn = excluded.usn",
    )
    .bind(record.id)
    .bind(&record.data)
    .bind(record.created_at)
    .bind(record.updated_at)
    .bind(&record.custom_data)
    .bind(record.parser_id)
    .bind(usn)
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;

    sqlx::query("DELETE FROM note_tag WHERE note_id = ?")
        .bind(record.id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if !record.tags.is_empty() {
        sqlx::query(&format!(
            "INSERT INTO note_tag (note_id, tag_id) SELECT ?, id FROM tag WHERE id IN ({})",
            id_list(record.tags.iter().copied())
        ))
        .bind(record.id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }

    let keywords: Vec<(String, bool)> = if rebuild {
        let parser_name: String = sqlx::query_scalar("SELECT name FROM parser WHERE id = ?")
            .bind(record.parser_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        let parser = find_parser(&parser_name, &get_all_parsers())?;
        update_note_links(&mut *conn, record.id, parser.as_ref(), &record.data).await?;
        sqlx::query(
            "INSERT INTO app_state (key, value) VALUES ('linked_notes_generated', 'false')
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
        sqlx::query("DELETE FROM note_keyword WHERE note_id = ?")
            .bind(record.id)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        extract_and_combine_keywords(parser.as_ref(), &record.data, &record.keywords)?
    } else {
        sqlx::query("DELETE FROM note_keyword WHERE note_id = ? AND embedded = 0")
            .bind(record.id)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        let embedded: BTreeSet<String> = sqlx::query_scalar(
            "SELECT keyword FROM note_keyword WHERE note_id = ? AND embedded = 1",
        )
        .bind(record.id)
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?
        .into_iter()
        .collect();
        record
            .keywords
            .iter()
            .filter(|k| !embedded.contains(*k))
            .map(|k| (k.clone(), false))
            .collect()
    };
    for (keyword, embedded) in keywords {
        sqlx::query("INSERT INTO note_keyword (note_id, keyword, embedded) VALUES (?, ?, ?)")
            .bind(record.id)
            .bind(keyword)
            .bind(i32::from(embedded))
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    Ok(())
}

/// Writes a card and its tags. Returns `false`, writing nothing, if its note does not exist here.
pub(crate) async fn upsert_card(
    conn: &mut SqliteConnection,
    record: &CardRecord,
    usn: i64,
) -> Result<bool, Error> {
    if !row_exists(conn, SyncKind::Note, record.note_id.packed()).await? {
        return Ok(false);
    }
    sqlx::query(
        r#"INSERT INTO card (id, note_id, "order", back_type, created_at, updated_at, due,
               stability, difficulty, desired_retention, special_state, state, custom_data, usn)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT (id) DO UPDATE SET note_id = excluded.note_id, "order" = excluded."order",
               back_type = excluded.back_type, created_at = excluded.created_at,
               updated_at = excluded.updated_at, due = excluded.due,
               stability = excluded.stability, difficulty = excluded.difficulty,
               desired_retention = excluded.desired_retention,
               special_state = excluded.special_state, state = excluded.state,
               custom_data = excluded.custom_data, usn = excluded.usn"#,
    )
    .bind(record.id)
    .bind(record.note_id)
    .bind(record.order)
    .bind(record.back_type)
    .bind(record.created_at)
    .bind(record.updated_at)
    .bind(record.due)
    .bind(record.stability)
    .bind(record.difficulty)
    .bind(record.desired_retention)
    .bind(record.special_state)
    .bind(record.state)
    .bind(&record.custom_data)
    .bind(usn)
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    sqlx::query("DELETE FROM card_tag WHERE card_id = ?")
        .bind(record.id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if !record.tags.is_empty() {
        sqlx::query(&format!(
            "INSERT INTO card_tag (card_id, tag_id) SELECT ?, id FROM tag WHERE id IN ({})",
            id_list(record.tags.iter().copied())
        ))
        .bind(record.id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    Ok(true)
}

/// Adds a review log, or links an existing one to its card and tag if this side had lost them.
/// A card or tag that does not exist here is left out (`NULL`), as when it is deleted.
pub(crate) async fn upsert_review_log(
    conn: &mut SqliteConnection,
    record: &ReviewLogRecord,
    usn: i64,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO review_log (id, card_id, reviewed_at, kind, rating, scheduler_name,
             scheduled_time, recall_duration, rate_duration, previous_state, tag_id, custom_data,
             usn)
         VALUES (?, (SELECT id FROM card WHERE id = ?), ?, ?, ?, ?, ?, ?, ?, ?,
             (SELECT id FROM tag WHERE id = ?), ?, ?)
         ON CONFLICT (id) DO UPDATE SET
             card_id = COALESCE(excluded.card_id, review_log.card_id),
             tag_id = COALESCE(excluded.tag_id, review_log.tag_id),
             usn = excluded.usn",
    )
    .bind(record.id)
    .bind(record.card_id)
    .bind(record.reviewed_at)
    .bind(record.kind)
    .bind(record.rating)
    .bind(&record.scheduler_name)
    .bind(record.scheduled_time)
    .bind(record.recall_duration)
    .bind(record.rate_duration)
    .bind(record.previous_state)
    .bind(record.tag_id)
    .bind(&record.custom_data)
    .bind(usn)
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

pub(crate) async fn row_exists(
    conn: &mut SqliteConnection,
    kind: SyncKind,
    id: i64,
) -> Result<bool, Error> {
    let found: Option<i64> =
        sqlx::query_scalar(&format!("SELECT 1 FROM {} WHERE id = ?", kind.name()))
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    Ok(found.is_some())
}

/// Deletes a row. Its children go with it (cards with their note; tags and review logs keep the
/// review, with its card or tag left out).
pub(crate) async fn delete_row(
    conn: &mut SqliteConnection,
    kind: SyncKind,
    id: i64,
) -> Result<(), Error> {
    sqlx::query(&format!("DELETE FROM {} WHERE id = ?", kind.name()))
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    sqlx::query("DELETE FROM sync_base WHERE kind = ? AND oid = ?")
        .bind(kind.name())
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

pub(crate) async fn insert_grave(
    conn: &mut SqliteConnection,
    grave: Grave,
    usn: i64,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO grave (oid, kind, usn) VALUES (?, ?, ?)
         ON CONFLICT (oid, kind) DO UPDATE SET usn = excluded.usn",
    )
    .bind(grave.oid)
    .bind(grave.kind.name())
    .bind(usn)
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

pub(crate) async fn remove_grave(conn: &mut SqliteConnection, grave: Grave) -> Result<(), Error> {
    sqlx::query("DELETE FROM grave WHERE kind = ? AND oid = ?")
        .bind(grave.kind.name())
        .bind(grave.oid)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// Marks rows as changed here, so the next push sends them, keeping their synced version as the
/// base first, as the triggers would.
pub(crate) async fn mark_dirty(
    conn: &mut SqliteConnection,
    kind: SyncKind,
    ids: &[i64],
) -> Result<(), Error> {
    for chunk in ids.chunks(crate::api::MAX_ROWS_IN_QUERY) {
        let ids = id_list(chunk.iter().copied());
        let snapshot = match kind {
            SyncKind::Note => Some("sync_note_snapshot"),
            SyncKind::Card => Some("sync_card_snapshot"),
            SyncKind::Parser | SyncKind::Tag => None,
        };
        if let Some(view) = snapshot {
            sqlx::query(&format!(
                "INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
                 SELECT ?, s.id, s.snapshot FROM {view} s JOIN {table} t ON t.id = s.id
                 WHERE s.id IN ({ids}) AND t.usn != -1",
                table = kind.name()
            ))
            .bind(kind.name())
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        }
        sqlx::query(&format!(
            "UPDATE {} SET usn = -1 WHERE id IN ({ids})",
            kind.name()
        ))
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    Ok(())
}

/// Clears a note's live-note key while its record is being rewritten, so that two live notes
/// swapping block orders in one sync do not clash on the unique index. The record puts it back.
pub(crate) async fn release_live_keys(
    conn: &mut SqliteConnection,
    note_ids: &[NoteId],
) -> Result<(), Error> {
    for chunk in note_ids.chunks(crate::api::MAX_ROWS_IN_QUERY) {
        sqlx::query(&format!(
            "UPDATE note SET custom_data = json_remove(custom_data, '$.live_block_order')
             WHERE id IN ({}) AND json_extract(custom_data, '$.live_block_order') IS NOT NULL",
            id_list(chunk.iter().map(|id| id.packed()))
        ))
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    Ok(())
}

/// What applying a set of changes touched, for regenerating files afterwards.
#[derive(Debug, Default)]
pub struct Applied {
    /// Notes whose text, tags or cards may have changed.
    pub notes_changed: BTreeSet<NoteId>,
    pub notes_deleted: BTreeSet<NoteId>,
}

/// Applies `changes` exactly as given, stamping every row with `usn`. The server does this with a
/// push, whose changes the client has already merged. Graves are kept (with `usn`) so that other
/// clients learn of the deletes.
pub(crate) async fn apply_changes(
    conn: &mut SqliteConnection,
    changes: &Changes,
    usn: i64,
) -> Result<Applied, Error> {
    let mut applied = Applied::default();
    for record in &changes.parsers {
        upsert_parser(conn, record, usn).await?;
    }
    for record in &changes.tags {
        upsert_tag(conn, record, usn).await?;
    }
    for grave in &changes.graves {
        if matches!(grave.kind, SyncKind::Card | SyncKind::Note) {
            delete_row(conn, grave.kind, grave.oid).await?;
            insert_grave(conn, *grave, usn).await?;
            if grave.kind == SyncKind::Note {
                applied.notes_deleted.insert(NoteId::from_packed(grave.oid));
            }
        }
    }
    let note_ids: Vec<NoteId> = changes.notes.iter().map(|n| n.id).collect();
    release_live_keys(conn, &note_ids).await?;
    for record in &changes.notes {
        let old_data: Option<String> = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
            .bind(record.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        let rebuild = old_data.as_deref() != Some(record.data.as_str());
        upsert_note(conn, record, usn, rebuild).await?;
        remove_grave(conn, Grave::note(record.id)).await?;
        applied.notes_changed.insert(record.id);
    }
    for record in &changes.cards {
        if upsert_card(conn, record, usn).await? {
            remove_grave(
                conn,
                Grave {
                    kind: SyncKind::Card,
                    oid: record.id,
                },
            )
            .await?;
            applied.notes_changed.insert(record.note_id);
        }
    }
    for record in &changes.review_logs {
        upsert_review_log(conn, record, usn).await?;
    }
    for kind in [SyncKind::Tag, SyncKind::Parser] {
        for grave in changes.graves.iter().filter(|g| g.kind == kind) {
            if kind == SyncKind::Tag {
                let note_ids: Vec<NoteId> =
                    sqlx::query_scalar("SELECT note_id FROM note_tag WHERE tag_id = ?")
                        .bind(grave.oid)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(sqlx_error)?;
                applied.notes_changed.extend(note_ids);
            }
            delete_row(conn, grave.kind, grave.oid).await?;
            insert_grave(conn, *grave, usn).await?;
        }
    }
    applied
        .notes_changed
        .retain(|id| !applied.notes_deleted.contains(id));
    Ok(applied)
}

impl Grave {
    pub fn note(id: NoteId) -> Self {
        Self {
            kind: SyncKind::Note,
            oid: id.packed(),
        }
    }
}
