//! Notes exported to share as one file: a zip of the notes' files and their
//! [bundled media](super::shared_media), as `spares note export --share` writes them into a
//! directory:
//!
//! ```text
//! export.md
//! media/image/cell-0123456789ab.png
//! media/image-occlusion/brain-0123456789ab.png
//! media/image-occlusion/brain-0123456789ab_clozes.svg
//! ```
//!
//! The web app downloads one (`POST /api/notes/export/bundle`), since a browser saves files, not
//! directories, and imports one ([`import_shared`]), as `spares import` does. A zip of the
//! directory, with the directory's name before every path, is read the same.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use std::path::Path;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;
use zip::ZipArchive;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::shared_media::SHARED_MEDIA_DIR;
use super::shared_media::bundled_media_references;
use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::adapters::get_adapter_from_string;
use crate::api::image::MAX_IMAGE_SIZE;
use crate::api::note::create_notes;
use crate::api::note::export::is_state_tag;
use crate::api::parser::create_parser;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::notes_vcs::is_warning;
use crate::parsers::NoteImportAction;
use crate::parsers::Parseable;
use crate::parsers::get_notes;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::ExportNotesResponse;
use crate::schema::parser::CreateParserRequest;
use crate::sync::media::store_media;

/// The most a bundle may hold once unzipped, so a small zip can't fill the disk.
const MAX_BUNDLE_SIZE: u64 = 512 * 1024 * 1024;

fn invalid(description: String) -> Error {
    Error::Library(LibraryError::Note(NoteErrorKind::Other { description }))
}

fn zip_error(e: &zip::result::ZipError) -> Error {
    invalid(format!("Not a zip of shared notes: {e}"))
}

/// `export` as a zip.
pub fn write_bundle(export: &ExportNotesResponse) -> Result<Vec<u8>, Error> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    let mut add = |name: &str, contents: &[u8]| -> Result<(), Error> {
        zip.start_file(name, options).map_err(|e| zip_error(&e))?;
        zip.write_all(contents).map_err(|e| Error::Io {
            description: format!("Failed to write {name} into the zip"),
            source: e,
        })
    };
    for (extension, contents) in export.files.iter().collect::<BTreeMap<_, _>>() {
        add(&format!("export.{extension}"), contents.as_bytes())?;
    }
    for media in &export.media {
        let path = media.relative_path();
        // Zips use `/`, whatever the system
        let name = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        add(&name, &media.contents)?;
    }
    Ok(zip.finish().map_err(|e| zip_error(&e))?.into_inner())
}

/// What a bundle holds.
#[derive(Debug, Default)]
pub struct Bundle {
    /// The notes' files, by name
    pub files: BTreeMap<String, String>,
    /// The bundled media, by path in the bundle, such as `media/image/cell-0123456789ab.png`
    pub media: HashMap<String, Vec<u8>>,
}

/// Whether `bytes` are a zip's
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

/// The files of the zip `bytes`. A directory every path starts with, as when a directory was
/// zipped, is left out of their names.
pub fn read_bundle(bytes: &[u8]) -> Result<Bundle, Error> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| zip_error(&e))?;
    let mut entries = Vec::new();
    let mut total = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| zip_error(&e))?;
        if entry.is_dir() {
            continue;
        }
        let Some(path) = entry.enclosed_name() else {
            return Err(invalid(format!(
                "The zip has a file outside it: {}",
                entry.name()
            )));
        };
        let name = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        // macOS's Finder adds these
        if name.first().is_some_and(|c| c == "__MACOSX")
            || name.last().is_some_and(|c| c == ".DS_Store")
        {
            continue;
        }
        let mut contents = Vec::new();
        let limit = MAX_BUNDLE_SIZE - total;
        entry
            .by_ref()
            .take(limit + 1)
            .read_to_end(&mut contents)
            .map_err(|e| Error::Io {
                description: format!("Failed to unzip {}", entry.name()),
                source: e,
            })?;
        total += contents.len() as u64;
        if total > MAX_BUNDLE_SIZE {
            return Err(invalid(format!(
                "The zip holds more than {} MB",
                MAX_BUNDLE_SIZE / 1024 / 1024
            )));
        }
        entries.push((name, contents));
    }
    // A zipped directory
    let first = entries.first().and_then(|(name, _)| name.first()).cloned();
    let common = first.filter(|dir| {
        entries
            .iter()
            .all(|(name, _)| name.len() > 1 && name.first() == Some(dir))
    });
    let mut bundle = Bundle::default();
    for (mut name, contents) in entries {
        if common.is_some() {
            name.remove(0);
        }
        let name = name.join("/");
        if name.starts_with(&format!("{SHARED_MEDIA_DIR}/")) {
            bundle.media.insert(name, contents);
        } else {
            let text = String::from_utf8(contents)
                .map_err(|_| invalid(format!("{name} in the zip isn't text")))?;
            bundle.files.insert(name, text);
        }
    }
    Ok(bundle)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ImportSharedResponse {
    /// How many notes were added
    pub note_count: usize,
    /// The events of the notes' creation, oldest first, to undo the import
    pub event_ids: Vec<i64>,
    /// The parsers the collection didn't have, which were added for the notes
    pub parsers_added: Vec<String>,
    /// How many bundled files were stored for the notes
    pub media_count: usize,
    /// Bundled files the notes name that the bundle doesn't have, so the notes name nothing
    pub missing_media: Vec<String>,
    /// Files of the bundle that are no parser's, which were left out
    pub skipped_files: Vec<String>,
}

fn names_ids(file_name: &str) -> Error {
    invalid(format!(
        "{file_name} names notes by their ids, as an export to edit notes does, so it is for \
         `spares import` in their collection. Export the notes to share instead."
    ))
}

/// Fails if a file of `bundle` names notes by id rather than adding them, as an export to edit
/// notes does. Importing one would change the notes with those ids in the importer's collection,
/// which have nothing to do with the shared ones.
pub fn refuse_note_ids(
    bundle: &Bundle,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(), Error> {
    let adapter = get_adapter_from_string("spares")?;
    for (file_name, contents) in &bundle.files {
        let Some(parser) = parser_of(file_name, all_parsers) else {
            continue;
        };
        let notes = get_notes(
            parser.as_ref(),
            None,
            contents,
            adapter.as_ref(),
            false,
            None,
        )?;
        if notes
            .iter()
            .any(|(settings, _)| settings.action != NoteImportAction::Add)
        {
            return Err(names_ids(file_name));
        }
    }
    Ok(())
}

/// The parser whose files have the extension of `name`
pub(crate) fn parser_of(
    name: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Option<Box<dyn Parseable>> {
    let extension = Path::new(name).extension()?.to_str()?;
    all_parsers
        .iter()
        .map(|parser| parser())
        .find(|parser| parser.file_extension() == extension)
}

/// The id of the parser `name` in the collection, which is added if it has none
async fn parser_id(db: &SqlitePool, name: &str, added: &mut Vec<String>) -> Result<i64, Error> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM parser WHERE name = ?")
        .bind(name)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    if let Some(id) = id {
        return Ok(id);
    }
    let request = CreateParserRequest {
        name: name.to_string(),
    };
    let parser = create_parser(db, request, false).await?;
    added.push(name.to_string());
    Ok(parser.id)
}

/// `contents` with the bundled files it names stored, once each (`stored` has the paths of those
/// stored so far, by their paths in the bundle), and named by their paths here. The ones the
/// bundle doesn't have are added to `missing`. Without `store` (a dry run), nothing is stored and
/// `contents` is returned as it is, after checking the bundle has what it names.
pub(crate) fn store_bundled(
    contents: &str,
    bundle: &Bundle,
    stored: &mut HashMap<String, String>,
    missing: &mut Vec<String>,
    store: bool,
) -> Result<String, Error> {
    let mut text = String::with_capacity(contents.len());
    let mut last = 0;
    for reference in bundled_media_references(contents) {
        let in_bundle = format!(
            "{SHARED_MEDIA_DIR}/{}/{}",
            reference.kind.as_str(),
            reference.name
        );
        let path = if let Some(path) = stored.get(&in_bundle) {
            path.clone()
        } else {
            let Some(media) = bundle.media.get(&in_bundle) else {
                if !missing.contains(&in_bundle) {
                    missing.push(in_bundle);
                }
                continue;
            };
            if media.len() > MAX_IMAGE_SIZE {
                return Err(invalid(format!("{in_bundle} is too large")));
            }
            if !store {
                continue;
            }
            let path = store_media(reference.kind, &reference.name, media)?
                .display()
                .to_string();
            stored.insert(in_bundle, path.clone());
            path
        };
        text.push_str(&contents[last..reference.range.start]);
        text.push_str(&path);
        last = reference.range.end;
    }
    if !store {
        return Ok(contents.to_string());
    }
    text.push_str(&contents[last..]);
    Ok(text)
}

/// Imports shared notes, named `name`, into the collection, as new notes: a file of shared notes
/// (`spares note export --share`), or a [bundle](self) of them. The files a bundle has for the
/// notes are stored first, as a device's uploads are, and the notes name them there.
///
/// Only notes to add are taken: a file that names notes by id, as an export to edit notes does,
/// is for `spares import` in the collection the notes are in. Nothing is added if any note has an
/// error.
pub async fn import_shared(
    db: &SqlitePool,
    name: &str,
    bytes: &[u8],
    at: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<ImportSharedResponse, Error> {
    let bundle = if is_zip(bytes) {
        read_bundle(bytes)?
    } else {
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|_| invalid(format!("{name} isn't text, or a zip")))?;
        Bundle {
            files: BTreeMap::from([(name.to_string(), text)]),
            media: HashMap::new(),
        }
    };
    let mut response = ImportSharedResponse::default();
    let adapter = get_adapter_from_string("spares")?;
    let config = read_external_config().ok();
    let overlapper = config.as_ref().map(|c| c.overlapper.clone());
    let defaults = SparesExternalConfig::default();
    let config = config.as_ref().unwrap_or(&defaults);
    let mut stored: HashMap<String, String> = HashMap::new();
    let mut parsed = Vec::new();
    let mut errors = Vec::new();
    for (file_name, contents) in &bundle.files {
        let Some(parser) = parser_of(file_name, all_parsers) else {
            if bundle.media.is_empty() && bundle.files.len() == 1 {
                return Err(invalid(format!(
                    "No parser reads {file_name}: shared notes are in .md, .typ or .tex files"
                )));
            }
            response.skipped_files.push(file_name.clone());
            continue;
        };
        let text = store_bundled(
            contents,
            &bundle,
            &mut stored,
            &mut response.missing_media,
            true,
        )?;

        let notes = get_notes(
            parser.as_ref(),
            None,
            &text,
            adapter.as_ref(),
            false,
            overlapper.as_ref(),
        )?;
        let mut requests = Vec::new();
        for (settings, data) in notes {
            for error in &settings.errors_and_warnings {
                if !is_warning(error) {
                    errors.push(format!("{file_name}: {error}"));
                }
            }
            if settings.action != NoteImportAction::Add {
                return Err(names_ids(file_name));
            }
            let Some(data) = data else {
                continue;
            };
            // What a note says of the sharer's collection, such as a `live:` tag that would make
            // it one of the importer's live notes, is left out, as an export to share leaves it
            // out
            let mut tags = settings.tags;
            tags.retain(|tag| !is_state_tag(tag, config));
            requests.push(CreateNoteRequest {
                data,
                keywords: settings.keywords,
                tags,
                is_suspended: settings.is_suspended,
                custom_data: serde_json::Map::default(),
            });
        }
        parsed.push((parser.get_parser_name(), requests));
    }
    response.media_count = stored.len();
    if !errors.is_empty() {
        return Err(invalid(errors.join("\n")));
    }
    for (parser_name, requests) in parsed {
        if requests.is_empty() {
            continue;
        }
        let parser_id = parser_id(db, parser_name, &mut response.parsers_added).await?;
        let created = create_notes(
            db,
            CreateNotesRequest {
                parser_id,
                requests,
            },
            at,
            all_parsers,
            true,
        )
        .await?;
        response.note_count += created.notes.len();
        response.event_ids.extend(created.event_id);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::note::shared_media::SharedMedia;
    use crate::sync::media::MediaKind;

    const SHARED: &str = "<!--- spares: start --->\n\n<!--- # action: add --->\n<!--- # tags: geo --->\n<!--- spares: note start --->\n{{ Paris }} is the capital\n<!--- spares: note end --->\n\n<!--- spares: end --->\n";

    async fn note_count(db: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM note")
            .fetch_one(db)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn test_import_shared_adds_notes_and_their_parser(pool: SqlitePool) {
        let parsers = crate::parsers::get_all_parsers();
        let imported = import_shared(&pool, "export.md", SHARED.as_bytes(), Utc::now(), &parsers)
            .await
            .unwrap();
        assert_eq!(imported.note_count, 1);
        assert_eq!(imported.parsers_added, ["markdown"]);
        assert_eq!(imported.event_ids.len(), 1);
        let tags: String =
            sqlx::query_scalar("SELECT t.name FROM note_tag nt JOIN tag t ON t.id = nt.tag_id")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tags, "geo");

        // A zip of it, the second time, with the parser there
        let export = ExportNotesResponse {
            files: HashMap::from([("md".to_string(), SHARED.to_string())]),
            ..ExportNotesResponse::default()
        };
        let zip = write_bundle(&export).unwrap();
        let imported = import_shared(&pool, "shared.zip", &zip, Utc::now(), &parsers)
            .await
            .unwrap();
        assert_eq!(imported.note_count, 1);
        assert_eq!(imported.parsers_added, Vec::<String>::new());
        assert_eq!(note_count(&pool).await, 2);
    }

    /// Tags that say how a note is doing in the sharer's collection, such as a `live:` tag that
    /// would make it one of the importer's live notes, are left out, as is custom data.
    #[sqlx::test]
    async fn test_import_shared_leaves_out_state(pool: SqlitePool) {
        let parsers = crate::parsers::get_all_parsers();
        let shared = SHARED.replace(
            "<!--- # tags: geo --->\n",
            "<!--- # tags: geo --->\n<!--- # tags: flagged --->\n<!--- # tags: live:lecture --->\n\
             <!--- # custom-data: {\"live_sync_name\": \"lecture\"} --->\n",
        );
        assert_ne!(shared, SHARED);
        import_shared(&pool, "export.md", shared.as_bytes(), Utc::now(), &parsers)
            .await
            .unwrap();
        let tags: Vec<String> =
            sqlx::query_scalar("SELECT t.name FROM note_tag nt JOIN tag t ON t.id = nt.tag_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(tags, ["geo"]);
        let custom_data: String = sqlx::query_scalar("SELECT custom_data FROM note")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(custom_data, "{}");
    }

    #[sqlx::test]
    async fn test_import_shared_refuses_notes_by_id_and_errors(pool: SqlitePool) {
        let parsers = crate::parsers::get_all_parsers();
        let by_id = SHARED.replace("# action: add", "# note-id: aaaa");
        let error = import_shared(&pool, "export.md", by_id.as_bytes(), Utc::now(), &parsers)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("names notes by their ids"),
            "{error}"
        );
        // As a CLI import of a zip checks
        let parsers_of = |text: &str| Bundle {
            files: BTreeMap::from([("export.md".to_string(), text.to_string())]),
            media: HashMap::new(),
        };
        assert!(refuse_note_ids(&parsers_of(&by_id), &parsers).is_err());
        refuse_note_ids(&parsers_of(SHARED), &parsers).unwrap();
        // An unknown note setting is an error, so neither note is added
        let erroneous = SHARED.replace(
            "<!--- spares: end --->",
            "<!--- # nonsense: 1 --->\n<!--- spares: note start --->\n{{ x }}\n<!--- spares: note end --->\n\n<!--- spares: end --->",
        );
        let error = import_shared(
            &pool,
            "export.md",
            erroneous.as_bytes(),
            Utc::now(),
            &parsers,
        )
        .await;
        assert!(error.is_err());
        assert_eq!(note_count(&pool).await, 0);
        let error = import_shared(&pool, "notes.txt", b"hi", Utc::now(), &parsers)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("No parser reads notes.txt"),
            "{error}"
        );
    }

    #[test]
    fn test_bundle_round_trips() {
        let export = ExportNotesResponse {
            files: HashMap::from([("md".to_string(), "notes".to_string())]),
            media: vec![SharedMedia {
                kind: MediaKind::ImageOcclusion,
                name: "a-0123456789ab.png".to_string(),
                contents: b"png".to_vec(),
            }],
            ..ExportNotesResponse::default()
        };
        let bytes = write_bundle(&export).unwrap();
        assert!(is_zip(&bytes));
        let bundle = read_bundle(&bytes).unwrap();
        assert_eq!(
            bundle.files,
            BTreeMap::from([("export.md".to_string(), "notes".to_string())])
        );
        assert_eq!(
            bundle.media,
            HashMap::from([(
                "media/image-occlusion/a-0123456789ab.png".to_string(),
                b"png".to_vec()
            )])
        );
    }

    #[test]
    fn test_read_bundle_of_a_zipped_directory() {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in [
            ("shared/export.md", &b"notes"[..]),
            ("shared/media/audio/a-0123456789ab.mp3", b"mp3"),
            ("__MACOSX/shared/._export.md", b"junk"),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(contents).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let bundle = read_bundle(&bytes).unwrap();
        assert_eq!(bundle.files.keys().collect::<Vec<_>>(), ["export.md"]);
        assert_eq!(
            bundle.media.keys().collect::<Vec<_>>(),
            ["media/audio/a-0123456789ab.mp3"]
        );
    }
}
