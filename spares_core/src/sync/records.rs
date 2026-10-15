//! The rows sync carries between replicas, and reading them from the database.
//!
//! A note travels with its tag ids and the keywords not embedded in its text; a card with its tag
//! ids. Links and embedded keywords are derived from the note's text, so each replica rebuilds
//! them instead (see [`super::apply`]). Columns are kept in their stored form (Unix times, enum
//! integers), so a record round-trips without loss and compares exactly.
//!
//! A note's and a card's record have the same JSON shape as their `sync_base` snapshot, so the
//! base, the local row and the remote row of a merge are the same type.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqliteConnection;

use super::SyncKind;
use crate::Error;
use crate::model::CardId;
use crate::model::NoteId;
use crate::model::TagId;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, sqlx::FromRow)]
pub struct ParserRecord {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, sqlx::FromRow)]
pub struct TagRecord {
    pub id: TagId,
    pub name: String,
    pub description: String,
    pub query: Option<String>,
    pub auto_delete: i64,
    /// Absent from snapshots taken before tags had it, which all rescheduled.
    #[serde(default = "reschedule_default")]
    pub reschedule: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

fn reschedule_default() -> i64 {
    1
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NoteRecord {
    pub id: NoteId,
    pub data: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub custom_data: Value,
    pub parser_id: i64,
    /// Sorted.
    pub tags: Vec<TagId>,
    /// The keywords not embedded in the text. Sorted.
    pub keywords: Vec<String>,
}

/// One side of a conflict, as `sync_conflict` keeps it: the note, and which card each order in
/// its text named on that side, so a text from that side can be applied here without moving
/// reviews to the wrong card.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConflictSide {
    #[serde(flatten)]
    pub note: NoteRecord,
    pub cards: std::collections::BTreeMap<u32, CardId>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CardRecord {
    pub id: CardId,
    pub note_id: NoteId,
    pub order: i64,
    pub back_type: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub due: i64,
    pub stability: f64,
    pub difficulty: f64,
    pub desired_retention: f64,
    pub special_state: Option<i64>,
    pub state: i64,
    pub custom_data: Value,
    /// Sorted.
    pub tags: Vec<TagId>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, sqlx::FromRow)]
pub struct ReviewLogRecord {
    pub id: i64,
    pub card_id: Option<CardId>,
    pub reviewed_at: i64,
    pub kind: i64,
    pub rating: Option<i64>,
    pub scheduler_name: String,
    pub scheduled_time: Option<i64>,
    pub recall_duration: Option<i64>,
    pub rate_duration: Option<i64>,
    pub previous_state: i64,
    pub tag_id: Option<TagId>,
    pub custom_data: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Grave {
    pub kind: SyncKind,
    pub oid: i64,
}

/// A set of changed rows, as one replica sends them to the other.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Changes {
    pub parsers: Vec<ParserRecord>,
    pub tags: Vec<TagRecord>,
    pub notes: Vec<NoteRecord>,
    pub cards: Vec<CardRecord>,
    pub review_logs: Vec<ReviewLogRecord>,
    pub graves: Vec<Grave>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.parsers.is_empty()
            && self.tags.is_empty()
            && self.notes.is_empty()
            && self.cards.is_empty()
            && self.review_logs.is_empty()
            && self.graves.is_empty()
    }

    pub fn len(&self) -> usize {
        self.parsers.len()
            + self.tags.len()
            + self.notes.len()
            + self.cards.len()
            + self.review_logs.len()
            + self.graves.len()
    }
}

/// Which rows to read: those changed locally and not synced yet (`usn = -1`), or those stamped
/// after a given usn (the server answering a pull).
#[derive(Clone, Copy, Debug)]
pub enum Selection {
    Dirty,
    After(i64),
}

impl Selection {
    fn condition(self) -> String {
        match self {
            Self::Dirty => "usn = -1".to_string(),
            Self::After(usn) => format!("usn > {usn}"),
        }
    }
}

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

fn json_error(e: &serde_json::Error) -> Error {
    Error::Library(crate::LibraryError::InvalidConfig(format!(
        "invalid sync snapshot: {e}"
    )))
}

pub(crate) async fn read_parsers(
    conn: &mut SqliteConnection,
    selection: Selection,
) -> Result<Vec<ParserRecord>, Error> {
    sqlx::query_as(&format!(
        "SELECT id, name FROM parser WHERE {} ORDER BY id",
        selection.condition()
    ))
    .fetch_all(&mut *conn)
    .await
    .map_err(sqlx_error)
}

pub(crate) async fn read_tags(
    conn: &mut SqliteConnection,
    selection: Selection,
) -> Result<Vec<TagRecord>, Error> {
    sqlx::query_as(&format!(
        "SELECT id, name, description, query, auto_delete, reschedule, created_at, updated_at FROM tag
         WHERE {} ORDER BY id",
        selection.condition()
    ))
    .fetch_all(&mut *conn)
    .await
    .map_err(sqlx_error)
}

async fn read_snapshots<T: serde::de::DeserializeOwned>(
    conn: &mut SqliteConnection,
    query: &str,
) -> Result<Vec<T>, Error> {
    let snapshots: Vec<String> = sqlx::query_scalar(query)
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    snapshots
        .iter()
        .map(|s| serde_json::from_str(s).map_err(|e| json_error(&e)))
        .collect()
}

pub(crate) async fn read_notes(
    conn: &mut SqliteConnection,
    selection: Selection,
) -> Result<Vec<NoteRecord>, Error> {
    read_snapshots(
        conn,
        &format!(
            "SELECT s.snapshot FROM sync_note_snapshot s
             WHERE s.id IN (SELECT id FROM note WHERE {}) ORDER BY s.id",
            selection.condition()
        ),
    )
    .await
}

pub(crate) async fn read_cards(
    conn: &mut SqliteConnection,
    selection: Selection,
) -> Result<Vec<CardRecord>, Error> {
    read_snapshots(
        conn,
        &format!(
            "SELECT s.snapshot FROM sync_card_snapshot s
             WHERE s.id IN (SELECT id FROM card WHERE {}) ORDER BY s.id",
            selection.condition()
        ),
    )
    .await
}

/// Every card of the given notes.
pub(crate) async fn read_cards_of_notes(
    conn: &mut SqliteConnection,
    note_ids: &[NoteId],
) -> Result<Vec<CardRecord>, Error> {
    let mut cards = Vec::new();
    for chunk in note_ids.chunks(crate::api::MAX_ROWS_IN_QUERY) {
        let ids = chunk
            .iter()
            .map(|id| id.packed().to_string())
            .collect::<Vec<_>>()
            .join(",");
        cards.extend(
            read_snapshots::<CardRecord>(
                conn,
                &format!(
                    "SELECT s.snapshot FROM sync_card_snapshot s
                     WHERE s.id IN (SELECT id FROM card WHERE note_id IN ({ids})) ORDER BY s.id"
                ),
            )
            .await?,
        );
    }
    Ok(cards)
}

pub(crate) async fn read_review_logs(
    conn: &mut SqliteConnection,
    selection: Selection,
) -> Result<Vec<ReviewLogRecord>, Error> {
    sqlx::query_as(&format!(
        "SELECT id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
                recall_duration, rate_duration, previous_state, tag_id, custom_data
         FROM review_log WHERE {} ORDER BY id",
        selection.condition()
    ))
    .fetch_all(&mut *conn)
    .await
    .map_err(sqlx_error)
}

pub(crate) async fn read_graves(
    conn: &mut SqliteConnection,
    selection: Selection,
) -> Result<Vec<Grave>, Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT kind, oid FROM grave WHERE {} ORDER BY kind, oid",
        selection.condition()
    ))
    .fetch_all(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    rows.into_iter()
        .map(|(kind, oid)| {
            Ok(Grave {
                kind: SyncKind::from_name(&kind)?,
                oid,
            })
        })
        .collect()
}

/// Every row in `selection`.
pub(crate) async fn read_changes(
    conn: &mut SqliteConnection,
    selection: Selection,
) -> Result<Changes, Error> {
    Ok(Changes {
        parsers: read_parsers(conn, selection).await?,
        tags: read_tags(conn, selection).await?,
        notes: read_notes(conn, selection).await?,
        cards: read_cards(conn, selection).await?,
        review_logs: read_review_logs(conn, selection).await?,
        graves: read_graves(conn, selection).await?,
    })
}

/// One row's current record, or `None` if it does not exist.
pub(crate) async fn read_note(
    conn: &mut SqliteConnection,
    id: NoteId,
) -> Result<Option<NoteRecord>, Error> {
    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT snapshot FROM sync_note_snapshot WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    snapshot
        .map(|s| serde_json::from_str(&s).map_err(|e| json_error(&e)))
        .transpose()
}

pub(crate) async fn read_card(
    conn: &mut SqliteConnection,
    id: CardId,
) -> Result<Option<CardRecord>, Error> {
    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT snapshot FROM sync_card_snapshot WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    snapshot
        .map(|s| serde_json::from_str(&s).map_err(|e| json_error(&e)))
        .transpose()
}

pub(crate) async fn read_tag(
    conn: &mut SqliteConnection,
    id: TagId,
) -> Result<Option<TagRecord>, Error> {
    sqlx::query_as(
        "SELECT id, name, description, query, auto_delete, reschedule, created_at, updated_at FROM tag
         WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(sqlx_error)
}

pub(crate) async fn read_parser(
    conn: &mut SqliteConnection,
    id: i64,
) -> Result<Option<ParserRecord>, Error> {
    sqlx::query_as("SELECT id, name FROM parser WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(sqlx_error)
}

/// The row's `usn`, or `None` if it does not exist.
pub(crate) async fn read_usn(
    conn: &mut SqliteConnection,
    kind: SyncKind,
    id: i64,
) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(&format!("SELECT usn FROM {} WHERE id = ?", kind.name()))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(sqlx_error)
}

/// The row's last synced version, if it changed since.
pub(crate) async fn read_base<T: serde::de::DeserializeOwned>(
    conn: &mut SqliteConnection,
    kind: SyncKind,
    id: i64,
) -> Result<Option<T>, Error> {
    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT snapshot FROM sync_base WHERE kind = ? AND oid = ?")
            .bind(kind.name())
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    snapshot
        .map(|s| serde_json::from_str(&s).map_err(|e| json_error(&e)))
        .transpose()
}

pub(crate) async fn has_grave(
    conn: &mut SqliteConnection,
    kind: SyncKind,
    id: i64,
) -> Result<bool, Error> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM grave WHERE kind = ? AND oid = ? AND usn = -1")
            .bind(kind.name())
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    Ok(found.is_some())
}
