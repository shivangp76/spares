//! Notes a sync could not merge on its own (see [`super::client`]), and resolving them.
//!
//! Conflicts can only be resolved on the client, where the merge runs and stores them. The server
//! has no `sync_conflict` rows: the web app there shows the `sync-conflict` tag, but not the sides.

use std::collections::HashMap;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;

use super::client::SYNC_CONFLICT_TAG;
use super::orders;
use super::records::ConflictSide;
use crate::Error;
use crate::api::note::delete_notes;
use crate::api::note::update_notes;
use crate::model::CardId;
use crate::model::NoteId;
use crate::parsers::Parseable;
use crate::parsers::find_parser;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Both sides changed the same lines. The local text was kept.
    Text,
    /// Deleted on the other side, edited here. The note was kept.
    DeletedThere,
    /// Deleted here, edited on the other side. The note came back with the other side's text.
    DeletedHere,
}

/// A conflict as shown for resolving it. The texts have card ids in place of card orders
/// (`o:c40` is card 40), so that the same card reads the same on both sides.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConflictView {
    pub id: i64,
    pub note_id: NoteId,
    pub kind: ConflictKind,
    pub created_at: i64,
    /// The text at the last sync, if known.
    pub base: Option<String>,
    /// The note's text now, or `None` if it has been deleted since.
    pub local: Option<String>,
    /// The other side's text, or `None` if it deleted the note.
    pub remote: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Keep the note as it is now.
    Keep,
    /// Delete the note.
    Delete,
    /// Set the note's text. Card ids in it (as in [`ConflictView`]) are turned back into orders.
    Text(String),
}

struct ConflictRow {
    id: i64,
    note_id: NoteId,
    created_at: i64,
    base: Option<ConflictSide>,
    local: Option<ConflictSide>,
    remote: Option<ConflictSide>,
}

async fn read_rows(db: &SqlitePool, id: Option<i64>) -> Result<Vec<ConflictRow>, Error> {
    let rows: Vec<(i64, NoteId, i64, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, note_id, created_at, base, local, remote FROM sync_conflict
         WHERE ?1 IS NULL OR id = ?1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(db)
    .await
    .map_err(sqlx_error)?;
    let parse = |json: &str| -> Result<Option<ConflictSide>, Error> {
        serde_json::from_str(json).map_err(|e| Error::Sync(format!("invalid conflict: {e}")))
    };
    rows.into_iter()
        .map(|(id, note_id, created_at, base, local, remote)| {
            Ok(ConflictRow {
                id,
                note_id,
                created_at,
                base: base.as_deref().map(parse).transpose()?.flatten(),
                local: parse(&local)?,
                remote: parse(&remote)?,
            })
        })
        .collect()
}

/// The note's parser and which card each order names now, if the note exists.
async fn current_note(
    db: &SqlitePool,
    note_id: NoteId,
) -> Result<Option<(Box<dyn Parseable>, String, Vec<(u32, CardId)>)>, Error> {
    let note: Option<(String, String)> = sqlx::query_as(
        "SELECT p.name, n.data FROM note n JOIN parser p ON p.id = n.parser_id WHERE n.id = ?",
    )
    .bind(note_id)
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)?;
    let Some((parser_name, data)) = note else {
        return Ok(None);
    };
    let parser = find_parser(&parser_name, &crate::parsers::get_all_parsers())?;
    let cards: Vec<(u32, CardId)> =
        sqlx::query_as(r#"SELECT "order", id FROM card WHERE note_id = ?"#)
            .bind(note_id)
            .fetch_all(db)
            .await
            .map_err(sqlx_error)?;
    Ok(Some((parser, data, cards)))
}

fn tokenized(parser: &dyn Parseable, side: &ConflictSide) -> String {
    let cards: HashMap<u32, CardId> = side.cards.iter().map(|(o, id)| (*o, *id)).collect();
    orders::to_card_ids(parser, &side.note.data, &cards).unwrap_or_else(|_| side.note.data.clone())
}

pub async fn list_conflicts(db: &SqlitePool) -> Result<Vec<ConflictView>, Error> {
    let mut views = Vec::new();
    for row in read_rows(db, None).await? {
        let current = current_note(db, row.note_id).await?;
        let kind = match (&row.local, &row.remote) {
            (_, None) => ConflictKind::DeletedThere,
            (None, _) => ConflictKind::DeletedHere,
            _ => ConflictKind::Text,
        };
        let (base, local, remote) = match &current {
            Some((parser, data, cards)) => {
                let cards: HashMap<u32, CardId> = cards.iter().copied().collect();
                (
                    row.base.as_ref().map(|b| tokenized(parser.as_ref(), b)),
                    Some(
                        orders::to_card_ids(parser.as_ref(), data, &cards)
                            .unwrap_or_else(|_| data.clone()),
                    ),
                    row.remote.as_ref().map(|r| tokenized(parser.as_ref(), r)),
                )
            }
            None => (
                row.base.map(|b| b.note.data),
                None,
                row.remote.map(|r| r.note.data),
            ),
        };
        views.push(ConflictView {
            id: row.id,
            note_id: row.note_id,
            kind,
            created_at: row.created_at,
            base,
            local,
            remote,
        });
    }
    Ok(views)
}

/// Resolves a conflict, as a normal change that the next sync sends. The note loses its
/// `sync-conflict` tag once it has no other conflicts.
pub async fn resolve_conflict(
    db: &SqlitePool,
    id: i64,
    resolution: Resolution,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(), Error> {
    let Some(row) = read_rows(db, Some(id)).await?.pop() else {
        return Err(Error::Sync(format!("there is no conflict {id}")));
    };
    let current = current_note(db, row.note_id).await?;
    let others: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sync_conflict WHERE note_id = ? AND id != ?")
            .bind(row.note_id)
            .bind(id)
            .fetch_one(db)
            .await
            .map_err(sqlx_error)?;
    match (resolution, current) {
        (Resolution::Delete, Some(_)) => {
            delete_notes(
                db,
                DeleteNotesRequest {
                    selector: NotesSelector::Ids(vec![row.note_id]),
                },
                all_parsers,
                true,
            )
            .await?;
        }
        (Resolution::Keep | Resolution::Delete, None) => {}
        (Resolution::Text(_), None) => {
            return Err(Error::Sync(format!(
                "note {} no longer exists",
                row.note_id.letters()
            )));
        }
        (resolution, Some((parser, _, cards))) => {
            let data = match resolution {
                Resolution::Text(text) => {
                    let by_id: HashMap<CardId, u32> =
                        cards.into_iter().map(|(order, id)| (id, order)).collect();
                    Some(orders::to_current_orders(parser.as_ref(), &text, &by_id)?)
                }
                _ => None,
            };
            let tags_to_remove = (others == 0).then(|| vec![SYNC_CONFLICT_TAG.to_string()]);
            if data.is_some() || tags_to_remove.is_some() {
                update_notes(
                    db,
                    UpdateNotesRequest {
                        selector: NotesSelector::Ids(vec![row.note_id]),
                        parser_id: None,
                        data,
                        keywords: None,
                        tags: UpdateTags::ModifyTags {
                            tags_to_remove,
                            tags_to_add: None,
                        },
                        custom_data: None,
                    },
                    Utc::now(),
                    all_parsers,
                    true,
                )
                .await?;
            }
        }
    }
    sqlx::query("DELETE FROM sync_conflict WHERE id = ?")
        .bind(id)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}
