//! Two-way sync between the laptop and the server. See `docs/src/sync_design.md`.
//!
//! Change tracking lives in the database (migration `20261014130000_sync`): every synced table
//! has a `usn` column that is `-1` for rows changed since the last sync, deletes leave a row in
//! `grave`, and the first change after a sync saves the row's synced version in `sync_base` for
//! the three-way merge. Triggers maintain all of this, so no write path can be missed. Sync turns
//! them off with [`set_applying`] while it writes the other replica's changes.

use serde::Deserialize;
use serde::Serialize;
use sqlx::SqliteConnection;

use crate::Error;

pub mod apply;
pub mod client;
pub mod conflicts;
pub mod files;
pub mod full;
pub mod http;
pub mod merge;
mod orders;
pub mod protocol;
pub mod records;
pub mod run;
pub mod server;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tracking_tests;

/// The `[sync]` table of `config.toml`. Only the machine that syncs uses it, so it is left out of
/// the copy `spares sync cloud` makes for the server (see [`crate::config::server_config`]).
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SyncConfig {
    /// The sync server's address, such as `https://spares.tail1234.ts.net`. Its API key is read
    /// from `SPARES_SYNC_API_KEY`. Two-way sync is off without it.
    pub url: Option<String>,
    /// With `url`, the local server syncs on startup and then every this many minutes. `0` turns
    /// automatic syncing off.
    pub interval_minutes: u64,
    /// Where `spares sync cloud` copies files over SSH, such as `user@myserver.com`.
    pub remote_host: Option<String>,
    /// Where `spares sync cloud` puts rendered files on `remote_host`. Relative paths are relative
    /// to the remote user's home directory. Must match the server's `SPARES_CACHE_DIR`.
    pub remote_rendered_dir: String,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            url: None,
            interval_minutes: 0,
            remote_host: None,
            remote_rendered_dir: ".cache/spares".to_string(),
        }
    }
}

/// `app_state` keys. On the server, `sync_usn` is its counter; on a client, the server's counter
/// at its last sync.
pub const SYNC_USN_KEY: &str = "sync_usn";
/// The server's epoch (see [`protocol::MetaResponse::epoch`]); on a client, the epoch of its last
/// sync.
pub const SYNC_EPOCH_KEY: &str = "sync_epoch";
pub const LAST_SYNC_AT_KEY: &str = "last_sync_at";
/// The newest event at the last sync. Undo refuses events at or below it, since what they did
/// has reached the other replica.
pub const LAST_SYNC_EVENT_ID_KEY: &str = "last_sync_event_id";
/// Set (to the reason) when only a full sync can bring the replicas back in step.
pub const NEEDS_FULL_SYNC_KEY: &str = "needs_full_sync";
/// Added to the newest note event id by [`crate::api::undo::get_latest_note_event_id`], so the
/// number readers use to notice note changes (the nvim plugin's keyword cache) also changes when
/// a sync changes notes, which writes no events. See [`bump_notes_version`].
pub const NOTES_VERSION_OFFSET_KEY: &str = "notes_version_offset";

/// The tables with a `usn` column.
pub(crate) const USN_TABLES: [&str; 5] = ["parser", "tag", "note", "card", "review_log"];

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

pub(crate) async fn get_state(
    conn: &mut SqliteConnection,
    key: &str,
) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT value FROM app_state WHERE key = ?")
        .bind(key)
        .fetch_optional(&mut *conn)
        .await
        .map_err(sqlx_error)
}

pub(crate) async fn get_state_i64(
    conn: &mut SqliteConnection,
    key: &str,
) -> Result<Option<i64>, Error> {
    Ok(get_state(conn, key)
        .await?
        .and_then(|value| value.parse().ok()))
}

pub(crate) async fn set_state(
    conn: &mut SqliteConnection,
    key: &str,
    value: impl ToString,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO app_state (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value.to_string())
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

pub(crate) async fn remove_state(conn: &mut SqliteConnection, key: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM app_state WHERE key = ?")
        .bind(key)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// Records the newest event as synced, so undo cannot reach past it.
pub(crate) async fn guard_undo(conn: &mut SqliteConnection) -> Result<(), Error> {
    let latest: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM event")
        .fetch_one(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    set_state(conn, LAST_SYNC_EVENT_ID_KEY, latest).await
}

/// Moves the note change number on after a sync changed notes. Call it before clearing `event`
/// (as a full sync does), with that clearing in the same transaction: the offset then grows by
/// more than the newest event id, so the number never comes back to a value it had before.
pub(crate) async fn bump_notes_version(conn: &mut SqliteConnection) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO app_state (key, value)
         VALUES (?1, (SELECT COALESCE(MAX(id), 0) + 1 FROM event))
         ON CONFLICT (key) DO UPDATE
         SET value = CAST(value AS INTEGER) + (SELECT COALESCE(MAX(id), 0) + 1 FROM event)",
    )
    .bind(NOTES_VERSION_OFFSET_KEY)
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

/// The newest migration this database has run.
pub(crate) async fn schema_version(conn: &mut SqliteConnection) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success")
        .fetch_one(&mut *conn)
        .await
        .map_err(sqlx_error)
}

pub(crate) async fn read_counts(conn: &mut SqliteConnection) -> Result<protocol::Counts, Error> {
    let (parsers, tags, notes, cards, review_logs): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM parser), (SELECT COUNT(*) FROM tag),
                (SELECT COUNT(*) FROM note), (SELECT COUNT(*) FROM card),
                (SELECT COUNT(*) FROM review_log)",
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(protocol::Counts {
        parsers,
        tags,
        notes,
        cards,
        review_logs,
    })
}

/// Whether anything changed here since the last sync.
pub(crate) async fn has_dirty(conn: &mut SqliteConnection) -> Result<bool, Error> {
    let query = USN_TABLES
        .iter()
        .chain(&["grave"])
        .map(|table| format!("SELECT 1 FROM {table} WHERE usn = -1"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let found: Option<i64> = sqlx::query_scalar(&format!("SELECT EXISTS ({query})"))
        .fetch_optional(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(found == Some(1))
}

/// Stamps every dirty row with `usn`.
pub(crate) async fn stamp_dirty(conn: &mut SqliteConnection, usn: i64) -> Result<(), Error> {
    for table in USN_TABLES {
        sqlx::query(&format!("UPDATE {table} SET usn = ? WHERE usn = -1"))
            .bind(usn)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    Ok(())
}

/// A table that sync carries, as named in `grave.kind` and `sync_base.kind`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncKind {
    Parser,
    Tag,
    Note,
    Card,
}

impl SyncKind {
    pub fn from_name(name: &str) -> Result<Self, Error> {
        match name {
            "parser" => Ok(Self::Parser),
            "tag" => Ok(Self::Tag),
            "note" => Ok(Self::Note),
            "card" => Ok(Self::Card),
            _ => Err(Error::Sync(format!("unknown row kind `{name}`"))),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Parser => "parser",
            Self::Tag => "tag",
            Self::Note => "note",
            Self::Card => "card",
        }
    }
}

/// Turns the change-tracking triggers off (`true`) or back on for the rest of the transaction
/// `conn` is in. Call it with `true` right after beginning the transaction and with `false`
/// before committing it: the flag is a row in `sync_applying`, which no other connection sees
/// while it is uncommitted, and which a rollback removes.
pub async fn set_applying(conn: &mut SqliteConnection, applying: bool) -> Result<(), Error> {
    let query = if applying {
        "INSERT INTO sync_applying (applying) VALUES (1)"
    } else {
        "DELETE FROM sync_applying"
    };
    sqlx::query(query)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}
