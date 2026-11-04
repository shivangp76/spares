//! Image occlusions made and edited in the browser.
//!
//! Notes refer to image occlusion files by absolute path, so the browser cannot hand the server
//! files of its own. Instead, it uploads the image and clozes SVG, and they are stored in the
//! image occlusion directory where `read_image_occlusion_data` would have moved them anyway.
//!
//! Stored files are never overwritten or shared. Card orders are written back into the clozes SVG
//! while a note is parsed, and the cards a note had before an update are parsed from the files its
//! old text refers to. If an edit overwrote the SVG in place, the old cards would be parsed from
//! the new SVG. Undo would also restore text that refers to the edited file. So each upload and
//! edit stores a new image and clozes pair under a unique name and points the note at it. Names
//! are not content addressed since the orders written into an SVG belong to one note.

use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::api::note::update_notes;
use crate::api::parser::get_parser_name;
use crate::config::read_external_config;
use crate::model::Note;
use crate::model::NoteId;
use crate::parsers::BackReveal;
use crate::parsers::FrontConceal;
use crate::parsers::Parseable;
use crate::parsers::find_parser;
use crate::parsers::image_occlusion::ConstructImageOcclusionType;
use crate::parsers::image_occlusion::ImageOcclusionData;
use crate::parsers::image_occlusion::ImageOcclusionEditorConfig;
use crate::parsers::image_occlusion::append_to_stem;
use crate::parsers::image_occlusion::back_emphasis_image_occlusion_default;
use crate::parsers::image_occlusion::get_clozes_from_svg_str;
use crate::parsers::image_occlusion::get_image_occlusion_directory;
use crate::parsers::image_occlusion::read_image_occlusions;
use crate::platform::create_dir_all;
use crate::platform::is_file;
use crate::platform::read;
use crate::platform::write;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateNotesResponse;
use crate::schema::note::UpdateTags;

/// The SVG new image occlusions start from. It has the layers the parser looks for.
pub const IMAGE_OCCLUSION_TEMPLATE: &str = include_str!("../parsers/image_occlusion/template.svg");

/// Image formats cards can show, and the card renderer can draw clozes over.
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

/// Length of the unique suffix appended to stored file stems.
pub(crate) const HASH_LENGTH: usize = 12;

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateImageOcclusionResponse {
    pub image_occlusion: ImageOcclusionData,
    /// The image occlusion block to insert into a note, in the parser's syntax.
    pub snippet: String,
}

/// What the image occlusion editor starts from: the template and the user's editor settings.
#[derive(Debug, Deserialize, Serialize)]
pub struct ImageOcclusionEditorSetup {
    pub template: String,
    #[serde(flatten)]
    pub config: ImageOcclusionEditorConfig,
}

pub fn get_image_occlusion_editor_config() -> Result<ImageOcclusionEditorSetup, Error> {
    Ok(ImageOcclusionEditorSetup {
        template: IMAGE_OCCLUSION_TEMPLATE.to_string(),
        config: read_external_config()?.image_occlusion.editor,
    })
}

pub(crate) fn other_error(description: String) -> Error {
    Error::Library(LibraryError::Note(NoteErrorKind::Other { description }))
}

/// Removes a suffix added by [`store_image_occlusion_files`], so editing a stored image occlusion
/// does not keep growing its name.
pub(crate) fn strip_hash_suffix(stem: &str) -> &str {
    match stem.rsplit_once('-') {
        Some((base, hash))
            if !base.is_empty()
                && hash.len() == HASH_LENGTH
                && hash.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            base
        }
        _ => stem,
    }
}

/// Whether `stem` ends in a suffix added by [`store_image_occlusion_files`], as the names of stored
/// image occlusion files do.
pub(crate) fn has_hash_suffix(stem: &str) -> bool {
    strip_hash_suffix(stem) != stem
}

/// Unused names in `directory` for an image occlusion's image and clozes SVG:
/// `<stem>-<suffix>.<extension>` and `<stem>-<suffix>_clozes.svg`, with a suffix of
/// [`HASH_LENGTH`] hex digits from `seed` and the time. `None` if none was found.
pub(crate) fn unused_names(
    directory: &Path,
    stem: &str,
    extension: &str,
    seed: &[&[u8]],
) -> Option<(PathBuf, PathBuf)> {
    let nanos = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (0u32..1000)
        .map(|attempt| {
            let mut hasher = blake3::Hasher::new();
            for part in seed {
                hasher.update(part);
            }
            hasher.update(&nanos.to_le_bytes());
            hasher.update(&attempt.to_le_bytes());
            let suffix = hasher.finalize().to_hex();
            let image_path =
                directory.join(format!("{stem}-{}.{extension}", &suffix[..HASH_LENGTH]));
            let mut clozes_path = append_to_stem(&image_path, "_clozes");
            clozes_path.set_extension("svg");
            (image_path, clozes_path)
        })
        .find(|(image_path, clozes_path)| !is_file(image_path) && !is_file(clozes_path))
}

/// The lowercase extension of `image_filename`, if it is an image format cards can show.
pub(crate) fn image_extension(image_filename: &str) -> Result<String, Error> {
    Path::new(image_filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .filter(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
        .ok_or_else(|| {
            other_error(format!(
                "Unsupported image: {image_filename}. Supported formats: {}.",
                IMAGE_EXTENSIONS.join(", ")
            ))
        })
}

/// The stem of `filename` without a suffix added when it was stored, keeping only characters that
/// are safe in a path and in every parser's syntax. `default` if nothing is left.
pub(crate) fn sanitized_stem(filename: &str, default: &str) -> String {
    let stem: String = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(strip_hash_suffix)
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() {
        default.to_string()
    } else {
        stem
    }
}

/// Fails if the parser could not read the clozes of `clozes_svg`.
pub(crate) fn check_clozes_svg(clozes_svg: &str) -> Result<(), Error> {
    get_clozes_from_svg_str(
        clozes_svg,
        FrontConceal::image_occlusion_default(),
        BackReveal::image_occlusion_default(),
        back_emphasis_image_occlusion_default(),
        &mut 0,
    )?;
    Ok(())
}

/// Stores an image and its clozes SVG in the image occlusion directory, where
/// `read_image_occlusion_data` expects them, and returns their paths.
///
/// `image_filename` is only used for the stem and extension of the stored name.
fn store_image_occlusion_files(
    image: &[u8],
    image_filename: &str,
    clozes_svg: &str,
) -> Result<(PathBuf, PathBuf), Error> {
    // Reject SVGs the parser could not read, before anything is written
    check_clozes_svg(clozes_svg)?;

    let extension = image_extension(image_filename)?;
    let stem = sanitized_stem(image_filename, "image_occlusion");

    let directory = get_image_occlusion_directory();
    create_dir_all(&directory).map_err(|e| Error::Io {
        description: format!("Failed to create {}", directory.display()),
        source: e,
    })?;
    let (image_path, clozes_path) = unused_names(
        &directory,
        &stem,
        &extension,
        &[image, clozes_svg.as_bytes()],
    )
    .ok_or_else(|| {
        other_error(format!(
            "Failed to find an unused image occlusion file name for {image_filename}"
        ))
    })?;

    write(&image_path, image).map_err(|e| Error::Io {
        description: format!("Failed to write {}", image_path.display()),
        source: e,
    })?;
    write(&clozes_path, clozes_svg).map_err(|e| Error::Io {
        description: format!("Failed to write {}", clozes_path.display()),
        source: e,
    })?;
    Ok((image_path, clozes_path))
}

async fn get_parser_for_id(
    db: &SqlitePool,
    parser_id: i64,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Box<dyn Parseable>, Error> {
    let parser_name = get_parser_name(db, parser_id).await?;
    find_parser(&parser_name, all_parsers)
}

/// Stores a new image occlusion and returns the block to insert into a note with the given parser.
pub async fn create_image_occlusion(
    db: &SqlitePool,
    parser_id: i64,
    image: &[u8],
    image_filename: &str,
    clozes_svg: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<CreateImageOcclusionResponse, Error> {
    let parser = get_parser_for_id(db, parser_id, all_parsers).await?;
    let (original_image_filepath, clozes_filepath) =
        store_image_occlusion_files(image, image_filename, clozes_svg)?;
    let image_occlusion = ImageOcclusionData {
        original_image_filepath,
        clozes_filepath,
        front_conceal: FrontConceal::image_occlusion_default(),
        back_reveal: BackReveal::image_occlusion_default(),
        back_emphasis: back_emphasis_image_occlusion_default(),
    };
    let snippet =
        parser.construct_image_occlusion(&image_occlusion, ConstructImageOcclusionType::Note);
    Ok(CreateImageOcclusionResponse {
        image_occlusion,
        snippet,
    })
}

async fn get_note_row(db: &SqlitePool, note_id: NoteId) -> Result<Note, Error> {
    sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

/// The image occlusions in a note, in the order they appear.
pub async fn list_note_image_occlusions(
    db: &SqlitePool,
    note_id: NoteId,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Vec<ImageOcclusionData>, Error> {
    let note = get_note_row(db, note_id).await?;
    let parser = get_parser_for_id(db, note.parser_id, all_parsers).await?;
    let image_occlusions = read_image_occlusions(&note.data, parser.as_ref(), false)?;
    Ok(image_occlusions.into_iter().map(|(_, data)| data).collect())
}

/// Replaces the clozes of the note's `index`th image occlusion (0 based) and updates the note so
/// its cards follow.
pub async fn update_note_image_occlusion(
    db: &SqlitePool,
    note_id: NoteId,
    index: usize,
    clozes_svg: &str,
    at: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<UpdateNotesResponse, Error> {
    let note = get_note_row(db, note_id).await?;
    let parser = get_parser_for_id(db, note.parser_id, all_parsers).await?;
    let image_occlusions = read_image_occlusions(&note.data, parser.as_ref(), false)?;
    let count = image_occlusions.len();
    let (range, old) = image_occlusions.into_iter().nth(index).ok_or_else(|| {
        other_error(format!(
            "Note {} has {count} image occlusion(s), so there is no image occlusion {}.",
            note_id.letters(),
            index + 1
        ))
    })?;

    let image = read(&old.original_image_filepath)
        .await
        .map_err(|e| Error::Io {
            description: format!("Failed to read {}", old.original_image_filepath.display()),
            source: e,
        })?;
    let image_filename = old
        .original_image_filepath
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default();
    let (original_image_filepath, clozes_filepath) =
        store_image_occlusion_files(&image, image_filename, clozes_svg)?;
    let new = ImageOcclusionData {
        original_image_filepath,
        clozes_filepath,
        ..old
    };
    let block = parser.construct_image_occlusion(&new, ConstructImageOcclusionType::Note);
    let mut data = note.data;
    data.replace_range(range.range, &block);

    update_notes(
        db,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            parser_id: None,
            data: Some(data),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        at,
        all_parsers,
        true,
    )
    .await
}

/// Reads a file from the image occlusion directory. Paths outside of it are rejected so this
/// cannot be used to read arbitrary files.
pub async fn read_image_occlusion_file(path: &str) -> Result<Vec<u8>, Error> {
    let not_found = || other_error(format!("No image occlusion file at {path}"));
    let path = image_occlusion_file_path(Path::new(path)).ok_or_else(not_found)?;
    read(&path).await.map_err(|e| Error::Io {
        description: format!("Failed to read {}", path.display()),
        source: e,
    })
}

/// `path`, if it is a file in the image occlusion directory, without symbolic links.
#[cfg(not(target_family = "wasm"))]
fn image_occlusion_file_path(path: &Path) -> Option<PathBuf> {
    let directory = get_image_occlusion_directory().canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    (path.starts_with(&directory) && path.is_file()).then_some(path)
}

/// `path`, if it is a file in the image occlusion directory. A browser's files have no links, so
/// only `..` could leave it.
#[cfg(target_family = "wasm")]
fn image_occlusion_file_path(path: &Path) -> Option<PathBuf> {
    let inside = path.starts_with(get_image_occlusion_directory())
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    (inside && is_file(path)).then(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::api::card::get_cards;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::undo::undo_event;
    use crate::parsers::get_all_parsers;
    use crate::parsers::image_occlusion::append_to_stem;
    use crate::parsers::impls::markdown::MarkdownParser;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::undo::UndoEventRequest;

    const IMAGE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400"><rect width="400" height="400" fill="#F97316"/></svg>"##;

    fn clozes_svg(shapes: &str) -> String {
        format!(
            r#"<svg width="400" height="400" xmlns="http://www.w3.org/2000/svg"><g class="layer" id="markup-group"><title>Markup</title></g><g class="layer" id="clozes-group"><title>Clozes</title>{shapes}</g></svg>"#
        )
    }

    const RECT_1: &str =
        r##"<rect id="svg_1" x="10" y="10" width="50" height="50" fill="#FFEBA2"/>"##;
    const RECT_2: &str =
        r##"<rect id="svg_2" x="100" y="100" width="50" height="50" fill="#FFEBA2"/>"##;

    #[test]
    fn test_editor_settings_default_when_missing() {
        let config: crate::config::SparesExternalConfig =
            toml_edit::de::from_str("[image_occlusion.editor]\nfill_color = \"#123456\"\n")
                .unwrap();
        let config = config.image_occlusion;
        assert_eq!(config.cloze_hint_fill, 0.75);
        assert_eq!(config.editor.fill_color, "#123456");
        assert_eq!(config.editor.stroke_color, "#000000");
        assert_eq!(config.editor.initial_tool, "rect");
    }

    #[test]
    fn test_strip_hash_suffix() {
        assert_eq!(strip_hash_suffix("brain-0123456789ab"), "brain");
        assert_eq!(
            strip_hash_suffix("brain-0123456789abcd"),
            "brain-0123456789abcd"
        );
        assert_eq!(strip_hash_suffix("left-brain"), "left-brain");
        assert_eq!(strip_hash_suffix("-0123456789ab"), "-0123456789ab");
    }

    #[test]
    fn test_store_rejects_invalid_files() {
        // No clozes layer
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        assert!(store_image_occlusion_files(IMAGE.as_bytes(), "a.svg", svg).is_err());
        // Unsupported image format
        let svg = clozes_svg(RECT_1);
        assert!(store_image_occlusion_files(b"text", "a.txt", &svg).is_err());
    }

    #[test]
    fn test_store_names_files_uniquely() {
        let svg = clozes_svg(RECT_1);
        let (image_path, clozes_path) =
            store_image_occlusion_files(IMAGE.as_bytes(), "My brain!.svg", &svg).unwrap();
        assert_eq!(
            image_path.parent().unwrap(),
            get_image_occlusion_directory()
        );
        let image_name = image_path.file_name().unwrap().to_str().unwrap();
        assert!(image_name.starts_with("My_brain_-"), "{image_name}");
        let mut expected_clozes_path = append_to_stem(&image_path, "_clozes");
        expected_clozes_path.set_extension("svg");
        assert_eq!(clozes_path, expected_clozes_path);

        // Storing a stored image again keeps its stem, but never reuses a name since the orders
        // written into a clozes file belong to one note
        let (image_path_again, _) = store_image_occlusion_files(
            IMAGE.as_bytes(),
            image_path.file_name().unwrap().to_str().unwrap(),
            &svg,
        )
        .unwrap();
        assert_ne!(image_path, image_path_again);
        let image_name_again = image_path_again.file_name().unwrap().to_str().unwrap();
        assert!(
            image_name_again.starts_with("My_brain_-"),
            "{image_name_again}"
        );
        assert_eq!(image_name_again.len(), image_name.len());
    }

    #[tokio::test]
    async fn test_read_file_only_from_image_occlusion_directory() {
        let svg = clozes_svg(RECT_1);
        let (image_path, _) =
            store_image_occlusion_files(IMAGE.as_bytes(), "read.svg", &svg).unwrap();
        assert_eq!(
            read_image_occlusion_file(image_path.to_str().unwrap())
                .await
                .unwrap(),
            IMAGE.as_bytes()
        );
        let escaping = get_image_occlusion_directory().join("../../../etc/passwd");
        let read = |path: String| async move { read_image_occlusion_file(&path).await };
        assert!(read(escaping.to_string_lossy().into_owned()).await.is_err());
        assert!(read("/etc/passwd".to_string()).await.is_err());
        let directory = get_image_occlusion_directory();
        assert!(
            read(directory.to_string_lossy().into_owned())
                .await
                .is_err()
        );
    }

    async fn create_note_with_image_occlusion(
        pool: &SqlitePool,
        parser_name: &str,
        image_filename: &str,
    ) -> (NoteId, String) {
        let parser = create_parser_helper(pool, parser_name).await;
        let created = create_image_occlusion(
            pool,
            parser.id,
            IMAGE.as_bytes(),
            image_filename,
            &clozes_svg(RECT_1),
            &get_all_parsers(),
        )
        .await
        .unwrap();
        let notes = create_notes(
            pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: format!("Before\n{}After\n", created.snippet),
                    keywords: Vec::new(),
                    tags: Vec::new(),
                    is_suspended: false,
                    custom_data: Map::new(),
                }],
            },
            Utc::now(),
            &get_all_parsers(),
            true,
        )
        .await
        .unwrap();
        (notes.notes[0].id, created.snippet)
    }

    /// A note's image occlusion may only name files in its own collection: on a host, every
    /// user's files belong to the server, which would otherwise move another user's file into
    /// this collection when the note is saved, or draw it into a card when the note is rendered.
    #[sqlx::test]
    async fn test_image_occlusion_files_outside_the_collection_are_refused(pool: SqlitePool) -> () {
        // Not in the collection's directories, which are in /tmp/spares under tests, like
        // another user's
        let outside =
            std::env::temp_dir().join(format!("spares-io-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        let name = format!("outside-{}", uuid::Uuid::new_v4().simple());
        let outside_image = outside.join(format!("{name}.svg"));
        let outside_clozes = outside.join(format!("{name}_clozes.svg"));
        std::fs::write(&outside_image, IMAGE).unwrap();
        std::fs::write(&outside_clozes, clozes_svg(RECT_1)).unwrap();

        let parser = create_parser_helper(&pool, "markdown").await;
        let created = create_image_occlusion(
            &pool,
            parser.id,
            IMAGE.as_bytes(),
            "inside.svg",
            &clozes_svg(RECT_1),
            &get_all_parsers(),
        )
        .await
        .unwrap();
        let inside = &created.image_occlusion;
        let naming = |image: &Path, clozes: &Path| {
            created
                .snippet
                .replace(
                    &inside.original_image_filepath.display().to_string(),
                    &image.display().to_string(),
                )
                .replace(
                    &inside.clozes_filepath.display().to_string(),
                    &clozes.display().to_string(),
                )
        };
        let markdown = MarkdownParser::new();
        for data in [
            naming(&outside_image, &inside.clozes_filepath),
            naming(&inside.original_image_filepath, &outside_clozes),
            naming(&outside_image, &outside_clozes),
        ] {
            // Saved, which moves an image occlusion's files into the collection
            let saved = create_notes(
                &pool,
                CreateNotesRequest {
                    parser_id: parser.id,
                    requests: vec![CreateNoteRequest {
                        data: data.clone(),
                        keywords: Vec::new(),
                        tags: Vec::new(),
                        is_suspended: false,
                        custom_data: Map::new(),
                    }],
                },
                Utc::now(),
                &get_all_parsers(),
                true,
            )
            .await;
            assert!(saved.is_err(), "{data}");
            // Or parsed as it is, as a render or a sync does
            assert!(
                crate::parsers::get_cards(&markdown, None, &data, false, false).is_err(),
                "{data}"
            );
        }
        // Left where they were
        assert!(outside_image.is_file() && outside_clozes.is_file());
        let moved = get_image_occlusion_directory().join(format!("{name}.svg"));
        let _ = std::fs::remove_file(&moved);
        let _ = std::fs::remove_file(append_to_stem(&moved, "_clozes"));
        std::fs::remove_dir_all(&outside).unwrap();
    }

    /// A note written by hand may name image occlusion files that weren't stored by spares, as
    /// `roster.png`. Saving the note renames them as stored files are named, since hosts send
    /// each other only such names, and points the note at them. Its caption stays.
    #[sqlx::test]
    async fn test_saving_a_note_names_its_image_occlusion_files_as_stored(pool: SqlitePool) -> () {
        let directory = get_image_occlusion_directory();
        let stem = format!("roster_{}", uuid::Uuid::new_v4().simple());
        let (image, clozes) = (
            directory.join(format!("{stem}.svg")),
            directory.join(format!("{stem}_clozes.svg")),
        );
        std::fs::write(&image, IMAGE).unwrap();
        std::fs::write(&clozes, clozes_svg(RECT_1)).unwrap();
        let parser = create_parser_helper(&pool, "markdown").await;
        let data = MarkdownParser::new().construct_image_occlusion(
            &ImageOcclusionData {
                original_image_filepath: image.clone(),
                clozes_filepath: clozes.clone(),
                front_conceal: FrontConceal::image_occlusion_default(),
                back_reveal: BackReveal::image_occlusion_default(),
                back_emphasis: back_emphasis_image_occlusion_default(),
            },
            ConstructImageOcclusionType::Note,
        );
        let notes = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data,
                    keywords: Vec::new(),
                    tags: Vec::new(),
                    is_suspended: false,
                    custom_data: Map::new(),
                }],
            },
            Utc::now(),
            &get_all_parsers(),
            true,
        )
        .await
        .unwrap();
        let note_id = notes.notes[0].id;

        let stored = list_note_image_occlusions(&pool, note_id, &get_all_parsers())
            .await
            .unwrap()
            .remove(0);
        let stored_stem = stored
            .original_image_filepath
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(has_hash_suffix(stored_stem), "{stored_stem}");
        assert_eq!(strip_hash_suffix(stored_stem), stem);
        assert_eq!(stored.original_image_filepath.parent().unwrap(), directory);
        let mut expected_clozes = append_to_stem(&stored.original_image_filepath, "_clozes");
        expected_clozes.set_extension("svg");
        assert_eq!(stored.clozes_filepath, expected_clozes);
        assert!(stored.original_image_filepath.is_file() && stored.clozes_filepath.is_file());
        assert!(!image.exists() && !clozes.exists());
        let data = &notes.notes[0].data;
        assert!(!data.contains(&format!("{stem}.svg")), "{data}");
        assert!(
            data.contains(&crate::helpers::to_title_case(&stem)),
            "the caption keeps the name: {data}"
        );
        assert_eq!(get_cards(&pool, note_id).await.unwrap().len(), 1);

        // Saved again, a stored name stays
        let again = update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![note_id]),
                parser_id: None,
                data: Some(data.clone()),
                keywords: None,
                tags: UpdateTags::None,
                custom_data: None,
            },
            Utc::now(),
            &get_all_parsers(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(&again.notes[0].data, data);

        // A note saved before then still names such files until it is edited. Its old text is
        // parsed as it is, so the files it names stay for its undo
        std::fs::copy(&stored.original_image_filepath, &image).unwrap();
        std::fs::copy(&stored.clozes_filepath, &clozes).unwrap();
        let old_data = data
            .replace(
                &stored.original_image_filepath.display().to_string(),
                &image.display().to_string(),
            )
            .replace(
                &stored.clozes_filepath.display().to_string(),
                &clozes.display().to_string(),
            );
        sqlx::query(r"UPDATE note SET data = ? WHERE id = ?")
            .bind(&old_data)
            .bind(note_id)
            .execute(&pool)
            .await
            .unwrap();
        update_note_image_occlusion(
            &pool,
            note_id,
            0,
            &clozes_svg(RECT_2),
            Utc::now(),
            &get_all_parsers(),
        )
        .await
        .unwrap();
        assert!(image.is_file() && clozes.is_file());
        let edited = list_note_image_occlusions(&pool, note_id, &get_all_parsers())
            .await
            .unwrap()
            .remove(0);
        assert_ne!(edited.original_image_filepath, image);
        for path in [
            &stored.original_image_filepath,
            &stored.clozes_filepath,
            &image,
            &clozes,
            &edited.original_image_filepath,
            &edited.clozes_filepath,
        ] {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[sqlx::test]
    async fn test_created_snippet_round_trips_for_every_parser(pool: SqlitePool) -> () {
        for parser_name in ["markdown", "typst", "latex-note"] {
            let (note_id, _) = create_note_with_image_occlusion(
                &pool,
                parser_name,
                &format!("round_trip_{parser_name}.svg"),
            )
            .await;
            assert_eq!(
                get_cards(&pool, note_id).await.unwrap().len(),
                1,
                "{parser_name}"
            );
            let image_occlusions = list_note_image_occlusions(&pool, note_id, &get_all_parsers())
                .await
                .unwrap();
            assert_eq!(image_occlusions.len(), 1, "{parser_name}");
            assert_eq!(
                image_occlusions[0]
                    .original_image_filepath
                    .parent()
                    .unwrap(),
                get_image_occlusion_directory()
            );
        }
    }

    #[sqlx::test]
    async fn test_update_note_image_occlusion(pool: SqlitePool) -> () {
        let (note_id, _) = create_note_with_image_occlusion(&pool, "markdown", "update.svg").await;
        let before = list_note_image_occlusions(&pool, note_id, &get_all_parsers())
            .await
            .unwrap()
            .remove(0);
        let before_svg = std::fs::read_to_string(&before.clozes_filepath).unwrap();

        let response = update_note_image_occlusion(
            &pool,
            note_id,
            0,
            &clozes_svg(&format!("{RECT_1}{RECT_2}")),
            Utc::now(),
            &get_all_parsers(),
        )
        .await
        .unwrap();
        let data = &response.notes[0].data;
        assert!(
            data.starts_with("Before\n") && data.ends_with("After\n"),
            "{data}"
        );
        assert_eq!(get_cards(&pool, note_id).await.unwrap().len(), 2);

        // The note points at new files, and the old ones are left untouched for undo
        let after = list_note_image_occlusions(&pool, note_id, &get_all_parsers())
            .await
            .unwrap()
            .remove(0);
        assert_ne!(after.clozes_filepath, before.clozes_filepath);
        assert_ne!(
            after.original_image_filepath,
            before.original_image_filepath
        );
        assert_eq!(after.front_conceal, before.front_conceal);
        assert_eq!(
            std::fs::read_to_string(&before.clozes_filepath).unwrap(),
            before_svg
        );

        undo_event(
            &pool,
            UndoEventRequest {
                event_id: None,
                undo_group: true,
            },
        )
        .await
        .unwrap();
        let undone = list_note_image_occlusions(&pool, note_id, &get_all_parsers())
            .await
            .unwrap()
            .remove(0);
        assert_eq!(undone.clozes_filepath, before.clozes_filepath);
        assert_eq!(get_cards(&pool, note_id).await.unwrap().len(), 1);

        // Out of range
        assert!(
            update_note_image_occlusion(
                &pool,
                note_id,
                1,
                &clozes_svg(RECT_1),
                Utc::now(),
                &get_all_parsers(),
            )
            .await
            .is_err()
        );
    }
}
