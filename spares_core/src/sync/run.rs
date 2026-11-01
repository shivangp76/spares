//! Running a sync from the client: deciding between a normal and a full sync, and the steps of
//! each. The server is reached through a [`SyncRemote`]: over HTTP, or in process in tests.
//!
//! # A normal sync
//!
//! No transaction stays open while the server answers. The server's push can wait for its notes
//! lock and renders the changed notes before it answers, and a browser's copy has a single
//! connection, which every request to it would wait for.
//!
//! 1. Pull. Then, in one transaction: merge, move the cursor (`sync_usn`) to the pull, make the
//!    pulled version the base of each merged row still dirty (it is now the common ancestor), and
//!    mark the dirty rows and graves as being pushed (`usn = -2`, never sent anywhere). Read them.
//! 2. Push. Writes go on meanwhile. Changing a row being pushed fires its trigger as for a synced
//!    row (`OLD.usn != -1`), so it is dirty (`-1`) again.
//! 3. In a second transaction: stamp the rows still at `-2` with the push's usn and drop their
//!    graves and bases. A row changed during the push stays dirty, and its base becomes the
//!    version pushed. Then compare the row counts read in step 1 with the server's.
//!
//! When the server refuses the push because it changed since the pull, the rows are dirty again,
//! and the sync starts over, pulling only what is new. When the push fails otherwise, its answer
//! may have been lost after the server applied it (a dropped connection, a timeout, or a page
//! frozen while hidden), so the rows stay at `-2` and the next sync settles them first
//! ([`settle_push`]): if the server's record of this replica's last sync is ahead of the cursor,
//! the push was applied, and is finished as in step 3; otherwise the rows are dirty again.
//! Without this, the next sync would take this replica for one restored from a backup.
//!
//! The server could answer before rendering. It doesn't: a push no longer holds anything here, so
//! its length doesn't get in the way, and a device downloads the rendered files right after a
//! sync, so it would get the old ones.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use log::info;
use serde::Deserialize;
use serde::Serialize;
use sqlx::AssertSqlSafe;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use super::LAST_SYNC_AT_KEY;
use super::NEEDS_FULL_SYNC_KEY;
use super::SYNC_EPOCH_KEY;
use super::SYNC_USN_KEY;
use super::SyncKind;
use super::USN_TABLES;
use super::apply::Applied;
use super::client::MergeOutcome;
use super::client::merge_pulled;
use super::devices::DEVICE_REPLICAS;
use super::full;
use super::get_state;
use super::get_state_i64;
use super::protocol::FullUploadResponse;
use super::protocol::IdCounterRecord;
use super::protocol::MetaResponse;
use super::protocol::PullRequest;
use super::protocol::PullResponse;
use super::protocol::PushRequest;
use super::protocol::PushResponse;
use super::protocol::RenderAssets;
use super::prune_received;
use super::read_counts;
use super::records::Changes;
use super::records::Selection;
use super::records::read_changes;
use super::remove_state;
use super::schema_version;
use super::set_applying;
use super::set_state;
use crate::Error;
use crate::config::SyncRole;
use crate::config::get_replica;
use crate::config::get_sync_role;
use crate::model::NoteId;

/// The clocks may differ by this much. Merges break some ties by time.
pub const MAX_CLOCK_SKEW_SECS: i64 = 300;

/// How often a normal sync starts over when the server changes while it runs.
const ATTEMPTS: usize = 3;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

/// The server, as the client sees it.
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
pub trait SyncRemote: Send + Sync {
    async fn meta(&self, replica: u8) -> Result<MetaResponse, Error>;
    async fn pull(&self, request: PullRequest) -> Result<PullResponse, Error>;
    async fn push(&self, request: PushRequest) -> Result<PushResponse, Error>;
    /// Writes the server's database to `path`.
    async fn download(&self, path: &Path) -> Result<(), Error>;
    /// Replaces the server's database with the one at `path`.
    async fn upload(&self, path: &Path, replica: u8) -> Result<FullUploadResponse, Error>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Exchange changes, or fail with [`Error::FullSyncRequired`] if that is not possible.
    Normal,
    /// Replace the server's database with this one.
    FullUpload,
    /// Replace this database with the server's.
    FullDownload,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SyncReport {
    /// `None` if there was nothing to do.
    pub mode: Option<SyncMode>,
    pub pulled: usize,
    pub pushed: usize,
    /// Notes tagged `sync-conflict` by this sync.
    pub conflicts: Vec<NoteId>,
    /// Notes whose files have to be regenerated here. After a full download, every note.
    pub notes_changed: BTreeSet<NoteId>,
    /// Notes deleted here, whose files have to go.
    pub notes_deleted: BTreeSet<NoteId>,
}

/// What this replica remembers of its last sync.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LocalSyncState {
    pub usn: Option<i64>,
    pub epoch: Option<String>,
    pub last_sync_at: Option<i64>,
    pub needs_full_sync: Option<String>,
    pub conflicts: i64,
}

pub async fn local_state(db: &SqlitePool) -> Result<LocalSyncState, Error> {
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    let conflicts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_conflict")
        .fetch_one(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(LocalSyncState {
        usn: get_state_i64(&mut conn, SYNC_USN_KEY).await?,
        epoch: get_state(&mut conn, SYNC_EPOCH_KEY).await?,
        last_sync_at: get_state_i64(&mut conn, LAST_SYNC_AT_KEY).await?,
        needs_full_sync: get_state(&mut conn, NEEDS_FULL_SYNC_KEY).await?,
        conflicts,
    })
}

/// Checks that the two sides can sync at all.
async fn check_meta(db: &SqlitePool, meta: &MetaResponse) -> Result<(), Error> {
    let replica = get_replica()?.number();
    if get_sync_role()? != SyncRole::Client {
        return Err(Error::Sync(
            "this instance is not a sync client (`SPARES_SYNC_ROLE=client`)".to_string(),
        ));
    }
    if !meta.is_server() {
        return Err(Error::Sync(format!(
            "the other side is a `{}`, not a sync server",
            meta.role
        )));
    }
    if meta.replica == replica {
        return Err(Error::Sync(format!(
            "both sides are replica {replica}. Each needs its own `SPARES_REPLICA`."
        )));
    }
    let skew = meta.server_time - Utc::now().timestamp();
    if skew.abs() > MAX_CLOCK_SKEW_SECS {
        return Err(Error::Sync(format!(
            "the clocks differ by {skew} seconds. Fix the clock on one side."
        )));
    }
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    let version = schema_version(&mut conn).await?;
    if meta.schema_version != version {
        return Err(Error::Sync(format!(
            "the server's database is at migration {} and this one at {version}. Update spares \
             on both sides first.",
            meta.schema_version
        )));
    }
    Ok(())
}

/// Why a normal sync is not possible, if it is not.
fn full_sync_reason(state: &LocalSyncState, meta: &MetaResponse) -> Option<String> {
    if let Some(reason) = &state.needs_full_sync {
        return Some(reason.clone());
    }
    let (Some(usn), Some(epoch)) = (state.usn, &state.epoch) else {
        return Some(if meta.empty {
            "this is the first sync, and the server has no notes yet. Upload this database."
                .to_string()
        } else {
            "this database has never synced with this server.".to_string()
        });
    };
    if *epoch != meta.epoch {
        return Some("the server's database was replaced since the last sync.".to_string());
    }
    if usn > meta.usn {
        return Some(
            "the server's database is older than at the last sync (restored from a backup?)."
                .to_string(),
        );
    }
    if meta.client_last_usn.is_some_and(|last| last > usn) {
        return Some(
            "this database is older than at its last sync (restored from a backup?).".to_string(),
        );
    }
    None
}

pub async fn sync(
    db: &SqlitePool,
    remote: &dyn SyncRemote,
    mode: SyncMode,
    temp_dir: &Path,
) -> Result<SyncReport, Error> {
    let replica = get_replica()?.number();
    let meta = remote.meta(replica).await?;
    check_meta(db, &meta).await?;
    match mode {
        SyncMode::FullUpload => full_upload(db, remote, replica, temp_dir).await,
        SyncMode::FullDownload => full_download(db, remote, temp_dir).await,
        SyncMode::Normal => {
            settle_push(db, &meta).await?;
            let state = local_state(db).await?;
            if let Some(reason) = full_sync_reason(&state, &meta) {
                return Err(Error::FullSyncRequired(reason));
            }
            normal_sync(db, remote, replica).await
        }
    }
}

pub(crate) async fn read_render_assets(
    conn: &mut sqlx::SqliteConnection,
) -> Result<RenderAssets, Error> {
    let assets = sqlx::query_as("SELECT path, hash, mtime, size FROM render_asset ORDER BY path")
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    let packages = sqlx::query_scalar("SELECT spec FROM render_package ORDER BY spec")
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(RenderAssets { assets, packages })
}

/// Replaces a device's render assets with the server's.
async fn replace_render_assets(
    conn: &mut sqlx::SqliteConnection,
    render_assets: &RenderAssets,
) -> Result<(), Error> {
    for table in ["render_asset", "render_package"] {
        sqlx::query(AssertSqlSafe(format!("DELETE FROM {table}")))
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    for asset in &render_assets.assets {
        sqlx::query("INSERT INTO render_asset (path, hash, mtime, size) VALUES (?, ?, ?, ?)")
            .bind(&asset.path)
            .bind(&asset.hash)
            .bind(asset.mtime)
            .bind(asset.size)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    for spec in &render_assets.packages {
        sqlx::query("INSERT INTO render_package (spec) VALUES (?)")
            .bind(spec)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    Ok(())
}

/// `replica`'s own `id_counter` rows, for the server to keep (see [`IdCounterRecord`]).
async fn read_id_counters(
    conn: &mut sqlx::SqliteConnection,
    replica: u8,
) -> Result<Vec<IdCounterRecord>, Error> {
    sqlx::query_as("SELECT tbl, last FROM id_counter WHERE replica = ? ORDER BY tbl")
        .bind(i64::from(replica))
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)
}

async fn normal_sync(
    db: &SqlitePool,
    remote: &dyn SyncRemote,
    replica: u8,
) -> Result<SyncReport, Error> {
    let mut report = SyncReport::default();
    // A device has the laptop's render assets from the server, so it doesn't send them back
    let device = DEVICE_REPLICAS.contains(&replica);
    for attempt in 1..=ATTEMPTS {
        let (since, render_assets_digest) = {
            let mut conn = db.acquire().await.map_err(sqlx_error)?;
            let since = get_state_i64(&mut conn, SYNC_USN_KEY).await?.unwrap_or(0);
            let digest = if device {
                Some(read_render_assets(&mut conn).await?.digest())
            } else {
                None
            };
            (since, digest)
        };
        let pulled = remote
            .pull(PullRequest {
                since,
                replica,
                render_assets_digest,
            })
            .await?;
        let now = Utc::now();
        // 1. Merge, move the cursor to the pull, and mark what is to be pushed, in one
        // transaction. If the merge fails, nothing changes here, and the next sync pulls the same
        // changes again.
        let mut tx = db.begin().await.map_err(sqlx_error)?;
        set_applying(&mut tx, true).await?;
        let outcome = merge_pulled(&mut tx, &pulled.changes, pulled.usn, now).await?;
        if let Some(render_assets) = &pulled.render_assets {
            replace_render_assets(&mut tx, render_assets).await?;
        }
        set_state(&mut tx, SYNC_USN_KEY, pulled.usn).await?;
        rebase_pulled(&mut tx, &pulled.changes).await?;
        mark_pushing(&mut tx, true).await?;
        let changes = read_changes(&mut tx, Selection::Pushing).await?;
        let RenderAssets {
            assets: render_assets,
            packages: render_packages,
        } = if device {
            RenderAssets::default()
        } else {
            read_render_assets(&mut tx).await?
        };
        let id_counters = read_id_counters(&mut tx, replica).await?;
        let counts = read_counts(&mut tx).await?;
        prune_received(&mut tx).await?;
        set_applying(&mut tx, false).await?;
        tx.commit().await.map_err(sqlx_error)?;
        report.add_pulled(pulled.changes.len(), outcome);

        // 2. Push, with no transaction open, so the database stays usable meanwhile: a browser's
        // has a single connection, and the server's push can take a while.
        let pushed = changes.len();
        let response = match remote
            .push(PushRequest {
                pull_usn: pulled.usn,
                replica,
                changes: changes.clone(),
                render_assets,
                render_packages,
                id_counters,
            })
            .await
        {
            Err(Error::SyncRetry) => {
                // Refused, so pushed again after pulling what is new
                let mut conn = db.acquire().await.map_err(sqlx_error)?;
                mark_pushing(&mut conn, false).await?;
                if attempt < ATTEMPTS {
                    info!("The server changed during the sync; starting over");
                    continue;
                }
                return Err(Error::SyncRetry);
            }
            // Its answer may have been lost after the server applied it, so the rows stay marked,
            // and the next sync finds out ([`settle_push`]).
            Err(e) => return Err(e),
            Ok(response) => response,
        };

        // 3. Stamp what was pushed.
        let mut tx = db.begin().await.map_err(sqlx_error)?;
        set_applying(&mut tx, true).await?;
        finish_push(&mut tx, response.usn, Some(&changes)).await?;
        set_state(&mut tx, LAST_SYNC_AT_KEY, now.timestamp()).await?;
        let mismatch = (counts != response.counts).then(|| {
            format!(
                "after the last sync the two sides had different numbers of rows (here \
                 {counts:?}, on the server {:?}).",
                response.counts
            )
        });
        if let Some(reason) = &mismatch {
            set_state(&mut tx, NEEDS_FULL_SYNC_KEY, reason).await?;
        }
        set_applying(&mut tx, false).await?;
        tx.commit().await.map_err(sqlx_error)?;
        if let Some(reason) = mismatch {
            return Err(Error::FullSyncRequired(reason));
        }
        report.pushed = pushed;
        report.mode = (report.pulled + pushed > 0).then_some(SyncMode::Normal);
        return Ok(report);
    }
    unreachable!("the last attempt returns")
}

impl SyncReport {
    /// Adds a merge to the report. Each attempt of a normal sync commits its own.
    fn add_pulled(&mut self, pulled: usize, outcome: MergeOutcome) {
        self.pulled += pulled;
        self.conflicts.extend(outcome.conflicts);
        let Applied {
            notes_changed,
            notes_deleted,
        } = outcome.applied;
        for id in &notes_changed {
            self.notes_deleted.remove(id);
        }
        for id in &notes_deleted {
            self.notes_changed.remove(id);
        }
        self.notes_changed.extend(notes_changed);
        self.notes_deleted.extend(notes_deleted);
        self.conflicts.sort_unstable();
        self.conflicts.dedup();
    }
}

/// Marks every dirty row and grave as being pushed (`usn = -2`), or (`false`) back to dirty.
async fn mark_pushing(conn: &mut SqliteConnection, pushing: bool) -> Result<(), Error> {
    let (from, to) = if pushing { (-1, -2) } else { (-2, -1) };
    for table in USN_TABLES.iter().chain(&["grave"]) {
        sqlx::query(AssertSqlSafe(format!(
            "UPDATE {table} SET usn = ? WHERE usn = ?"
        )))
        .bind(to)
        .bind(from)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    Ok(())
}

/// Every row record in `changes` with its kind and id, as `sync_base` keeps it. Review logs are
/// never changed, so they have no base.
fn snapshots(changes: &Changes) -> Result<Vec<(SyncKind, i64, String)>, Error> {
    fn snapshot<T: Serialize>(
        kind: SyncKind,
        id: i64,
        record: &T,
    ) -> Result<(SyncKind, i64, String), Error> {
        let json = serde_json::to_string(record)
            .map_err(|e| Error::Sync(format!("cannot write a sync snapshot: {e}")))?;
        Ok((kind, id, json))
    }
    let mut out = Vec::new();
    for record in &changes.parsers {
        out.push(snapshot(SyncKind::Parser, record.id, record)?);
    }
    for record in &changes.tags {
        out.push(snapshot(SyncKind::Tag, record.id, record)?);
    }
    for record in &changes.notes {
        out.push(snapshot(SyncKind::Note, record.id.packed(), record)?);
    }
    for record in &changes.cards {
        out.push(snapshot(SyncKind::Card, record.id, record)?);
    }
    for record in &changes.saved_searches {
        out.push(snapshot(SyncKind::SavedSearch, record.id, record)?);
    }
    Ok(out)
}

/// Makes the pulled version the base of each pulled row still dirty after the merge, since it is
/// now what both sides last had in common, and drops the base of the others. A pulled grave leaves
/// no base: a row the merge kept is new to the server.
async fn rebase_pulled(conn: &mut SqliteConnection, pulled: &Changes) -> Result<(), Error> {
    for (kind, id, snapshot) in snapshots(pulled)? {
        let table = kind.name();
        sqlx::query(AssertSqlSafe(format!(
            "INSERT OR REPLACE INTO sync_base (kind, oid, snapshot)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM {table} WHERE id = ?2 AND usn = -1)"
        )))
        .bind(table)
        .bind(id)
        .bind(&snapshot)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
        sqlx::query(AssertSqlSafe(format!(
            "DELETE FROM sync_base WHERE kind = ?1 AND oid = ?2
             AND NOT EXISTS (SELECT 1 FROM {table} WHERE id = ?2 AND usn = -1)"
        )))
        .bind(table)
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    for grave in &pulled.graves {
        delete_base(conn, grave.kind, grave.oid).await?;
    }
    Ok(())
}

async fn delete_base(conn: &mut SqliteConnection, kind: SyncKind, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM sync_base WHERE kind = ? AND oid = ?")
        .bind(kind.name())
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// Stamps the rows still marked as pushed with the push's `usn`, and drops their graves and bases.
/// A row changed during the push stays dirty, and its base becomes the version `pushed`, if known.
async fn finish_push(
    conn: &mut SqliteConnection,
    usn: i64,
    pushed: Option<&Changes>,
) -> Result<(), Error> {
    if let Some(pushed) = pushed {
        for (kind, id, snapshot) in snapshots(pushed)? {
            let table = kind.name();
            sqlx::query(AssertSqlSafe(format!(
                "INSERT OR REPLACE INTO sync_base (kind, oid, snapshot)
                 SELECT ?1, ?2, ?3
                 WHERE NOT EXISTS (SELECT 1 FROM {table} WHERE id = ?2 AND usn = -2)"
            )))
            .bind(table)
            .bind(id)
            .bind(&snapshot)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        }
        // A row brought back during the push is new to the server, which deleted it
        for grave in &pushed.graves {
            delete_base(conn, grave.kind, grave.oid).await?;
        }
    }
    for table in USN_TABLES {
        if table != "review_log" {
            sqlx::query(AssertSqlSafe(format!(
                "DELETE FROM sync_base WHERE kind = '{table}'
                 AND oid IN (SELECT id FROM {table} WHERE usn = -2)"
            )))
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        }
        sqlx::query(AssertSqlSafe(format!(
            "UPDATE {table} SET usn = ? WHERE usn = -2"
        )))
        .bind(usn)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    for query in [
        "DELETE FROM sync_base WHERE EXISTS
         (SELECT 1 FROM grave g WHERE g.kind = sync_base.kind AND g.oid = sync_base.oid
          AND g.usn = -2)",
        "DELETE FROM grave WHERE usn = -2",
    ] {
        sqlx::query(query)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    set_state(conn, SYNC_USN_KEY, usn).await
}

/// Settles a push whose sync never finished: its answer was lost, or the page was frozen or closed
/// meanwhile. Its rows are still marked as pushed. Only this replica pushes as itself, so if the
/// server's record of its last sync is ahead of the cursor, the push was applied, and is finished
/// here as if its answer had come. Rows changed since keep their older base, which at worst flags
/// a conflict. Otherwise the rows are dirty again.
async fn settle_push(db: &SqlitePool, meta: &MetaResponse) -> Result<(), Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    set_applying(&mut tx, true).await?;
    let query = USN_TABLES
        .iter()
        .chain(&["grave"])
        .map(|table| format!("SELECT 1 FROM {table} WHERE usn = -2"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let pushing: bool = sqlx::query_scalar(AssertSqlSafe(format!("SELECT EXISTS ({query})")))
        .fetch_one(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    if !pushing {
        return Ok(());
    }
    let cursor = get_state_i64(&mut tx, SYNC_USN_KEY).await?;
    let epoch = get_state(&mut tx, SYNC_EPOCH_KEY).await?;
    match (meta.client_last_usn, cursor) {
        (Some(last), Some(cursor)) if last > cursor && epoch.as_ref() == Some(&meta.epoch) => {
            info!("Finishing the last sync, whose push the server applied");
            finish_push(&mut tx, last, None).await?;
            set_state(&mut tx, LAST_SYNC_AT_KEY, Utc::now().timestamp()).await?;
        }
        _ => mark_pushing(&mut tx, false).await?,
    }
    set_applying(&mut tx, false).await?;
    tx.commit().await.map_err(sqlx_error)
}

/// Where a full sync keeps its snapshot. A browser has no process id, and only one sync runs
/// there at a time.
fn temp_file(temp_dir: &Path, name: &str) -> PathBuf {
    if cfg!(target_family = "wasm") {
        return temp_dir.join(format!("{name}.sqlite"));
    }
    temp_dir.join(format!("{name}-{}.sqlite", std::process::id()))
}

async fn all_note_ids(db: &SqlitePool) -> Result<BTreeSet<NoteId>, Error> {
    let ids: Vec<NoteId> = sqlx::query_scalar("SELECT id FROM note")
        .fetch_all(db)
        .await
        .map_err(sqlx_error)?;
    Ok(ids.into_iter().collect())
}

async fn full_upload(
    db: &SqlitePool,
    remote: &dyn SyncRemote,
    replica: u8,
    temp_dir: &Path,
) -> Result<SyncReport, Error> {
    // Everything in the snapshot counts as synced once it is uploaded. Changes made while it
    // uploads stay dirty, with what was uploaded as their base. Until the upload succeeds, a
    // normal sync is refused.
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    set_applying(&mut tx, true).await?;
    for table in USN_TABLES {
        sqlx::query(AssertSqlSafe(format!("UPDATE {table} SET usn = 0")))
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    for table in ["grave", "sync_base"] {
        sqlx::query(AssertSqlSafe(format!("DELETE FROM {table}")))
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    set_state(
        &mut tx,
        NEEDS_FULL_SYNC_KEY,
        "a full upload did not finish. Run it again.",
    )
    .await?;
    set_applying(&mut tx, false).await?;
    tx.commit().await.map_err(sqlx_error)?;

    let path = temp_file(temp_dir, "spares-upload");
    full::snapshot(db, &path).await?;
    let result = remote.upload(&path, replica).await;
    let _ = std::fs::remove_file(&path);
    let response = result?;

    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    set_state(&mut conn, SYNC_USN_KEY, response.usn).await?;
    set_state(&mut conn, SYNC_EPOCH_KEY, &response.epoch).await?;
    set_state(&mut conn, LAST_SYNC_AT_KEY, Utc::now().timestamp()).await?;
    remove_state(&mut conn, NEEDS_FULL_SYNC_KEY).await?;
    prune_received(&mut conn).await?;
    let counts = read_counts(&mut conn).await?;
    Ok(SyncReport {
        mode: Some(SyncMode::FullUpload),
        pushed: usize::try_from(counts.notes + counts.cards + counts.review_logs).unwrap_or(0),
        ..SyncReport::default()
    })
}

async fn full_download(
    db: &SqlitePool,
    remote: &dyn SyncRemote,
    temp_dir: &Path,
) -> Result<SyncReport, Error> {
    let before = all_note_ids(db).await?;
    let path = temp_file(temp_dir, "spares-download");
    let result = async {
        remote.download(&path).await?;
        full::restore(db, &path).await
    }
    .await;
    let _ = std::fs::remove_file(&path);
    let restored = result?;
    {
        // Released before `all_note_ids` takes one: a browser's pool has a single connection.
        let mut conn = db.acquire().await.map_err(sqlx_error)?;
        set_state(&mut conn, SYNC_USN_KEY, restored.usn).await?;
        if let Some(epoch) = &restored.epoch {
            set_state(&mut conn, SYNC_EPOCH_KEY, epoch).await?;
        }
        set_state(&mut conn, LAST_SYNC_AT_KEY, Utc::now().timestamp()).await?;
        remove_state(&mut conn, NEEDS_FULL_SYNC_KEY).await?;
    }
    let notes_changed = all_note_ids(db).await?;
    Ok(SyncReport {
        mode: Some(SyncMode::FullDownload),
        pulled: notes_changed.len(),
        notes_deleted: before.difference(&notes_changed).copied().collect(),
        notes_changed,
        ..SyncReport::default()
    })
}
