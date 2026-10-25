//! spares in a browser: a copy of the collection that works offline and syncs with the server like
//! the laptop does (see `spares_core::sync`, "More replicas").
//!
//! The web app runs it in a Dedicated Worker, where the browser's storage is reachable
//! synchronously, and sends it requests over `postMessage`:
//!
//! - [`open`] opens the copy in the browser's storage ([`storage`]).
//! - [`handle`] answers an API request with the server's own router, as far as it can without the
//!   server ([`spares_server::route::create_local_router`]). Requests it can't answer get
//!   `503 {"needs_server": true}`, which the web app sends to the server instead.
//! - [`enroll`], [`sync`] and [`unenroll`] talk to the server, through the browser's `fetch`.
//! - [`pending_notes`] and [`render_assets`] tell the web app what to show for notes the server
//!   hasn't rendered yet, and what to download for Typst cards. [`web_renders`] tells the device
//!   which HTML files for web clients the web app has downloaded.
//!
//! **What runs where.** The device runs `spares_core` on its own database: reviews and their
//! scheduling, searches, statistics, creating and editing notes, tags and undo. It writes no note
//! or card files and renders nothing (see [`spares_core::platform`]): the server does that once a
//! sync brings it the device's changes. Settings come from the server's `config.toml` at each
//! sync, and are read-only here.
//!
//! Errors are JS `Error`s with a `message`, and `full_sync_required` (then the user has to choose
//! between downloading the server's copy and uploading this one) or `not_enrolled` (the device has
//! to set up its copy again) when those apply.
#![cfg(target_family = "wasm")]

mod remote;
mod storage;

use std::path::Path;

use axum::body::Body;
use spares_core::Error;
use spares_core::config::get_replica;
use spares_core::sync::http::HttpRemote;
use spares_core::sync::run::SyncMode;
use tower::ServiceExt;
use wasm_bindgen::prelude::*;

use crate::remote::DeviceRemote;
use crate::storage::opened;

/// The largest response body [`handle`] collects: a whole note export fits.
const MAX_RESPONSE: usize = 512 * 1024 * 1024;

fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

/// `e` as a JS `Error`, with `full_sync_required` or `not_enrolled` set when they apply.
fn core_error(e: &Error) -> JsValue {
    let error = js_sys::Error::new(&e.to_string());
    let flag = match e {
        Error::FullSyncRequired(_) => Some("full_sync_required"),
        Error::NotEnrolled(_) => Some("not_enrolled"),
        _ => None,
    };
    if let Some(flag) = flag {
        let _ = js_sys::Reflect::set(&error, &flag.into(), &JsValue::TRUE);
    }
    error.into()
}

/// A serializable value as a JS one, through JSON
fn to_js(value: &impl serde::Serialize) -> Result<JsValue, JsValue> {
    let json = serde_json::to_string(value).map_err(|e| js_error(&e.to_string()))?;
    js_sys::JSON::parse(&json)
}

fn remote(server: &str, key: Option<String>) -> Result<DeviceRemote, JsValue> {
    Ok(DeviceRemote(
        HttpRemote::new(server, key).map_err(|e| core_error(&e))?,
    ))
}

/// Opens the copy, creating an empty one the first time, and returns [`status`].
#[wasm_bindgen]
pub async fn open() -> Result<JsValue, JsValue> {
    console_error_panic_hook::set_once();
    storage::open().await?;
    status().await
}

/// `{replica, dirty, usn, epoch, last_sync_at, needs_full_sync, conflicts}`: the device's replica
/// number (`null` before it enrolls), how many rows changed since the last sync, and the rest of
/// `spares_core::sync::run::LocalSyncState`.
#[wasm_bindgen]
pub async fn status() -> Result<JsValue, JsValue> {
    let opened = opened()?;
    let mut state = serde_json::to_value(
        spares_core::sync::run::local_state(&opened.db)
            .await
            .map_err(|e| core_error(&e))?,
    )
    .map_err(|e| js_error(&e.to_string()))?;
    let dirty = spares_core::sync::dirty_count(&opened.db)
        .await
        .map_err(|e| core_error(&e))?;
    state["dirty"] = dirty.into();
    state["replica"] = get_replica().ok().map(|r| r.number()).into();
    to_js(&state)
}

/// Answers an API request: `{status, headers, body}`, where `headers` are `[name, value]` pairs
/// and `body` a `Uint8Array`. `headers` are pairs too.
#[wasm_bindgen]
pub async fn handle(
    method: String,
    uri: String,
    headers: js_sys::Array,
    body: Option<Vec<u8>>,
) -> Result<JsValue, JsValue> {
    let router = opened()?.router;
    let mut request = axum::http::Request::builder()
        .method(method.as_str())
        .uri(&uri);
    for pair in headers.iter() {
        let pair = js_sys::Array::from(&pair);
        if let (Some(name), Some(value)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
            request = request.header(name, value);
        }
    }
    let request = request
        .body(Body::from(body.unwrap_or_default()))
        .map_err(|e| js_error(&format!("{method} {uri}: {e}")))?;
    let response = match router.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let (parts, body) = response.into_parts();
    let body = axum::body::to_bytes(body, MAX_RESPONSE)
        .await
        .map_err(|e| js_error(&format!("{method} {uri}: {e}")))?;
    let headers = js_sys::Array::new();
    for (name, value) in &parts.headers {
        if let Ok(value) = value.to_str() {
            headers.push(&js_sys::Array::of2(&name.as_str().into(), &value.into()));
        }
    }
    let result = js_sys::Object::new();
    js_sys::Reflect::set(&result, &"status".into(), &parts.status.as_u16().into())?;
    js_sys::Reflect::set(&result, &"headers".into(), &headers)?;
    js_sys::Reflect::set(
        &result,
        &"body".into(),
        &js_sys::Uint8Array::from(body.as_ref()),
    )?;
    Ok(result.into())
}

/// Asks `server` for a replica number for this device, named `name` in its list of devices, and
/// keeps it. Then [`sync`] with `"full_download"` fills the copy.
#[wasm_bindgen]
pub async fn enroll(server: String, key: Option<String>, name: String) -> Result<u8, JsValue> {
    let opened = opened()?;
    if let Some(replica) = storage::load_replica(&opened.device).await? {
        return Err(js_error(&format!(
            "this device is already replica {}",
            replica.number()
        )));
    }
    let response = remote(&server, key)?
        .0
        .enroll(&name)
        .await
        .map_err(|e| core_error(&e))?;
    let replica = response
        .replica
        .to_string()
        .parse()
        .map_err(|e: String| js_error(&e))?;
    storage::save_replica(&opened.device, Some(replica)).await?;
    Ok(response.replica)
}

/// Syncs with `server` and returns `spares_core::sync::run::SyncReport`. `mode` is `"normal"`,
/// `"full_download"` (replace this copy with the server's) or `"full_upload"` (replace the
/// server's, which makes every other device download it). Then it updates the config and parser
/// templates from the server.
#[wasm_bindgen]
pub async fn sync(server: String, key: Option<String>, mode: String) -> Result<JsValue, JsValue> {
    let opened = opened()?;
    let mode: SyncMode = serde_json::from_value(serde_json::Value::String(mode.clone()))
        .map_err(|_| js_error(&format!("unknown sync mode `{mode}`")))?;
    let remote = remote(&server, key)?;
    let report = spares_core::sync::run::sync(&opened.db, &remote, mode, Path::new(""))
        .await
        .map_err(|e| core_error(&e));
    // A download's snapshot, which `restore` attached
    for file in storage::TEMP_FILES {
        storage::remove_file(&opened.vfs, file)?;
    }
    let report = report?;
    let config = remote.0.device_config().await.map_err(|e| core_error(&e))?;
    storage::save_config(&opened.device, &config).await?;
    to_js(&report)
}

/// Frees this device's replica number on `server`, and deletes the copy. Sync first: changes that
/// haven't reached the server are lost.
#[wasm_bindgen]
pub async fn unenroll(server: String, key: Option<String>) -> Result<(), JsValue> {
    let opened = opened()?;
    if let Some(replica) = storage::load_replica(&opened.device).await? {
        remote(&server, key)?
            .0
            .unenroll(replica.number())
            .await
            .map_err(|e| core_error(&e))?;
    }
    storage::forget(&opened).await
}

/// Deletes the copy without telling the server, for a device the server no longer has enrolled
/// (`not_enrolled`): its number may belong to another device by now.
#[wasm_bindgen]
pub async fn forget() -> Result<(), JsValue> {
    storage::forget(&opened()?).await
}

/// The ids of the notes changed here since the last sync, which the server hasn't rendered yet.
#[wasm_bindgen]
pub async fn pending_notes() -> Result<Vec<i64>, JsValue> {
    sqlx::query_scalar("SELECT id FROM note WHERE usn < 0")
        .fetch_all(&opened()?.db)
        .await
        .map_err(|e| js_error(&e.to_string()))
}

/// `[{path, hash}]`: every file the server read while rendering notes (`render_asset`), such as
/// Typst preambles and images, which Typst cards need to compile offline. A device downloads them
/// with `GET /api/render-assets?path=`, again when the hash changes.
#[wasm_bindgen]
pub async fn render_assets() -> Result<JsValue, JsValue> {
    let assets: Vec<(String, String)> = sqlx::query_as("SELECT path, hash FROM render_asset")
        .fetch_all(&opened()?.db)
        .await
        .map_err(|e| js_error(&e.to_string()))?;
    let assets: Vec<serde_json::Value> = assets
        .into_iter()
        .map(|(path, hash)| serde_json::json!({ "path": path, "hash": hash }))
        .collect();
    to_js(&assets)
}

/// Records `paths`, relative to the files directory, as the non-empty HTML files for web clients
/// the web app has downloaded (`GET /api/sync/files` lists them next to each rendered file). Notes
/// and cards are shown with them (`web_path`) only then, since the device can't look at the files.
#[wasm_bindgen]
pub async fn web_renders(paths: Vec<String>) -> Result<(), JsValue> {
    storage::save_web_renders(&opened()?.device, &paths.into_iter().collect()).await
}
