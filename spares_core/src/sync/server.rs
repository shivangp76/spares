//! The server's half of a sync: answering the client's requests.
//!
//! The server never merges. A pull first stamps everything changed on the server since the last
//! sync (phone edits are `usn = -1` like any local change) with a new usn, then returns what
//! changed since the client's last sync. The client merges and pushes the result, which the
//! server applies as given. A push is refused (and the client starts over) if the server changed
//! since the pull, so a phone edit made while the client merges is never overwritten.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::Utc;
use rand::RngExt;
use sqlx::SqlitePool;

use super::SYNC_EPOCH_KEY;
use super::SYNC_USN_KEY;
use super::apply::Applied;
use super::apply::apply_changes;
use super::full;
use super::get_state;
use super::get_state_i64;
use super::guard_undo;
use super::has_dirty;
use super::protocol::FullUploadResponse;
use super::protocol::MetaResponse;
use super::protocol::PullRequest;
use super::protocol::PullResponse;
use super::protocol::PushRequest;
use super::protocol::PushResponse;
use super::read_counts;
use super::records::Selection;
use super::records::read_cards_of_notes;
use super::records::read_changes;
use super::schema_version;
use super::set_applying;
use super::set_state;
use super::stamp_dirty;
use crate::Error;
use crate::config::get_replica;
use crate::config::get_sync_role;
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
        role: get_sync_role()?.to_string(),
        empty,
        client_last_usn,
    })
}

pub async fn pull(db: &SqlitePool, request: PullRequest) -> Result<PullResponse, Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    // A write first, so the transaction holds the write lock before it reads anything.
    set_applying(&mut tx, true).await?;
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
    guard_undo(&mut tx).await?;
    set_applying(&mut tx, false).await?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(PullResponse { usn, changes })
}

/// Applies a client's push. Returns what it changed, for regenerating files.
pub async fn push(db: &SqlitePool, request: PushRequest) -> Result<(PushResponse, Applied), Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    set_applying(&mut tx, true).await?;
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
    for asset in &request.render_assets {
        sqlx::query(
            "INSERT INTO render_asset (path, hash, mtime, size) VALUES (?, ?, ?, ?)
             ON CONFLICT (path) DO UPDATE SET hash = excluded.hash, mtime = excluded.mtime,
                 size = excluded.size",
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
    guard_undo(&mut tx).await?;
    let counts = read_counts(&mut tx).await?;
    set_applying(&mut tx, false).await?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok((
        PushResponse {
            usn: new_usn,
            counts,
        },
        applied,
    ))
}

/// Writes the server's database to `path`, for a client's full download.
pub async fn full_download(db: &SqlitePool, path: &Path) -> Result<(), Error> {
    // The snapshot carries the epoch and usn the client continues from.
    ensure_epoch(db).await?;
    full::snapshot(db, path).await
}

/// Replaces the server's database with the client's at `path`. Every other client has to do a
/// full sync afterwards, since the epoch changes.
pub async fn full_upload(
    db: &SqlitePool,
    path: &Path,
    replica: u8,
) -> Result<FullUploadResponse, Error> {
    full::restore(db, path).await?;
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    let usn = get_state_i64(&mut tx, SYNC_USN_KEY).await?.unwrap_or(0) + 1;
    let epoch = new_epoch();
    set_state(&mut tx, SYNC_USN_KEY, usn).await?;
    set_state(&mut tx, SYNC_EPOCH_KEY, &epoch).await?;
    sqlx::query("DELETE FROM sync_client")
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    sqlx::query("INSERT INTO sync_client (replica, last_usn, last_sync_at) VALUES (?, ?, ?)")
        .bind(i64::from(replica))
        .bind(usn)
        .bind(Utc::now().timestamp())
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(FullUploadResponse { usn, epoch })
}
