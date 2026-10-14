//! Running a sync from the client: deciding between a normal and a full sync, and the steps of
//! each. The server is reached through a [`SyncRemote`]: over HTTP, or in process in tests.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use log::info;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;

use super::LAST_SYNC_AT_KEY;
use super::NEEDS_FULL_SYNC_KEY;
use super::SYNC_EPOCH_KEY;
use super::SYNC_USN_KEY;
use super::USN_TABLES;
use super::client::merge_pulled;
use super::full;
use super::get_state;
use super::get_state_i64;
use super::guard_undo;
use super::protocol::FullUploadResponse;
use super::protocol::MetaResponse;
use super::protocol::PullRequest;
use super::protocol::PullResponse;
use super::protocol::PushRequest;
use super::protocol::PushResponse;
use super::protocol::RenderAssetRecord;
use super::read_counts;
use super::records::Selection;
use super::records::read_changes;
use super::remove_state;
use super::schema_version;
use super::set_applying;
use super::set_state;
use super::stamp_dirty;
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
#[async_trait]
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
            let state = local_state(db).await?;
            if let Some(reason) = full_sync_reason(&state, &meta) {
                return Err(Error::FullSyncRequired(reason));
            }
            normal_sync(db, remote, replica, state.usn.unwrap_or(0)).await
        }
    }
}

async fn read_render_assets(
    conn: &mut sqlx::SqliteConnection,
) -> Result<(Vec<RenderAssetRecord>, Vec<String>), Error> {
    let assets = sqlx::query_as("SELECT path, hash, mtime, size FROM render_asset ORDER BY path")
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    let packages = sqlx::query_scalar("SELECT spec FROM render_package ORDER BY spec")
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok((assets, packages))
}

async fn normal_sync(
    db: &SqlitePool,
    remote: &dyn SyncRemote,
    replica: u8,
    since: i64,
) -> Result<SyncReport, Error> {
    for attempt in 1..=ATTEMPTS {
        let pulled = remote.pull(PullRequest { since, replica }).await?;
        let now = Utc::now();
        // The whole merge, the push and the stamping are one local transaction, held while the
        // server applies the push. If anything fails, nothing changes here, and the next sync
        // finds the same changes on both sides, which merge to themselves.
        let mut tx = db.begin().await.map_err(sqlx_error)?;
        set_applying(&mut tx, true).await?;
        let outcome = merge_pulled(&mut tx, &pulled.changes, pulled.usn, now).await?;
        let changes = read_changes(&mut tx, Selection::Dirty).await?;
        let (render_assets, render_packages) = read_render_assets(&mut tx).await?;
        let pushed = changes.len();
        let response = match remote
            .push(PushRequest {
                pull_usn: pulled.usn,
                replica,
                changes,
                render_assets,
                render_packages,
            })
            .await
        {
            Err(Error::SyncRetry) if attempt < ATTEMPTS => {
                info!("The server changed during the sync; starting over");
                tx.rollback().await.map_err(sqlx_error)?;
                continue;
            }
            result => result?,
        };
        stamp_dirty(&mut tx, response.usn).await?;
        for table in ["grave", "sync_base"] {
            sqlx::query(&format!("DELETE FROM {table}"))
                .execute(&mut *tx)
                .await
                .map_err(sqlx_error)?;
        }
        set_state(&mut tx, SYNC_USN_KEY, response.usn).await?;
        set_state(&mut tx, LAST_SYNC_AT_KEY, now.timestamp()).await?;
        guard_undo(&mut tx).await?;
        let counts = read_counts(&mut tx).await?;
        if counts != response.counts {
            tx.rollback().await.map_err(sqlx_error)?;
            let reason = format!(
                "after the last sync the two sides had different numbers of rows (here \
                 {counts:?}, on the server {:?}).",
                response.counts
            );
            let mut conn = db.acquire().await.map_err(sqlx_error)?;
            set_state(&mut conn, NEEDS_FULL_SYNC_KEY, &reason).await?;
            return Err(Error::FullSyncRequired(reason));
        }
        set_applying(&mut tx, false).await?;
        tx.commit().await.map_err(sqlx_error)?;
        return Ok(SyncReport {
            mode: (pulled.changes.len() + pushed > 0).then_some(SyncMode::Normal),
            pulled: pulled.changes.len(),
            pushed,
            conflicts: outcome.conflicts.into_iter().collect(),
            notes_changed: outcome.applied.notes_changed,
            notes_deleted: outcome.applied.notes_deleted,
        });
    }
    unreachable!("the last attempt returns")
}

fn temp_file(temp_dir: &Path, name: &str) -> PathBuf {
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
        sqlx::query(&format!("UPDATE {table} SET usn = 0"))
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    for table in ["grave", "sync_base"] {
        sqlx::query(&format!("DELETE FROM {table}"))
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
    guard_undo(&mut conn).await?;
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
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    set_state(&mut conn, SYNC_USN_KEY, restored.usn).await?;
    if let Some(epoch) = &restored.epoch {
        set_state(&mut conn, SYNC_EPOCH_KEY, epoch).await?;
    }
    set_state(&mut conn, LAST_SYNC_AT_KEY, Utc::now().timestamp()).await?;
    remove_state(&mut conn, NEEDS_FULL_SYNC_KEY).await?;
    let notes_changed = all_note_ids(db).await?;
    Ok(SyncReport {
        mode: Some(SyncMode::FullDownload),
        pulled: notes_changed.len(),
        notes_deleted: before.difference(&notes_changed).copied().collect(),
        notes_changed,
        ..SyncReport::default()
    })
}
