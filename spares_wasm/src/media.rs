//! The device's images, audio and image occlusions, in `device.sqlite` (`media_file`), named by the
//! path they have on the server (see `spares_core::sync::media`).
//!
//! - **Added here**: `spares_core` writes them through [`spares_core::platform`] while answering a
//!   request, and [`save_written`] keeps them afterwards, not uploaded yet. [`upload`] sends them
//!   before a sync's push. Image occlusions wait until a note names them: the parse that saves the
//!   note writes card orders into the clozes SVG, and the server never replaces a file.
//! - **The server's image occlusions**: the parser reads their clozes SVGs and checks that their
//!   images exist, and editing one copies its image. [`download`] gets the ones the device doesn't
//!   have after each sync, and SVGs again when the server rewrote them.
//!
//! Clozes SVGs stay in memory, since the parser reads them synchronously. The rest is read from
//! here when needed ([`load_file`]), such as an image to show while it isn't on the server.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;

use futures::StreamExt;
use spares_core::platform;
use spares_core::sync::http::HttpRemote;
use spares_core::sync::media::MediaKind;
use sqlx::SqlitePool;
use wasm_bindgen::JsValue;

use crate::core_error;
use crate::js_error;
use crate::storage::opened;
use crate::storage::sqlx_error;

/// `uploaded` is 0 for a file added here that isn't on the server yet. `mtime` is the server's,
/// once the device has seen the file there. `width` and `height` are an image's, recorded when it
/// is stored, so that the parser has them without reading it (see
/// [`spares_core::platform::image_size`]).
pub(crate) const CREATE_TABLE: &str = "CREATE TABLE IF NOT EXISTS media_file (
    path TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    contents BLOB NOT NULL,
    mtime INTEGER,
    uploaded INTEGER NOT NULL,
    width INTEGER,
    height INTEGER
)";

/// Adds `width` and `height` to a `media_file` made before them, and records them for the images it
/// has.
pub(crate) async fn migrate(device: &SqlitePool) -> Result<(), JsValue> {
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('media_file')")
            .fetch_all(device)
            .await
            .map_err(sqlx_error)?;
    if columns.iter().any(|column| column == "width") {
        return Ok(());
    }
    for statement in [
        "ALTER TABLE media_file ADD COLUMN width INTEGER",
        "ALTER TABLE media_file ADD COLUMN height INTEGER",
    ] {
        sqlx::query(statement)
            .execute(device)
            .await
            .map_err(sqlx_error)?;
    }
    let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM media_file WHERE kind != ?")
        .bind(MediaKind::Audio.as_str())
        .fetch_all(device)
        .await
        .map_err(sqlx_error)?;
    for path in paths {
        if let Some(contents) = read(device, &path).await? {
            set_size(device, &path, &contents).await?;
        }
    }
    Ok(())
}

/// Records the size of the file at `path`, if it is an image, from its `contents`.
async fn set_size(device: &SqlitePool, path: &str, contents: &[u8]) -> Result<(), JsValue> {
    let size = platform::image_dimensions(contents);
    sqlx::query("UPDATE media_file SET width = ?, height = ? WHERE path = ?")
        .bind(size.map(|(width, _)| width))
        .bind(size.map(|(_, height)| height))
        .bind(path)
        .execute(device)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// Image occlusions downloaded at once
const DOWNLOAD_CONCURRENCY: usize = 4;

/// Whether the parser reads `path` synchronously, so it is kept in memory.
fn in_memory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_clozes.svg"))
}

fn parse_kind(kind: &str) -> Result<MediaKind, JsValue> {
    serde_json::from_value(serde_json::Value::String(kind.to_string()))
        .map_err(|_| js_error(&format!("unknown kind of file `{kind}`")))
}

/// The kind of a file `spares_core` wrote at `path`, if it is one the device keeps.
fn kind_of(path: &Path) -> Option<MediaKind> {
    let dirs = platform::media_dirs()?;
    let parent = path.parent()?;
    [
        (&dirs.image, MediaKind::Image),
        (&dirs.audio, MediaKind::Audio),
        (&dirs.image_occlusion, MediaKind::ImageOcclusion),
    ]
    .into_iter()
    .find_map(|(dir, kind)| (parent == dir).then_some(kind))
}

/// Puts the device's files in [`spares_core::platform`], and has it read the others from here.
pub(crate) async fn load(device: &SqlitePool) -> Result<(), JsValue> {
    platform::set_loader(load_file);
    let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM media_file")
        .fetch_all(device)
        .await
        .map_err(sqlx_error)?;
    let sizes: Vec<(String, u32, u32)> = sqlx::query_as(
        "SELECT path, width, height FROM media_file WHERE width IS NOT NULL AND height IS NOT NULL",
    )
    .fetch_all(device)
    .await
    .map_err(sqlx_error)?;
    platform::load_sizes(
        sizes
            .into_iter()
            .map(|(path, width, height)| (PathBuf::from(path), (width, height))),
    );
    let (in_memory, stored): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .into_iter()
        .map(PathBuf::from)
        .partition(|p| in_memory(p));
    platform::load_stored(stored);
    for path in in_memory {
        let contents: Vec<u8> =
            sqlx::query_scalar("SELECT contents FROM media_file WHERE path = ?")
                .bind(path.to_string_lossy())
                .fetch_one(device)
                .await
                .map_err(sqlx_error)?;
        platform::load_files([(path, contents)]);
    }
    Ok(())
}

/// The contents of the file at `path`, if the device has it.
pub(crate) async fn read(device: &SqlitePool, path: &str) -> Result<Option<Vec<u8>>, JsValue> {
    sqlx::query_scalar("SELECT contents FROM media_file WHERE path = ?")
        .bind(path)
        .fetch_optional(device)
        .await
        .map_err(sqlx_error)
}

/// [`platform::Loader`]
fn load_file(path: PathBuf) -> Pin<Box<dyn Future<Output = io::Result<Vec<u8>>> + Send>> {
    let device = opened()
        .map(|opened| opened.device)
        .map_err(|_| io::Error::other("the copy isn't open"));
    Box::pin(async move {
        sqlx::query_scalar("SELECT contents FROM media_file WHERE path = ?")
            .bind(path.to_string_lossy().into_owned())
            .fetch_optional(&device?)
            .await
            .map_err(io::Error::other)?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} doesn't exist", path.display()),
                )
            })
    })
}

/// Keeps the files `spares_core` wrote since the last call. Those it changed that the server has
/// keep their state: the server will rewrite them too, as the same parse runs there.
pub(crate) async fn save_written(device: &SqlitePool) -> Result<(), JsValue> {
    let mut stored = Vec::new();
    for (path, contents) in platform::take_written() {
        // Such as a default config.toml
        let Some(kind) = kind_of(&path) else { continue };
        sqlx::query(
            "INSERT INTO media_file (path, kind, contents, mtime, uploaded)
             VALUES (?, ?, ?, NULL, 0)
             ON CONFLICT (path) DO UPDATE SET contents = excluded.contents",
        )
        .bind(path.to_string_lossy())
        .bind(kind.as_str())
        .bind(&contents)
        .execute(device)
        .await
        .map_err(sqlx_error)?;
        set_size(device, &path.to_string_lossy(), &contents).await?;
        if !in_memory(&path) {
            stored.push(path);
        }
    }
    platform::load_stored(stored);
    Ok(())
}

/// Whether a note in the copy names `path`.
async fn named_by_a_note(db: &SqlitePool, path: &str) -> Result<bool, JsValue> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM note WHERE instr(data, ?) > 0)")
        .bind(path)
        .fetch_one(db)
        .await
        .map_err(sqlx_error)
}

/// Uploads the files added here that aren't on the server yet, before a sync's push. An image
/// occlusion waits until a note names it (see the module docs).
pub(crate) async fn upload(
    db: &SqlitePool,
    device: &SqlitePool,
    remote: &HttpRemote,
) -> Result<(), JsValue> {
    let pending: Vec<(String, String)> =
        sqlx::query_as("SELECT path, kind FROM media_file WHERE uploaded = 0 ORDER BY path")
            .fetch_all(device)
            .await
            .map_err(sqlx_error)?;
    for (path, kind) in pending {
        let kind = parse_kind(&kind)?;
        if kind == MediaKind::ImageOcclusion && !named_by_a_note(db, &path).await? {
            continue;
        }
        let Some(contents) = read(device, &path).await? else {
            continue;
        };
        let name = Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| js_error(&format!("{path} has no file name")))?;
        remote
            .upload_media(kind, name, contents, None)
            .await
            .map_err(|e| core_error(&e))?;
        sqlx::query("UPDATE media_file SET uploaded = 1 WHERE path = ?")
            .bind(&path)
            .execute(device)
            .await
            .map_err(sqlx_error)?;
    }
    Ok(())
}

/// Downloads the server's image occlusions the device doesn't have, and the clozes SVGs the server
/// rewrote. A file added here keeps its contents, and takes the server's time once it is there.
pub(crate) async fn download(device: &SqlitePool, remote: &HttpRemote) -> Result<(), JsValue> {
    let Some(dirs) = platform::media_dirs() else {
        return Ok(());
    };
    let kind = MediaKind::ImageOcclusion;
    let listed = remote.list_media(kind).await.map_err(|e| core_error(&e))?;
    let known: HashMap<String, (Option<i64>, bool)> =
        sqlx::query_as::<_, (String, Option<i64>, bool)>(
            "SELECT path, mtime, uploaded FROM media_file WHERE kind = ?",
        )
        .bind(kind.as_str())
        .fetch_all(device)
        .await
        .map_err(sqlx_error)?
        .into_iter()
        .map(|(path, mtime, uploaded)| (path, (mtime, uploaded)))
        .collect();
    let mut wanted = Vec::new();
    for file in listed {
        let path = dirs.image_occlusion.join(&file.name);
        let path_text = path.to_string_lossy().into_owned();
        match known.get(&path_text) {
            Some((Some(mtime), true)) if *mtime == file.mtime => {}
            // Added here, or seen before the server's time was known
            Some((None, _) | (Some(_), false)) => {
                sqlx::query("UPDATE media_file SET mtime = ?, uploaded = 1 WHERE path = ?")
                    .bind(file.mtime)
                    .bind(&path_text)
                    .execute(device)
                    .await
                    .map_err(sqlx_error)?;
            }
            _ => wanted.push((file, path)),
        }
    }
    let mut downloads = futures::stream::iter(wanted)
        .map(|(file, path)| async move {
            let contents = remote.download_media(kind, &file.name).await;
            (file, path, contents)
        })
        .buffer_unordered(DOWNLOAD_CONCURRENCY);
    while let Some((file, path, contents)) = downloads.next().await {
        let contents = contents.map_err(|e| core_error(&e))?;
        sqlx::query(
            "INSERT INTO media_file (path, kind, contents, mtime, uploaded) VALUES (?, ?, ?, ?, 1)
             ON CONFLICT (path) DO UPDATE SET contents = excluded.contents,
                 mtime = excluded.mtime, uploaded = 1",
        )
        .bind(path.to_string_lossy())
        .bind(kind.as_str())
        .bind(&contents)
        .bind(file.mtime)
        .execute(device)
        .await
        .map_err(sqlx_error)?;
        set_size(device, &path.to_string_lossy(), &contents).await?;
        if in_memory(&path) {
            platform::load_files([(path, contents)]);
        } else {
            if let Some(size) = platform::image_dimensions(&contents) {
                platform::load_sizes([(path.clone(), size)]);
            }
            platform::load_stored([path]);
        }
    }
    Ok(())
}
