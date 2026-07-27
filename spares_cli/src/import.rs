use std::collections::HashMap;
use std::fs::read_to_string;
use std::fs::write;
use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use clap::Args;
use colored::Colorize;
use indicatif::ProgressBar;
use indicatif::ProgressIterator;
use serde_json::Value;
use spares_core::Error;
use spares_core::LibraryError;
use spares_core::ParserErrorKind;
use spares_core::adapters::SrsAdapter;
use spares_core::config::SparesExternalConfig;
use spares_core::config::read_external_config;
use spares_core::model::NoteId;
use spares_core::parsers::NoteImportAction;
use spares_core::parsers::NoteSettings;
use spares_core::parsers::Parseable;
use spares_core::parsers::add_cloze_uid_to_note_data;
use spares_core::parsers::get_all_parsers;
use spares_core::parsers::get_notes;
use spares_core::parsers::remove_cloze_uid_from_note_data;

#[derive(Args, Debug)]
pub(crate) struct ImportArgs {
    // NOTE: To import to spares-local-files, refer to `spares generate`
    #[arg(short, long, default_value = "spares")]
    pub(crate) adapter: String,

    /// If this is not specified, then spares will attempt to automatically determine the parser.
    #[arg(short, long, required = false)]
    pub(crate) parser: Option<String>,

    /// Parser to convert notes to before importing
    #[arg(short, long, required = false)]
    pub(crate) to_parser: Option<String>,

    #[arg(short, long, default_value_t = false)]
    pub(crate) dry_run: bool,

    /// Strip liveness from live notes (remove `live_sync_name`, `live_block_order`,
    /// `live:NAME` tag, `card.cloze_uid`, and `id:` keys from cloze syntax) instead of
    /// importing them. The notes must already exist in the database.
    #[arg(long, default_value_t = false)]
    pub(crate) strip_liveness: bool,

    /// Input file(s)
    #[arg(required = true, value_delimiter = ' ', num_args = 1..)]
    pub(crate) files: Vec<PathBuf>,
}

fn print_notes(notes: &[(NoteSettings, Option<String>)], quiet: bool, dry_run: bool) {
    let warnings = notes
        .iter()
        .enumerate()
        .filter(|(_, (s, _))| !s.errors_and_warnings.is_empty())
        .map(|(i, (s, _))| (i, s.errors_and_warnings.clone()))
        .collect::<Vec<_>>();
    let notes_len = notes.len();
    let mut total_card_count = 0;
    for (i, (local_settings, note_data_res)) in notes.iter().enumerate() {
        if !quiet {
            let Some(note_data) = note_data_res else {
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
    }
}

type ParsedNotes = Vec<(NoteSettings, Option<String>)>;
type GuessResult<'a> = Option<(&'a dyn Parseable, ParsedNotes)>;

struct ParserAggregate<'a> {
    parser: &'a dyn Parseable,
    notes: Vec<(NoteSettings, Option<String>)>,
    update_note_ids: Vec<NoteId>,
}

fn collect_blocks<'a>(parser: &dyn Parseable, contents: &'a str) -> Result<Vec<&'a str>, Error> {
    let mut blocks = Vec::new();
    for result in parser.start_end_regex().captures_iter(contents) {
        let caps = result.map_err(|e| {
            Error::Library(LibraryError::Parser(ParserErrorKind::FailedToGuess(
                format!("regex error in collect_blocks: {e}"),
            )))
        })?;
        let block = caps
            .get(1)
            .ok_or_else(|| {
                Error::Library(LibraryError::Parser(ParserErrorKind::FailedToGuess(
                    "capture group 1 not found in start_end_regex".to_string(),
                )))
            })?
            .as_str();
        blocks.push(block);
    }
    Ok(blocks)
}

fn parse_blocks(
    parser: &dyn Parseable,
    to_parser_opt: Option<&dyn Parseable>,
    contents: &str,
    adapter: &mut dyn SrsAdapter,
    dry_run: bool,
    external_config: Option<&SparesExternalConfig>,
) -> Result<ParsedNotes, Error> {
    let mut all_notes = Vec::new();
    for block in collect_blocks(parser, contents)? {
        let notes = get_notes(
            parser,
            to_parser_opt,
            block,
            adapter,
            !dry_run,
            external_config.as_ref().map(|c| &c.overlapper),
        )?;
        all_notes.extend(notes);
    }
    Ok(all_notes)
}

fn guess_and_parse_notes<'a>(
    file_contents: &str,
    parsers_to_try: &[&'a dyn Parseable],
    to_parser_opt: Option<&dyn Parseable>,
    adapter: &mut dyn SrsAdapter,
    dry_run: bool,
    external_config: Option<&SparesExternalConfig>,
) -> Result<GuessResult<'a>, Error> {
    let mut max_parser: Option<&dyn Parseable> = None;
    let mut max_parser_all_notes = Vec::new();
    let mut max_notes_count = 0;
    for parser in parsers_to_try {
        let all_notes = parse_blocks(
            *parser,
            to_parser_opt,
            file_contents,
            adapter,
            dry_run,
            external_config,
        )?;
        if !all_notes.is_empty() {
            if max_notes_count > 0 {
                return Err(Error::Library(LibraryError::Parser(
                    ParserErrorKind::FailedToGuess(
                        "More than one parser parsed some notes from the file.".to_string(),
                    ),
                )));
            }
            max_notes_count = all_notes.len();
            max_parser = Some(*parser);
            max_parser_all_notes = all_notes;
        }
    }
    if max_notes_count == 0 {
        Ok(None)
    } else {
        let parser = max_parser.ok_or_else(|| {
            Error::Library(LibraryError::Parser(ParserErrorKind::FailedToGuess(
                "no parser matched but notes were found (internal error)".to_string(),
            )))
        })?;
        Ok(Some((parser, max_parser_all_notes)))
    }
}

/// Enriches a file's clozes with `id:` keys if any are missing.
/// Returns the enriched file content if any keys were minted, `None` otherwise.
/// When keys are minted, the enriched content is also queued for deferred file write.
fn handle_live_sync(
    parser: &dyn Parseable,
    file_path: &Path,
    file_contents: &str,
    notes: &[(NoteSettings, Option<String>)],
    dry_run: bool,
    deferred_writes: &mut Vec<(PathBuf, String)>,
) -> Result<Option<String>, Error> {
    let any_live = notes
        .iter()
        .any(|(s, _)| s.custom_data.contains_key("live_sync_name"));

    if any_live {
        let (enriched_contents, mint_map) = add_cloze_uid_to_note_data(parser, file_contents)?;
        if !mint_map.is_empty() {
            if !dry_run {
                deferred_writes.push((file_path.to_path_buf(), enriched_contents.clone()));
            }
            return Ok(Some(enriched_contents));
        }
    }
    Ok(None)
}

/// Strips liveness from live notes in a file. Removes `live_sync_name`,
/// `live_block_order`, the `live:{lsn}` tag, and `id:` keys from cloze syntax.
/// Sets the note action to `Update` so the existing live note in the DB is patched.
async fn handle_strip_liveness(
    parser: &dyn Parseable,
    file_path: &Path,
    file_contents: &str,
    notes: &[(NoteSettings, Option<String>)],
    adapter: &mut dyn SrsAdapter,
    dry_run: bool,
    deferred_writes: &mut Vec<(PathBuf, String)>,
) -> Result<Option<ParsedNotes>, Error> {
    let any_live = notes
        .iter()
        .any(|(s, _)| s.custom_data.contains_key("live_sync_name"));

    if !any_live {
        return Ok(None);
    }

    // Strip ids from the full file text
    let (stripped_contents, strip_map) = remove_cloze_uid_from_note_data(parser, file_contents)?;
    if !strip_map.is_empty() && !dry_run {
        deferred_writes.push((file_path.to_path_buf(), stripped_contents));
    }

    // Process each note
    let mut result = Vec::with_capacity(notes.len());
    let mut counters: HashMap<String, i64> = HashMap::new();
    for (settings, note_data_opt) in notes {
        let mut stripped_settings = settings.clone();

        let lsn_opt = stripped_settings
            .custom_data
            .get("live_sync_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(ref lsn) = lsn_opt {
            let bo = counters.entry(lsn.clone()).or_insert(0);
            let block_order = *bo;
            *bo += 1;

            // Find the existing live note so we can update (not re-create) it
            let existing_note_id = adapter
                .find_live_note_by_block_order(lsn, block_order)
                .await?;

            match existing_note_id {
                Some(note_id) => {
                    stripped_settings.action = NoteImportAction::Update(note_id);
                }
                None => {
                    return Err(Error::Library(LibraryError::Parser(
                        ParserErrorKind::FailedToGuess(format!(
                            "Cannot strip liveness from note at \
                             (live_sync_name: {lsn}, block_order: {block_order}): \
                             no matching note in the database. Import it first \
                             with `spares import` without `--strip-liveness`."
                        )),
                    )));
                }
            }

            // Remove the synthetic live tag
            let live_tag = format!("live:{lsn}");
            stripped_settings.tags.retain(|t| t != &live_tag);

            // Strip the live keys from custom_data
            stripped_settings.custom_data.remove("live_sync_name");
            stripped_settings.custom_data.remove("live_block_order");
        }

        // Strip id: keys only from live notes (non-live notes keep their id: keys)
        let stripped_data = if lsn_opt.is_some() {
            match note_data_opt {
                Some(data) => {
                    let (stripped, _) = remove_cloze_uid_from_note_data(parser, data)?;
                    Some(stripped)
                }
                None => None,
            }
        } else {
            note_data_opt.clone()
        };
        result.push((stripped_settings, stripped_data));
    }

    Ok(Some(result))
}

async fn aggregate_note_into_parser_group(
    mut local_settings: NoteSettings,
    note_data_res: Option<String>,
    agg: &mut ParserAggregate<'_>,
    adapter: &mut dyn SrsAdapter,
    file_counters: &mut HashMap<String, i64>,
) -> Result<(), Error> {
    if let Some(lsn) = local_settings
        .custom_data
        .get("live_sync_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        let bo = file_counters.entry(lsn.clone()).or_insert(0);
        let block_order = *bo;
        *bo += 1;

        let existing_note_id = adapter
            .find_live_note_by_block_order(&lsn, block_order)
            .await?;

        match existing_note_id {
            Some(note_id) => {
                local_settings.action = NoteImportAction::Update(note_id);
                agg.update_note_ids.push(note_id);
            }
            None => {
                local_settings.action = NoteImportAction::Add;
            }
        }

        let live_tag = format!("live:{lsn}");
        if !local_settings.tags.iter().any(|t| t == &live_tag) {
            local_settings.tags.push(live_tag);
        }

        local_settings.custom_data.insert(
            "live_block_order".to_string(),
            Value::Number(block_order.into()),
        );
    }

    agg.notes.push((local_settings, note_data_res));
    Ok(())
}

/// If `parser.is_none()`, then this function will attempt to automatically determine the parser.
#[expect(clippy::too_many_lines)]
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
    if dry_run {
        println!("{}\n", "DRY RUN".black().on_bright_yellow());
    }

    let all_parsers = get_all_parsers()
        .into_iter()
        .map(|x| x())
        .collect::<Vec<_>>();
    assert!(!all_parsers.is_empty(), "not possible by validation test");
    let parsers_to_try = if let Some(parser) = parser_opt {
        vec![parser]
    } else {
        all_parsers.iter().map(|x| x.as_ref()).collect::<Vec<_>>()
    };

    let external_config = read_external_config().ok();
    let count = file_paths.len();
    let progress_bar = if quiet {
        ProgressBar::hidden()
    } else {
        // count is the number of file paths, always fits in u64
        ProgressBar::new(count as u64)
    };

    let mut aggregates: HashMap<&str, ParserAggregate<'_>> = HashMap::new();
    let mut skipped_note_count: usize = 0;
    let mut deferred_writes: Vec<(PathBuf, String)> = Vec::new();

    for file_path in file_paths.iter().progress_with(progress_bar) {
        let file_contents = read_to_string(file_path).map_err(|e| Error::Io {
            description: format!("Failed to read {}", file_path.as_ref().display()),
            source: e,
        })?;

        let parse_result = guess_and_parse_notes(
            &file_contents,
            &parsers_to_try,
            to_parser_opt,
            adapter,
            dry_run,
            external_config.as_ref(),
        )?;

        let Some((parser, mut max_parser_all_notes)) = parse_result else {
            if parsers_to_try.len() > 1 {
                return Err(Error::Library(LibraryError::Parser(
                    ParserErrorKind::FailedToGuess("All parsers parsed 0 notes.".to_string()),
                )));
            }
            if parser_opt.is_none() {
                return Err(Error::Library(LibraryError::Parser(
                    ParserErrorKind::FailedToGuess(String::new()),
                )));
            }
            return Ok(());
        };

        if strip_liveness {
            if let Some(stripped) = handle_strip_liveness(
                parser,
                file_path.as_ref(),
                &file_contents,
                &max_parser_all_notes,
                adapter,
                dry_run,
                &mut deferred_writes,
            )
            .await?
            {
                max_parser_all_notes = stripped;
            }
        } else if let Some(enriched_contents) = handle_live_sync(
            parser,
            file_path.as_ref(),
            &file_contents,
            &max_parser_all_notes,
            dry_run,
            &mut deferred_writes,
        )? {
            // Re-parse enriched content so note data carries the same id: keys
            // as what will be written to the file (fixes double-UID-minting bug).
            let reparsed = guess_and_parse_notes(
                &enriched_contents,
                &parsers_to_try,
                to_parser_opt,
                adapter,
                dry_run,
                external_config.as_ref(),
            )?;
            if let Some((_, reparsed_notes)) = reparsed {
                max_parser_all_notes = reparsed_notes;
            }
        }

        let parser_name = parser.get_parser_name();
        let agg = aggregates
            .entry(parser_name)
            .or_insert_with(|| ParserAggregate {
                parser,
                notes: Vec::new(),
                update_note_ids: Vec::new(),
            });

        let mut file_counters: HashMap<String, i64> = HashMap::new();
        for (local_settings, note_data_res) in max_parser_all_notes {
            if note_data_res.is_none() {
                skipped_note_count += 1;
                continue;
            }
            aggregate_note_into_parser_group(
                local_settings,
                note_data_res,
                agg,
                adapter,
                &mut file_counters,
            )
            .await?;
        }
    }

    if skipped_note_count > 0 && !quiet {
        eprintln!(
            "{} {} note(s) had no parseable data and were skipped.",
            "Warning:".black().on_bright_yellow(),
            skipped_note_count
        );
    }

    let at = Utc::now();
    for (_name, mut agg) in aggregates {
        print_notes(&agg.notes, quiet, dry_run);
        let mut update_ids = std::mem::take(&mut agg.update_note_ids);
        update_ids.sort_unstable();
        update_ids.dedup();
        adapter
            .process_data(
                std::mem::take(&mut agg.notes),
                agg.parser,
                dry_run,
                quiet,
                at,
                update_ids,
            )
            .await?;
    }

    // Flush deferred file writes only after DB commits (fixes Issue #1:
    // file enriched/stripped before DB → partial inconsistency on crash).
    // The enrichment/strip is idempotent, so if we crash after this point
    // the file simply has its processed content and the DB is consistent.
    if !dry_run {
        for (file_path, contents) in &deferred_writes {
            let tmp_path = file_path.with_extension("tmp");
            write(&tmp_path, contents).map_err(|e| Error::Io {
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
    use spares_core::model::Card;
    use spares_core::schema::FilterOptions;
    use spares_core::schema::parser::CreateParserRequest;
    use sqlx::SqlitePool;

    use super::*;

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

    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn test_live_note_reimport_updates(pool: SqlitePool) {
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
}
