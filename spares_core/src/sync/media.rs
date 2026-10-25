//! Files a device needs besides the database: rendered cards to show offline, and the images and
//! audio it adds to notes offline.
//!
//! - **Rendered files** stay on the server, which renders every note. A device lists them with
//!   `GET /api/sync/files` ([`rendered_files`]) and downloads the ones that changed into the
//!   browser's Cache Storage, where the web app already keeps the files of its offline queue.
//!   Typst cards compile in the browser instead, from render assets the device fetches by the
//!   hashes in its own `render_asset` table, which follows the server's at each sync.
//! - **Images and audio** added on a device are named as the server names them, `<stem>-<hash>.<ext>`
//!   in its image or audio directory (see [`super::devices::DeviceConfig`]). The device uploads each
//!   with `PUT /api/sync/media/{kind}/{name}` ([`store_media`]) before the push whose notes name it.
//!   The name ends in the start of the file's blake3 hash, so an upload never replaces a file with
//!   other contents, and sending one again changes nothing.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::api::audio::MAX_AUDIO_SIZE;
use crate::api::audio::get_audio_directory;
use crate::api::image::MAX_IMAGE_SIZE;
use crate::api::image::get_image_directory;
use crate::api::image_occlusion::HASH_LENGTH;
use crate::api::image_occlusion::image_extension;
use crate::config::get_files_dir;
use crate::parsers::audio::is_audio_path;

/// Directories in the files directory that hold the server's own state rather than rendered
/// files: sync's snapshots and the hashes that tell a note needs rendering again.
const INTERNAL_DIRS: [&str; 2] = ["sync", "note-hashes"];

/// The kinds of rendered file web clients fetch from `/files`: rendered notes and cards, their
/// HTML for web clients, and rendered image occlusions. Others are by-products, such as LaTeX's.
const RENDERED_EXTENSIONS: [&str; 8] = ["pdf", "html", "svg", "png", "jpg", "jpeg", "gif", "webp"];

/// `GET /api/sync/files`: a file in the files directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RenderedFile {
    /// Relative to the files directory, as `/files/<path>` serves it
    pub path: String,
    pub size: u64,
    /// When it last changed, as Unix time
    pub mtime: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RenderedFilesQuery {
    /// Only files changed after this Unix time. Without it, every file, so a device can also drop
    /// the ones that are gone.
    pub since: Option<i64>,
}

/// The rendered files in the files directory, by path.
pub fn rendered_files(query: &RenderedFilesQuery) -> Result<Vec<RenderedFile>, Error> {
    let root = get_files_dir();
    let entries = std::fs::read_dir(&root).map_err(|e| Error::Io {
        description: format!("Failed to list {}", root.display()),
        source: e,
    })?;
    let mut files = Vec::new();
    add_rendered_files(&root, entries, query.since, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Adds the rendered files among `entries` and below them. A file or directory that goes away
/// meanwhile, as rendering does to some, is left out.
fn add_rendered_files(
    root: &Path,
    entries: std::fs::ReadDir,
    since: Option<i64>,
    files: &mut Vec<RenderedFile>,
) {
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            let internal = path.parent() == Some(root) && INTERNAL_DIRS.contains(&name);
            if let (false, Ok(entries)) = (internal, std::fs::read_dir(&path)) {
                add_rendered_files(root, entries, since, files);
            }
            continue;
        }
        let rendered = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| RENDERED_EXTENSIONS.contains(&e.to_lowercase().as_str()));
        if !rendered {
            continue;
        }
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or_default();
        if since.is_some_and(|since| mtime <= since) {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        files.push(RenderedFile {
            path: relative.to_string_lossy().into_owned(),
            size: metadata.len(),
            mtime,
        });
    }
}

/// What a device uploads with `PUT /api/sync/media/{kind}/{name}`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Audio,
}

impl MediaKind {
    fn directory(self) -> PathBuf {
        match self {
            Self::Image => get_image_directory(),
            Self::Audio => get_audio_directory(),
        }
    }

    const fn max_size(self) -> usize {
        match self {
            Self::Image => MAX_IMAGE_SIZE,
            Self::Audio => MAX_AUDIO_SIZE,
        }
    }
}

fn invalid(message: String) -> Error {
    Error::Sync(message)
}

/// Stores an image or clip a device added, under `name` in the server's image or audio directory,
/// and returns its path there. `name` must be `<stem>-<hash>.<ext>`, with the start of the blake3
/// hash of `contents`, as the device named it.
pub fn store_media(kind: MediaKind, name: &str, contents: &[u8]) -> Result<PathBuf, Error> {
    if contents.len() > kind.max_size() {
        return Err(invalid(format!(
            "{name} is larger than {} MB",
            kind.max_size() / 1024 / 1024
        )));
    }
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !plain {
        return Err(invalid(format!("`{name}` is not a file name")));
    }
    match kind {
        MediaKind::Image => {
            image_extension(name)?;
        }
        MediaKind::Audio if !is_audio_path(name) => {
            return Err(invalid(format!("`{name}` is not an audio file")));
        }
        MediaKind::Audio => {}
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let hash = blake3::hash(contents).to_hex();
    let expected = &hash[..HASH_LENGTH];
    if !stem.ends_with(&format!("-{expected}")) {
        return Err(invalid(format!(
            "`{name}` doesn't end in its contents' hash, `{expected}`"
        )));
    }
    let directory = kind.directory();
    std::fs::create_dir_all(&directory).map_err(|e| Error::Io {
        description: format!("Failed to create {}", directory.display()),
        source: e,
    })?;
    let path = directory.join(name);
    if !path.exists() {
        // Write then rename, so a partly written file is never used
        let temp_path = directory.join(format!(".{name}.tmp"));
        std::fs::write(&temp_path, contents)
            .and_then(|()| std::fs::rename(&temp_path, &path))
            .map_err(|e| Error::Io {
                description: format!("Failed to write {}", path.display()),
                source: e,
            })?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(stem: &str, contents: &[u8], extension: &str) -> String {
        format!(
            "{stem}-{}.{extension}",
            &blake3::hash(contents).to_hex()[..HASH_LENGTH]
        )
    }

    #[test]
    fn stores_media_only_under_its_hash() {
        let contents = b"spares media test image";
        let name = named("diagram", contents, "png");
        let path = store_media(MediaKind::Image, &name, contents).unwrap();
        assert_eq!(path, get_image_directory().join(&name));
        assert_eq!(std::fs::read(&path).unwrap(), contents);
        // Again: nothing changes.
        assert_eq!(
            store_media(MediaKind::Image, &name, contents).unwrap(),
            path
        );
        // Other contents under that name, a path, the wrong kind
        assert!(store_media(MediaKind::Image, &name, b"other").is_err());
        let traversal = format!("../{name}");
        assert!(store_media(MediaKind::Image, &traversal, contents).is_err());
        assert!(store_media(MediaKind::Audio, &name, contents).is_err());
        assert!(store_media(MediaKind::Image, &named("x", contents, "exe"), contents).is_err());
        std::fs::remove_file(&path).unwrap();

        let clip = b"spares media test clip";
        let name = named("clip", clip, "mp3");
        let path = store_media(MediaKind::Audio, &name, clip).unwrap();
        assert_eq!(path, get_audio_directory().join(&name));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn lists_rendered_files_only() {
        let root = get_files_dir();
        let dir = root.join("spares-media-test");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for file in [
            "card-1-front.pdf",
            "card-1-front.html",
            "card.aux",
            "sub/occlusion.svg",
        ] {
            std::fs::write(dir.join(file), "x").unwrap();
        }
        let files = rendered_files(&RenderedFilesQuery::default());
        let future = rendered_files(&RenderedFilesQuery {
            since: Some(i64::MAX),
        });
        std::fs::remove_dir_all(&dir).unwrap();
        let paths: Vec<String> = files
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .filter(|p| p.starts_with("spares-media-test"))
            .collect();
        assert_eq!(
            paths,
            [
                "spares-media-test/card-1-front.html",
                "spares-media-test/card-1-front.pdf",
                "spares-media-test/sub/occlusion.svg"
            ]
        );
        assert_eq!(future.unwrap(), []);
    }
}
