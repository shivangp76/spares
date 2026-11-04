//! Two-way sync (see `spares_core::sync`). Every host answers `meta`, `pull`, `push` and `full`
//! for its users' devices, or relays them to the collection's upstream if it has one (see
//! [`super::relay`]). A collection with an upstream also syncs with it, when asked (`run`) and
//! automatically (`crate::auto_sync`).

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use spares_core::Error;
use spares_core::config::get_cache_dir;
use spares_core::config::get_replica;
use spares_core::config::read_external_config;
#[cfg(not(target_family = "wasm"))]
use spares_core::notes_vcs::WRITER_WAIT;
#[cfg(not(target_family = "wasm"))]
use spares_core::notes_vcs::exclusive_guard;
use spares_core::notes_vcs::run_exclusive;
use spares_core::parsers::get_all_parsers;
use spares_core::sync::conflicts;
use spares_core::sync::conflicts::Resolution;
use spares_core::sync::devices;
use spares_core::sync::devices::EnrollRequest;
use spares_core::sync::devices::UnenrollRequest;
use spares_core::sync::files;
use spares_core::sync::media;
use spares_core::sync::media::ChangedFilesRequest;
use spares_core::sync::media::MediaKind;
use spares_core::sync::protocol::PullRequest;
use spares_core::sync::protocol::PushRequest;
use spares_core::sync::run::LocalSyncState;
use spares_core::sync::run::SyncMode;
use spares_core::sync::run::local_state;
use spares_core::sync::server;

use crate::Coll;
use crate::handlers::error_to_response;

/// Pushes and full uploads carry whole databases.
pub(crate) const SYNC_BODY_LIMIT: usize = 1024 * 1024 * 1024;

/// A device's uploaded image or clip, with room above `spares_core`'s own limits so that it, not
/// the body limit, says what is too large.
pub(crate) const MEDIA_BODY_LIMIT: usize = 128 * 1024 * 1024;

type HandlerError = (StatusCode, Json<Value>);

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

/// A database snapshot's file in [`temp_dir`], one for each request: `snapshot` removes the file
/// before writing it, so two full downloads at once must not share one.
fn temp_file(name: &str) -> Result<PathBuf, HandlerError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    Ok(temp_dir()?.join(format!("{name}-{}-{n}.sqlite", std::process::id())))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReplicaQuery {
    replica: Option<u8>,
}

pub(crate) async fn sync_meta_handler(
    Coll(data): Coll,
    Query(ReplicaQuery { replica }): Query<ReplicaQuery>,
) -> Result<impl IntoResponse, HandlerError> {
    let meta = server::meta(&data.db, replica)
        .await
        .map_err(error_to_response)?;
    Ok(Json(meta))
}

pub(crate) async fn sync_pull_handler(
    Coll(data): Coll,
    Json(body): Json<PullRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let response = server::pull(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(response))
}

pub(crate) async fn sync_push_handler(
    Coll(data): Coll,
    Json(body): Json<PushRequest>,
) -> Result<impl IntoResponse, HandlerError> {
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
    Coll(data): Coll,
) -> Result<impl IntoResponse, HandlerError> {
    let path = temp_file("download")?;
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

// Spawns on Tokio
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn sync_full_upload_handler(
    Coll(data): Coll,
    Query(ReplicaQuery { replica }): Query<ReplicaQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, HandlerError> {
    let replica = replica.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "missing `replica`" })),
        )
    })?;
    let guard = exclusive_guard(Some(WRITER_WAIT))
        .await
        .map_err(error_to_response)?;
    let path = temp_file("upload")?;
    std::fs::write(&path, &body).map_err(|e| {
        error_to_response(Error::Io {
            description: format!("Failed to write {}", path.display()),
            source: e,
        })
    })?;
    let db = data.db.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // Answers once the database is replaced, then regenerates every note's files, still holding the
    // guard. Without a cache that takes minutes, which the device uploading shouldn't wait for.
    spares_core::collection::spawn(async move {
        let _guard = guard;
        let uploaded = async {
            let before = files::note_ids(&db).await?;
            let response = server::full_upload(&db, &path, replica).await?;
            let after = files::note_ids(&db).await?;
            Ok::<_, Error>((response, before, after))
        }
        .await;
        let _ = std::fs::remove_file(&path);
        match uploaded {
            Ok((response, before, after)) => {
                let _ = sender.send(Ok(response));
                let deleted = before.difference(&after).copied().collect();
                files::after_sync(&db, &after, &deleted).await;
            }
            Err(e) => {
                let _ = sender.send(Err(e));
            }
        }
    });
    let result = receiver
        .await
        .map_err(|_| error_to_response(Error::Sync("the full upload stopped".to_string())))?;
    Ok(Json(result.map_err(error_to_response)?))
}

/// Gives a device a replica number of its own: a browser that keeps a copy of the collection, or
/// another host's copy. A host's number is recorded on its device token too, so revoking the token
/// frees it.
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn sync_enroll_handler(
    Coll(data): Coll,
    auth: Option<axum::Extension<crate::handlers::auth::Auth>>,
    host: Option<axum::Extension<std::sync::Arc<crate::host::Host>>>,
    Json(body): Json<EnrollRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let is_host = body.host;
    let response = devices::enroll(&data.db, body)
        .await
        .map_err(error_to_response)?;
    if is_host
        && let (Some(auth), Some(host)) = (auth, host)
        && let Some(device) = &auth.device
    {
        crate::host::auth::set_token_replica(&host.accounts, device.id, Some(response.replica))
            .await
            .map_err(error_to_response)?;
    }
    Ok(Json(response))
}

pub(crate) async fn sync_unenroll_handler(
    Coll(data): Coll,
    Json(body): Json<UnenrollRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let removed = devices::unenroll(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(json!({ "removed": removed })))
}

/// What a device needs from the server's config directory (`spares_core::sync::devices`).
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn sync_config_handler() -> Result<impl IntoResponse, HandlerError> {
    Ok(Json(devices::device_config().map_err(error_to_response)?))
}

/// Where `spares sync cloud` copies files to, over SSH (`spares_core::sync::push`). Only for a
/// device token: the web app has no use for the server's paths.
#[cfg(not(target_family = "wasm"))]
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn sync_push_target_handler(
    auth: Option<axum::Extension<crate::handlers::auth::Auth>>,
) -> Result<impl IntoResponse, HandlerError> {
    require_device_token(auth.as_ref().map(|auth| &auth.0))?;
    Ok(Json(
        spares_core::sync::push::push_target().map_err(error_to_response)?,
    ))
}

/// `spares sync cloud` sending its machine's `config.toml`, which replaces this collection's but
/// for its `[sync]` table (`spares_core::sync::push::apply_pushed_config`).
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn sync_push_config_handler(
    auth: Option<axum::Extension<crate::handlers::auth::Auth>>,
    body: String,
) -> Result<impl IntoResponse, HandlerError> {
    require_device_token(auth.as_ref().map(|auth| &auth.0))?;
    // Not while a render reads it
    let changed = run_exclusive(async move { spares_core::sync::push::apply_pushed_config(&body) })
        .await
        .map_err(|e| match e {
            Error::Library(spares_core::LibraryError::InvalidConfig(message)) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": format!("Invalid config.toml: {message}") })),
            ),
            e => error_to_response(e),
        })?;
    Ok(Json(json!({ "changed": changed })))
}

/// Refuses the web app's session, for the requests only `spares sync cloud` makes. A host without
/// accounts has no [`Auth`](crate::handlers::auth::Auth) to check.
#[cfg(not(target_family = "wasm"))]
fn require_device_token(auth: Option<&crate::handlers::auth::Auth>) -> Result<(), HandlerError> {
    if auth.is_some_and(|auth| auth.device.is_none()) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Only a device token can do this" })),
        ));
    }
    Ok(())
}

/// Every rendered file a device can show offline (`spares_core::sync::media`).
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn sync_files_handler() -> Result<impl IntoResponse, HandlerError> {
    Ok(Json(media::rendered_files().map_err(error_to_response)?))
}

/// The rendered files a device can show offline that changed since it last asked
/// (`spares_core::sync::media`).
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn sync_changed_files_handler(
    Json(request): Json<ChangedFilesRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    Ok(Json(
        media::changed_rendered_files(&request).map_err(error_to_response)?,
    ))
}

/// The server's files of a kind, which a device downloads (`spares_core::sync::media`).
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn sync_media_list_handler(
    Path(kind): Path<MediaKind>,
) -> Result<impl IntoResponse, HandlerError> {
    Ok(Json(media::list_media(kind).map_err(error_to_response)?))
}

/// One of them.
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn sync_media_download_handler(
    Path((kind, name)): Path<(MediaKind, String)>,
) -> Result<impl IntoResponse, HandlerError> {
    media::read_media(kind, &name)
        .map_err(error_to_response)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": format!("The server has no {name}") })),
            )
        })
}

/// A file a device added offline, before the push whose notes name it.
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn sync_media_handler(
    Path((kind, name)): Path<(MediaKind, String)>,
    auth: Option<axum::Extension<crate::handlers::auth::Auth>>,
    body: Bytes,
) -> Result<impl IntoResponse, HandlerError> {
    // Another host's copy, with its sync-only token
    let from_host = auth.is_some_and(|auth| {
        auth.device
            .as_ref()
            .is_some_and(|device| device.scope == crate::host::auth::Scope::Sync)
    });
    let path = if from_host {
        media::store_media_from_host(kind, &name, &body)
    } else {
        media::store_media(kind, &name, &body)
    }
    .map_err(error_to_response)?;
    // The upstream renders the notes that name it too, once the device's push reaches it.
    #[cfg(not(target_family = "wasm"))]
    if let Some((url, token)) = crate::handlers::relay::upstream() {
        let remote = spares_core::sync::http::HttpRemote::new(
            &url,
            spares_core::sync::http::RemoteAuth::token(token),
        )
        .map_err(error_to_response)?;
        if let Err(e) = remote.upload_media(kind, &name, body.to_vec()).await {
            log::warn!("Failed to send {name} on to {url}: {e}");
        }
    }
    Ok(Json(json!({ "path": path })))
}

pub(crate) async fn sync_devices_handler(
    Coll(data): Coll,
) -> Result<impl IntoResponse, HandlerError> {
    let devices = devices::list(&data.db).await.map_err(error_to_response)?;
    Ok(Json(devices))
}

/// Frees a replica number of `collection` that a revoked device enrolled as.
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn free_replica(
    collection: &std::sync::Arc<spares_core::collection::Collection>,
    replica: u8,
) -> Result<(), Error> {
    let db = collection.db.clone();
    collection
        .scope(async move { devices::unenroll(&db, UnenrollRequest { replica }).await })
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(crate) struct SyncRunRequest {
    #[serde(default = "normal")]
    mode: SyncMode,
}

fn normal() -> SyncMode {
    SyncMode::Normal
}

/// Syncs this (client) server with its `sync.url`.
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn sync_run_handler(
    Coll(data): Coll,
    Json(SyncRunRequest { mode }): Json<SyncRunRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let db = data.db.clone();
    let temp_dir = temp_dir()?;
    let report = run_exclusive(async move { files::sync_now(&db, mode, &temp_dir).await })
        .await
        .map_err(error_to_response)?;
    Ok(Json(report))
}

#[derive(Debug, Serialize)]
pub(crate) struct SyncStatusResponse {
    replica: u8,
    /// The collection's upstream, if this copy has one. Without one, it is the root.
    sync_url: Option<String>,
    /// Whether this host has a device token for `sync_url`, which it needs to reach it.
    sync_token_set: bool,
    #[serde(flatten)]
    state: LocalSyncState,
}

pub(crate) async fn sync_status_handler(
    Coll(data): Coll,
) -> Result<impl IntoResponse, HandlerError> {
    let state = local_state(&data.db).await.map_err(error_to_response)?;
    let config = read_external_config().ok();
    Ok(Json(SyncStatusResponse {
        replica: get_replica().map_err(error_to_response)?.number(),
        sync_url: config.as_ref().and_then(|c| c.sync.url.clone()),
        sync_token_set: config.as_ref().is_some_and(|c| c.sync.token.is_some()),
        state,
    }))
}

pub(crate) async fn list_sync_conflicts_handler(
    Coll(data): Coll,
) -> Result<impl IntoResponse, HandlerError> {
    let conflicts = conflicts::list_conflicts(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(conflicts))
}

pub(crate) async fn resolve_sync_conflict_handler(
    Coll(data): Coll,
    Path(id): Path<i64>,
    Json(resolution): Json<Resolution>,
) -> Result<impl IntoResponse, HandlerError> {
    let db = data.db.clone();
    run_exclusive(async move {
        conflicts::resolve_conflict(&db, id, resolution, &get_all_parsers()).await
    })
    .await
    .map_err(error_to_response)?;
    Ok(Json(json!({})))
}

/// `PUT /api/sync/upstream`: what `spares sync login` set up on the upstream.
#[cfg(not(target_family = "wasm"))]
#[derive(Debug, Deserialize)]
pub(crate) struct UpstreamRequest {
    url: String,
    /// The sync-only device token this host got there
    token: String,
    /// The replica the upstream gave this host's copy
    replica: u8,
}

/// `PUT /api/sync/upstream`: makes the collection's copy here a device of an upstream, with the
/// token and replica it got there, and starts syncing with it automatically if its config says
/// to. `DELETE` makes it the root of its sync chain again, replica 1.
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn set_upstream_handler(
    Coll(collection): Coll,
    axum::Extension(host): axum::Extension<std::sync::Arc<crate::host::Host>>,
    Json(request): Json<UpstreamRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let replica = spares_core::config::Replica::try_from(request.replica)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "message": e }))))?;
    let url = request.url.trim_end_matches('/').to_string();
    write_sync_values(
        &collection,
        [
            ("url", url.clone().into()),
            ("token", request.token.into()),
            ("replica", i64::from(replica.number()).into()),
        ],
        &[],
    )?;
    collection.set_replica(Some(replica));
    host.sync.start(std::sync::Arc::clone(&collection)).await;
    Ok(Json(json!({ "url": url, "replica": replica.number() })))
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn remove_upstream_handler(
    Coll(collection): Coll,
    axum::Extension(host): axum::Extension<std::sync::Arc<crate::host::Host>>,
) -> Result<impl IntoResponse, HandlerError> {
    write_sync_values(&collection, [], &["url", "token", "replica"])?;
    collection.set_replica(Some(spares_core::config::Replica::ROOT));
    host.sync.start(std::sync::Arc::clone(&collection)).await;
    Ok(Json(
        json!({ "replica": spares_core::config::Replica::ROOT.number() }),
    ))
}

/// Sets and removes keys of `[sync]` in the collection's config.toml, keeping the rest as written.
#[cfg(not(target_family = "wasm"))]
fn write_sync_values<const N: usize>(
    collection: &spares_core::collection::Collection,
    set: [(&'static str, toml_edit::Value); N],
    remove: &[&str],
) -> Result<(), HandlerError> {
    let path = collection.paths.config_file();
    let io = |e: std::io::Error| {
        error_to_response(Error::Io {
            description: format!("Failed to update {}", path.display()),
            source: e,
        })
    };
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    let contents =
        spares_core::config::set_sync_values(&contents, set).map_err(error_to_response)?;
    let mut doc: toml_edit::DocumentMut = contents
        .parse()
        .map_err(|e: toml_edit::TomlError| error_to_response(Error::Sync(e.to_string())))?;
    if let Some(sync) = doc.get_mut("sync").and_then(|s| s.as_table_like_mut()) {
        for key in remove {
            sync.remove(key);
        }
    }
    std::fs::create_dir_all(&collection.paths.config).map_err(io)?;
    std::fs::write(&path, doc.to_string()).map_err(io)
}
