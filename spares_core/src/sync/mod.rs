//! Two-way sync between the laptop, the server and browsers that keep a copy of the collection. How
//! to set it up and use it is in the manual's Getting Started and Deploying pages.
//!
//! # Goal
//!
//! Review and edit notes from the phone anywhere, even when the laptop is off. The laptop and a
//! small server (reached over Tailscale) both hold the whole database and sync like Anki and
//! `AnkiWeb`. Notes can be edited on the laptop (CLI, note files, live notes) or on the web. A
//! browser can also keep a copy of its own and sync it like the laptop, so the phone works offline
//! ([More replicas](#more-replicas)).
//!
//! **Unsynced laptop edits must never be silently overwritten by later phone edits.** Laptop edits
//! often sit unsynced for a while.
//!
//! # Decisions
//!
//! 1. **Stay on SQLite.**
//!    - A central Postgres would need the network for everything.
//!    - Two-way Postgres replication (pgEdge Spock) uses last-writer-wins and long snowflake ids,
//!      and it would mean porting all the SQLite-specific SQL (`strftime`, `VACUUM INTO`, JSON,
//!      sqlx types).
//!    - Turso's offline sync is last-push-wins per row by default.
//!    - Merging note text is application logic whatever the database.
//! 2. **What we take from Anki:** a per-row `usn` with the server's counter as the "since" cursor,
//!    tombstones (graves), review-log union, clock, schema and count checks, and a full upload or
//!    download as the escape hatch.
//! 3. **What we don't take from Anki: last-writer-wins.** Rows changed on both sides get a
//!    three-way merge against the last synced version ([`merge`], [`client`]). A real conflict is
//!    stored and flagged, never discarded.
//! 4. **An edit beats a delete**, the opposite of Anki.
//! 5. **Live notes are read-only on the web**, because their source file is on the laptop. Their
//!    tags and keywords stay editable.
//! 6. **A replica's identity comes from the environment, never the database.** A full download
//!    replaces the database, and the laptop must not take on the server's identity. There are two
//!    settings, both required with no default (tests default to replica `0`, role `client`):
//!    - `SPARES_REPLICA`, `0`–`7` ([`crate::config::Replica`]): which id form the replica writes
//!      and its residue for the other tables (see [`crate::ids`]). `0` is the laptop, `1` the
//!      server. Any other has to be enrolled with the server first (`spares sync devices add`),
//!      which only hands out `2` to `4` ([`devices`]).
//!    - `SPARES_SYNC_ROLE`, `server` or `client` ([`crate::config::SyncRole`]): whether this
//!      instance answers `/api/sync/*` or runs the client side, including the merge.
//!
//!    Keeping them apart means a new replica only takes a new number, and code never infers the
//!    role from the number.
//!
//!    A browser has no environment. Its number comes from the server when it enrolls, and it keeps
//!    it in its own storage next to the database, so a full download doesn't change it either. A
//!    browser is always a client ([`devices`]).
//!
//! # Change tracking
//!
//! Migration `20261014130000_sync` adds:
//!
//! - **`usn`** on `parser`, `tag`, `note`, `card`, `review_log` and (since
//!   `20261017140000_saved_search_sync`) `saved_search`, defaulting to `-1` (changed and not yet
//!   synced), so new rows are dirty without a trigger. A client marks the rows it is pushing `-2`
//!   (see [`run`]); they count as dirty too.
//! - **`grave(oid, kind, usn)`**: ids deleted here and not synced yet. Without it, "deleted over
//!   there" looks the same as "never synced", and the deleted row would come back. Soft deletes
//!   would mean `deleted_at IS NULL` in every query. Inserting a row under an id with a grave (undo
//!   of a delete) removes the grave. Since `20261101130000_undo_after_sync`, review logs have
//!   graves too, as an undo deletes the review it takes back; nothing else deletes one.
//! - **`sync_received(kind, oid, event_id)`**: each note, card, tag and parser sync wrote with
//!   a change, and the newest event then, which undo checks (see [`crate::api::undo`]).
//! - **`sync_base(kind, oid, snapshot)`**: a row's last synced version, saved on its first change
//!   or delete after a sync, for the three-way merge. For a note: its data, custom data, parser,
//!   tag ids and non-embedded keywords; for a card: its fields and tag ids. The views
//!   `sync_note_snapshot` and `sync_card_snapshot` build the same JSON for current rows.
//! - **`sync_conflict`**: unresolved conflicts ([`conflicts`]). **`sync_client`** (server only):
//!   each client's last sync, to catch a client restored from a backup.
//! - **Triggers maintain all of this**, so no write path can be missed:
//!   - after an update that changed a synced column (not `updated_at` alone, which review snapshot
//!     tags rely on), snapshot the old row if it isn't dirty yet and set `usn = -1`;
//!   - before inserts and deletes on `note_tag`, `card_tag` and non-embedded `note_keyword` rows,
//!     snapshot and dirty the parent. When the parent or the tag is being deleted, the cascade runs
//!     after that row is gone, so the triggers skip it: deleting a tag doesn't mark every note that
//!     had it;
//!   - before a delete, snapshot the row with its tags; after it, insert a grave. Keeping the base
//!     of deleted cards lets the merge know which card each order in a note's synced text named.
//! - **`sync_applying`** holds one row while sync writes the other side's changes, which turns
//!   every trigger off ([`set_applying`]). A trigger can't read a TEMP table, so it is a regular
//!   table whose row is never committed.
//!
//! `note_link` and embedded keywords are derived from the text, so they aren't tracked or sent;
//! they are rebuilt wherever a note is applied. `event` and `app_state` aren't synced.
//! `render_asset` and `render_package` rows are sent in full on every push, since assets are only
//! added on the laptop, and the server keeps a row's newer file. A device sends none, and
//! gets the server's in a pull whenever they differ from its own (`render_assets_digest`), so it
//! has the laptop's latest templates, and never puts its older rows back on the server.
//!
//! # More replicas
//!
//! The laptop (`0`) and the server (`1`) were the first replicas. Browsers that keep a copy of the
//! collection are replicas `2` to `4`, clients like the laptop:
//!
//! - **Enrolling** gives one a number ([`devices`]), then it does a full download. Its note ids are
//!   its own letter after the laptop note they follow ([`crate::ids`]), and its other ids take its
//!   residue modulo 8.
//! - **Nothing in the protocol assumes one client.** The usn is the server's counter, whichever
//!   client a change came from. A pull returns everything after the client's last sync, other
//!   clients' pushes included. A push is refused while anything changed on the server since that
//!   client's pull, another client's push included, and the client starts over. `sync_client`
//!   checks each replica's last usn on its own.
//! - **A full upload replaces the server's database for everyone**: its new epoch makes every
//!   other client do a full sync. So a browser never full-uploads on its own. When it can't sync
//!   normally, the user chooses: download the server's copy and lose the browser's changes, or
//!   upload its own and make the other devices download. The server answers a full upload once
//!   the database is replaced, and regenerates every note's files afterwards, which takes minutes.
//! - **What a browser needs besides the database:** the config and parser templates
//!   (`GET /api/sync/config`, read through [`crate::platform`]), and the files its cards show
//!   ([`media`]): rendered files, and the render assets Typst cards read, which the web app
//!   downloads into Cache Storage after each sync. A browser writes no note files and renders
//!   nothing; the server does that after each push ([`files::after_sync`]). Images, audio and image
//!   occlusions added on a browser are uploaded before its push, and it downloads the server's
//!   image occlusions, which notes are parsed with ([`media`]).
//! - **When a browser syncs**: when the app opens, when it is hidden or closed, from the Sync
//!   button, after reconnecting, and 30 seconds after its last change, so a review is one sync
//!   (`src/local/copy.ts` in the web app). Unlike in Anki, a sync doesn't end undo: undoing a
//!   change after it is a change like any other, which the next sync sends. Only a change whose
//!   rows a sync wrote since can't be undone ([`crate::api::undo`]), since the undo would write
//!   over the other side's change.
//! - **API responses are `Cache-Control: no-store`.** Firefox otherwise answers a sync's
//!   `GET /api/sync/meta` from its HTTP cache, with a usn from before, and the sync takes the
//!   server for one restored from a backup.
//!
//! # The rest
//!
//! - Note and other ids that are unique across replicas: [`crate::ids`].
//! - Merge rules: [`client`] and [`merge`]; card orders in merged text: `orders`.
//! - The protocol: [`protocol`]; the server's side: [`server`]; a full sync: [`full`].
//! - Files after a sync: [`files`]; the change number after a sync: [`NOTES_VERSION_OFFSET_KEY`].
//!
//! # Sources
//!
//! - Anki's sync: <https://github.com/ankitects/anki/tree/main/rslib/src/sync> (`normal.rs`,
//!   `chunks.rs`, `graves.rs`, `meta.rs`); <https://docs.ankiweb.net/syncing.html>,
//!   <https://docs.ankiweb.net/sync-server.html>
//! - pgEdge Spock: <https://github.com/pgEdge/spock>
//! - Turso offline writes: <https://turso.tech/blog/introducing-offline-writes-for-turso>

use serde::Deserialize;
use serde::Serialize;
use sqlx::AssertSqlSafe;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use crate::Error;

pub mod apply;
pub mod client;
pub mod conflicts;
pub mod devices;
pub mod files;
pub mod full;
pub mod http;
pub mod media;
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
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SyncConfig {
    /// The sync server's address, such as `https://spares.tail1234.ts.net`. Its API key is read
    /// from `SPARES_SYNC_API_KEY`. Two-way sync is off without it.
    pub url: Option<String>,
    /// With `url`, the local server syncs on startup, on waking from sleep and then every this many
    /// minutes. `0` turns automatic syncing off, including after changes.
    pub interval_minutes: u64,
    /// With automatic syncing on, the local server also syncs once local changes (reviews, edits)
    /// have stopped for this many seconds, and at least every two minutes while they keep coming,
    /// so they reach the server without waiting for the interval. `0` turns this off.
    pub push_delay_seconds: u64,
    /// Where `spares sync cloud` copies files over SSH, such as `user@myserver.com`.
    pub remote_host: Option<String>,
    /// Where `spares sync cloud` puts rendered files on `remote_host`. Relative paths are relative
    /// to the remote user's home directory. Must match the files directory of the collection on
    /// the server.
    pub remote_rendered_dir: String,
    /// This host's replica number for the collection (see [`crate::config::Replica`]). A
    /// collection without `url` is the root of its sync chain, replica `1`, unless this says
    /// otherwise. With `url`, it is set when this host enrolls with the upstream.
    pub replica: Option<u8>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            url: None,
            interval_minutes: 0,
            push_delay_seconds: 30,
            remote_host: None,
            remote_rendered_dir: ".cache/spares".to_string(),
            replica: None,
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
/// Set (to the reason) when only a full sync can bring the replicas back in step.
pub const NEEDS_FULL_SYNC_KEY: &str = "needs_full_sync";
/// Added to the newest note event id by [`crate::api::undo::get_latest_note_event_id`], so the
/// number readers use to notice note changes (the nvim plugin's keyword cache) also changes when
/// a sync changes notes, which writes no events. Every sync that changes notes moves it on (see
/// [`bump_notes_version`]). A full sync, which clears `event`, adds more than the newest event id,
/// so the number never repeats.
pub const NOTES_VERSION_OFFSET_KEY: &str = "notes_version_offset";

/// The tables with a `usn` column.
pub(crate) const USN_TABLES: [&str; 6] = [
    "parser",
    "tag",
    "note",
    "card",
    "review_log",
    "saved_search",
];

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

/// Drops the rows of `sync_received` older than every event, which can't stop an undo any more
/// (see [`crate::api::undo`]).
pub(crate) async fn prune_received(conn: &mut SqliteConnection) -> Result<(), Error> {
    sqlx::query("DELETE FROM sync_received WHERE event_id < (SELECT MIN(id) FROM event)")
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(())
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
    let (parsers, tags, notes, cards, review_logs, saved_searches): (i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM parser), (SELECT COUNT(*) FROM tag),
                    (SELECT COUNT(*) FROM note), (SELECT COUNT(*) FROM card),
                    (SELECT COUNT(*) FROM review_log), (SELECT COUNT(*) FROM saved_search)",
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
        saved_searches,
    })
}

/// Whether anything changed here since the last sync.
pub(crate) async fn has_dirty(conn: &mut SqliteConnection) -> Result<bool, Error> {
    let query = USN_TABLES
        .iter()
        .chain(&["grave"])
        .map(|table| format!("SELECT 1 FROM {table} WHERE usn < 0"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let found: Option<i64> = sqlx::query_scalar(AssertSqlSafe(format!("SELECT EXISTS ({query})")))
        .fetch_optional(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(found == Some(1))
}

/// How many rows changed here since the last sync, those being pushed included. Automatic sync
/// watches it to tell when changes stop coming; changing a row that is already dirty leaves it the
/// same.
pub async fn dirty_count(db: &SqlitePool) -> Result<i64, Error> {
    let query = USN_TABLES
        .iter()
        .chain(&["grave"])
        .map(|table| format!("(SELECT COUNT(*) FROM {table} WHERE usn < 0)"))
        .collect::<Vec<_>>()
        .join(" + ");
    sqlx::query_scalar(AssertSqlSafe(format!("SELECT {query}")))
        .fetch_one(db)
        .await
        .map_err(sqlx_error)
}

/// Stamps every dirty row with `usn`.
pub(crate) async fn stamp_dirty(conn: &mut SqliteConnection, usn: i64) -> Result<(), Error> {
    for table in USN_TABLES {
        sqlx::query(AssertSqlSafe(format!(
            "UPDATE {table} SET usn = ? WHERE usn = -1"
        )))
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
    SavedSearch,
    /// Only graves: review logs are only ever added, apart from the ones an undo deletes
    ReviewLog,
}

impl SyncKind {
    pub fn from_name(name: &str) -> Result<Self, Error> {
        match name {
            "parser" => Ok(Self::Parser),
            "tag" => Ok(Self::Tag),
            "note" => Ok(Self::Note),
            "card" => Ok(Self::Card),
            "saved_search" => Ok(Self::SavedSearch),
            "review_log" => Ok(Self::ReviewLog),
            _ => Err(Error::Sync(format!("unknown row kind `{name}`"))),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Parser => "parser",
            Self::Tag => "tag",
            Self::Note => "note",
            Self::Card => "card",
            Self::SavedSearch => "saved_search",
            Self::ReviewLog => "review_log",
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
