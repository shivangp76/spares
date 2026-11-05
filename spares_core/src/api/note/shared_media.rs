//! The images, audio and image occlusions of notes exported to share (see
//! [`super::export`]), which go with the notes' files.
//!
//! Notes name these files by absolute path, which means nothing on someone else's machine. An
//! export to share copies each file a note names into its [bundle](SharedMedia), as
//! `media/<kind>/<name>` next to the notes' files, and names it there instead, relative to them:
//!
//! ```md
//! ![](/Users/me/.local/share/spares/users/abc/media/images/cell-0123456789ab.png)
//! ```
//! becomes
//! ```md
//! ![](media/image/cell-0123456789ab.png)
//! ```
//!
//! `spares import` uploads the files of the `media` directory next to the file it imports to the
//! host, as a device uploads the files it adds (see [`crate::sync::media`]), and names them by
//! the host's paths again ([`bundled_media_references`]). A file keeps its name, which is that of
//! a stored file. One elsewhere in the media directory is named as an uploaded file would be,
//! `cell-<hash>.png`.
//!
//! A path is a file's if it is absolute (or starts with `~/`), ends in the extension of an image
//! or audio clip, and a file is there, in the collection's media directory. Paths with spaces
//! aren't found. Any other path is left as it is, and listed in
//! [`ExportNotesResponse::missing_media`](crate::schema::note::ExportNotesResponse::missing_media),
//! since it names a file on this machine, if any. A file outside the media directory is never
//! read, even if it is there: on a host, it could be another account's.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use serde_with::base64::Base64;
use serde_with::serde_as;

use crate::Error;
use crate::api::image_occlusion::HASH_LENGTH;
use crate::api::image_occlusion::sanitized_stem;
use crate::helpers::get_or_compile_regex;
use crate::parsers::audio::is_audio_path;
use crate::parsers::audio::regex_ranges;
use crate::sync::media::MediaKind;

/// An absolute path (or one from `~/`) ending in the extension of an image or audio clip, not
/// inside a longer path or word.
const MEDIA_PATH_REGEX: &str = r"(?<![\w./~-])(?:/|~/)[^\s\x22'()\[\]{}<>\\,;|*?]*\.(?i:png|jpe?g|gif|webp|bmp|svg|mp3|m4a|aac|ogg|oga|opus|wav|flac)(?![\w.-])";

/// The directory, next to the notes' files, that holds their bundled files.
pub const SHARED_MEDIA_DIR: &str = "media";

const KINDS: [MediaKind; 3] = [
    MediaKind::Image,
    MediaKind::Audio,
    MediaKind::ImageOcclusion,
];

/// A file the shared notes name, as it goes in the bundle: at `media/<kind>/<name>`.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SharedMedia {
    pub kind: MediaKind,
    pub name: String,
    #[serde_as(as = "Base64")]
    pub contents: Vec<u8>,
}

impl SharedMedia {
    /// Where it goes, relative to the notes' files
    pub fn relative_path(&self) -> PathBuf {
        Path::new(SHARED_MEDIA_DIR)
            .join(self.kind.as_str())
            .join(&self.name)
    }
}

/// How notes name a bundled file, relative to their file
fn bundled_reference(kind: MediaKind, name: &str) -> String {
    format!("{SHARED_MEDIA_DIR}/{}/{name}", kind.as_str())
}

/// The files that [`Self::bundle`] found in notes, and the paths that had none.
#[derive(Debug, Default)]
pub struct MediaBundle {
    files: BTreeMap<(MediaKind, String), Vec<u8>>,
    /// The reference of each file found, by its path as the note has it
    references: BTreeMap<String, String>,
    missing: BTreeSet<String>,
}

impl MediaBundle {
    /// `note_data` with each file it names put in the bundle, and named by its place there.
    pub fn bundle(&mut self, note_data: &str) -> Result<String, Error> {
        let mut result = String::with_capacity(note_data.len());
        let mut last = 0;
        for range in regex_ranges(MEDIA_PATH_REGEX, note_data) {
            let Some(reference) = self.reference(&note_data[range.clone()])? else {
                continue;
            };
            result.push_str(&note_data[last..range.start]);
            result.push_str(&reference);
            last = range.end;
        }
        result.push_str(&note_data[last..]);
        Ok(result)
    }

    /// How a shared note names the file at `path`, once it is in the bundle. `None` if no file is
    /// there.
    fn reference(&mut self, path: &str) -> Result<Option<String>, Error> {
        if let Some(reference) = self.references.get(path) {
            return Ok(Some(reference.clone()));
        }
        let Some(file) = in_media(Path::new(shellexpand::tilde(path).as_ref())) else {
            self.missing.insert(path.to_string());
            return Ok(None);
        };
        let contents = std::fs::read(&file).map_err(|e| Error::Io {
            description: format!("Failed to read {}", file.display()),
            source: e,
        })?;
        let (kind, name) = stored_name(&file, &contents);
        let reference = bundled_reference(kind, &name);
        self.files.insert((kind, name), contents);
        self.references.insert(path.to_string(), reference.clone());
        Ok(Some(reference))
    }

    /// The files, and the paths with no file there
    pub fn finish(self) -> (Vec<SharedMedia>, Vec<String>) {
        let files = self
            .files
            .into_iter()
            .map(|((kind, name), contents)| SharedMedia {
                kind,
                name,
                contents,
            })
            .collect();
        (files, self.missing.into_iter().collect())
    }
}

/// `path`, with its links followed, if it is a file in the collection's media directory. Only
/// those are bundled: on a host, a note could otherwise name another account's files, or any other
/// file the server can read, and download it in a bundle.
fn in_media(path: &Path) -> Option<PathBuf> {
    let file = path.canonicalize().ok()?;
    let media = crate::config::get_media_dir().canonicalize().ok()?;
    (file.starts_with(media) && file.is_file()).then_some(file)
}

/// The kind and name of `file` in the bundle: as stored, if it is in the directory of a kind,
/// else named by its contents, as an uploaded image or clip is.
fn stored_name(file: &Path, contents: &[u8]) -> (MediaKind, String) {
    let file_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let canonical = |dir: &Path| std::fs::canonicalize(dir).ok();
    let parent = file.parent().and_then(canonical);
    for kind in KINDS {
        if parent.is_some() && parent == canonical(&kind.directory()) {
            return (kind, file_name.to_string());
        }
    }
    let (kind, default) = if is_audio_path(file_name) {
        (MediaKind::Audio, "audio")
    } else {
        (MediaKind::Image, "image")
    };
    let stem = sanitized_stem(file_name, default);
    let hash = blake3::hash(contents).to_hex();
    let extension = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    (kind, format!("{stem}-{}.{extension}", &hash[..HASH_LENGTH]))
}

/// A file of a bundle that a shared file names: `media/<kind>/<name>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundledMediaReference {
    /// Where the file names it
    pub range: Range<usize>,
    pub kind: MediaKind,
    pub name: String,
}

impl BundledMediaReference {
    /// The file, in the bundle next to the file at `shared_file`
    pub fn path(&self, shared_file: &Path) -> PathBuf {
        shared_file
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(SHARED_MEDIA_DIR)
            .join(self.kind.as_str())
            .join(&self.name)
    }
}

/// The files of a bundle that the shared file `contents` names, in order.
pub fn bundled_media_references(contents: &str) -> Vec<BundledMediaReference> {
    let regex = get_or_compile_regex(&format!(
        r"(?<![\w./~-]){SHARED_MEDIA_DIR}/(image|audio|image-occlusion)/([\w.-]+)(?![\w.-])"
    ))
    .expect("the pattern is valid");
    regex
        .captures_iter(contents)
        .filter_map(Result::ok)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            let kind = KINDS
                .into_iter()
                .find(|kind| kind.as_str() == &captures[1])?;
            let name = captures[2].to_string();
            // As `check_media` takes names: no directory, and not hidden
            (!name.starts_with('.')).then(|| BundledMediaReference {
                range: whole.range(),
                kind,
                name,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundle_names_files_by_their_place_in_it() {
        // In the media directory, but not in a kind's
        let dir = crate::config::get_media_dir()
            .join(format!("spares-shared-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Not in a media directory, so named by its contents, with its extension in lowercase
        let image = dir.join("Cell.PNG");
        std::fs::write(&image, b"png").unwrap();
        let clip = dir.join("bonjour.mp3");
        std::fs::write(&clip, b"mp3").unwrap();
        let image = image.display().to_string();
        let clip = clip.display().to_string();

        let mut bundle = MediaBundle::default();
        let data = format!(
            "![]({image}) {{{{ [listen]({clip}) }}}} #image(\"{image}\") ![](/nowhere/x.png) /a/b"
        );
        let shared = bundle.bundle(&data).unwrap();
        let png = &blake3::hash(b"png").to_hex()[..HASH_LENGTH];
        let mp3 = &blake3::hash(b"mp3").to_hex()[..HASH_LENGTH];
        assert_eq!(
            shared,
            format!(
                "![](media/image/Cell-{png}.png) {{{{ [listen](media/audio/bonjour-{mp3}.mp3) }}}} \
                 #image(\"media/image/Cell-{png}.png\") ![](/nowhere/x.png) /a/b"
            )
        );
        let (files, missing) = bundle.finish();
        assert_eq!(missing, ["/nowhere/x.png"]);
        assert_eq!(
            files
                .iter()
                .map(|f| (f.kind, f.name.as_str(), f.contents.as_slice()))
                .collect::<Vec<_>>(),
            [
                (
                    MediaKind::Image,
                    format!("Cell-{png}.png").as_str(),
                    &b"png"[..]
                ),
                (
                    MediaKind::Audio,
                    format!("bonjour-{mp3}.mp3").as_str(),
                    &b"mp3"[..]
                ),
            ]
        );

        // Importing finds them again
        let references = bundled_media_references(&shared);
        assert_eq!(
            references
                .iter()
                .map(|r| (&shared[r.range.clone()], r.kind))
                .collect::<Vec<_>>(),
            [
                (
                    format!("media/image/Cell-{png}.png").as_str(),
                    MediaKind::Image
                ),
                (
                    format!("media/audio/bonjour-{mp3}.mp3").as_str(),
                    MediaKind::Audio
                ),
                (
                    format!("media/image/Cell-{png}.png").as_str(),
                    MediaKind::Image
                ),
            ]
        );
        assert_eq!(
            references[0].path(Path::new("shared/export.md")),
            Path::new("shared/media/image").join(format!("Cell-{png}.png"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn test_bundle_reads_nothing_outside_the_media_directory() {
        let id = std::process::id();
        let outside = std::env::temp_dir().join(format!("spares-not-media-{id}"));
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.png");
        std::fs::write(&secret, b"secret").unwrap();
        // A link in the media directory to it
        let media = crate::config::get_media_dir().join(format!("spares-media-link-{id}"));
        std::fs::create_dir_all(&media).unwrap();
        let link = media.join("link.png");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        let mut bundle = MediaBundle::default();
        let data = format!("![]({}) ![]({})", secret.display(), link.display());
        assert_eq!(bundle.bundle(&data).unwrap(), data);
        let (files, missing) = bundle.finish();
        assert!(files.is_empty(), "{files:?}");
        assert_eq!(missing.len(), 2);
        std::fs::remove_dir_all(outside).unwrap();
        std::fs::remove_dir_all(media).unwrap();
    }

    #[test]
    fn test_bundled_media_references_skip_other_paths() {
        for text in [
            "social/media/image/a.png",
            "../media/audio/b.mp3",
            "media/video/a.mp4",
            "media/image/.hidden",
        ] {
            assert_eq!(bundled_media_references(text), [], "{text}");
        }
    }
}
