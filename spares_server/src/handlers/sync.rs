//! Two-way sync. A `server` (`SPARES_SYNC_ROLE=server`) answers `meta`, `pull`, `push` and
//! `full`; a `client` runs a sync against its `sync_url` when asked (`run`). See
//! `spares_core::sync`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use spares_core::Error;
use spares_core::config::SyncRole;
use spares_core::config::get_cache_dir;
use spares_core::config::get_replica;
use spares_core::config::get_sync_role;
use spares_core::config::read_external_config;
use spares_core::notes_vcs::run_exclusive;
use spares_core::sync::files;
use spares_core::sync::protocol::PullRequest;
use spares_core::sync::protocol::PushRequest;
use spares_core::sync::run::LocalSyncState;
use spares_core::sync::run::SyncMode;
use spares_core::sync::run::local_state;
use spares_core::sync::server;

use crate::AppState;
use crate::handlers::error_to_response;

/// Pushes and full uploads carry whole databases.
pub(crate) const SYNC_BODY_LIMIT: usize = 1024 * 1024 * 1024;

type HandlerError = (StatusCode, Json<Value>);

fn require_role(role: SyncRole) -> Result<(), HandlerError> {
    let actual = get_sync_role().map_err(error_to_response)?;
    if actual == role {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        Json(json!({
            "message": format!("this server is a sync {actual}, not a {role} (SPARES_SYNC_ROLE)")
        })),
    ))
}

/// Where sync keeps database snapshots while sending them.
fn temp_dir() -> Result<PathBuf, HandlerError> {
    let dir = get_cache_dir().join("sync");
    std::fs::create_dir_all(&dir).map_err(|e| {
        error_to_response(Error::Io {
            description: format!("Failed to create {}", dir.display()),
            source: e,
        })
    })?;
    Ok(dir)
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReplicaQuery {
    replica: Option<u8>,
}

pub(crate) async fn sync_meta_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Query(ReplicaQuery { replica }): Query<ReplicaQuery>,
) -> Result<impl IntoResponse, HandlerError> {
    require_role(SyncRole::Server)?;
    let meta = server::meta(&data.db, replica)
        .await
        .map_err(error_to_response)?;
    Ok(Json(meta))
}

pub(crate) async fn sync_pull_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<PullRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    require_role(SyncRole::Server)?;
    let response = server::pull(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(response))
}

pub(crate) async fn sync_push_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<PushRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    require_role(SyncRole::Server)?;
    let db = data.db.clone();
    let response = run_exclusive(async move {
        let (response, applied) = server::push(&db, body).await?;
        files::after_sync(&db, &applied.notes_changed, &applied.notes_deleted).await;
        Ok(response)
    })
    .await
    .map_err(error_to_response)?;
    Ok(Json(response))
}

pub(crate) async fn sync_full_download_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, HandlerError> {
    require_role(SyncRole::Server)?;
    let path = temp_dir()?.join(format!("download-{}.sqlite", std::process::id()));
    let result = server::full_download(&data.db, &path).await;
    let bytes = result.and_then(|()| {
        std::fs::read(&path).map_err(|e| Error::Io {
            description: format!("Failed to read {}", path.display()),
            source: e,
        })
    });
    let _ = std::fs::remove_file(&path);
    let bytes = bytes.map_err(error_to_response)?;
    Ok(([(CONTENT_TYPE, "application/vnd.sqlite3")], bytes))
}

pub(crate) async fn sync_full_upload_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Query(ReplicaQuery { replica }): Query<ReplicaQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, HandlerError> {
    require_role(SyncRole::Server)?;
    let replica = replica.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "missing `replica`" })),
        )
    })?;
    let path = temp_dir()?.join(format!("upload-{}.sqlite", std::process::id()));
    std::fs::write(&path, &body).map_err(|e| {
        error_to_response(Error::Io {
            description: format!("Failed to write {}", path.display()),
            source: e,
        })
    })?;
    let db = data.db.clone();
    let upload_path = path.clone();
    let result = run_exclusive(async move {
        let before = files::note_ids(&db).await?;
        let response = server::full_upload(&db, &upload_path, replica).await?;
        let after = files::note_ids(&db).await?;
        let deleted = before.difference(&after).copied().collect();
        files::after_sync(&db, &after, &deleted).await;
        Ok(response)
    })
    .await;
    let _ = std::fs::remove_file(&path);
    Ok(Json(result.map_err(error_to_response)?))
}

#[derive(Debug, Deserialize)]
pub(crate) struct SyncRunRequest {
    #[serde(default = "normal")]
    mode: SyncMode,
}

fn normal() -> SyncMode {
    SyncMode::Normal
}

/// Syncs this (client) server with its `sync_url`.
pub(crate) async fn sync_run_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(SyncRunRequest { mode }): Json<SyncRunRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    require_role(SyncRole::Client)?;
    let db = data.db.clone();
    let temp_dir = temp_dir()?;
    let report = run_exclusive(async move { files::sync_now(&db, mode, &temp_dir).await })
        .await
        .map_err(error_to_response)?;
    Ok(Json(report))
}

#[derive(Debug, Serialize)]
pub(crate) struct SyncStatusResponse {
    role: String,
    replica: u8,
    sync_url: Option<String>,
    #[serde(flatten)]
    state: LocalSyncState,
}

pub(crate) async fn sync_status_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, HandlerError> {
    let state = local_state(&data.db).await.map_err(error_to_response)?;
    Ok(Json(SyncStatusResponse {
        role: get_sync_role().map_err(error_to_response)?.to_string(),
        replica: get_replica().map_err(error_to_response)?.number(),
        sync_url: read_external_config().ok().and_then(|c| c.sync_url),
        state,
    }))
}
