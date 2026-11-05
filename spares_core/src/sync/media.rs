//! Files a device needs besides the database: rendered cards to show offline, and the images and
//! audio it adds to notes offline.
//!
//! - **Rendered files** stay on the server, which renders every note. A device asks which changed
//!   with `POST /api/sync/files/changed` ([`changed_rendered_files`]) and downloads them into the
//!   browser's Cache Storage. The answer has the files of the [buckets](FILE_BUCKETS) whose
//!   digest differs from the one the device last saw, so a refresh after a sync that changed one
//!   note sends a few files rather than every one, and still shows the device the files that
//!   are gone. A digest is of each file's path, size and mtime, not of an mtime since the last
//!   time, because `spares sync cloud` copies files with the mtimes they had on the laptop.
//!   `GET /api/sync/files` ([`rendered_files`]) lists every file.
//!   Typst cards compile in the browser instead, from render assets the device fetches by the
//!   hashes in its own `render_asset` table, which follows the server's at each sync.
//! - **Images and audio** added on a device are named as the server names them,
//!   `<stem>-<hash>.<ext>` in its image or audio directory (see
//!   [`super::devices::DeviceConfig`]), and uploaded with `PUT /api/sync/media/{kind}/{name}`
//!   ([`store_media`]) before the push whose notes name them. The name ends in the start of the
//!   file's blake3 hash, so an upload never replaces a file with other contents, and sending one
//!   again changes nothing. Until then, the web app shows them from the device.
//! - **Image occlusions** are parsed from their files, so a device downloads the server's
//!   ([`list_media`], [`read_media`]) after each sync, and a clozes SVG again when the server
//!   rewrote it. One made on a device is named as on the server too, and uploaded like an image
//!   once a note names it: saving the note parses it, which writes card orders into the SVG, and
//!   a device never replaces a file. Where a device keeps them: `spares_wasm`'s `media`.
//!   A host, such as the laptop, compares its image occlusions with its upstream's by their
//!   [hashes](MediaFile::hash) at each sync, since their files change under one name: parsing a
//!   note rewrites a clozes SVG, and an image can be replaced with another. Of two that differ, the
//!   one changed last replaces the other ([`replace_media`]), and the notes naming it are rendered
//!   again (see [`super::files::sync_now`]).

use std::collections::BTreeMap;
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
use crate::api::image_occlusion::check_clozes_svg;
use crate::api::image_occlusion::image_extension;
use crate::config::get_files_dir;
use crate::parsers::audio::is_audio_path;
use crate::parsers::image_occlusion::get_image_occlusion_directory;

/// Directories in the files directory that hold the server's own state rather than rendered
/// files: sync's snapshots.
const INTERNAL_DIRS: [&str; 1] = ["sync"];

/// The kinds of rendered file web clients fetch from `/files`: rendered notes and cards, their
/// HTML for web clients, and rendered image occlusions. Others are by-products, such as LaTeX's.
const RENDERED_EXTENSIONS: [&str; 8] = ["pdf", "html", "svg", "png", "jpg", "jpeg", "gif", "webp"];

/// How many buckets [`changed_rendered_files`] sorts rendered files into, by the first byte of
/// their path's blake3 hash. A digest per bucket is 16 hex digits, so the digests of every bucket
/// are about 6 KB, while each holds a 256th of the files.
pub const FILE_BUCKETS: usize = 256;

/// `GET /api/sync/files`: a file in the files directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RenderedFile {
    /// Relative to the files directory, as `/files/<path>` serves it
    pub path: String,
    pub size: u64,
    /// When it last changed, as Unix time
    pub mtime: i64,
    /// Its bucket (see [`FILE_BUCKETS`]), as two hex digits
    pub bucket: String,
}

/// `POST /api/sync/files/changed`: the digests of the buckets the device has seen.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ChangedFilesRequest {
    /// By bucket. Empty the first time, which lists every file.
    #[serde(default)]
    pub buckets: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangedFilesResponse {
    /// The digest of every bucket that has files. A bucket the device has seen and that is missing
    /// here has none left.
    pub buckets: BTreeMap<String, String>,
    /// Every file of each bucket whose digest isn't the device's, by path
    pub files: Vec<RenderedFile>,
}

/// The rendered files in the files directory, by path.
pub fn rendered_files() -> Result<Vec<RenderedFile>, Error> {
    let root = get_files_dir();
    let entries = std::fs::read_dir(&root).map_err(|e| Error::Io {
        description: format!("Failed to list {}", root.display()),
        source: e,
    })?;
    let mut files = Vec::new();
    add_rendered_files(&root, entries, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// The digest of every bucket, and the files of those whose digest isn't in `request`.
pub fn changed_rendered_files(
    request: &ChangedFilesRequest,
) -> Result<ChangedFilesResponse, Error> {
    let mut by_bucket: BTreeMap<String, Vec<RenderedFile>> = BTreeMap::new();
    for file in rendered_files()? {
        by_bucket.entry(file.bucket.clone()).or_default().push(file);
    }
    let mut response = ChangedFilesResponse::default();
    for (bucket, files) in by_bucket {
        let digest = bucket_digest(&files);
        if request.buckets.get(&bucket) != Some(&digest) {
            response.files.extend(files);
        }
        response.buckets.insert(bucket, digest);
    }
    Ok(response)
}

/// The bucket of a file at `path` (see [`FILE_BUCKETS`]).
fn file_bucket(path: &str) -> String {
    format!("{:02x}", blake3::hash(path.as_bytes()).as_bytes()[0])
}

/// Of the files of a bucket, sorted by path.
fn bucket_digest(files: &[RenderedFile]) -> String {
    let mut hasher = blake3::Hasher::new();
    for file in files {
        hasher.update(file.path.as_bytes());
        hasher.update(&[0]);
        hasher.update(&file.size.to_le_bytes());
        hasher.update(&file.mtime.to_le_bytes());
    }
    hasher.finalize().to_hex()[..16].to_string()
}

/// Adds the rendered files among `entries` and below them. A file or directory that goes away
/// meanwhile, as rendering does to some, is left out.
fn add_rendered_files(root: &Path, entries: std::fs::ReadDir, files: &mut Vec<RenderedFile>) {
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
                add_rendered_files(root, entries, files);
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
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        files.push(RenderedFile {
            bucket: file_bucket(&relative),
            path: relative,
            size: metadata.len(),
            mtime: mtime(&metadata),
        });
    }
}

fn mtime(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or_default()
}

/// The files of `/api/sync/media/{kind}`: those a device adds, and image occlusions, which it
/// also downloads.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaKind {
    Image,
    Audio,
    /// Both files of each: the image, and its clozes SVG, `<image stem>_clozes.svg`
    ImageOcclusion,
}

impl MediaKind {
    /// Where the server keeps them, and a device names them by
    pub fn directory(self) -> PathBuf {
        match self {
            Self::Image => get_image_directory(),
            Self::Audio => get_audio_directory(),
            Self::ImageOcclusion => get_image_occlusion_directory(),
        }
    }

    /// As in the URL
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::ImageOcclusion => "image-occlusion",
        }
    }

    const fn max_size(self) -> usize {
        match self {
            Self::Image | Self::ImageOcclusion => MAX_IMAGE_SIZE,
            Self::Audio => MAX_AUDIO_SIZE,
        }
    }
}

fn invalid(message: String) -> Error {
    Error::Sync(message)
}

/// Whether `name` is a file's name, with no directory.
fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Whether `stem` ends in `-` and a suffix of [`HASH_LENGTH`] hex digits, as stored names do.
fn has_suffix(stem: &str) -> bool {
    stem.rsplit_once('-').is_some_and(|(_, suffix)| {
        suffix.len() == HASH_LENGTH && suffix.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Checks that `contents` may be stored as `name`.
///
/// Images and clips are named by their contents: `<stem>-<hash>.<ext>`, with the start of their
/// blake3 hash. Image occlusions are named `<stem>-<suffix>.<ext>` and
/// `<stem>-<suffix>_clozes.svg` by the device that made them, with a suffix that is unique rather
/// than a hash, since card orders are written into the SVG while a note is parsed.
fn check_media(kind: MediaKind, name: &str, contents: &[u8]) -> Result<(), Error> {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match kind {
        MediaKind::Image => {
            image_extension(name)?;
        }
        MediaKind::Audio if !is_audio_path(name) => {
            return Err(invalid(format!("`{name}` is not an audio file")));
        }
        MediaKind::Audio => {}
        MediaKind::ImageOcclusion => {
            let stem = match stem.strip_suffix("_clozes") {
                // As `read_image_occlusion_data` names it, in lowercase
                Some(image_stem) if Path::new(name).extension() == Some("svg".as_ref()) => {
                    let svg = std::str::from_utf8(contents)
                        .map_err(|_| invalid(format!("`{name}` is not UTF-8")))?;
                    check_clozes_svg(svg)?;
                    image_stem
                }
                _ => {
                    image_extension(name)?;
                    stem
                }
            };
            if !has_suffix(stem) {
                return Err(invalid(format!(
                    "`{name}` isn't named as image occlusions are stored"
                )));
            }
            return Ok(());
        }
    }
    let hash = blake3::hash(contents).to_hex();
    let expected = &hash[..HASH_LENGTH];
    if !stem.ends_with(&format!("-{expected}")) {
        return Err(invalid(format!(
            "`{name}` doesn't end in its contents' hash, `{expected}`"
        )));
    }
    Ok(())
}

/// Stores a file a device or another host added, under `name` in the server's directory for
/// `kind`, and returns its path there. `name` is as the device named it (see `check_media`). A
/// file that is already there is kept: the server never replaces one this way (see
/// [`replace_media`]).
pub fn store_media(kind: MediaKind, name: &str, contents: &[u8]) -> Result<PathBuf, Error> {
    store(kind, name, contents, None).map(|(path, _)| path)
}

/// What [`replace_media`] did
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Replaced {
    /// The file wasn't there, so it was stored
    Stored,
    /// The file had the contents it replaces, and now has the new ones
    Replaced,
    /// The file already had the new contents
    Unchanged,
}

/// Replaces the image occlusion file `name` with `contents`, if it still has the contents whose
/// [`hash`](MediaFile::hash) is `replaces`, as when it was listed. Its files change under one name:
/// parsing a note writes card orders into a clozes SVG, and an image can be replaced. Fails, and
/// keeps the file, if it changed since, so the newer file is never replaced by an older one.
/// Images and clips are named by their contents, so they are never replaced. Checked as by
/// [`store_media`].
pub fn replace_media(
    kind: MediaKind,
    name: &str,
    contents: &[u8],
    replaces: &str,
) -> Result<(PathBuf, Replaced), Error> {
    if kind != MediaKind::ImageOcclusion {
        return Err(invalid(format!(
            "`{name}` is named by its contents, so it can't be replaced"
        )));
    }
    store(kind, name, contents, Some(replaces))
}

/// The hash of a media file's contents, as [`MediaFile::hash`] has it
fn contents_hash(contents: &[u8]) -> String {
    blake3::hash(contents).to_hex().to_string()
}

fn store(
    kind: MediaKind,
    name: &str,
    contents: &[u8],
    replaces: Option<&str>,
) -> Result<(PathBuf, Replaced), Error> {
    if contents.len() > kind.max_size() {
        return Err(invalid(format!(
            "{name} is larger than {} MB",
            kind.max_size() / 1024 / 1024
        )));
    }
    if !is_plain(name) {
        return Err(invalid(format!("`{name}` is not a file name")));
    }
    check_media(kind, name, contents)?;
    let directory = kind.directory();
    std::fs::create_dir_all(&directory).map_err(|e| Error::Io {
        description: format!("Failed to create {}", directory.display()),
        source: e,
    })?;
    let path = directory.join(name);
    let outcome = match (replaces, std::fs::read(&path)) {
        (_, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Replaced::Stored,
        (_, Err(e)) => {
            return Err(Error::Io {
                description: format!("Failed to read {}", path.display()),
                source: e,
            });
        }
        (None, Ok(_)) => return Ok((path, Replaced::Unchanged)),
        (Some(_), Ok(current)) if current == contents => return Ok((path, Replaced::Unchanged)),
        (Some(replaces), Ok(current)) if contents_hash(&current) == replaces => Replaced::Replaced,
        (Some(_), Ok(_)) => {
            return Err(invalid(format!(
                "`{name}` changed since it was listed, so it was kept"
            )));
        }
    };
    // Write then rename, so a partly written file is never used
    let temp_path = directory.join(format!(".{name}.tmp"));
    std::fs::write(&temp_path, contents)
        .and_then(|()| std::fs::rename(&temp_path, &path))
        .map_err(|e| Error::Io {
            description: format!("Failed to write {}", path.display()),
            source: e,
        })?;
    Ok((path, outcome))
}

/// `GET /api/sync/media/{kind}`: a file in the server's directory for a kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MediaFile {
    pub name: String,
    pub size: u64,
    /// When it last changed, as Unix time. The server rewrites an image occlusion's clozes SVG
    /// when a note's settings change, so a device downloads it again then.
    pub mtime: i64,
    /// The blake3 hash of its contents, as hex, for image occlusions only. Their files change under
    /// one name (see [`replace_media`]), so a sync compares them by it. Images and clips are named
    /// by their hash already, so their names are compared, without reading every one at each
    /// sync. `None` from a server from before it was listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// The files in the server's directory for `kind`, by name.
pub fn list_media(kind: MediaKind) -> Result<Vec<MediaFile>, Error> {
    let directory = kind.directory();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Error::Io {
                description: format!("Failed to list {}", directory.display()),
                source: e,
            });
        }
    };
    let mut files: Vec<MediaFile> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let metadata = entry.metadata().ok()?;
            if !is_plain(&name) || !metadata.is_file() {
                return None;
            }
            let hash = if kind == MediaKind::ImageOcclusion {
                // A file that can't be read is left out, as one being written
                Some(contents_hash(&std::fs::read(entry.path()).ok()?))
            } else {
                None
            };
            Some(MediaFile {
                name,
                size: metadata.len(),
                mtime: mtime(&metadata),
                hash,
            })
        })
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// `GET /api/sync/media/{kind}/{name}`: the file `name` in the server's directory for `kind`, if
/// it is there.
pub fn read_media(kind: MediaKind, name: &str) -> Result<Option<Vec<u8>>, Error> {
    if !is_plain(name) {
        return Ok(None);
    }
    let path = kind.directory().join(name);
    match std::fs::read(&path) {
        Ok(contents) => Ok(Some(contents)),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => Ok(None),
        Err(e) => Err(Error::Io {
            description: format!("Failed to read {}", path.display()),
            source: e,
        }),
    }
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
    fn stores_image_occlusions_once() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><g class="layer" id="clozes-group"><rect id="a" x="1" y="1" width="2" height="2"/></g></svg>"#;
        let image = "spares-media-test-0123456789ab.png";
        let clozes = "spares-media-test-0123456789ab_clozes.svg";
        let image_path = store_media(MediaKind::ImageOcclusion, image, b"png").unwrap();
        assert_eq!(image_path, get_image_occlusion_directory().join(image));
        let clozes_path = store_media(MediaKind::ImageOcclusion, clozes, svg.as_bytes()).unwrap();
        // Never replaced, unlike the server's own writes
        let other = svg.replace("width=\"2\"", "width=\"3\"");
        store_media(MediaKind::ImageOcclusion, clozes, other.as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(&clozes_path).unwrap(), svg);

        let listed = list_media(MediaKind::ImageOcclusion).unwrap();
        let names: Vec<&str> = listed
            .iter()
            .map(|f| f.name.as_str())
            .filter(|n| n.starts_with("spares-media-test-"))
            .collect();
        assert_eq!(names, [image, clozes]);
        assert_eq!(
            read_media(MediaKind::ImageOcclusion, clozes).unwrap(),
            Some(svg.as_bytes().to_vec())
        );
        assert_eq!(
            read_media(MediaKind::ImageOcclusion, "../x.png").unwrap(),
            None
        );
        assert_eq!(
            read_media(MediaKind::ImageOcclusion, "spares-media-test-none.png").unwrap(),
            None
        );

        // Not named as stored, not an image, clozes the parser can't read
        for (name, contents) in [
            ("spares-media-test.png", b"png".as_slice()),
            ("spares-media-test-0123456789ab.txt", b"x"),
            ("spares-media-test-0123456789ab_clozes.svg", b"<svg/>"),
            ("spares-media-test_clozes.svg", svg.as_bytes()),
        ] {
            assert!(
                store_media(MediaKind::ImageOcclusion, name, contents).is_err(),
                "{name}"
            );
        }
        std::fs::remove_file(&image_path).unwrap();
        std::fs::remove_file(&clozes_path).unwrap();
    }

    #[test]
    fn replaces_image_occlusion_files_it_still_has() {
        let svg = |width: u32| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg"><g class="layer" id="clozes-group"><rect id="a" x="1" y="1" width="{width}" height="2"/></g></svg>"#
            )
        };
        let kind = MediaKind::ImageOcclusion;
        let (image, clozes) = (
            "spares-replace-test-0123456789ab.png",
            "spares-replace-test-0123456789ab_clozes.svg",
        );
        // Left by a failed run
        for name in [image, clozes] {
            let _ = std::fs::remove_file(kind.directory().join(name));
        }
        let hash = |name: &str| {
            list_media(kind)
                .unwrap()
                .into_iter()
                .find(|f| f.name == name)
                .and_then(|f| f.hash)
        };

        // Stored if it isn't there, whatever it replaces
        let (path, replaced) = replace_media(kind, image, b"first", "none").unwrap();
        assert_eq!(replaced, Replaced::Stored);
        let first = hash(image).unwrap();
        assert_eq!(first, contents_hash(b"first"));
        // Replaced only if it still has what it replaces
        assert_eq!(
            replace_media(kind, image, b"second", &first).unwrap().1,
            Replaced::Replaced
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(
            replace_media(kind, image, b"third", &first).is_err(),
            "a stale listing"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            replace_media(kind, image, b"second", &first).unwrap().1,
            Replaced::Unchanged
        );
        // Named as when stored, as image occlusions from another host are too
        assert!(replace_media(kind, "spares-replace-test.png", b"first", "none").is_err());
        // Clozes are checked as when stored
        let clozes_path = store_media(kind, clozes, svg(2).as_bytes()).unwrap();
        let old = hash(clozes).unwrap();
        assert!(replace_media(kind, clozes, b"<svg/>", &old).is_err());
        replace_media(kind, clozes, svg(3).as_bytes(), &old).unwrap();
        assert_eq!(std::fs::read_to_string(&clozes_path).unwrap(), svg(3));
        // Without a replacement in mind, a file is still never replaced
        store_media(kind, clozes, svg(4).as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(&clozes_path).unwrap(), svg(3));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&clozes_path).unwrap();

        // Images and clips are named by their contents, so never replaced, nor hashed when listed
        let contents = b"spares replace test image";
        let name = named("diagram", contents, "png");
        let path = store_media(MediaKind::Image, &name, contents).unwrap();
        let listed = list_media(MediaKind::Image).unwrap();
        assert_eq!(listed.iter().find(|f| f.name == name).unwrap().hash, None);
        let hash = contents_hash(contents);
        assert!(replace_media(MediaKind::Image, &name, b"other", &hash).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), contents);
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
        let files = rendered_files();
        let first = changed_rendered_files(&ChangedFilesRequest::default()).unwrap();
        let unchanged = changed_rendered_files(&ChangedFilesRequest {
            buckets: first.buckets.clone(),
        })
        .unwrap();
        // Rewritten with another size, and one gone: their buckets are listed again, whole
        std::fs::write(dir.join("card-1-front.pdf"), "xy").unwrap();
        std::fs::remove_file(dir.join("sub/occlusion.svg")).unwrap();
        let changed = changed_rendered_files(&ChangedFilesRequest {
            buckets: first.buckets.clone(),
        })
        .unwrap();
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
        assert!(
            first
                .files
                .iter()
                .any(|f| f.path == "spares-media-test/card-1-front.pdf")
        );
        assert!(
            !unchanged
                .files
                .iter()
                .any(|f| f.path.starts_with("spares-media-test")),
            "unchanged buckets aren't listed"
        );
        let pdf = changed
            .files
            .iter()
            .find(|f| f.path == "spares-media-test/card-1-front.pdf")
            .unwrap();
        assert_eq!(pdf.size, 2);
        assert!(
            changed
                .files
                .iter()
                .all(|f| f.path != "spares-media-test/sub/occlusion.svg")
        );
        let gone = file_bucket("spares-media-test/sub/occlusion.svg");
        assert_ne!(
            changed.buckets.get(&gone),
            first.buckets.get(&gone),
            "the bucket of a file that is gone changes, or is gone"
        );
    }
}
