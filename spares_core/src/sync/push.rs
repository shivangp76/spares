//! What `spares sync cloud` asks the upstream before it copies files there over SSH, which the
//! sync itself doesn't carry: render assets, templates and the files notes read.
//!
//! The files go over SSH to `sync.remote_host`, while the sync goes over HTTP to `sync.url`, so
//! nothing but this says they reach the same collection. `GET /api/sync/push-target`
//! ([`push_target`]) answers where that collection's directories are, and writes a random value
//! to [`PushTarget::check_file`], which the CLI reads back over SSH before copying anything: if
//! the value matches, the directories it copies into are the ones the server serves.

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::config::get_cache_dir;
use crate::config::get_config_dir;
use crate::config::get_data_dir;
use crate::config::get_external_config_file;
use crate::config::get_files_dir;
use crate::config::get_media_dir;
use crate::config::get_replica;
use crate::config::merge_pushed_config;

/// `GET /api/sync/push-target`: where the collection's directories are on the server.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PushTarget {
    /// The collection's directory. Notes name images and the files they read by absolute path,
    /// so it must be the same path as on the machine that pushes.
    pub root: PathBuf,
    /// `config.toml` and the parser templates
    pub config: PathBuf,
    /// Images, audio and image occlusions, which the sync exchanges itself
    pub media: PathBuf,
    /// Rendered files, served at `/files`
    pub files: PathBuf,
    /// The content-addressed store of render assets (see `api::browser_render`)
    pub render_assets: PathBuf,
    /// The server's replica of the collection
    pub replica: u8,
    /// Parsers rendered on the server (not in the browser) whose programs the server has, so
    /// it renders their notes itself after a sync. The rendered files of the others are copied.
    pub renders: Vec<String>,
    /// A file holding [`Self::check`], written for this request
    pub check_file: PathBuf,
    pub check: String,
}

/// The file [`push_target`] writes its check value to, in the cache's `sync` directory.
const CHECK_FILE_NAME: &str = "push-check";

/// Answers `GET /api/sync/push-target` for the current collection, writing a new check value.
pub fn push_target() -> Result<PushTarget, Error> {
    let check = random_check();
    let dir = get_cache_dir().join("sync");
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io {
        description: format!("Failed to create {}", dir.display()),
        source: e,
    })?;
    let check_file = dir.join(CHECK_FILE_NAME);
    std::fs::write(&check_file, &check).map_err(|e| Error::Io {
        description: format!("Failed to write {}", check_file.display()),
        source: e,
    })?;
    Ok(PushTarget {
        root: get_data_dir(),
        config: get_config_dir(),
        media: get_media_dir(),
        files: get_files_dir(),
        render_assets: crate::api::browser_render::get_render_assets_dir(),
        replica: get_replica()?.number(),
        renders: rendering_parsers(),
        check_file,
        check,
    })
}

/// `PUT /api/sync/push-config`: replaces the current collection's `config.toml` with what
/// `spares sync cloud` sent (`pushed`, that machine's), keeping this one's `[sync]` table (see
/// [`merge_pushed_config`]). Returns whether the file changed.
pub fn apply_pushed_config(pushed: &str) -> Result<bool, Error> {
    let path = get_external_config_file();
    let current = match std::fs::read_to_string(&path) {
        Ok(current) => current,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(Error::Io {
                description: format!("Failed to read {}", path.display()),
                source: e,
            });
        }
    };
    let merged = merge_pushed_config(&current, pushed)?;
    if merged == current {
        return Ok(false);
    }
    let io = |e| Error::Io {
        description: format!("Failed to write {}", path.display()),
        source: e,
    };
    std::fs::create_dir_all(get_config_dir()).map_err(io)?;
    // Written whole, then renamed, so a render never reads half a file.
    let staging = path.with_extension("toml.pushed");
    std::fs::write(&staging, merged)
        .and_then(|()| std::fs::rename(&staging, &path))
        .map_err(io)?;
    Ok(true)
}

/// Parsers rendered on this machine (not in the browser) whose programs it has.
fn rendering_parsers() -> Vec<String> {
    crate::parsers::get_all_parsers()
        .into_iter()
        .map(|parser| parser())
        .filter(|parser| !parser.renders_in_browser())
        .filter(|parser| match parser.get_parser_name() {
            // pandoc's PDF goes through LaTeX, as `setup_vps.sh --no-latex` leaves both out
            "markdown" => {
                crate::parsers::impls::markdown::pandoc_version().is_some()
                    && has_program("latexmk")
            }
            "latex-note" => has_program("latexmk"),
            _ => true,
        })
        .map(|parser| parser.get_parser_name().to_string())
        .collect()
}

fn has_program(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

fn random_check() -> String {
    use rand::RngExt;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..32)
        .map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_check_value_each_time() {
        let (a, b) = (random_check(), random_check());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
