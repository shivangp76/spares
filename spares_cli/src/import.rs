//! `spares import`: adding and updating notes from files. The import itself, and how live notes
//! work, is in `spares_core::api::note::import`: this reads the files, uploads the media bundled
//! with shared ones, prints what the import did, and writes back the files it rewrote.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::read_to_string;
use std::fs::write;
use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use clap::Args;
use colored::Colorize;
use indicatif::ProgressBar;
use indicatif::ProgressIterator;
use itertools::Itertools;
use serde_json::Value;
use spares_core::Error;
use spares_core::LibraryError;
use spares_core::NoteErrorKind;
use spares_core::adapters::SrsAdapter;
use spares_core::api::note::import::ImportFile;
use spares_core::api::note::import::ImportOptions;
use spares_core::api::note::import::ImportOutcome;
use spares_core::api::note::import::ParserImport;
use spares_core::api::note::share_bundle::read_bundle;
use spares_core::api::note::share_bundle::refuse_note_ids;
use spares_core::api::note::shared_media::bundled_media_references;
use spares_core::parsers::Parseable;
use spares_core::parsers::get_all_parsers;

use crate::utils::with_spinner;

#[derive(Args, Debug)]
pub(crate) struct ImportArgs {
    // NOTE: To write note files from the database, refer to `spares note generate`
    #[arg(short, long, default_value = "spares")]
    pub(crate) adapter: String,

    /// If this is not specified, then spares will attempt to automatically determine the parser.
    #[arg(short, long, required = false)]
    pub(crate) parser: Option<String>,

    /// Parser to convert notes to before importing, e.g.
    /// `spares import --to-parser markdown aaa.tex aab.tex`.
    ///
    /// With notes in a git or jj repository, converting cannot be done by pushing (it shows up as a
    /// deleted note and a new one). Run this on the files in your clone instead: spares moves the
    /// notes' files to the new parser's directory and commits that, so fetch afterwards.
    #[arg(short, long, required = false)]
    pub(crate) to_parser: Option<String>,

    #[arg(short, long, default_value_t = false)]
    pub(crate) dry_run: bool,

    /// Strip liveness from live notes (remove `live_sync_name`, `live_block_order`,
    /// `live:NAME` tag, `card.cloze_uid`, and `id:` keys from cloze syntax) instead of
    /// importing them, once the notes are stable. The notes must already exist in the database.
    /// See "Live notes" in the docs of `spares_core::api::note::import`.
    #[arg(long, default_value_t = false)]
    pub(crate) strip_liveness: bool,

    /// Input file(s)
    #[arg(required = true, value_delimiter = ' ', num_args = 1..)]
    pub(crate) files: Vec<PathBuf>,
}

fn print_notes(import: &ParserImport, quiet: bool, dry_run: bool) {
    let notes = &import.notes;
    let warnings = notes
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.settings.errors_and_warnings.is_empty())
        .map(|(i, n)| (i, n.settings.errors_and_warnings.clone()))
        .collect::<Vec<_>>();
    let notes_len = notes.len();
    let mut total_card_count = 0;
    for (i, note) in notes.iter().enumerate() {
        if !quiet {
            let local_settings = &note.settings;
            let Some(note_data) = &note.data else {
                continue;
            };
            let card_count = local_settings.cards_count.unwrap();
            total_card_count += card_count;
            println!("Note {} (Card count: {})", i + 1, card_count);
            println!("Action:       {:?}", local_settings.action);
            println!("Tags:         {:?}", local_settings.tags);
            println!("Keywords:     {:?}", local_settings.keywords);
            println!("Linked Notes: {:?}", local_settings.linked_notes);
            if local_settings.is_suspended {
                println!("{}", "Will Suspend Cards".purple());
            }
            if !local_settings.custom_data.is_empty() {
                println!("Custom Data: ");
                for (key, value) in &local_settings.custom_data {
                    println!(
                        "- {}: {}",
                        key.black().on_bright_green(),
                        value.to_string().black().on_bright_green()
                    );
                }
            }
            if !local_settings.errors_and_warnings.is_empty() {
                println!("{}", "Warnings: ".black().on_bright_yellow());
                for warning in &local_settings.errors_and_warnings {
                    println!("- {:?}", warning.to_string());
                }
            }
            println!("Data: {}", note_data.green());
            println!();
        }
    }

    if !quiet {
        println!("SUMMARY");
        if dry_run {
            println!("{}\n", "DRY RUN".black().on_bright_yellow());
        }
        println!("Note Count: {}", notes_len);
        println!("Card Count: {}", total_card_count);
        if !warnings.is_empty() {
            println!("Warnings:");
            for (note_index, note_warnings) in &warnings {
                println!(
                    "- {} {}:",
                    "Note".black().on_yellow(),
                    (note_index + 1).to_string().black().on_yellow()
                );
                for warning in note_warnings {
                    println!("  - {:?}", miette::Report::new(warning.clone()));
                }
            }
        }
        if !import.deleted_live_note_ids.is_empty() {
            println!(
                "{} {} live note(s) past the end of their sync group: {}",
                if dry_run { "Would delete" } else { "Deleted" },
                import.deleted_live_note_ids.len(),
                import.deleted_live_note_ids.iter().join(", ")
            );
        }
    }
}

/// Prints what an import did, or would do in a dry run
fn print_outcome(outcome: &ImportOutcome, quiet: bool, dry_run: bool) {
    if outcome.skipped_note_count > 0 && !quiet {
        eprintln!(
            "{} {} note(s) had no parseable data and were skipped.",
            "Warning:".black().on_bright_yellow(),
            outcome.skipped_note_count
        );
    }
    for import in &outcome.parsers {
        print_notes(import, quiet, dry_run);
    }
}

/// Uploads the images, audio and image occlusions bundled with a shared file (see
/// `spares_core::api::note::shared_media`) to the host, so its notes name them there.
pub(crate) struct MediaUploader<'a> {
    pub(crate) base_url: &'a str,
    pub(crate) client: &'a reqwest::Client,
}

impl MediaUploader<'_> {
    /// `contents` of the file at `file_path`, naming the files of the bundle next to it by their
    /// paths on the host, once they are uploaded. In a dry run, nothing is uploaded and
    /// `contents` is unchanged.
    async fn upload_bundled(
        &self,
        file_path: &Path,
        contents: String,
        dry_run: bool,
    ) -> Result<String, Error> {
        let references = bundled_media_references(&contents)
            .into_iter()
            .filter(|r| r.path(file_path).is_file())
            .collect::<Vec<_>>();
        if references.is_empty() {
            return Ok(contents);
        }
        let mut uploaded: HashMap<PathBuf, String> = HashMap::new();
        for reference in &references {
            let path = reference.path(file_path);
            if dry_run || uploaded.contains_key(&path) {
                continue;
            }
            let body = std::fs::read(&path).map_err(|e| Error::Io {
                description: format!("Failed to read {}", path.display()),
                source: e,
            })?;
            let url = format!(
                "{}/api/sync/media/{}/{}",
                self.base_url,
                reference.kind.as_str(),
                reference.name
            );
            let response = self.client.put(url).body(body).send().await?;
            let status = response.status();
            let answer: Value = response.json().await?;
            let host_path = answer.get("path").and_then(Value::as_str);
            let Some(host_path) = host_path.filter(|_| status.is_success()) else {
                let message = answer
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| status.to_string(), ToString::to_string);
                return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
                    description: format!("Failed to upload {}: {message}", path.display()),
                })));
            };
            uploaded.insert(path, host_path.to_string());
        }
        if dry_run {
            println!(
                "Would upload {} images, audio clips and image occlusion files",
                references
                    .iter()
                    .map(|r| r.path(file_path))
                    .collect::<HashSet<_>>()
                    .len()
            );
            return Ok(contents);
        }
        let mut result = String::with_capacity(contents.len());
        let mut last = 0;
        for reference in &references {
            result.push_str(&contents[last..reference.range.start]);
            result.push_str(&uploaded[&reference.path(file_path)]);
            last = reference.range.end;
        }
        result.push_str(&contents[last..]);
        Ok(result)
    }
}

/// `files`, with each zip of shared notes (`spares note export --share --zip`) unpacked into a
/// directory of its own under `dir`, as `--share` without `--zip` writes them: its files of notes
/// in place of the zip, next to their `media`.
///
/// `dir` is created, and must not be there yet, so no one else can have put anything in it: on
/// Unix, only this user can read or write it. A zip whose files name notes by id is refused (see
/// [`refuse_note_ids`]), so a shared zip only ever adds notes.
pub(crate) fn unpack_bundles(files: &[PathBuf], dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut unpacked = Vec::new();
    let mut created = false;
    for (i, file) in files.iter().enumerate() {
        if file
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("zip"))
        {
            unpacked.push(file.clone());
            continue;
        }
        let io = |path: &Path, e| Error::Io {
            description: format!("Failed to unpack {}", path.display()),
            source: e,
        };
        let bytes = std::fs::read(file).map_err(|e| io(file, e))?;
        let bundle = read_bundle(&bytes)?;
        refuse_note_ids(&bundle, &get_all_parsers())?;
        if !created {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(dir).map_err(|e| io(dir, e))?;
            created = true;
        }
        let out = dir.join(i.to_string());
        let write = |name: &str, contents: &[u8]| -> Result<PathBuf, Error> {
            let path = out.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
            }
            std::fs::write(&path, contents).map_err(|e| io(&path, e))?;
            Ok(path)
        };
        for (name, contents) in &bundle.media {
            write(name, contents)?;
        }
        for (name, contents) in &bundle.files {
            unpacked.push(write(name, contents.as_bytes())?);
        }
    }
    Ok(unpacked)
}

/// If `parser.is_none()`, then this function will attempt to automatically determine the parser.
pub(crate) async fn import_from_files<P>(
    adapter: &mut dyn SrsAdapter,
    parser_opt: Option<&dyn Parseable>,
    to_parser_opt: Option<&dyn Parseable>,
    file_paths: &[P],
    dry_run: bool,
    quiet: bool,
    strip_liveness: bool,
) -> Result<(), Error>
where
    P: AsRef<Path>,
{
    import_files(
        adapter,
        parser_opt,
        to_parser_opt,
        file_paths,
        dry_run,
        quiet,
        strip_liveness,
        None,
    )
    .await
}

/// [`import_from_files`], uploading the files bundled with shared files with `media_uploader`.
#[expect(clippy::too_many_arguments)]
pub(crate) async fn import_files<P>(
    adapter: &mut dyn SrsAdapter,
    parser_opt: Option<&dyn Parseable>,
    to_parser_opt: Option<&dyn Parseable>,
    file_paths: &[P],
    dry_run: bool,
    quiet: bool,
    strip_liveness: bool,
    media_uploader: Option<&MediaUploader<'_>>,
) -> Result<(), Error>
where
    P: AsRef<Path>,
{
    if dry_run {
        println!("{}\n", "DRY RUN".black().on_bright_yellow());
    }

    let progress_bar = if quiet {
        ProgressBar::hidden()
    } else {
        // count is the number of file paths, always fits in u64
        ProgressBar::new(file_paths.len() as u64)
    };
    let mut files = Vec::with_capacity(file_paths.len());
    for file_path in file_paths.iter().progress_with(progress_bar) {
        let file_path = file_path.as_ref();
        let contents = read_to_string(file_path).map_err(|e| Error::Io {
            description: format!("Failed to read {}", file_path.display()),
            source: e,
        })?;
        let contents = match media_uploader {
            Some(uploader) => {
                uploader
                    .upload_bundled(file_path, contents, dry_run)
                    .await?
            }
            None => contents,
        };
        files.push(ImportFile {
            name: file_path.display().to_string(),
            contents,
        });
    }

    let options = ImportOptions {
        parser: parser_opt,
        to_parser: to_parser_opt,
        dry_run,
        strip_liveness,
    };
    let import = spares_core::api::note::import::import_files(adapter, &files, options, Utc::now());
    let outcome = if quiet {
        import.await?
    } else {
        with_spinner("Importing notes", import).await?
    };
    print_outcome(&outcome, quiet, dry_run);

    // Written only after the database changed, so a crash in between leaves the file as it was
    // and the database consistent. Rewriting is idempotent, so the next import does it again.
    if !dry_run {
        for rewritten in &outcome.rewritten {
            let file_path = file_paths[rewritten.file].as_ref();
            let tmp_path = file_path.with_extension("tmp");
            write(&tmp_path, &rewritten.contents).map_err(|e| Error::Io {
                description: format!("Failed to write {}", tmp_path.display()),
                source: e,
            })?;
            std::fs::rename(&tmp_path, file_path).map_err(|e| Error::Io {
                description: format!(
                    "Failed to rename {} to {}",
                    tmp_path.display(),
                    file_path.display()
                ),
                source: e,
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use spares_core::adapters::impls::spares::SparesAdapter;
    use spares_core::adapters::impls::spares::SparesRequestProcessor;
    use spares_core::api::note::list_notes;
    use spares_core::api::parser::create_parser;
    use spares_core::api::undo::redo_event;
    use spares_core::api::undo::undo_event;
    use spares_core::model::Card;
    use spares_core::schema::FilterOptions;
    use spares_core::schema::parser::CreateParserRequest;
    use spares_core::schema::undo::RedoEventRequest;
    use spares_core::schema::undo::UndoEventRequest;
    use sqlx::SqlitePool;

    use super::*;

    /// `spares_core` is compiled as a normal (non-test) dependency of this test binary, so its
    /// own `cfg!(test)` checks are `false` here. Without this, note-file generation writes to the
    /// user's real config/cache/data directories instead of test-local temp dirs. See
    /// `spares_core::config::is_test_mode`.
    fn enable_test_mode() {
        // SAFETY: `cargo test` runs tests concurrently in one process, so other tests may read
        // this env var while it's being set. That's benign here: every value ever written is "1",
        // and any test observing it only becomes more isolated from the user's real directories,
        // never less.
        unsafe {
            std::env::set_var("SPARES_TEST_MODE", "1");
        }
    }

    fn test_file_content() -> &'static str {
        "<!--- spares: start --->\n\
         <!--- # live-sync-name: lecture_notes_501 --->\n\
         <!--- spares: note start --->\n\
         This is a live note.\n\
         <!--- spares: note end --->\n\
         <!--- spares: note start --->\n\
         This is a normal note.\n\
         <!--- spares: note end --->\n\
         <!--- spares: end --->\n"
    }

    fn test_file_two_syncs() -> &'static str {
        "<!--- spares: start --->\n\
         <!--- # live-sync-name: a --->\n\
         <!--- spares: note start --->\n\
         Live note sync a.\n\
         <!--- spares: note end --->\n\
         <!--- # live-sync-name: b --->\n\
         <!--- spares: note start --->\n\
         Live note sync b.\n\
         <!--- spares: note end --->\n\
         <!--- spares: note start --->\n\
         Normal note.\n\
         <!--- spares: note end --->\n\
         <!--- spares: end --->\n"
    }

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_mixed_live_and_normal_notes(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir = std::env::temp_dir().join(format!("spares_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test_notes.md");
        std::fs::write(&file_path, test_file_content()).unwrap();

        let paths = [file_path.as_path()];

        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        let notes = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();

        assert_eq!(notes.len(), 2, "expected 2 notes in DB");
        // The import can be undone, as one through the server can
        let create_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event WHERE kind = ?")
            .bind(spares_core::model::EventType::CreateNotes)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(create_events, 1);

        let live_note = notes
            .iter()
            .find(|n| {
                n.custom_data.get("live_sync_name").and_then(|v| v.as_str())
                    == Some("lecture_notes_501")
            })
            .expect("expected a live note");

        assert_eq!(
            live_note
                .custom_data
                .get("live_block_order")
                .and_then(|v| v.as_i64()),
            Some(0),
            "live note should have block_order 0"
        );

        // Verify the live note has the live tag set on its custom_data
        // (tags are pushed into local_settings.tags, which process_data stores
        // via the update_tags pathway; we verify indirectly that the metadata is correct)

        let normal_note = notes
            .iter()
            .find(|n| n.custom_data.get("live_sync_name").is_none())
            .expect("expected a normal note (no live_sync_name)");

        assert!(
            normal_note.custom_data.get("live_block_order").is_none(),
            "normal note should not have live_block_order"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_two_live_syncs_in_one_file(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir =
            std::env::temp_dir().join(format!("spares_test_two_syncs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test_two_syncs.md");
        std::fs::write(&file_path, test_file_two_syncs()).unwrap();

        let paths = [file_path.as_path()];

        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        let notes = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();

        assert_eq!(notes.len(), 3, "expected 3 notes in DB");

        let sync_a = notes
            .iter()
            .find(|n| n.custom_data.get("live_sync_name").and_then(|v| v.as_str()) == Some("a"))
            .expect("expected note with live_sync_name 'a'");

        let sync_b = notes
            .iter()
            .find(|n| n.custom_data.get("live_sync_name").and_then(|v| v.as_str()) == Some("b"))
            .expect("expected note with live_sync_name 'b'");

        assert_eq!(
            sync_a
                .custom_data
                .get("live_block_order")
                .and_then(|v| v.as_i64()),
            Some(0),
            "sync 'a' note should have block_order 0"
        );
        assert_eq!(
            sync_b
                .custom_data
                .get("live_block_order")
                .and_then(|v| v.as_i64()),
            Some(0),
            "sync 'b' note should have block_order 0 (separate counter)"
        );

        assert!(
            notes
                .iter()
                .any(|n| n.custom_data.get("live_sync_name").is_none()),
            "expected a normal note without live_sync_name"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_non_live_file_not_written(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let content = "<!--- spares: start --->\n\
                       <!--- spares: note start --->\n\
                       A normal note without any live settings.\n\
                       <!--- spares: note end --->\n\
                       <!--- spares: end --->\n";

        let dir = std::env::temp_dir().join(format!("spares_test_non_live_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("normal_notes.md");
        std::fs::write(&file_path, content).unwrap();

        let paths = [file_path.as_path()];

        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        let notes = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();

        assert_eq!(notes.len(), 1, "expected 1 normal note in DB");

        let note = &notes[0];
        assert!(
            note.custom_data.get("live_sync_name").is_none(),
            "normal note should not have live_sync_name"
        );
        assert!(
            note.custom_data.get("live_block_order").is_none(),
            "normal note should not have live_block_order"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn live_only_file_content() -> &'static str {
        "<!--- spares: start --->\n\
         <!--- # live-sync-name: lecture_notes_501 --->\n\
         <!--- spares: note start --->\n\
         This is a live note.\n\
         <!--- spares: note end --->\n\
         <!--- spares: end --->\n"
    }

    /// A live note imported without any clozes has no cards. Adding its first cloze must create a
    /// card; the importer's positional `o:1` must not be read as a reference to an old card.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_live_note_reimport_first_cloze(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });
        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();
        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();
        let dir =
            std::env::temp_dir().join(format!("spares_test_first_cloze_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("live.md");
        std::fs::write(&file_path, live_only_file_content()).unwrap();
        let paths = [file_path.as_path()];
        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();
        std::fs::write(
            &file_path,
            live_only_file_content().replace("a live note", "a {{ live }} note"),
        )
        .unwrap();
        let res = import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await;
        let _ = std::fs::remove_dir_all(&dir);
        res.unwrap();
    }

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_live_note_reimport_updates(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir = std::env::temp_dir().join(format!("spares_test_reimport_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("live_reimport.md");
        // Use a file with only a live note (no normal note) so re-import doesn't duplicate
        std::fs::write(&file_path, live_only_file_content()).unwrap();

        let paths = [file_path.as_path()];

        // First import: live note is created as Add
        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        let notes_after_first = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            notes_after_first.len(),
            1,
            "expected 1 note after first import"
        );

        // Second import: should match existing live note by (live_sync_name, block_order) and Update it
        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        let notes_after_second = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();
        // Still 1 live note (updated, not duplicated)
        assert_eq!(
            notes_after_second.len(),
            1,
            "expected 1 note after re-import (no duplicates)"
        );

        // The live note should still have the same live_sync_name and block_order
        let live_note = &notes_after_second[0];
        assert_eq!(
            live_note
                .custom_data
                .get("live_sync_name")
                .and_then(|v| v.as_str()),
            Some("lecture_notes_501")
        );
        assert_eq!(
            live_note
                .custom_data
                .get("live_block_order")
                .and_then(|v| v.as_i64()),
            Some(0),
            "live note should still have block_order 0 after re-import"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_file_three_live_notes() -> &'static str {
        "<!--- spares: start --->\n\
         <!--- # g-live-sync-name: per_note_sync --->\n\
         <!--- spares: note start --->\n\
         Block zero {{ alpha }}\n\
         <!--- spares: note end --->\n\
         <!--- spares: note start --->\n\
         Block one {{ beta }}\n\
         <!--- spares: note end --->\n\
         <!--- spares: note start --->\n\
         Block two {{ gamma }} and {{ delta }} and {{ epsilon }}\n\
         <!--- spares: note end --->\n\
         <!--- spares: end --->\n"
    }

    /// A live note can only be imported from its source file: an edit made anywhere else would be
    /// overwritten by the next import of the source file.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_live_note_only_imported_from_source_file(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir = std::env::temp_dir().join(format!("spares_test_per_note_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let live_file = dir.join("live_source.md");
        std::fs::write(&live_file, test_file_three_live_notes()).unwrap();

        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &[live_file.as_path()],
            false,
            true,
            false,
        )
        .await
        .unwrap();

        let list = || {
            list_notes(
                &pool,
                FilterOptions {
                    page: Some(1),
                    limit: Some(9999),
                },
            )
        };
        let notes_before = list().await.unwrap();
        assert_eq!(
            notes_before.len(),
            3,
            "expected 3 live notes after first import"
        );
        let block_two = notes_before
            .iter()
            .find(|n| {
                n.custom_data
                    .get("live_block_order")
                    .and_then(|v| v.as_i64())
                    == Some(2)
            })
            .expect("expected a note at block_order 2");
        let id = block_two.id.letters();

        let note = "<!--- spares: note start --->\n\
                    Block two edited {{ gamma }} and {{ delta }} and {{ epsilon }}\n\
                    <!--- spares: note end --->\n";
        let cases = [
            (
                "a per-note file, as `spares note generate` writes it",
                format!(
                    "<!--- # note-id: {id} --->\n\
                     <!--- # custom-data: {{\"live_sync_name\":\"per_note_sync\",\"live_block_order\":2}} --->\n"
                ),
            ),
            (
                "a file naming the note without its live custom data",
                format!("<!--- # note-id: {id} --->\n"),
            ),
            (
                "a file naming the note's position without its id",
                "<!--- # custom-data: {\"live_sync_name\":\"per_note_sync\",\"live_block_order\":2} --->\n"
                    .to_string(),
            ),
        ];
        for (case, settings) in cases {
            let file = dir.join("not_the_source.md");
            std::fs::write(
                &file,
                format!("<!--- spares: start --->\n{settings}{note}<!--- spares: end --->\n"),
            )
            .unwrap();
            let result = import_from_files(
                &mut adapter,
                Some(markdown.as_ref()),
                None,
                &[file.as_path()],
                false,
                true,
                false,
            )
            .await;
            let err = result.expect_err(case).to_string();
            assert!(
                err.contains("can only be imported from its source file"),
                "{case}: {err}"
            );
            let notes_after = list().await.unwrap();
            assert_eq!(
                notes_after
                    .iter()
                    .map(|n| (n.id, &n.data))
                    .collect::<Vec<_>>(),
                notes_before
                    .iter()
                    .map(|n| (n.id, &n.data))
                    .collect::<Vec<_>>(),
                "{case}: no note may change"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--strip-liveness` on a per-note file strips the note the file names. The position counter
    /// restarts at 0 for every file, so before this was fixed it stripped block 0 of the sync group
    /// instead, and overwrote that note with the file's data.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_strip_liveness_from_per_note_file(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir =
            std::env::temp_dir().join(format!("spares_test_strip_per_note_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let live_file = dir.join("live_source.md");
        std::fs::write(&live_file, test_file_three_live_notes()).unwrap();
        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &[live_file.as_path()],
            false,
            true,
            false,
        )
        .await
        .unwrap();

        let list = || {
            list_notes(
                &pool,
                FilterOptions {
                    page: Some(1),
                    limit: Some(9999),
                },
            )
        };
        let notes_before = list().await.unwrap();
        let note_at = |block_order: i64| {
            notes_before
                .iter()
                .find(|n| {
                    n.custom_data
                        .get("live_block_order")
                        .and_then(|v| v.as_i64())
                        == Some(block_order)
                })
                .unwrap_or_else(|| panic!("expected a note at block_order {block_order}"))
                .clone()
        };
        let block_zero = note_at(0);
        let block_two = note_at(2);

        let import_strip = |file: PathBuf| {
            let mut adapter =
                SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });
            async move {
                import_from_files(
                    &mut adapter,
                    Some(markdown.as_ref()),
                    None,
                    &[file.as_path()],
                    false,
                    true,
                    true,
                )
                .await
            }
        };
        let note = "<!--- spares: note start --->\n\
                    Block two {{ gamma }} and {{ delta }} and {{ epsilon }}\n\
                    <!--- spares: note end --->\n";

        // Naming a live note without its live custom data is not a strip, so it is still refused.
        let unnamed_live = dir.join("unnamed_live.md");
        std::fs::write(
            &unnamed_live,
            format!(
                "<!--- spares: start --->\n<!--- # note-id: {} --->\n{note}<!--- spares: end --->\n",
                block_two.id.letters()
            ),
        )
        .unwrap();
        let err = import_strip(unnamed_live)
            .await
            .expect_err("was accepted")
            .to_string();
        assert!(
            err.contains("can only be imported from its source file"),
            "{err}"
        );

        // A per-note file as `spares note generate` writes it.
        let per_note_file = dir.join("block_two.md");
        std::fs::write(
            &per_note_file,
            format!(
                "<!--- spares: start --->\n\
                 <!--- # note-id: {} --->\n\
                 <!--- # tags: live:per_note_sync --->\n\
                 <!--- # custom-data: {{\"live_sync_name\":\"per_note_sync\",\"live_block_order\":2}} --->\n\
                 {note}<!--- spares: end --->\n",
                block_two.id.letters()
            ),
        )
        .unwrap();
        import_strip(per_note_file).await.unwrap();

        let notes_after = list().await.unwrap();
        assert_eq!(notes_after.len(), 3, "stripping must not create a note");
        let after = |id| notes_after.iter().find(|n| n.id == id).unwrap();
        let block_two_after = after(block_two.id);
        assert!(
            block_two_after.custom_data.get("live_sync_name").is_none(),
            "the named note should have been stripped: {:?}",
            block_two_after.custom_data
        );
        assert_eq!(
            block_two_after.data,
            "Block two {{[o:1] gamma }} and {{[o:2] delta }} and {{[o:3] epsilon }}",
            "the named note should hold the file's data, without `id:` keys"
        );
        let block_zero_after = after(block_zero.id);
        assert_eq!(
            (&block_zero_after.data, &block_zero_after.custom_data),
            (&block_zero.data, &block_zero.custom_data),
            "block 0 of the sync group must be left alone"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_file_with_clozes() -> &'static str {
        "<!--- spares: start --->\n\
         <!--- # live-sync-name: cloze_sync --->\n\
         <!--- spares: note start --->\n\
         pre {{[o:1] A B }} C {{[g:1] D E }} post\n\
         <!--- spares: note end --->\n\
         <!--- spares: end --->\n"
    }

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_live_note_import_preserves_inter_cloze_content(pool: SqlitePool) {
        enable_test_mode();

        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir = std::env::temp_dir().join(format!("spares_test_clozes_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test_clozes.md");
        std::fs::write(&file_path, test_file_with_clozes()).unwrap();

        let paths = [file_path.as_path()];

        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        // Read file back from disk to check preserved content
        let on_disk = std::fs::read_to_string(&file_path).unwrap();

        // Every fragment of surrounding/inter-cloze text must survive
        assert!(on_disk.contains("pre"), "missing 'pre'");
        assert!(on_disk.contains("A B"), "missing 'A B'");
        assert!(on_disk.contains(" C "), "missing ' C '");
        assert!(on_disk.contains("D E"), "missing 'D E'");
        assert!(on_disk.contains(" post"), "missing ' post'");

        // Both clozes got an id: key
        assert_eq!(
            on_disk.matches("id:").count(),
            2,
            "expected 2 id: keys, got content: {on_disk}"
        );

        // Relative ordering is preserved
        let pre_pos = on_disk.find("pre").unwrap();
        let ab_pos = on_disk.find("A B").unwrap();
        let c_pos = on_disk.find(" C ").unwrap();
        let de_pos = on_disk.find("D E").unwrap();
        assert!(pre_pos < ab_pos, "'pre' should come before 'A B'");
        assert!(ab_pos < c_pos, "'A B' should come before ' C '");
        assert!(c_pos < de_pos, "' C ' should come before 'D E'");

        // DB sanity check: note was actually created
        let notes = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();

        assert_eq!(notes.len(), 1, "expected 1 note in DB");
        let live_note = notes
            .iter()
            .find(|n| {
                n.custom_data.get("live_sync_name").and_then(|v| v.as_str()) == Some("cloze_sync")
            })
            .expect("expected a live note with sync name 'cloze_sync'");
        assert_eq!(
            live_note
                .custom_data
                .get("live_block_order")
                .and_then(|v| v.as_i64()),
            Some(0)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_find_live_note_by_block_order_database(pool: SqlitePool) {
        use spares_core::adapters::SrsAdapter;
        use spares_core::adapters::impls::spares::SparesAdapter;
        use spares_core::adapters::impls::spares::SparesRequestProcessor;
        use spares_core::api::parser::create_parser;
        use spares_core::parsers::get_all_parsers;
        use spares_core::schema::parser::CreateParserRequest;

        enable_test_mode();

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir =
            std::env::temp_dir().join(format!("spares_test_adapter_live_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("live_adapter_test.md");
        std::fs::write(&file_path, live_only_file_content()).unwrap();

        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &[file_path.as_path()],
            false,
            true,
            false,
        )
        .await
        .unwrap();

        // Look up the imported live note
        let found = adapter
            .find_live_note_by_block_order("lecture_notes_501", 0)
            .await
            .unwrap();
        assert!(
            found.is_some(),
            "expected to find the imported live note by (live_sync_name, block_order)"
        );

        // Wrong block_order -> None
        assert_eq!(
            adapter
                .find_live_note_by_block_order("lecture_notes_501", 1)
                .await
                .unwrap(),
            None
        );

        // Wrong live_sync_name -> None
        assert_eq!(
            adapter
                .find_live_note_by_block_order("nonexistent", 0)
                .await
                .unwrap(),
            None
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn strip_test_file_content() -> &'static str {
        "<!--- spares: start --->\n\
         <!--- # live-sync-name: strip_test --->\n\
         <!--- spares: note start --->\n\
         pre {{[o:1] A B }} C {{[g:1] D E }} post\n\
         <!--- spares: note end --->\n\
         <!--- spares: end --->\n"
    }

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_strip_liveness_removes_all_live_fields(pool: SqlitePool) {
        enable_test_mode();
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });

        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();

        let all_parsers = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .collect::<Vec<_>>();
        let markdown = all_parsers
            .iter()
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();

        let dir = std::env::temp_dir().join(format!("spares_test_strip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("strip_test.md");
        std::fs::write(&file_path, strip_test_file_content()).unwrap();

        let paths = [file_path.as_path()];

        // First: import normally (adds liveness, mints id: keys)
        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        // Verify DB has live note with live fields
        let notes = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();
        assert_eq!(notes.len(), 1, "expected 1 note after first import");
        let note = &notes[0];
        assert_eq!(
            note.custom_data
                .get("live_sync_name")
                .and_then(|v| v.as_str()),
            Some("strip_test")
        );
        assert_eq!(
            note.custom_data
                .get("live_block_order")
                .and_then(|v| v.as_i64()),
            Some(0)
        );

        // Source file should now have id: keys
        let on_disk = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            on_disk.contains("id:"),
            "expected id: keys after first import"
        );

        // Card should have cloze_uid in custom_data
        let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(note.id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(cards.len(), 2, "expected 2 cards");
        for card in &cards {
            assert!(
                card.custom_data.get("cloze_uid").is_some(),
                "expected cloze_uid on card {}",
                card.id
            );
        }

        // Second: import with strip_liveness = true
        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &paths,
            false,
            true,
            true,
        )
        .await
        .unwrap();

        // Verify: no note has live_sync_name
        let notes_after = list_notes(
            &pool,
            FilterOptions {
                page: Some(1),
                limit: Some(9999),
            },
        )
        .await
        .unwrap();
        assert_eq!(notes_after.len(), 1, "expected still 1 note after strip");
        let note_sync_name = notes_after
            .iter()
            .find(|n| n.custom_data.get("live_sync_name").is_some());
        assert!(
            note_sync_name.is_none(),
            "no note should have live_sync_name after strip"
        );
        let note_block_order = notes_after
            .iter()
            .find(|n| n.custom_data.get("live_block_order").is_some());
        assert!(
            note_block_order.is_none(),
            "no note should have live_block_order after strip"
        );

        // Verify: cards no longer have cloze_uid
        let cards_after: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(notes_after[0].id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(cards_after.len(), 2, "expected still 2 cards");
        for card in &cards_after {
            assert!(
                card.custom_data.get("cloze_uid").is_none(),
                "expected no cloze_uid after strip on card {}",
                card.id
            );
        }

        // Source file should no longer have id: keys
        let on_disk = std::fs::read_to_string(&file_path).unwrap();
        assert!(!on_disk.contains("id:"), "expected no id: keys after strip");

        // Surrounding content must be preserved
        assert!(on_disk.contains("pre"), "missing 'pre'");
        assert!(on_disk.contains("A B"), "missing 'A B'");
        assert!(on_disk.contains(" C "), "missing ' C '");
        assert!(on_disk.contains("D E"), "missing 'D E'");
        assert!(on_disk.contains(" post"), "missing ' post'");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn live_file(notes: &[&str]) -> String {
        let mut file =
            "<!--- spares: start --->\n<!--- # g-live-sync-name: moving --->\n".to_string();
        for note in notes {
            file.push_str("<!--- spares: note start --->\n");
            file.push_str(note);
            file.push_str("\n<!--- spares: note end --->\n");
        }
        file.push_str("<!--- spares: end --->\n");
        file
    }

    async fn import_live_file(pool: &SqlitePool, file_path: &Path) -> Result<(), Error> {
        let mut adapter =
            SparesAdapter::new(SparesRequestProcessor::Database { pool: pool.clone() });
        let markdown = get_all_parsers()
            .into_iter()
            .map(|x| x())
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();
        import_from_files(
            &mut adapter,
            Some(markdown.as_ref()),
            None,
            &[file_path],
            false,
            true,
            false,
        )
        .await
    }

    /// The live notes' cards by cloze uid: (card id, note's block order, card order, stability).
    async fn live_cards_by_uid(pool: &SqlitePool) -> HashMap<String, (i64, i64, u32, f64)> {
        let rows: Vec<(String, i64, i64, u32, f64)> = sqlx::query_as(
            r#"SELECT json_extract(c.custom_data, '$.cloze_uid'), c.id,
                      json_extract(n.custom_data, '$.live_block_order'), c."order", c.stability
               FROM card c JOIN note n ON n.id = c.note_id"#,
        )
        .fetch_all(pool)
        .await
        .unwrap();
        let count = rows.len();
        let cards: HashMap<_, _> = rows
            .into_iter()
            .map(|(uid, id, block_order, order, stability)| {
                (uid, (id, block_order, order, stability))
            })
            .collect();
        assert_eq!(cards.len(), count, "a cloze uid has more than one card");
        cards
    }

    /// Inserting, splitting, or deleting a live note moves the clozes after it into other notes of
    /// the sync group, since notes are matched by position. Their cards move along, keeping their
    /// review history, and notes left past the end of the group are deleted.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_live_cards_follow_their_clozes_to_other_notes(pool: SqlitePool) {
        enable_test_mode();
        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();
        let dir = std::env::temp_dir().join(format!("spares_test_moving_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("moving.md");

        std::fs::write(&file_path, live_file(&["A {{ a }}", "B {{ b1 }} {{ b2 }}"])).unwrap();
        import_live_file(&pool, &file_path).await.unwrap();
        let uids = std::fs::read_to_string(&file_path)
            .unwrap()
            .split("[id:")
            .skip(1)
            .map(|rest| rest[..12].to_string())
            .collect::<Vec<_>>();
        let [a, b1, b2] = uids.as_slice() else {
            panic!("expected 3 minted uids, got {uids:?}");
        };
        // Stand-in for review history
        sqlx::query("UPDATE card SET stability = id")
            .execute(&pool)
            .await
            .unwrap();
        let before = live_cards_by_uid(&pool).await;
        let card_id = |uid: &String| before[uid].0;

        // Insert a note with a new cloze before B, and split B in two: B's clozes move to notes
        // the import creates.
        std::fs::write(
            &file_path,
            live_file(&[
                &format!("A {{{{[id:{a}] a }}}}"),
                "N {{ n }}",
                &format!("B {{{{[id:{b1}] b1 }}}}"),
                &format!("C {{{{[id:{b2}] b2 }}}}"),
            ]),
        )
        .unwrap();
        import_live_file(&pool, &file_path).await.unwrap();
        let after = live_cards_by_uid(&pool).await;
        assert_eq!(after.len(), 4, "{after:?}");
        for (uid, block_order) in [(a, 0), (b1, 2), (b2, 3)] {
            let (id, actual_block_order, order, stability) = after[uid];
            assert_eq!(id, card_id(uid), "card of {uid} was recreated");
            assert_eq!(stability, before[uid].3, "card of {uid} lost its history");
            assert_eq!(
                (actual_block_order, order),
                (block_order, 1),
                "card of {uid}"
            );
        }

        // The import is one undo group, which undo reverts to exactly what was there
        let note_count = async || -> i64 {
            sqlx::query_scalar("SELECT COUNT(*) FROM note")
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        assert_eq!(note_count().await, 4);
        let undone = undo_event(
            &pool,
            UndoEventRequest {
                event_id: None,
                undo_group: true,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            undone
                .undone_events
                .iter()
                .any(|e| e.description.starts_with("Move")),
            "{:?}",
            undone.undone_events
        );
        assert_eq!(note_count().await, 2);
        assert_eq!(live_cards_by_uid(&pool).await, before);
        redo_event(
            &pool,
            RedoEventRequest {
                event_id: None,
                redo_group: true,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(live_cards_by_uid(&pool).await, after);

        // Delete N: the clozes after it move up into existing notes, and the last note is left
        // past the end of the group.
        std::fs::write(
            &file_path,
            live_file(&[
                &format!("A {{{{[id:{a}] a }}}}"),
                &format!("B {{{{[id:{b1}] b1 }}}}"),
                &format!("C {{{{[id:{b2}] b2 }}}}"),
            ]),
        )
        .unwrap();
        import_live_file(&pool, &file_path).await.unwrap();
        let after = live_cards_by_uid(&pool).await;
        assert_eq!(after.len(), 3, "{after:?}");
        for (uid, block_order) in [(a, 0), (b1, 1), (b2, 2)] {
            let (id, actual_block_order, order, stability) = after[uid];
            assert_eq!(id, card_id(uid), "card of {uid} was recreated");
            assert_eq!(stability, before[uid].3, "card of {uid} lost its history");
            assert_eq!(
                (actual_block_order, order),
                (block_order, 1),
                "card of {uid}"
            );
        }
        let block_orders: Vec<i64> = sqlx::query_scalar(
            r"SELECT json_extract(custom_data, '$.live_block_order') FROM note ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(block_orders, [0, 1, 2]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Skipping a live note that cannot be parsed would shift every later note of its sync group
    /// onto the database note before it, so the import is refused instead.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_unparseable_live_note_is_refused(pool: SqlitePool) {
        enable_test_mode();
        create_parser(
            &pool,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            true,
        )
        .await
        .unwrap();
        let dir =
            std::env::temp_dir().join(format!("spares_test_unparseable_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("unparseable.md");
        std::fs::write(&file_path, live_file(&["A {{ a }}", "B {{}}", "C {{ c }}"])).unwrap();

        let error = import_live_file(&pool, &file_path).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("block 1 of live-sync-name `moving`"),
            "{error}"
        );
        let note_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(note_count, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A zip is unpacked into a directory made for it, which no one else can have filled, and one
    /// whose notes are named by id is refused.
    #[test]
    fn test_unpack_bundles_into_a_new_private_directory() {
        use spares_core::api::note::share_bundle::write_bundle;
        use spares_core::schema::note::ExportNotesResponse;

        let base = std::env::temp_dir().join(format!("spares-unpack-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let shared = "<!--- spares: start --->\n\n<!--- # action: add --->\n<!--- spares: note start --->\n{{ Paris }} is the capital\n<!--- spares: note end --->\n\n<!--- spares: end --->\n";
        let zip_of = |text: &str, name: &str| {
            let export = ExportNotesResponse {
                files: std::collections::HashMap::from([("md".to_string(), text.to_string())]),
                ..ExportNotesResponse::default()
            };
            let path = base.join(name);
            std::fs::write(&path, write_bundle(&export).unwrap()).unwrap();
            path
        };
        let zip = zip_of(shared, "shared.zip");

        let dir = base.join("unpacked");
        let files = unpack_bundles(std::slice::from_ref(&zip), &dir).unwrap();
        assert_eq!(files, [dir.join("0").join("export.md")]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        // Not into a directory that is already there
        assert!(unpack_bundles(std::slice::from_ref(&zip), &dir).is_err());

        let by_id = zip_of(
            &shared.replace("# action: add", "# note-id: aaaa"),
            "by_id.zip",
        );
        let error = unpack_bundles(&[by_id], &base.join("by_id")).unwrap_err();
        assert!(
            error.to_string().contains("names notes by their ids"),
            "{error}"
        );
        assert!(!base.join("by_id").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
