//! Full sync: one replica's database replaces the other's. The escape hatch, and the first sync.
//!
//! The incoming database is copied in table by table from an attached snapshot, in one
//! transaction, rather than by swapping the database file, so it works while the server has the
//! database open. The snapshot is made with `VACUUM INTO`, and its integrity and schema version
//! are checked first. Afterwards every row has `usn = 0`, and graves, bases and undo history are
//! cleared.
//!
//! - **Upload:** the client marks everything synced and sets `needs_full_sync` until the upload
//!   succeeds. Edits made during the upload stay dirty, with what was uploaded as their base. The
//!   server gets a new epoch, so any other client must full sync too.
//! - **Download:** the client continues from the `sync_usn` and epoch in the server's snapshot.
//!   Rows that were dirty on the server are stamped and sent again on the next pull, where they
//!   merge to themselves.

use std::path::Path;

use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use super::SYNC_EPOCH_KEY;
use super::SYNC_USN_KEY;
use super::USN_TABLES;
use super::bump_notes_version;
use super::guard_undo;
use super::schema_version;
use super::set_applying;
use super::set_state;
use crate::Error;

/// The tables a full sync copies, parents first. `id_counter` is merged instead (see
/// [`restore`]). Undo history, graves, bases and other local state are not copied.
const DATA_TABLES: [&str; 12] = [
    "parser",
    "tag",
    "saved_search",
    "note",
    "note_keyword",
    "note_link",
    "note_tag",
    "card",
    "card_tag",
    "review_log",
    "render_asset",
    "render_package",
];

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

fn path_str(path: &Path) -> Result<&str, Error> {
    path.to_str()
        .ok_or_else(|| Error::Sync(format!("{} is not valid UTF-8", path.display())))
}

/// Writes a consistent copy of the database to `path`.
pub async fn snapshot(db: &SqlitePool, path: &Path) -> Result<(), Error> {
    // `VACUUM INTO` refuses to overwrite a file.
    let _ = std::fs::remove_file(path);
    sqlx::query("VACUUM INTO ?")
        .bind(path_str(path)?)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// The sync state a restored database carried.
#[derive(Debug)]
pub struct RestoredState {
    /// The `sync_usn` and `sync_epoch` the incoming database had. A client that downloaded the
    /// server's database continues from them.
    pub usn: i64,
    pub epoch: Option<String>,
}

/// Replaces this database's contents with the database at `path`. Every row ends up synced
/// (`usn = 0`), graves, bases and undo history are cleared, and this replica's id counters never
/// go down, so it never hands out an id again that it used before.
pub async fn restore(db: &SqlitePool, path: &Path) -> Result<RestoredState, Error> {
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    // ATTACH cannot run inside a transaction.
    sqlx::query("ATTACH DATABASE ? AS incoming")
        .bind(path_str(path)?)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    let result = restore_attached(&mut conn).await;
    let detached = sqlx::query("DETACH DATABASE incoming")
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error);
    let state = result?;
    detached?;
    Ok(state)
}

async fn restore_attached(conn: &mut sqlx::SqliteConnection) -> Result<RestoredState, Error> {
    let check: Vec<String> = sqlx::query_scalar("PRAGMA incoming.integrity_check")
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if check != ["ok"] {
        return Err(Error::Sync(format!(
            "the incoming database is damaged: {}",
            check.join("; ")
        )));
    }
    let incoming_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) FROM incoming._sqlx_migrations WHERE success",
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    let version = schema_version(conn).await?;
    if incoming_version != version {
        return Err(Error::Sync(format!(
            "the incoming database is at migration {incoming_version} and this one at \
             {version}. Update spares on both sides first."
        )));
    }
    let usn: Option<String> =
        sqlx::query_scalar("SELECT value FROM incoming.app_state WHERE key = ?")
            .bind(SYNC_USN_KEY)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    let epoch: Option<String> =
        sqlx::query_scalar("SELECT value FROM incoming.app_state WHERE key = ?")
            .bind(SYNC_EPOCH_KEY)
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?;

    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    set_applying(&mut tx, true).await?;
    sqlx::query("PRAGMA defer_foreign_keys = ON")
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    for table in DATA_TABLES.iter().rev() {
        sqlx::query(AssertSqlSafe(format!("DELETE FROM main.{table}")))
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    for table in DATA_TABLES {
        sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO main.{table} SELECT * FROM incoming.{table}"
        )))
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    }
    sqlx::query(
        "INSERT INTO main.id_counter (replica, tbl, last)
         SELECT replica, tbl, last FROM incoming.id_counter WHERE true
         ON CONFLICT (replica, tbl) DO UPDATE SET last = MAX(last, excluded.last)",
    )
    .execute(&mut *tx)
    .await
    .map_err(sqlx_error)?;
    for table in USN_TABLES {
        sqlx::query(AssertSqlSafe(format!("UPDATE main.{table} SET usn = 0")))
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    bump_notes_version(&mut tx).await?;
    for table in ["grave", "sync_base", "event"] {
        sqlx::query(AssertSqlSafe(format!("DELETE FROM main.{table}")))
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    set_state(&mut tx, "linked_notes_generated", "false").await?;
    guard_undo(&mut tx).await?;
    set_applying(&mut tx, false).await?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(RestoredState {
        usn: usn.and_then(|u| u.parse().ok()).unwrap_or(0),
        epoch,
    })
}
