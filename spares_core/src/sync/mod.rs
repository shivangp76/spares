//! Two-way sync between the laptop and the server. See sections 5–7 of `docs/sync-plan.md`.
//!
//! Change tracking lives in the database (migration `20261014130000_sync`): every synced table
//! has a `usn` column that is `-1` for rows changed since the last sync, deletes leave a row in
//! `grave`, and the first change after a sync saves the row's synced version in `sync_base` for
//! the three-way merge. Triggers maintain all of this, so no write path can be missed. Sync turns
//! them off with [`set_applying`] while it writes the other replica's changes.

use sqlx::SqliteConnection;

use crate::Error;

#[cfg(test)]
mod tracking_tests;

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
