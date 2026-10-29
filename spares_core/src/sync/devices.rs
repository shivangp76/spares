//! Browsers that keep a copy of the collection: replicas `2` to `4`, enrolled with the server.
//!
//! A browser has no environment to read `SPARES_REPLICA` from, so it asks the server for a number
//! (`POST /api/sync/enroll`). The server hands out the smallest free one and records it in
//! `sync_device`; the device keeps it in its own storage, next to its copy of the database, never
//! in a table, since a full download replaces those (decision 6 in [`super`]). Then the device does
//! a full download, so that its first note ids follow the newest note it has seen (see
//! [`crate::ids`]). Each of them has its own letters for those, which is what limits them to
//! three.
//!
//! **A replica number is never shared.** A full upload keeps `sync_device`, so a number stays
//! taken until its device is unenrolled. The server refuses to
//! sync with a replica from `2` up that isn't enrolled ([`Error::NotEnrolled`]), so a device
//! that was unenrolled can't come back under a number that has since gone to another one. That
//! also holds a second laptop with `SPARES_REPLICA=2`: it has to be enrolled too (`spares sync
//! devices add`).
//!
//! Reusing an unenrolled number is safe: every push sends the replica's `id_counter` rows, which
//! the server keeps by taking the larger value, and a full download merges them the same way. So
//! the new device starts above every id the old one handed out before its last sync, deleted ones
//! too, and the ones it handed out after reached no one.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use crate::Error;
use crate::api::audio::get_audio_directory;
use crate::api::image::get_image_directory;
use crate::config::get_config_dir;
use crate::config::get_external_config_file;
use crate::config::read_external_config;
use crate::config::server_config;
use crate::parsers::get_all_parsers;
use crate::parsers::image_occlusion::get_image_occlusion_directory;
use crate::parsers::impls::markdown::pandoc_version;
use crate::parsers::impls::markdown::web_defaults_filepath;

/// The replica numbers devices are enrolled under. `0` is the laptop and `1` the server.
pub const DEVICE_REPLICAS: std::ops::RangeInclusive<u8> = 2..=4;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

/// `POST /api/sync/enroll`
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnrollRequest {
    /// What the device is called in the list of devices, such as "iPhone (Safari)".
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnrollResponse {
    pub replica: u8,
}

/// `POST /api/sync/unenroll`
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UnenrollRequest {
    pub replica: u8,
}

/// `GET /api/sync/devices`: an enrolled device.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, sqlx::FromRow)]
pub struct Device {
    pub replica: u8,
    pub name: String,
    /// Unix time
    pub enrolled_at: i64,
    /// Unix time of its last push, if it has pushed. A full upload keeps it.
    pub last_sync_at: Option<i64>,
}

/// Gives a device the smallest free replica number from `2` to `4`. A number is taken while a
/// device is enrolled under it, or while `sync_client` records a replica that synced under it
/// without enrolling (from before enrollment existed).
pub async fn enroll(db: &SqlitePool, request: EnrollRequest) -> Result<EnrollResponse, Error> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(Error::Sync("a device needs a name".to_string()));
    }
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    let taken: Vec<i64> =
        sqlx::query_scalar("SELECT replica FROM sync_device UNION SELECT replica FROM sync_client")
            .fetch_all(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    let replica = DEVICE_REPLICAS
        .clone()
        .find(|r| !taken.contains(&i64::from(*r)))
        .ok_or_else(|| {
            Error::Sync(format!(
                "every device number ({}–{}) is taken. Remove one that no longer keeps a copy \
                 first, in Settings → This Device or with `spares sync devices remove`.",
                DEVICE_REPLICAS.start(),
                DEVICE_REPLICAS.end()
            ))
        })?;
    sqlx::query("INSERT INTO sync_device (replica, name, enrolled_at) VALUES (?, ?, ?)")
        .bind(i64::from(replica))
        .bind(name)
        .bind(Utc::now().timestamp())
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(EnrollResponse { replica })
}

/// Frees a device's number. Returns whether it was enrolled.
pub async fn unenroll(db: &SqlitePool, request: UnenrollRequest) -> Result<bool, Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    let removed = sqlx::query("DELETE FROM sync_device WHERE replica = ?")
        .bind(i64::from(request.replica))
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?
        .rows_affected()
        > 0;
    sqlx::query("DELETE FROM sync_client WHERE replica = ?")
        .bind(i64::from(request.replica))
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    tx.commit().await.map_err(sqlx_error)?;
    Ok(removed)
}

/// The enrolled devices, by replica number.
pub async fn list(db: &SqlitePool) -> Result<Vec<Device>, Error> {
    sqlx::query_as(
        "SELECT d.replica, d.name, d.enrolled_at, c.last_sync_at
         FROM sync_device d LEFT JOIN sync_client c ON c.replica = d.replica
         ORDER BY d.replica",
    )
    .fetch_all(db)
    .await
    .map_err(sqlx_error)
}

/// Refuses a replica from `2` up that isn't enrolled. Only `2` to `4` can be.
pub(crate) async fn check_enrolled(conn: &mut SqliteConnection, replica: u8) -> Result<(), Error> {
    if replica < *DEVICE_REPLICAS.start() {
        return Ok(());
    }
    let enrolled: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sync_device WHERE replica = ?)")
            .bind(i64::from(replica))
            .fetch_one(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    if enrolled {
        Ok(())
    } else {
        Err(Error::NotEnrolled(replica))
    }
}

/// `GET /api/sync/config`: the files a device needs from the server's config directory, which it
/// loads into [`crate::platform`] and saves at each sync. Settings are changed on the laptop, so
/// they are read-only on a device.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceConfig {
    /// By path relative to the config directory: `config.toml` (made by [`server_config`], as for
    /// the server itself) and every file in each parser's `templates` directory, such as
    /// `typst/templates/card_template.typ`. Typst cards compile in the browser from their template,
    /// which can import the files next to it. Also `markdown/web-defaults.yaml`, when there is one,
    /// with which a device converts the Markdown cards it shows before the server renders them.
    pub files: BTreeMap<String, String>,
    /// Where the server stores images, audio and image occlusions. Notes name these files by their
    /// path, so a device names the ones it adds by the path they get on the server.
    pub image_dir: PathBuf,
    pub audio_dir: PathBuf,
    pub image_occlusion_dir: PathBuf,
    /// The version of the server's pandoc, which a device compares with its own WebAssembly build.
    #[serde(default)]
    pub pandoc_version: Option<String>,
}

/// The server's [`DeviceConfig`]. Files that aren't UTF-8 are left out.
pub fn device_config() -> Result<DeviceConfig, Error> {
    // Writes the default config.toml if there is none yet.
    read_external_config()?;
    let path = get_external_config_file();
    let contents = std::fs::read_to_string(&path).map_err(|e| Error::Io {
        description: format!("Failed to read {}", path.display()),
        source: e,
    })?;
    let config_dir = get_config_dir();
    let mut files = BTreeMap::from([("config.toml".to_string(), server_config(&contents)?)]);
    for parser in get_all_parsers() {
        let templates = config_dir
            .join(parser().get_parser_name())
            .join("templates");
        add_text_files(&config_dir, &templates, &mut files)?;
    }
    let web_defaults = web_defaults_filepath();
    if let Ok(contents) = std::fs::read_to_string(&web_defaults) {
        let relative = web_defaults
            .strip_prefix(&config_dir)
            .unwrap_or(&web_defaults);
        files.insert(relative.to_string_lossy().into_owned(), contents);
    }
    Ok(DeviceConfig {
        files,
        image_dir: get_image_directory(),
        audio_dir: get_audio_directory(),
        image_occlusion_dir: get_image_occlusion_directory(),
        pandoc_version: pandoc_version().map(str::to_string),
    })
}

/// Adds the UTF-8 files under `dir` to `files`, by their path relative to `base`.
fn add_text_files(
    base: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            add_text_files(base, &path, files)?;
        } else if let Ok(contents) = std::fs::read_to_string(&path) {
            let relative = path.strip_prefix(base).unwrap_or(&path);
            files.insert(relative.to_string_lossy().into_owned(), contents);
        } else {
            log::warn!(
                "Not sending {} to devices: not a UTF-8 text file",
                path.display()
            );
        }
    }
    Ok(())
}
