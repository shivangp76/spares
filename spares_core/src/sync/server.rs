//! The server's half of a sync: answering the client's requests.
//!
//! The server never merges. A pull first stamps everything changed on the server since the last
//! sync (phone edits are `usn = -1` like any local change) with a new usn, then returns what
//! changed since the client's last sync. The client merges and pushes the result, which the
//! server applies as given. A push is refused (and the client starts over) if the server changed
//! since the pull, so a phone edit made while the client merges is never overwritten.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;

use chrono::Utc;
use log::error;
use log::info;
use rand::RngExt;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use super::SYNC_EPOCH_KEY;
use super::SYNC_USN_KEY;
use super::apply::Applied;
use super::apply::apply_changes;
use super::devices::DEVICE_REPLICAS;
use super::devices::check_enrolled;
use super::full;
use super::get_state;
use super::get_state_i64;
use super::has_dirty;
use super::protocol::FullUploadResponse;
use super::protocol::MetaResponse;
use super::protocol::PullRequest;
use super::protocol::PullResponse;
use super::protocol::PushRequest;
use super::protocol::PushResponse;
use super::protocol::RenderAssetRecord;
use super::prune_received;
use super::read_counts;
use super::records::Selection;
use super::records::read_cards_of_notes;
use super::records::read_changes;
use super::run::read_render_assets;
use super::schema_version;
use super::set_applying;
use super::set_state;
use super::stamp_dirty;
use crate::Error;
use crate::api::browser_render::RenderReads;
use crate::api::browser_render::remove_unread_render_assets;
use crate::config::get_replica;
use crate::model::NoteId;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

fn new_epoch() -> String {
    format!("{:016x}", rand::rng().random::<u64>())
}

/// The server's epoch, created on first use.
async fn ensure_epoch(db: &SqlitePool) -> Result<String, Error> {
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    if let Some(epoch) = get_state(&mut conn, SYNC_EPOCH_KEY).await? {
        return Ok(epoch);
    }
    let epoch = new_epoch();
    sqlx::query("INSERT OR IGNORE INTO app_state (key, value) VALUES (?, ?)")
        .bind(SYNC_EPOCH_KEY)
        .bind(&epoch)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    Ok(get_state(&mut conn, SYNC_EPOCH_KEY).await?.unwrap_or(epoch))
}

pub async fn meta(db: &SqlitePool, client_replica: Option<u8>) -> Result<MetaResponse, Error> {
    let epoch = ensure_epoch(db).await?;
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    let usn = get_state_i64(&mut conn, SYNC_USN_KEY).await?.unwrap_or(0);
    let empty: bool = sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM note)")
        .fetch_one(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if let Some(replica) = client_replica {
        check_enrolled(&mut conn, replica).await?;
    }
    let client_last_usn = match client_replica {
        Some(replica) => sqlx::query_scalar("SELECT last_usn FROM sync_client WHERE replica = ?")
            .bind(i64::from(replica))
            .fetch_optional(&mut *conn)
            .await
            .map_err(sqlx_error)?,
        None => None,
    };
    Ok(MetaResponse {
        usn,
        epoch,
        server_time: Utc::now().timestamp(),
        schema_version: schema_version(&mut conn).await?,
        replica: get_replica()?.number(),
        protocol_version: super::protocol::SYNC_PROTOCOL_VERSION,
        empty,
        client_last_usn,
    })
}

pub async fn pull(db: &SqlitePool, request: PullRequest) -> Result<PullResponse, Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    // A write first, so the transaction holds the write lock before it reads anything.
    set_applying(&mut tx, true).await?;
    check_enrolled(&mut tx, request.replica).await?;
    let mut usn = get_state_i64(&mut tx, SYNC_USN_KEY).await?.unwrap_or(0);
    if has_dirty(&mut tx).await? {
        usn += 1;
        stamp_dirty(&mut tx, usn).await?;
        sqlx::query("UPDATE grave SET usn = ? WHERE usn = -1")
            .bind(usn)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
        // Only clients merge, so the server needs no bases.
        sqlx::query("DELETE FROM sync_base")
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
        set_state(&mut tx, SYNC_USN_KEY, usn).await?;
    }
    let mut changes = read_changes(&mut tx, Selection::After(request.since)).await?;
    // Every card of a changed note, so the client can rebuild a note it merges or brings back.
    let note_ids: Vec<NoteId> = changes.notes.iter().map(|n| n.id).collect();
    let known: BTreeSet<i64> = changes.cards.iter().map(|c| c.id).collect();
    changes.cards.extend(
        read_cards_of_notes(&mut tx, &note_ids)
            .await?
            .into_iter()
            .filter(|card| !known.contains(&card.id)),
    );
    let render_assets = match &request.render_assets_digest {
        Some(digest) => {
            let render_assets = read_render_assets(&mut tx).await?;
            (render_assets.digest() != *digest).then_some(render_assets)
        }
        None => None,
    };
    prune_received(&mut tx).await?;
    set_applying(&mut tx, false).await?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(PullResponse {
        usn,
        changes,
        render_assets,
    })
}

/// Applies a client's push. Returns what it changed, for regenerating files.
pub async fn push(db: &SqlitePool, request: PushRequest) -> Result<(PushResponse, Applied), Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    set_applying(&mut tx, true).await?;
    check_enrolled(&mut tx, request.replica).await?;
    let usn = get_state_i64(&mut tx, SYNC_USN_KEY).await?.unwrap_or(0);
    if usn != request.pull_usn || has_dirty(&mut tx).await? {
        return Err(Error::SyncRetry);
    }
    let new_usn = if request.changes.is_empty() {
        usn
    } else {
        usn + 1
    };
    // Foreign keys are checked at commit, so the order rows arrive in cannot fail the push.
    sqlx::query("PRAGMA defer_foreign_keys = ON")
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    let applied = apply_changes(&mut tx, &request.changes, new_usn).await?;
    let prune_render_assets = prunes_render_assets(&mut tx, request.replica).await?;
    let orphaned = replaces_a_hash(&mut tx, &request.render_assets).await?;
    // Not over a newer file's: a client's rows may be older than another's, as a device's were
    // before it stopped sending them
    for asset in &request.render_assets {
        sqlx::query(
            "INSERT INTO render_asset (path, hash, mtime, size) VALUES (?, ?, ?, ?)
             ON CONFLICT (path) DO UPDATE SET hash = excluded.hash, mtime = excluded.mtime,
                 size = excluded.size
             WHERE excluded.mtime >= render_asset.mtime",
        )
        .bind(&asset.path)
        .bind(&asset.hash)
        .bind(asset.mtime)
        .bind(asset.size)
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    }
    for spec in &request.render_packages {
        sqlx::query("INSERT OR IGNORE INTO render_package (spec) VALUES (?)")
            .bind(spec)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    for counter in &request.id_counters {
        sqlx::query(
            "INSERT INTO id_counter (replica, tbl, last) VALUES (?, ?, ?)
             ON CONFLICT (replica, tbl) DO UPDATE SET last = MAX(last, excluded.last)",
        )
        .bind(i64::from(request.replica))
        .bind(&counter.tbl)
        .bind(counter.last)
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    }
    set_state(&mut tx, SYNC_USN_KEY, new_usn).await?;
    sqlx::query(
        "INSERT INTO sync_client (replica, last_usn, last_sync_at) VALUES (?, ?, ?)
         ON CONFLICT (replica) DO UPDATE SET last_usn = excluded.last_usn,
             last_sync_at = excluded.last_sync_at",
    )
    .bind(i64::from(request.replica))
    .bind(new_usn)
    .bind(Utc::now().timestamp())
    .execute(&mut *tx)
    .await
    .map_err(sqlx_error)?;
    prune_received(&mut tx).await?;
    let counts = read_counts(&mut tx).await?;
    set_applying(&mut tx, false).await?;
    tx.commit().await.map_err(sqlx_error)?;
    if prune_render_assets {
        remove_unpushed_render_assets(db, &request, orphaned).await;
    }
    Ok((
        PushResponse {
            usn: new_usn,
            counts,
        },
        applied,
    ))
}

/// Whether `replica`'s push removes the render assets it no longer sends. A host sends every one
/// it has, so those it doesn't are no longer read, unless the notes of another host that sends
/// them read them: the rows don't say which host sent them, so then they all stay.
async fn prunes_render_assets(conn: &mut SqliteConnection, replica: u8) -> Result<bool, Error> {
    if DEVICE_REPLICAS.contains(&replica) {
        return Ok(false);
    }
    let other_hosts: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sync_client WHERE replica != ?1 AND replica NOT BETWEEN ?2 AND ?3)",
    )
    .bind(i64::from(replica))
    .bind(i64::from(*DEVICE_REPLICAS.start()))
    .bind(i64::from(*DEVICE_REPLICAS.end()))
    .fetch_one(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(!other_hosts)
}

/// Whether the push of `assets` replaces a row's hash, so no row may name its old stored copy.
async fn replaces_a_hash(
    conn: &mut SqliteConnection,
    assets: &[RenderAssetRecord],
) -> Result<bool, Error> {
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT path, hash, mtime FROM render_asset")
            .fetch_all(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    let rows: HashMap<String, (String, i64)> = rows
        .into_iter()
        .map(|(path, hash, mtime)| (path, (hash, mtime)))
        .collect();
    // As the push applies them, not over a newer row
    Ok(assets.iter().any(|asset| {
        rows.get(&asset.path)
            .is_some_and(|(hash, mtime)| *hash != asset.hash && asset.mtime >= *mtime)
    }))
}

/// Removes the render assets that `request`'s host no longer sends and no note here reads (see
/// [`remove_unread_render_assets`]). The push is in, so a failure is only logged: it leaves files
/// behind until the next push.
async fn remove_unpushed_render_assets(db: &SqlitePool, request: &PushRequest, orphaned: bool) {
    let pushed = RenderReads {
        files: request
            .render_assets
            .iter()
            .map(|asset| asset.path.clone())
            .collect(),
        packages: request.render_packages.iter().cloned().collect(),
    };
    match remove_unread_render_assets(db, Some(&pushed), orphaned).await {
        Ok(0) => {}
        Ok(removed) => info!("Removed {removed} render assets no note reads any more"),
        Err(e) => error!("Failed to remove render assets no note reads: {e}"),
    }
}

/// Writes the server's database to `path`, for a client's full download.
pub async fn full_download(db: &SqlitePool, path: &Path) -> Result<(), Error> {
    // The snapshot carries the epoch and usn the client continues from.
    ensure_epoch(db).await?;
    full::snapshot(db, path).await
}

/// Replaces the server's database with the client's at `path`. Every other client has to do a
/// full sync afterwards, since the epoch changes. Enrolled devices stay enrolled.
pub async fn full_upload(
    db: &SqlitePool,
    path: &Path,
    replica: u8,
) -> Result<FullUploadResponse, Error> {
    check_enrolled(&mut *db.acquire().await.map_err(sqlx_error)?, replica).await?;
    full::restore(db, path).await?;
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    let usn = get_state_i64(&mut tx, SYNC_USN_KEY).await?.unwrap_or(0) + 1;
    let epoch = new_epoch();
    set_state(&mut tx, SYNC_USN_KEY, usn).await?;
    set_state(&mut tx, SYNC_EPOCH_KEY, &epoch).await?;
    // Every other client's last usn is from the old epoch, so it says nothing about the client
    // any more. 0 is below every usn, which is never taken for a backup or an applied push (see
    // `super::run`). When each last synced stays, for the list of devices.
    sqlx::query("UPDATE sync_client SET last_usn = 0")
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    sqlx::query(
        "INSERT INTO sync_client (replica, last_usn, last_sync_at) VALUES (?, ?, ?)
         ON CONFLICT (replica) DO UPDATE SET last_usn = excluded.last_usn,
             last_sync_at = excluded.last_sync_at",
    )
    .bind(i64::from(replica))
    .bind(usn)
    .bind(Utc::now().timestamp())
    .execute(&mut *tx)
    .await
    .map_err(sqlx_error)?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(FullUploadResponse { usn, epoch })
}
