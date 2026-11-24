//! The browser's storage: SQLite files in the origin private file system (OPFS), through the
//! `SAHPool` VFS of sqlite-wasm-vfs.
//!
//! - `spares.sqlite`: the copy of the collection. A full download replaces its contents
//!   ([`spares_core::sync::full::restore`]), never the file, so its own `id_counter` is merged.
//! - `device.sqlite`: what the device knows about itself, kept out of the collection (decision 6
//!   in `spares_core::sync`): its replica number, and the config and parser templates from the
//!   server's last `GET /api/sync/config`, which [`spares_core::platform`] reads, and which
//!   HTML files for web clients the web app has downloaded. Also its images, audio and image
//!   occlusions ([`crate::media`]).
//! - `spares-download.sqlite` and `spares-upload.sqlite`: a full sync's snapshot, while it runs.
//!
//! The VFS has no locks between tabs, so one tab at a time may open the storage (the web app
//! holds a Web Lock for it).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use rsqlite_vfs::VfsFilesManager;
use spares_core::collection::Collection;
use spares_core::config::Replica;
use spares_core::config::get_config_dir;
use spares_core::config::get_files_dir;
use spares_core::config::set_device_replica;
use spares_core::platform::MediaDirs;
use spares_core::sync::devices::DeviceConfig;
use spares_server::route::create_local_router;
use sqlite_wasm_vfs::sahpool::OpfsSAHPoolCfgBuilder;
use sqlite_wasm_vfs::sahpool::OpfsSAHPoolUtil;
use sqlite_wasm_vfs::sahpool::install;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use wasm_bindgen::JsValue;

use crate::js_error;

pub(crate) const DB_FILE: &str = "spares.sqlite";
const DEVICE_FILE: &str = "device.sqlite";
/// Where full syncs keep their snapshots (see `spares_core::sync::run`'s `temp_file`)
pub(crate) const TEMP_FILES: [&str; 2] = ["spares-download.sqlite", "spares-upload.sqlite"];

/// What [`open`] set up, for the rest of the worker's life.
#[derive(Clone)]
pub(crate) struct Opened {
    pub(crate) vfs: OpfsSAHPoolUtil,
    pub(crate) db: SqlitePool,
    pub(crate) device: SqlitePool,
    pub(crate) router: axum::Router,
}

thread_local! {
    static VFS: RefCell<Option<OpfsSAHPoolUtil>> = const { RefCell::new(None) };
    static OPENED: RefCell<Option<Opened>> = const { RefCell::new(None) };
}

/// Installs the VFS once per worker.
async fn vfs() -> Result<OpfsSAHPoolUtil, JsValue> {
    if let Some(vfs) = VFS.with(|v| v.borrow().clone()) {
        return Ok(vfs);
    }
    // Room for both databases with their journals and a full sync's snapshot
    let cfg = OpfsSAHPoolCfgBuilder::new()
        .directory("spares")
        .initial_capacity(10)
        .build();
    let vfs = install::<sqlite_wasm_rs::WasmOsCallback>(&cfg, true)
        .await
        .map_err(|e| {
            js_error(&format!(
                "This browser's storage for a copy can't be opened, e.g. in a private window ({e})."
            ))
        })?;
    VFS.with(|v| *v.borrow_mut() = Some(vfs.clone()));
    Ok(vfs)
}

/// The storage, once [`open`] has run.
pub(crate) fn opened() -> Result<Opened, JsValue> {
    OPENED
        .with(|o| o.borrow().clone())
        .ok_or_else(|| js_error("the copy isn't open yet: call open() first"))
}

#[allow(clippy::needless_pass_by_value, reason = "for `map_err`")]
pub(crate) fn sqlx_error(e: sqlx::Error) -> JsValue {
    js_error(&e.to_string())
}

async fn connect(file: &str) -> Result<SqlitePool, JsValue> {
    // SAHPool is one connection per file, and has no WAL. `mmap_size` isn't set, unlike on the
    // server: there is no memory map in a browser.
    let options = SqliteConnectOptions::new()
        .filename(file)
        .create_if_missing(true)
        .with_regexp();
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(sqlx_error)
}

/// Installs the VFS, opens both databases, migrates the collection, and makes the device's replica
/// and config the ones `spares_core` uses. Later calls return what the first set up.
pub(crate) async fn open() -> Result<Opened, JsValue> {
    if let Ok(opened) = opened() {
        return Ok(opened);
    }
    let vfs = vfs().await?;
    // Snapshots a sync left behind when the page closed during it
    for file in TEMP_FILES {
        remove_file(&vfs, file)?;
    }
    let device = connect(DEVICE_FILE).await?;
    for statement in [
        "CREATE TABLE IF NOT EXISTS device_state (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS config_file (path TEXT PRIMARY KEY NOT NULL, contents TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS web_render_file (path TEXT PRIMARY KEY NOT NULL)",
        crate::media::CREATE_TABLE,
    ] {
        sqlx::query(statement)
            .execute(&device)
            .await
            .map_err(sqlx_error)?;
    }
    let db = connect(DB_FILE).await?;
    sqlx::migrate!("../spares_core/migrations")
        .run(&db)
        .await
        .map_err(|e| js_error(&format!("failed to migrate the copy: {e}")))?;
    // The device's one collection, current everywhere in the worker
    let collection = Arc::new(Collection::new(
        "device",
        db.clone(),
        spares_core::collection::device_paths(),
    ));
    spares_core::collection::set_default(Some(Arc::clone(&collection)));
    set_device_replica(load_replica(&device).await?);
    load_config(&device).await?;
    load_media_dirs(&device).await?;
    load_web_renders(&device).await?;
    crate::media::load(&device).await?;
    let router = create_local_router(collection);
    let opened = Opened {
        vfs,
        db,
        device,
        router,
    };
    OPENED.with(|o| *o.borrow_mut() = Some(opened.clone()));
    Ok(opened)
}

/// Removes `file` from the VFS, if it is there.
pub(crate) fn remove_file(vfs: &OpfsSAHPoolUtil, file: &str) -> Result<(), JsValue> {
    if vfs
        .contains(file)
        .map_err(|e| js_error(&format!("{e:?}")))?
    {
        vfs.remove(file).map_err(|e| js_error(&format!("{e:?}")))?;
    }
    Ok(())
}

async fn get_state(device: &SqlitePool, key: &str) -> Result<Option<String>, JsValue> {
    sqlx::query_scalar("SELECT value FROM device_state WHERE key = ?")
        .bind(key)
        .fetch_optional(device)
        .await
        .map_err(sqlx_error)
}

async fn set_state(device: &SqlitePool, key: &str, value: Option<&str>) -> Result<(), JsValue> {
    match value {
        Some(value) => sqlx::query(
            "INSERT INTO device_state (key, value) VALUES (?, ?)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value),
        None => sqlx::query("DELETE FROM device_state WHERE key = ?").bind(key),
    }
    .execute(device)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

/// The replica number the server gave this device, if it has enrolled.
pub(crate) async fn load_replica(device: &SqlitePool) -> Result<Option<Replica>, JsValue> {
    get_state(device, "replica")
        .await?
        .map(|value| value.parse().map_err(|e: String| js_error(&e)))
        .transpose()
}

/// Records the device's replica number (`None` when it unenrolls), and uses it from now on.
pub(crate) async fn save_replica(
    device: &SqlitePool,
    replica: Option<Replica>,
) -> Result<(), JsValue> {
    let value = replica.map(|r| r.number().to_string());
    set_state(device, "replica", value.as_deref()).await?;
    set_device_replica(replica);
    Ok(())
}

/// Puts the saved config files in [`spares_core::platform`].
async fn load_config(device: &SqlitePool) -> Result<(), JsValue> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT path, contents FROM config_file")
        .fetch_all(device)
        .await
        .map_err(sqlx_error)?;
    let dir = get_config_dir();
    let files: BTreeMap<PathBuf, Vec<u8>> = rows
        .into_iter()
        .map(|(path, contents)| (dir.join(path), contents.into_bytes()))
        .collect();
    spares_core::platform::load_dir(&dir, files);
    Ok(())
}

/// Replaces the saved config files with the server's, and uses them from now on.
pub(crate) async fn save_config(device: &SqlitePool, config: &DeviceConfig) -> Result<(), JsValue> {
    let mut tx = device.begin().await.map_err(sqlx_error)?;
    sqlx::query("DELETE FROM config_file")
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    for (path, contents) in &config.files {
        sqlx::query("INSERT INTO config_file (path, contents) VALUES (?, ?)")
            .bind(path)
            .bind(contents)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    tx.commit().await.map_err(sqlx_error)?;
    for (key, dir) in [
        ("image_dir", &config.image_dir),
        ("audio_dir", &config.audio_dir),
        ("image_occlusion_dir", &config.image_occlusion_dir),
    ] {
        set_state(device, key, Some(&dir.to_string_lossy())).await?;
    }
    set_state(device, "pandoc_version", config.pandoc_version.as_deref()).await?;
    load_config(device).await?;
    load_media_dirs(device).await
}

/// The server's pandoc version and its web defaults file, as the last sync brought them.
pub(crate) async fn pandoc_config(
    device: &SqlitePool,
) -> Result<(Option<String>, Option<String>), JsValue> {
    let web_defaults = sqlx::query_scalar("SELECT contents FROM config_file WHERE path = ?")
        .bind(WEB_DEFAULTS_FILE)
        .fetch_optional(device)
        .await
        .map_err(sqlx_error)?;
    Ok((get_state(device, "pandoc_version").await?, web_defaults))
}

/// Where [`DeviceConfig::files`] has the web defaults file, relative to the config directory
const WEB_DEFAULTS_FILE: &str = "markdown/web-defaults.yaml";

/// Makes the server's directories for images, audio and image occlusions the ones
/// [`spares_core::platform`] names files by, once a sync has brought them.
async fn load_media_dirs(device: &SqlitePool) -> Result<(), JsValue> {
    let image = get_state(device, "image_dir").await?;
    let audio = get_state(device, "audio_dir").await?;
    let image_occlusion = get_state(device, "image_occlusion_dir").await?;
    let dirs = match (image, audio, image_occlusion) {
        (Some(image), Some(audio), Some(image_occlusion)) => Some(MediaDirs {
            image: image.into(),
            audio: audio.into(),
            image_occlusion: image_occlusion.into(),
        }),
        _ => None,
    };
    spares_core::platform::set_media_dirs(dirs);
    Ok(())
}

/// Puts the saved HTML files for web clients in [`spares_core::platform`].
async fn load_web_renders(device: &SqlitePool) -> Result<(), JsValue> {
    let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM web_render_file")
        .fetch_all(device)
        .await
        .map_err(sqlx_error)?;
    let dir = get_files_dir();
    let paths = paths.into_iter().map(|path| dir.join(path)).collect();
    spares_core::platform::load_web_renders(paths);
    Ok(())
}

/// Replaces the saved HTML files for web clients with `paths`, relative to the files directory, and
/// uses them from now on.
pub(crate) async fn save_web_renders(
    device: &SqlitePool,
    paths: &BTreeSet<String>,
) -> Result<(), JsValue> {
    let mut tx = device.begin().await.map_err(sqlx_error)?;
    sqlx::query("DELETE FROM web_render_file")
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    for path in paths {
        sqlx::query("INSERT INTO web_render_file (path) VALUES (?)")
            .bind(path)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    tx.commit().await.map_err(sqlx_error)?;
    load_web_renders(device).await
}

/// Forgets the device's copy: its collection, replica number, config, media and downloaded files. The
/// next [`open`] in a new worker starts empty.
pub(crate) async fn forget(opened: &Opened) -> Result<(), JsValue> {
    opened.db.close().await;
    opened.device.close().await;
    OPENED.with(|o| *o.borrow_mut() = None);
    set_device_replica(None);
    spares_core::collection::set_default(None);
    spares_core::platform::clear();
    spares_core::platform::set_media_dirs(None);
    spares_core::platform::load_web_renders(BTreeSet::new());
    for file in [DB_FILE, DEVICE_FILE]
        .into_iter()
        .chain(TEMP_FILES)
        .flat_map(|file| [file.to_string(), format!("{file}-journal")])
    {
        remove_file(&opened.vfs, &file)?;
    }
    Ok(())
}
