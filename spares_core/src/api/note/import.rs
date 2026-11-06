//! Importing files of notes into the collection, adding, updating and deleting notes as the files
//! say: `spares import`, and the web app's import (`POST /api/notes/import`). Unlike an
//! [import of shared notes](super::share_bundle::import_shared), which only adds notes, a file
//! can name notes by id, as an export to edit them does, and can hold live notes.
//!
//! [`import_files`] parses the files and sends the notes to an [`SrsAdapter`]: the CLI's sends them
//! to the host, and the server's writes them to its database. It does not write the files: a file
//! whose text the import changed (live notes' clozes given `id:` keys, or stripped of them) is
//! returned in [`ImportOutcome::rewritten`], for the CLI to write back and the web app to offer
//! for download.
//!
//! # Live notes
//!
//! Live notes let you write notes in a source file and keep them in sync with the database across
//! repeated imports. Each "live sync group" is identified by a `live-sync-name` set in the file's
//! global settings. Notes with the same `live-sync-name` are matched to the database by their
//! position in the file (block 0, block 1, …), so edits to existing notes are updated rather than
//! duplicated.
//!
//! Note boundaries are determined by file position. You can freely add, delete, or reorder notes in
//! the file — each note is matched to the database by `(live_sync_name, block_order)`, where
//! `block_order` is its 0-based index within that sync group. Content outside note boundaries
//! (headings, comments, prose) is preserved verbatim across imports.
//!
//! ## 1. Mark notes as live
//!
//! Add `live-sync-name` to the global settings of your file, *before* any notes:
//!
//! ```md
//! <!--- spares: start --->
//! <!--- # live-sync-name: lecture_notes_501 --->
//!
//! # Oceans
//!
//! <!--- spares: note start --->
//! The Pacific Ocean's deepest point is {{the Mariana Trench}}.
//! <!--- spares: note end --->
//!
//! Some geographers split the Pacific into the North and South Pacific.
//!
//! <!--- spares: note start --->
//! The Atlantic Ocean's deepest point is {{the Puerto Rico Trench}}.
//! <!--- spares: note end --->
//!
//! <!--- spares: end --->
//! ```
//!
//! Notes after a `live-sync-name` setting belong to that sync group and are assigned
//! `live_block_order` 0, 1, 2, … in file order. The headings (`# Oceans`) and prose (`Some
//! geographers split...`) sit outside note boundaries and are not part of any note — they are
//! preserved across imports without being parsed. You can switch to a different sync group by
//! setting `live-sync-name` again later in the file.
//!
//! ## 2. Import
//!
//! ```sh
//! spares import --parser markdown ./oceans.md
//! ```
//!
//! On import, spares:
//! - Mints unique `id:` keys on each cloze (e.g. `{{[id:a1b2c3d4e5f6] the Mariana Trench}}`) for
//!   stable card matching.
//! - Sets `live_block_order` (the note's position) and `live:<name>` tag.
//! - Creates a new note in the DB, or updates an existing one matched by `(live_sync_name,
//!   block_order)`.
//!
//! The file is rewritten with the new `id:` keys, so subsequent imports see the same IDs: `spares
//! import` rewrites it in place, and the web app offers the rewritten file to download in place of
//! the one imported:
//!
//! ```md
//! <!--- spares: start --->
//! <!--- # live-sync-name: lecture_notes_501 --->
//!
//! # Oceans
//!
//! <!--- spares: note start --->
//! The Pacific Ocean's deepest point is {{[id:a1b2c3d4e5f6] the Mariana Trench}}.
//! <!--- spares: note end --->
//!
//! Some geographers split the Pacific into the North and South Pacific.
//!
//! <!--- spares: note start --->
//! The Atlantic Ocean's deepest point is {{[id:f6e5d4c3b2a1] the Puerto Rico Trench}}.
//! <!--- spares: note end --->
//!
//! <!--- spares: end --->
//! ```
//!
//! ## 3. Edit and re-import
//!
//! Edit the source file freely and re-run the same import command:
//!
//! ```sh
//! spares import --parser markdown ./oceans.md
//! ```
//!
//! Only notes whose content actually changed are updated. Existing cards keep their review history.
//!
//! A live note can only be imported from its source file. Importing it from any other file, such as
//! a per-note file from `spares note generate` (which names its note by id), is refused, since the
//! next import of the source file would overwrite the edit. `spares note edit` and the notes
//! repository likewise refuse changes to a live note's data or custom data; its tags and keywords
//! stay editable there and on the web.
//!
//! Matching is **positional**: the first note in the file matches DB entry `(live_sync_name, 0)`,
//! the second matches `(live_sync_name, 1)`, and so on. This means you can freely adjust note
//! boundaries and content between them — what matters is each note's ordinal position within its
//! sync group.
//!
//! Here is the file from step 2 after some heavy editing — the Pacific note was deleted, a new
//! Indian Ocean note was inserted at the top, and non-note content was added. Crucially, the
//! Atlantic note is **unchanged**:
//!
//! ```md
//! <!--- spares: start --->
//! <!--- # live-sync-name: lecture_notes_501 --->
//!
//! # Deepest Ocean Points
//!
//! The five oceans are Pacific, Atlantic, Indian, Southern, and Arctic.
//!
//! <!--- spares: note start --->
//! The Indian Ocean's deepest point is {{the Java Trench}}, which reaches over 7,000 meters.
//! <!--- spares: note end --->
//!
//! Contrast this with the Atlantic:
//!
//! <!--- spares: note start --->
//! The Atlantic Ocean's deepest point is {{[id:f6e5d4c3b2a1] the Puerto Rico Trench}}.
//! <!--- spares: note end --->
//!
//! The Atlantic also contains the Romanche Trench near the equator.
//!
//! <!--- spares: end --->
//! ```
//!
//! On re-import:
//! - The heading changed to `# Deepest Ocean Points` — non-note content, preserved as-is.
//! - The **new Indian Ocean note** at position 0 gets `block_order = 0` → matches the old Pacific
//!   DB entry (`a1b2c3d4e5f6`) → **updates** it (the DB entry now stores Indian Ocean content). Its
//!   cloze has no `id:` yet, so a fresh one is minted.
//! - The **Atlantic note** stays at position 1 with `id:f6e5d4c3b2a1` intact → gets
//!   `block_order = 1` → matches its own old DB entry → **updates** it. Because the `id:` key and
//!   note content are the same, the existing card keeps its review history unchanged.
//!
//! The text outside note boundaries (`Contrast this with the Atlantic`, `The Atlantic also
//! contains...`) is **not parsed** — only content inside `note start`/`note end` is.
//!
//! > **Key insight:** Because matching is by `(live_sync_name, block_order)` and block_order is the
//! > note's file position, deleting or inserting notes shifts the positions of everything that
//! > follows. Existing clozes keep their review history as long as their `id:` key survives (the
//! > Atlantic cloze `f6e5d4c3b2a1` is untouched). When you insert a new note, its clozes get fresh
//! > `id:` keys. The non-note content (headings, commentary) stays untouched.
//!
//! A cloze whose note moved to another position, or that you moved to another note (say, by
//! splitting a note in two), takes its card along to the database note now at that position, so
//! the card keeps its review history. The database notes themselves stay at their positions:
//! - Database notes are only created at the end of the sync group, even for a note inserted into
//!   the middle of the file, so reviewing new cards in the order their notes were created
//!   (`new_order = "created"`) follows the file, even after stripping liveness.
//! - A note's tags and keywords set outside the file (on the web, say) belong to its position, so
//!   they apply to whichever note is there after notes are inserted or deleted before it.
//! - When the file has fewer notes than before, the database notes past its end are deleted, along
//!   with the cards of their clozes that are no longer in the file.
//!
//! Since a note that cannot be parsed would shift the positions of the notes after it, the import of
//! a file with one is refused.
//!
//! ## 4. Strip liveness (finalize)
//!
//! Once the notes are stable, remove all live-sync machinery:
//!
//! ```sh
//! spares import --parser markdown --strip-liveness ./oceans.md
//! ```
//!
//! This:
//! - Removes `live_sync_name`, `live_block_order` from custom data.
//! - Removes the `live:<name>` tag from each note.
//! - Removes `id:` keys from every cloze in the file (the file is rewritten).
//! - Sets the action to `Update` (matched by `note-id`) so the existing database note is patched
//!   instead of re-created.
//!
//! After stripping, the notes are ordinary notes and can no longer be matched by `live_sync_name`
//! on subsequent imports.
//!
//! > **Note:** `--strip-liveness` requires the notes to already exist in the database. Import them
//! > first without the flag.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::ParserErrorKind;
use crate::adapters::SrsAdapter;
use crate::adapters::impls::spares::SparesAdapter;
use crate::adapters::impls::spares::SparesRequestProcessor;
use crate::api::note::share_bundle::is_zip;
use crate::api::note::share_bundle::parser_of;
use crate::api::note::share_bundle::read_bundle;
use crate::api::note::share_bundle::refuse_note_ids;
use crate::api::note::share_bundle::store_bundled;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::model::NoteId;
use crate::parsers::NoteImportAction;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;
use crate::parsers::add_cloze_uid_to_file;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;
use crate::parsers::get_file_notes;
use crate::parsers::remove_cloze_uid_from_file;
use crate::parsers::remove_cloze_uid_from_note_data;

/// A file of notes to import
#[derive(Clone, Debug)]
pub struct ImportFile {
    /// Its name or path, for messages
    pub name: String,
    pub contents: String,
}

/// How to import files of notes. See `spares import --help`.
#[derive(Clone, Copy, Default)]
pub struct ImportOptions<'a> {
    /// The parser the files are in. Without one, each file's is the one parser that finds notes
    /// in it.
    pub parser: Option<&'a dyn Parseable>,
    /// The parser to convert the notes to before importing them
    pub to_parser: Option<&'a dyn Parseable>,
    /// Parse the files and say what importing them would do, without changing anything
    pub dry_run: bool,
    /// Strip liveness from the files' live notes instead of importing them (see the [module
    /// docs](self))
    pub strip_liveness: bool,
}

/// A note of an import, as its file gave it after the import worked out what to do with it
#[derive(Clone, Debug)]
pub struct ImportedNote {
    /// Its file's index among the files imported
    pub file: usize,
    pub settings: NoteSettings,
    /// `None` for a note with no parseable data, which is skipped
    pub data: Option<String>,
}

/// The notes of an import that one parser reads
#[derive(Clone, Debug)]
pub struct ParserImport {
    pub parser_name: String,
    /// In the order of the files, and of the notes in each
    pub notes: Vec<ImportedNote>,
    /// Live notes past the end of their sync group, which no note of its file matches any more,
    /// and which were deleted
    pub deleted_live_note_ids: Vec<NoteId>,
}

/// A file whose text an import changed
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RewrittenFile {
    /// Its index among the files imported
    pub file: usize,
    pub name: String,
    pub contents: String,
}

/// What [`import_files`] did, or would do in a dry run
#[derive(Clone, Debug, Default)]
pub struct ImportOutcome {
    /// By parser name
    pub parsers: Vec<ParserImport>,
    /// Notes with no parseable data, which were skipped
    pub skipped_note_count: usize,
    /// The files whose text the import changed, which their owner should replace with these once
    /// the import succeeded: live notes' clozes given `id:` keys, or stripped of them. In a dry
    /// run, what they would be.
    pub rewritten: Vec<RewrittenFile>,
}

type ParsedNotes = Vec<(NoteSettings, Option<String>)>;
type GuessResult<'a> = Option<(&'a dyn Parseable, ParsedNotes)>;

struct ParserAggregate<'a> {
    parser: &'a dyn Parseable,
    notes: Vec<ImportedNote>,
    update_note_ids: Vec<NoteId>,
    /// Live notes past the end of their sync group, which no note of its file matches any more
    stale_live_note_ids: Vec<NoteId>,
}

fn parse_blocks(
    parser: &dyn Parseable,
    to_parser_opt: Option<&dyn Parseable>,
    contents: &str,
    adapter: &dyn SrsAdapter,
    dry_run: bool,
    external_config: Option<&SparesExternalConfig>,
) -> Result<ParsedNotes, Error> {
    get_file_notes(
        parser,
        to_parser_opt,
        contents,
        adapter,
        !dry_run,
        external_config.as_ref().map(|c| &c.overlapper),
    )
}

fn guess_and_parse_notes<'a>(
    file_contents: &str,
    parsers_to_try: &[&'a dyn Parseable],
    to_parser_opt: Option<&dyn Parseable>,
    adapter: &dyn SrsAdapter,
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

/// Enriches the clozes between a file's start and end markers with `id:` keys if any are missing.
/// Returns the enriched file content if any keys were minted, `None` otherwise.
fn handle_live_sync(
    parser: &dyn Parseable,
    file_contents: &str,
    notes: &[(NoteSettings, Option<String>)],
) -> Result<Option<String>, Error> {
    let any_live = notes
        .iter()
        .any(|(s, _)| s.custom_data.contains_key("live_sync_name"));

    if any_live {
        let (enriched_contents, minted_count) = add_cloze_uid_to_file(parser, file_contents)?;
        if minted_count > 0 {
            return Ok(Some(enriched_contents));
        }
    }
    Ok(None)
}

/// Strips liveness from live notes in a file. Removes `live_sync_name`,
/// `live_block_order`, the `live:{lsn}` tag, and `id:` keys from cloze syntax.
/// Sets the note action to `Update` so the existing live note in the DB is patched, and adds its id
/// to `stripped_note_ids`. Returns the notes, and the file's text without the `id:` keys if it
/// had any.
async fn handle_strip_liveness(
    parser: &dyn Parseable,
    file_contents: &str,
    notes: &[(NoteSettings, Option<String>)],
    adapter: &dyn SrsAdapter,
    stripped_note_ids: &mut HashSet<NoteId>,
) -> Result<Option<(ParsedNotes, Option<String>)>, Error> {
    let any_live = notes
        .iter()
        .any(|(s, _)| s.custom_data.contains_key("live_sync_name"));

    if !any_live {
        return Ok(None);
    }

    // Strip ids from the full file text
    let (stripped_contents, stripped_count) = remove_cloze_uid_from_file(parser, file_contents)?;
    let rewritten = (stripped_count > 0).then_some(stripped_contents);

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
            // Find the existing live note so we can update (not re-create) it. A file that names
            // its note, such as a per-note file from `spares note generate`, is trusted, since the
            // position counter restarts at 0 for every file. Otherwise the note is found by its
            // position: as the file states it, or as its place among the file's blocks.
            let explicit_note_id = match stripped_settings.action {
                NoteImportAction::Update(note_id) => Some(note_id),
                NoteImportAction::Add | NoteImportAction::Delete(_) => None,
            };
            let explicit_block_order = stripped_settings
                .custom_data
                .get("live_block_order")
                .and_then(Value::as_i64);
            let existing_note_id = if let Some(note_id) = explicit_note_id {
                if adapter.get_live_sync_name(note_id).await?.as_deref() != Some(lsn) {
                    return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
                        description: format!(
                            "Cannot strip liveness from note {note_id}: it is not a live note \
                             of live-sync-name `{lsn}` in the database."
                        ),
                    })));
                }
                note_id
            } else {
                let block_order = explicit_block_order.unwrap_or_else(|| {
                    let bo = counters.entry(lsn.clone()).or_insert(0);
                    let block_order = *bo;
                    *bo += 1;
                    block_order
                });
                adapter
                    .find_live_note_by_block_order(lsn, block_order)
                    .await?
                    .ok_or_else(|| {
                        Error::Library(LibraryError::Parser(ParserErrorKind::FailedToGuess(
                            format!(
                                "Cannot strip liveness from note at \
                                 (live_sync_name: {lsn}, block_order: {block_order}): \
                                 no matching note in the database. Import it first \
                                 without stripping liveness."
                            ),
                        )))
                    })?
            };
            stripped_settings.action = NoteImportAction::Update(existing_note_id);
            stripped_note_ids.insert(existing_note_id);

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

    Ok(Some((result, rewritten)))
}

async fn aggregate_note_into_parser_group(
    file: usize,
    mut local_settings: NoteSettings,
    note_data_res: Option<String>,
    agg: &mut ParserAggregate<'_>,
    adapter: &dyn SrsAdapter,
    file_counters: &mut HashMap<String, i64>,
    stripped_note_ids: &HashSet<NoteId>,
) -> Result<(), Error> {
    let lsn_opt = local_settings
        .custom_data
        .get("live_sync_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // A live note is imported only from its source file, whose blocks carry neither a note id nor
    // a `live_block_order`, since their position within the file is their identity. Any other
    // file, such as a per-note file from `spares note generate`, would be overwritten by the next
    // import of the source file. A note whose liveness was just stripped is exempt.
    let explicit_note_id = match local_settings.action {
        NoteImportAction::Update(note_id) => Some(note_id),
        NoteImportAction::Add | NoteImportAction::Delete(_) => None,
    };
    if !explicit_note_id.is_some_and(|note_id| stripped_note_ids.contains(&note_id)) {
        let explicit_block_order = local_settings
            .custom_data
            .get("live_block_order")
            .and_then(Value::as_i64);
        let named_live_note = match explicit_note_id {
            Some(note_id) => adapter
                .get_live_sync_name(note_id)
                .await?
                .or_else(|| lsn_opt.clone())
                .map(|lsn| format!("Note {note_id} is a live note (live-sync-name `{lsn}`)")),
            None => lsn_opt.as_ref().zip(explicit_block_order).map(|(lsn, block_order)| {
                format!(
                    "The note at block {block_order} of live-sync-name `{lsn}` names its position"
                )
            }),
        };
        if let Some(description) = named_live_note {
            return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "{description}, so it can only be imported from its source file. Edit it \
                     there and import that file instead."
                ),
            })));
        }
    }

    if let Some(lsn) = lsn_opt {
        let bo = file_counters.entry(lsn.clone()).or_insert(0);
        let block_order = *bo;
        *bo += 1;
        match adapter
            .find_live_note_by_block_order(&lsn, block_order)
            .await?
        {
            Some(note_id) => {
                local_settings.action = NoteImportAction::Update(note_id);
                agg.update_note_ids.push(note_id);
            }
            None => {
                local_settings.action = NoteImportAction::Add;
            }
        }
        local_settings.custom_data.insert(
            "live_block_order".to_string(),
            Value::Number(block_order.into()),
        );

        let live_tag = format!("live:{lsn}");
        if !local_settings.tags.iter().any(|t| t == &live_tag) {
            local_settings.tags.push(live_tag);
        }
    }

    agg.notes.push(ImportedNote {
        file,
        settings: local_settings,
        data: note_data_res,
    });
    Ok(())
}

/// Imports `files` of notes with `adapter`: adds, updates and deletes the notes they say to, in
/// one group of events, so one undo reverts the import. See the [module docs](self).
///
/// Nothing is imported if a file fails to parse. The notes of each parser are imported together,
/// so if importing one parser's notes fails, the notes of the parsers before it (by name) stay
/// imported.
#[expect(clippy::too_many_lines)]
pub async fn import_files(
    adapter: &mut dyn SrsAdapter,
    files: &[ImportFile],
    options: ImportOptions<'_>,
    at: DateTime<Utc>,
) -> Result<ImportOutcome, Error> {
    let ImportOptions {
        parser: parser_opt,
        to_parser: to_parser_opt,
        dry_run,
        strip_liveness,
    } = options;
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

    // By parser name, so the parsers' notes are imported in the same order every time
    let mut aggregates: BTreeMap<&str, ParserAggregate<'_>> = BTreeMap::new();
    let mut outcome = ImportOutcome::default();
    let mut stripped_note_ids: HashSet<NoteId> = HashSet::new();

    for (file_index, file) in files.iter().enumerate() {
        let file_contents = &file.contents;
        let parse_result = guess_and_parse_notes(
            file_contents,
            &parsers_to_try,
            to_parser_opt,
            &*adapter,
            dry_run,
            external_config.as_ref(),
        )?;

        let Some((parser, mut max_parser_all_notes)) = parse_result else {
            if parsers_to_try.len() > 1 {
                return Err(Error::Library(LibraryError::Parser(
                    ParserErrorKind::FailedToGuess(format!(
                        "All parsers parsed 0 notes from {}.",
                        file.name
                    )),
                )));
            }
            if parser_opt.is_none() {
                return Err(Error::Library(LibraryError::Parser(
                    ParserErrorKind::FailedToGuess(String::new()),
                )));
            }
            // The one parser given finds no notes in the file, so there is nothing to import
            return Ok(ImportOutcome::default());
        };

        let rewritten = if strip_liveness {
            match handle_strip_liveness(
                parser,
                file_contents,
                &max_parser_all_notes,
                &*adapter,
                &mut stripped_note_ids,
            )
            .await?
            {
                Some((stripped, rewritten)) => {
                    max_parser_all_notes = stripped;
                    rewritten
                }
                None => None,
            }
        } else if let Some(enriched_contents) =
            handle_live_sync(parser, file_contents, &max_parser_all_notes)?
        {
            // Re-parse enriched content so note data carries the same id: keys
            // as what will be written to the file (fixes double-UID-minting bug).
            let reparsed = guess_and_parse_notes(
                &enriched_contents,
                &parsers_to_try,
                to_parser_opt,
                &*adapter,
                dry_run,
                external_config.as_ref(),
            )?;
            if let Some((_, reparsed_notes)) = reparsed {
                max_parser_all_notes = reparsed_notes;
            }
            Some(enriched_contents)
        } else {
            None
        };
        if let Some(contents) = rewritten {
            outcome.rewritten.push(RewrittenFile {
                file: file_index,
                name: file.name.clone(),
                contents,
            });
        }

        let parser_name = parser.get_parser_name();
        let agg = aggregates
            .entry(parser_name)
            .or_insert_with(|| ParserAggregate {
                parser,
                notes: Vec::new(),
                update_note_ids: Vec::new(),
                stale_live_note_ids: Vec::new(),
            });

        let mut file_counters: HashMap<String, i64> = HashMap::new();
        for (local_settings, note_data_res) in max_parser_all_notes {
            if note_data_res.is_none() {
                // Skipping a live note would match every later note of its sync group to the
                // database note of the one before it.
                if let Some(lsn) = local_settings
                    .custom_data
                    .get("live_sync_name")
                    .and_then(Value::as_str)
                {
                    let block_order = file_counters.get(lsn).copied().unwrap_or(0);
                    let errors = local_settings
                        .errors_and_warnings
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
                        description: format!(
                            "The note at block {block_order} of live-sync-name `{lsn}` in {} \
                             could not be parsed: {errors}. Live notes are matched by position, \
                             so fix it before importing the file.",
                            file.name
                        ),
                    })));
                }
                outcome.skipped_note_count += 1;
                continue;
            }
            aggregate_note_into_parser_group(
                file_index,
                local_settings,
                note_data_res,
                agg,
                &*adapter,
                &mut file_counters,
                &stripped_note_ids,
            )
            .await?;
        }

        // A sync group with fewer notes than before leaves database notes past its end. The cards
        // of their clozes that moved to other notes go along with them (see `process_data`).
        for (lsn, count) in &file_counters {
            let mut block_order = *count;
            while let Some(note_id) = adapter
                .find_live_note_by_block_order(lsn, block_order)
                .await?
            {
                agg.stale_live_note_ids.push(note_id);
                block_order += 1;
            }
        }
    }

    for (name, mut agg) in aggregates {
        let mut notes = agg
            .notes
            .iter()
            .map(|note| (note.settings.clone(), note.data.clone()))
            .collect::<Vec<_>>();
        notes.extend(agg.stale_live_note_ids.iter().map(|&note_id| {
            let settings = NoteSettings {
                action: NoteImportAction::Delete(note_id),
                ..NoteSettings::default()
            };
            (settings, Some(String::new()))
        }));
        let mut update_ids = std::mem::take(&mut agg.update_note_ids);
        update_ids.sort_unstable();
        update_ids.dedup();
        adapter
            .process_data(notes, agg.parser, dry_run, true, at, update_ids)
            .await?;
        outcome.parsers.push(ParserImport {
            parser_name: name.to_string(),
            notes: agg.notes,
            deleted_live_note_ids: agg.stale_live_note_ids,
        });
    }
    Ok(outcome)
}

/// What an import changed, or would change in a dry run, for the web app (see
/// [`ImportOutcome`])
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ImportResponse {
    pub dry_run: bool,
    /// In the order of the files, and of the notes in each
    pub notes: Vec<ImportNoteSummary>,
    /// Notes with no parseable data, which were skipped
    pub skipped_note_count: usize,
    /// Live notes past the end of their sync group, deleted
    pub deleted_live_note_ids: Vec<NoteId>,
    /// Files whose text the import changed, to download in place of the ones imported
    pub rewritten_files: Vec<RewrittenFile>,
    /// The first event of the import, whose group the rest are in, so undoing it with its group
    /// reverts the import. None in a dry run, or if nothing changed.
    pub event_id: Option<i64>,
    /// How many bundled files (images, audio, image occlusions) of zips were stored for the notes
    pub media_count: usize,
    /// Bundled files the notes name that their zip doesn't have
    pub missing_media: Vec<String>,
    /// Files of zips that are no parser's, which were left out
    pub skipped_files: Vec<String>,
}

/// A note of an import, for [`ImportResponse`]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImportNoteSummary {
    /// The name of its file
    pub file: String,
    pub parser: String,
    /// `add`, `update` or `delete`
    pub action: String,
    /// The note it updates or deletes
    pub note_id: Option<NoteId>,
    pub card_count: usize,
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
    pub is_suspended: bool,
    /// Whether it is a live note (see the [module docs](self))
    pub live: bool,
    /// Problems with its settings that did not stop it from being imported
    pub warnings: Vec<String>,
    pub data: String,
}

impl ImportResponse {
    /// `outcome` of importing `files`
    pub fn new(outcome: &ImportOutcome, files: &[ImportFile], dry_run: bool) -> Self {
        let mut notes = outcome
            .parsers
            .iter()
            .flat_map(|parser| {
                parser.notes.iter().map(|note| {
                    let (action, note_id) = match note.settings.action {
                        NoteImportAction::Add => ("add", None),
                        NoteImportAction::Update(id) => ("update", Some(id)),
                        NoteImportAction::Delete(id) => ("delete", Some(id)),
                    };
                    (
                        note.file,
                        ImportNoteSummary {
                            file: files[note.file].name.clone(),
                            parser: parser.parser_name.clone(),
                            action: action.to_string(),
                            note_id,
                            card_count: note.settings.cards_count.unwrap_or(0),
                            tags: note.settings.tags.clone(),
                            keywords: note.settings.keywords.clone(),
                            is_suspended: note.settings.is_suspended,
                            live: note.settings.custom_data.contains_key("live_sync_name"),
                            warnings: note
                                .settings
                                .errors_and_warnings
                                .iter()
                                .map(ToString::to_string)
                                .collect(),
                            data: note.data.clone().unwrap_or_default(),
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        // Stable, so each file's notes keep their order
        notes.sort_by_key(|(file, _)| *file);
        Self {
            dry_run,
            notes: notes.into_iter().map(|(_, note)| note).collect(),
            skipped_note_count: outcome.skipped_note_count,
            deleted_live_note_ids: outcome
                .parsers
                .iter()
                .flat_map(|p| p.deleted_live_note_ids.iter().copied())
                .collect(),
            rewritten_files: outcome.rewritten.clone(),
            ..Self::default()
        }
    }
}

/// A file uploaded to import: a file of notes, or a zip of them with their media (see
/// [`share_bundle`](super::share_bundle))
#[derive(Clone, Debug)]
pub struct Upload {
    /// Its name, whose extension says which parser reads a file of notes
    pub name: String,
    pub bytes: Vec<u8>,
}

/// [`ImportOptions`] by parser name, for an API request
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ImportRequestOptions {
    /// Without one, each file's parser is guessed (see [`ImportOptions::parser`])
    #[serde(default)]
    pub parser: Option<String>,
    #[serde(default)]
    pub to_parser: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub strip_liveness: bool,
}

fn invalid(description: String) -> Error {
    Error::Library(LibraryError::Note(NoteErrorKind::Other { description }))
}

/// Imports `uploads` into the collection, as `spares import` imports files: the server's import
/// for the web app (`POST /api/notes/import`). A zip's files are imported with the media it
/// bundles, which are stored first (except in a dry run), as `spares import` uploads them. As
/// there, a zip is shared notes, so one whose files name notes by id is refused (see
/// [`refuse_note_ids`]); upload such files on their own.
pub async fn import_uploaded(
    db: &SqlitePool,
    uploads: &[Upload],
    options: &ImportRequestOptions,
    at: DateTime<Utc>,
) -> Result<ImportResponse, Error> {
    let all_parsers = get_all_parsers();
    let parser = options
        .parser
        .as_deref()
        .map(|name| find_parser(name, &all_parsers))
        .transpose()?;
    let to_parser = options
        .to_parser
        .as_deref()
        .map(|name| find_parser(name, &all_parsers))
        .transpose()?;
    let mut response = ImportResponse::default();
    let mut files = Vec::new();
    let mut stored: HashMap<String, String> = HashMap::new();
    for upload in uploads {
        if !is_zip(&upload.bytes) {
            let contents = String::from_utf8(upload.bytes.clone())
                .map_err(|_| invalid(format!("{} isn't text, or a zip", upload.name)))?;
            files.push(ImportFile {
                name: upload.name.clone(),
                contents,
            });
            continue;
        }
        let bundle = read_bundle(&upload.bytes)?;
        refuse_note_ids(&bundle, &all_parsers)?;
        for (name, contents) in &bundle.files {
            let name = format!("{}/{name}", upload.name);
            if parser_of(&name, &all_parsers).is_none() {
                response.skipped_files.push(name);
                continue;
            }
            let contents = store_bundled(
                contents,
                &bundle,
                &mut stored,
                &mut response.missing_media,
                !options.dry_run,
            )?;
            files.push(ImportFile { name, contents });
        }
    }
    if files.is_empty() {
        return Err(invalid(
            "There are no files of notes to import: they are .md, .typ or .tex files, or zips of \
             them"
                .to_string(),
        ));
    }
    let mut adapter = SparesAdapter::new(SparesRequestProcessor::Database { pool: db.clone() });
    let import_options = ImportOptions {
        parser: parser.as_deref(),
        to_parser: to_parser.as_deref(),
        dry_run: options.dry_run,
        strip_liveness: options.strip_liveness,
    };
    let outcome = import_files(&mut adapter, &files, import_options, at).await?;
    Ok(ImportResponse {
        event_id: adapter.import_group(),
        media_count: stored.len(),
        missing_media: response.missing_media,
        skipped_files: response.skipped_files,
        ..ImportResponse::new(&outcome, &files, options.dry_run)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::note::share_bundle::write_bundle;
    use crate::api::parser::create_parser;
    use crate::api::undo::undo_event;
    use crate::schema::note::ExportNotesResponse;
    use crate::schema::parser::CreateParserRequest;
    use crate::schema::undo::UndoEventRequest;

    const NOTES: &str = "<!--- spares: start --->\n\n<!--- # tags: geo --->\n<!--- spares: note start --->\n{{ Paris }} is the capital\n<!--- spares: note end --->\n\n<!--- spares: end --->\n";

    const LIVE: &str = "<!--- spares: start --->\n<!--- # live-sync-name: lecture --->\n# Oceans\n<!--- spares: note start --->\nThe deepest point is {{the Mariana Trench}}.\n<!--- spares: note end --->\n<!--- spares: end --->\n";

    async fn markdown(pool: &SqlitePool) {
        let request = CreateParserRequest {
            name: "markdown".to_string(),
        };
        create_parser(pool, request, true).await.unwrap();
    }

    fn upload(name: &str, text: &str) -> Upload {
        Upload {
            name: name.to_string(),
            bytes: text.as_bytes().to_vec(),
        }
    }

    async fn note_data(pool: &SqlitePool) -> Vec<(NoteId, String)> {
        sqlx::query_as("SELECT id, data FROM note ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// A dry run says what the import would do and changes nothing; then the import does it.
    #[sqlx::test]
    async fn test_dry_run_then_import(pool: SqlitePool) {
        markdown(&pool).await;
        let options = ImportRequestOptions {
            dry_run: true,
            ..ImportRequestOptions::default()
        };
        let preview = import_uploaded(&pool, &[upload("a.md", NOTES)], &options, Utc::now())
            .await
            .unwrap();
        assert!(preview.dry_run);
        assert_eq!(preview.notes.len(), 1);
        let note = &preview.notes[0];
        assert_eq!(
            (
                note.file.as_str(),
                note.parser.as_str(),
                note.action.as_str()
            ),
            ("a.md", "markdown", "add")
        );
        assert_eq!(note.card_count, 1);
        assert_eq!(note.tags, ["geo"]);
        assert_eq!(preview.event_id, None);
        assert_eq!(note_data(&pool).await.len(), 0);

        let imported = import_uploaded(
            &pool,
            &[upload("a.md", NOTES)],
            &ImportRequestOptions::default(),
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(!imported.dry_run);
        assert!(imported.event_id.is_some());
        assert_eq!(note_data(&pool).await.len(), 1);
    }

    /// Unlike an import of shared notes, a file can update the notes it names by id, and one undo
    /// of the import's group reverts every file of it.
    #[sqlx::test]
    async fn test_update_by_id_and_undo_the_import(pool: SqlitePool) {
        markdown(&pool).await;
        let options = ImportRequestOptions::default();
        import_uploaded(&pool, &[upload("a.md", NOTES)], &options, Utc::now())
            .await
            .unwrap();
        let (id, _) = note_data(&pool).await[0].clone();
        let edited = NOTES
            .replace(
                "<!--- # tags: geo --->\n",
                &format!("<!--- # note-id: {} --->\n", id.letters()),
            )
            .replace("is the capital", "is the capital of France");
        let imported = import_uploaded(
            &pool,
            &[
                upload("edit.md", &edited),
                upload("b.md", &NOTES.replace("Paris", "Rome")),
            ],
            &options,
            Utc::now(),
        )
        .await
        .unwrap();
        let actions = imported
            .notes
            .iter()
            .map(|n| (n.file.as_str(), n.action.as_str(), n.note_id))
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            [("edit.md", "update", Some(id)), ("b.md", "add", None)]
        );
        let notes = note_data(&pool).await;
        assert_eq!(notes.len(), 2);
        assert!(notes[0].1.contains("capital of France"), "{notes:?}");

        undo_event(
            &pool,
            UndoEventRequest {
                event_id: imported.event_id,
                undo_group: true,
            },
        )
        .await
        .unwrap();
        let notes = note_data(&pool).await;
        assert_eq!(notes.len(), 1);
        assert!(!notes[0].1.contains("France"), "{notes:?}");
    }

    /// A live note's file comes back with its clozes' `id:` keys, to download in place of the one
    /// uploaded; reimporting that updates the note rather than adding another.
    #[sqlx::test]
    async fn test_live_notes_return_their_rewritten_file(pool: SqlitePool) {
        markdown(&pool).await;
        let options = ImportRequestOptions::default();
        let imported = import_uploaded(&pool, &[upload("live.md", LIVE)], &options, Utc::now())
            .await
            .unwrap();
        assert!(imported.notes[0].live);
        let [rewritten] = imported.rewritten_files.as_slice() else {
            panic!("{:?}", imported.rewritten_files);
        };
        assert_eq!(rewritten.name, "live.md");
        assert!(
            rewritten.contents.contains("[id:"),
            "{}",
            rewritten.contents
        );
        assert!(rewritten.contents.contains("# Oceans"));

        let again = import_uploaded(
            &pool,
            &[upload("live.md", &rewritten.contents)],
            &options,
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(again.notes[0].action, "update");
        assert!(again.rewritten_files.is_empty());
        assert_eq!(note_data(&pool).await.len(), 1);

        // Stripping liveness gives the file back without the keys
        let stripped = import_uploaded(
            &pool,
            &[upload("live.md", &rewritten.contents)],
            &ImportRequestOptions {
                strip_liveness: true,
                ..ImportRequestOptions::default()
            },
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(!stripped.notes[0].live);
        assert!(!stripped.rewritten_files[0].contents.contains("[id:"));
    }

    /// A zip's files are imported, and those that are no parser's left out. As in `spares import`,
    /// a zip that names notes by id is refused.
    #[sqlx::test]
    async fn test_zips(pool: SqlitePool) {
        markdown(&pool).await;
        let export = ExportNotesResponse {
            files: HashMap::from([("md".to_string(), NOTES.to_string())]),
            ..ExportNotesResponse::default()
        };
        let zip = write_bundle(&export).unwrap();
        let imported = import_uploaded(
            &pool,
            &[Upload {
                name: "shared.zip".to_string(),
                bytes: zip,
            }],
            &ImportRequestOptions::default(),
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(imported.notes.len(), 1);
        assert_eq!(imported.notes[0].file, "shared.zip/export.md");

        let by_id = ExportNotesResponse {
            files: HashMap::from([(
                "md".to_string(),
                NOTES.replace("<!--- # tags: geo --->", "<!--- # note-id: aaaa --->"),
            )]),
            ..ExportNotesResponse::default()
        };
        let error = import_uploaded(
            &pool,
            &[Upload {
                name: "edit.zip".to_string(),
                bytes: write_bundle(&by_id).unwrap(),
            }],
            &ImportRequestOptions::default(),
            Utc::now(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("ids"), "{error}");

        let error = import_uploaded(
            &pool,
            &[upload("notes.txt", "hi")],
            &ImportRequestOptions::default(),
            Utc::now(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("parsed 0 notes"), "{error}");
    }
}
