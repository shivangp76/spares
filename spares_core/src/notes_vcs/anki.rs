//! The notes repository's `anki` branch, which describes Anki's notes. Fetching it shows Anki's
//! state, and pushing to it writes to Anki, so moving notes between Anki and spares is a merge
//! between `anki` and `main`.
//!
//! git cannot run anything on fetch, so the branch is brought up to date explicitly by [`refresh`]
//! (`spares vcs refresh anki`). It is never checked out: it is written with a temporary index, so
//! the worktree, which holds `main`, is not touched.
//!
//! Only Anki notes of the note type spares writes are on the branch (see
//! [`crate::adapters::impls::anki::backend`]). Anki holds only part of a note (its first card,
//! split into front and back), so a snapshot compares Anki's note with what spares would have
//! written to Anki for the note in the parent commit, and keeps the parent's file when they match.
//! Otherwise, merging the branch into `main` would replace notes with Anki's partial copies.
//!
//! A snapshot's tree, relative to its parent commit `P`:
//! - Files other than notes are `P`'s.
//! - An Anki note is at `<parser>/<id>.<ext>` if its `SparesId` is a note of the database or of
//!   `P`, or else at the path of the note of the database, or of `P`, that has its Anki id
//!   (`anki-note-id` in its custom data). Otherwise it is at `<parser>/new-anki-<Anki id>.<ext>`,
//!   which merging into `main` creates.
//! - Its file is `P`'s (or the database's, if `P` has none there) if Anki holds what spares writes
//!   for that file, with `anki-note-id` set. Otherwise it is Anki's version, keeping that file's
//!   custom data and the note's linked notes.
//! - A note of `P` that Anki does not have is left out if it has an Anki id, since it was deleted
//!   in Anki, and kept otherwise, since it was never sent to Anki. So the branch never deletes
//!   notes spares has and Anki never had.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;

use super::git::git;
use super::git::git_command;
use super::git::git_error;
use super::git::git_output;
use super::git::nul_split;
use super::git::rev_parse;
use super::lock::NotesGuard;
use super::lock::WRITER_WAIT;
use super::lock::exclusive_guard;
use super::lock::run_holding;
use super::note_file::construct_db_note_file;
use super::note_file::db_note_fields;
use super::note_file::file_note_fields;
use super::note_file::linked_notes_from_map;
use super::note_file::parse_new_note_file;
use super::note_file::parse_note_file;
use super::note_file::same_note;
use super::paths::enabled;
use super::paths::note_rel_path;
use super::paths::parse_new_note_rel_path;
use super::paths::parse_note_rel_path;
use super::paths::server_repo_dir;
use super::receive::README;
use super::receive::WATCHDOG_DELAY;
use super::receive::conflict_marker_line;
use super::receive::short;
use super::receive::wrong_repo;
use crate::Error;
use crate::adapters::impls::anki::AnkiNote;
use crate::adapters::impls::anki::backend::AnkiBackend;
use crate::adapters::impls::anki::backend::AnkiRawNote;
use crate::adapters::impls::anki::to_anki_fields;
use crate::api::note::RenderNoteData;
use crate::api::note::get_note;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::api::note::update_notes;
use crate::helpers::get_or_compile_regex;
use crate::model::CustomData;
use crate::model::NOTE_ID_KEY;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::parsers::NoteImportAction;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;
use crate::parsers::TemplateType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::generate_files::construct_note_file_contents;
use crate::parsers::get_all_parsers;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;

/// The branch describing Anki
pub const ANKI_REF: &str = "refs/heads/anki";

/// Where an Anki note's id is kept in a spares note's custom data
pub const ANKI_NOTE_ID_KEY: &str = "anki-note-id";

/// Parser of Anki notes whose `SparesParserName` is empty or not installed, if spares has no notes
/// to take the most used parser from
const FALLBACK_PARSER: &str = "markdown";

/// `spares vcs refresh anki`
#[derive(Debug, Deserialize, Serialize)]
pub struct RefreshResponse {
    /// `anki` before the refresh, if it existed
    pub old: Option<String>,
    /// `anki` after the refresh
    pub new: String,
    /// Whether the refresh committed a new snapshot, so there is something to fetch
    pub changed: bool,
}

/// Brings `anki` up to date with Anki: commits a snapshot of it on top of `anki`, or of `main` the
/// first time, if it differs. Holds the exclusive notes lock, since it reads the database's notes
/// and must not interleave with a push to `anki`.
pub async fn refresh(
    db: SqlitePool,
    backend: Arc<dyn AnkiBackend>,
) -> Result<RefreshResponse, Error> {
    let guard = exclusive_guard(Some(WRITER_WAIT)).await?;
    run_holding(
        guard,
        async move { refresh_locked(&db, backend.as_ref()).await },
    )
    .await
}

pub(crate) async fn refresh_locked(
    db: &SqlitePool,
    backend: &dyn AnkiBackend,
) -> Result<RefreshResponse, Error> {
    if !enabled() {
        return Err(Error::NotesVcs(
            "this spares server does not keep notes in a git repository (run `spares vcs init`)"
                .to_string(),
        ));
    }
    let repo = server_repo_dir();
    let old = anki_tip(&repo)?;
    let parent = match &old {
        Some(old) => old.clone(),
        None => rev_parse(&repo, "HEAD")?,
    };
    let notes = backend.read_notes().await?;
    let new =
        match commit_snapshot(db, &repo, &parent, old.as_deref(), &notes, &HashMap::new()).await? {
            Some(new) => new,
            // Anki holds what `main` does: `anki` starts there.
            None if old.is_none() => {
                git(&repo, &["update-ref", ANKI_REF, &parent, ""], None)?;
                parent
            }
            None => parent,
        };
    Ok(RefreshResponse {
        changed: old.as_ref() != Some(&new),
        new,
        old,
    })
}

/// The commit `anki` points at, if it exists.
pub(crate) fn anki_tip(repo: &Path) -> Result<Option<String>, Error> {
    let output = git_output(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            ANKI_REF,
        ],
        None,
    )?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

/// Commits the snapshot of `notes` on top of `parent` and moves `anki` to it, if `anki` is still
/// at `expected` (`None`: does not exist). Returns the commit, or `None` if the snapshot's tree is
/// `parent`'s.
pub(crate) async fn commit_snapshot(
    db: &SqlitePool,
    repo: &Path,
    parent: &str,
    expected: Option<&str>,
    notes: &[AnkiRawNote],
    hints: &HashMap<i64, String>,
) -> Result<Option<String>, Error> {
    let tree = snapshot_tree(db, repo, parent, notes, hints).await?;
    commit_tree(repo, &tree, parent, expected)
}

/// Commits `tree` on top of `parent` and moves `anki` to it, if `anki` is still at `expected`.
/// Returns `None` without committing if `tree` is `parent`'s.
fn commit_tree(
    repo: &Path,
    tree: &str,
    parent: &str,
    expected: Option<&str>,
) -> Result<Option<String>, Error> {
    let parent_tree = rev_parse(repo, &format!("{parent}^{{tree}}"))?;
    if tree == parent_tree {
        return Ok(None);
    }
    let commit = String::from_utf8_lossy(&git(
        repo,
        &[
            "commit-tree",
            tree,
            "-p",
            parent,
            "-m",
            "spares: snapshot Anki",
        ],
        None,
    )?)
    .trim()
    .to_string();
    git(
        repo,
        &[
            "update-ref",
            ANKI_REF,
            &commit,
            expected.unwrap_or_default(),
        ],
        None,
    )?;
    Ok(Some(commit))
}

/// A file of a commit's tree
struct TreeEntry {
    mode: String,
    sha: String,
    path: String,
}

/// What a path of the parent holds
enum NotePath {
    /// `<parser>/<id>.<ext>`
    Note(Box<dyn Parseable>, NoteId),
    /// `<parser>/new-<name>.<ext>`
    New(Box<dyn Parseable>),
}

impl NotePath {
    fn parse(path: &str) -> Option<Self> {
        let path = Path::new(path);
        parse_note_rel_path(path)
            .map(|(parser, id)| Self::Note(parser, id))
            .or_else(|| parse_new_note_rel_path(path).map(Self::New))
    }

    fn parser(&self) -> &dyn Parseable {
        match self {
            Self::Note(parser, _) | Self::New(parser) => parser.as_ref(),
        }
    }

    fn id(&self) -> Option<NoteId> {
        match self {
            Self::Note(_, id) => Some(*id),
            Self::New(_) => None,
        }
    }
}

/// The notes of the database, as the snapshot needs them
struct DbNotes {
    rows: HashMap<NoteId, RenderNoteData>,
    links: HashMap<NoteId, Vec<NoteLink>>,
    by_anki_id: HashMap<i64, NoteId>,
    default_parser: String,
}

impl DbNotes {
    async fn read(db: &SqlitePool) -> Result<Self, Error> {
        let rows: HashMap<NoteId, RenderNoteData> = get_render_note_data(db, None)
            .await?
            .into_iter()
            .map(|row| (row.note_id, row))
            .collect();
        let ids: Vec<NoteId> = rows.keys().copied().collect();
        let links = get_note_links_map(db, &ids).await?;
        let by_anki_id = rows
            .values()
            .filter_map(|row| {
                Some((
                    anki_id_of(row.custom_data.get(ANKI_NOTE_ID_KEY)?)?,
                    row.note_id,
                ))
            })
            .collect();
        let mut parser_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for row in rows.values() {
            *parser_counts.entry(row.parser_name.as_str()).or_default() += 1;
        }
        let default_parser = parser_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map_or(FALLBACK_PARSER, |(name, _)| name)
            .to_string();
        Ok(Self {
            rows,
            links,
            by_anki_id,
            default_parser,
        })
    }
}

fn anki_id_of(value: &Value) -> Option<i64> {
    match value {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// The Anki id in a note file's custom data, found without parsing the file
fn blob_anki_id(blob: &str) -> Option<i64> {
    let regex = get_or_compile_regex(r#"anki-note-id"?\s*:\s*"?(\d+)"#).ok()?;
    let caps = regex.captures(blob).ok()??;
    caps.get(1)?.as_str().parse().ok()
}

#[expect(clippy::too_many_lines)]
/// `hints` are paths of Anki notes that a push just added from files without a note id, which Anki
/// knows no path for.
async fn snapshot_tree(
    db: &SqlitePool,
    repo: &Path,
    parent: &str,
    notes: &[AnkiRawNote],
    hints: &HashMap<i64, String>,
) -> Result<String, Error> {
    let entries = read_tree(repo, parent)?;
    let note_shas: Vec<&str> = entries
        .iter()
        .filter(|entry| NotePath::parse(&entry.path).is_some())
        .map(|entry| entry.sha.as_str())
        .collect();
    let blobs = read_blobs(repo, &note_shas)?;
    let db_notes = DbNotes::read(db).await?;
    let parsers = get_all_parsers();

    // The parent's notes, by id and by Anki id
    let mut by_id: HashMap<NoteId, &str> = HashMap::new();
    let mut by_anki_id: HashMap<i64, &str> = HashMap::new();
    for entry in &entries {
        let Some(note_path) = NotePath::parse(&entry.path) else {
            continue;
        };
        if let Some(id) = note_path.id() {
            by_id.insert(id, &entry.path);
        }
        if let Some(anki_id) = blobs.get(&entry.sha).and_then(|blob| blob_anki_id(blob)) {
            by_anki_id.entry(anki_id).or_insert(&entry.path);
        }
    }
    let entry_at: HashMap<&str, &TreeEntry> = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();

    // The path of a note of the database or the parent
    let path_of_id = |id: NoteId| -> Option<String> {
        if let Some(path) = by_id.get(&id) {
            return Some((*path).to_string());
        }
        let row = db_notes.rows.get(&id)?;
        let parser = find_parser(&row.parser_name, &parsers).ok()?;
        Some(
            note_rel_path(parser.as_ref(), id)
                .to_string_lossy()
                .into_owned(),
        )
    };

    // Path → (mode, sha) of the snapshot
    let mut tree: BTreeMap<String, (String, String)> = BTreeMap::new();
    for raw in notes {
        let anki_note = raw.to_note();
        let known_id = anki_note
            .spares_id
            .filter(|id| by_id.contains_key(id) || db_notes.rows.contains_key(id));
        let mut path = known_id
            .and_then(path_of_id)
            .or_else(|| hints.get(&raw.id).cloned())
            .or_else(|| {
                db_notes
                    .by_anki_id
                    .get(&raw.id)
                    .and_then(|id| path_of_id(*id))
            })
            .or_else(|| by_anki_id.get(&raw.id).map(|path| (*path).to_string()));
        // Two Anki notes with the same `SparesId` (a duplicated note): the later is a new note.
        if path.as_ref().is_some_and(|path| tree.contains_key(path)) {
            path = None;
        }
        let path = if let Some(path) = path {
            path
        } else {
            let parser = find_parser(&anki_note.parser_name, &parsers)
                .or_else(|_| find_parser(&db_notes.default_parser, &parsers))
                .or_else(|_| find_parser(FALLBACK_PARSER, &parsers))?;
            format!(
                "{}/new-anki-{}.{}",
                parser.get_parser_name(),
                raw.id,
                parser.file_extension()
            )
        };
        let Some(note_path) = NotePath::parse(&path) else {
            continue;
        };
        // The parent's file, or if it has none (a note spares created from an Anki note after
        // the parent), the file spares writes, so `anki` gets the file `main` has.
        let parent_entry = entry_at.get(path.as_str()).copied();
        let base: Option<String> = match parent_entry {
            Some(entry) => blobs.get(&entry.sha).cloned(),
            None => note_path
                .id()
                .and_then(|id| db_notes.rows.get(&id))
                .filter(|row| row.parser_name == note_path.parser().get_parser_name())
                .map(|row| construct_db_note_file(note_path.parser(), row, &db_notes.links))
                .transpose()?,
        };
        let contents = note_contents(&note_path, base.as_deref(), raw, &anki_note, &db_notes)?;
        let entry = match (contents, parent_entry, base) {
            (None, Some(entry), _) => (entry.mode.clone(), entry.sha.clone()),
            (Some(contents), _, _) | (None, None, Some(contents)) => {
                ("100644".to_string(), hash_blob(repo, &contents)?)
            }
            (None, None, None) => continue,
        };
        tree.insert(path, entry);
    }

    for entry in &entries {
        if tree.contains_key(&entry.path) {
            continue;
        }
        let in_anki_once = NotePath::parse(&entry.path).is_some()
            && blobs
                .get(&entry.sha)
                .is_some_and(|blob| blob_anki_id(blob).is_some());
        // A note that was in Anki and is not anymore was deleted there.
        if !in_anki_once {
            tree.insert(entry.path.clone(), (entry.mode.clone(), entry.sha.clone()));
        }
    }
    write_tree(repo, &tree)
}

/// The file of an Anki note at `note_path`, or `None` to keep `base`, the parent's file there.
fn note_contents(
    note_path: &NotePath,
    base: Option<&str>,
    raw: &AnkiRawNote,
    anki_note: &AnkiNote,
    db_notes: &DbNotes,
) -> Result<Option<String>, Error> {
    let parser = note_path.parser();
    let parsed = base.and_then(|base| parse_file(note_path, base));
    let anki_id_value = Value::String(raw.id.to_string());
    if let (Some(base), Some((settings, data))) = (base, &parsed)
        && holds(parser, note_path.id(), settings, data, raw, anki_note)
    {
        if settings.custom_data.get(ANKI_NOTE_ID_KEY) == Some(&anki_id_value) {
            return Ok(None);
        }
        // Anki has the note, but the file does not record it yet (it was just added to Anki):
        // write it as `main` does once spares records the id, so merging both is clean.
        let mut custom_data = settings.custom_data.clone();
        custom_data.insert(ANKI_NOTE_ID_KEY.to_string(), anki_id_value);
        if let Some(id) = note_path.id()
            && let Some(row) = db_notes.rows.get(&id)
            && row.parser_name == parser.get_parser_name()
        {
            let mut row_custom_data = row.custom_data.as_object().cloned().unwrap_or_default();
            row_custom_data.insert(
                ANKI_NOTE_ID_KEY.to_string(),
                Value::String(raw.id.to_string()),
            );
            let row = RenderNoteData {
                note_id: row.note_id,
                data: row.data.clone(),
                keywords_value: row.keywords_value.clone(),
                custom_data: Value::Object(row_custom_data),
                parser_name: row.parser_name.clone(),
                tags_value: row.tags_value.clone(),
            };
            let file_fields = file_note_fields(parser, base, id).map(|mut fields| {
                fields.custom_data.clone_from(&custom_data);
                fields
            });
            if file_fields
                .is_some_and(|fields| same_note(parser, id, &fields, &db_note_fields(&row)))
            {
                return construct_db_note_file(parser, &row, &db_notes.links).map(Some);
            }
        }
        let request = GenerateNoteFilesRequest {
            note_id: note_path.id().unwrap_or(PLACEHOLDER_ID),
            keywords: extra_keywords(parser, settings, data),
            tags: settings.tags.clone(),
            linked_notes: note_path
                .id()
                .and_then(|id| linked_notes_from_map(&db_notes.links, id)),
            note_data: data.clone(),
            custom_data,
        };
        return render(parser, note_path.id(), &request).map(Some);
    }

    // Anki's version, with what only spares has from the parent's file
    let mut custom_data: CustomData = parsed
        .as_ref()
        .map(|(settings, _)| settings.custom_data.clone())
        .unwrap_or_default();
    for (key, value) in &anki_note.custom_data {
        custom_data.insert(key.clone(), value.clone());
    }
    custom_data.shift_remove(NOTE_ID_KEY);
    let request = GenerateNoteFilesRequest {
        note_id: note_path.id().unwrap_or(PLACEHOLDER_ID),
        keywords: anki_note.keywords.clone(),
        tags: anki_note.tags.clone(),
        linked_notes: note_path
            .id()
            .and_then(|id| linked_notes_from_map(&db_notes.links, id)),
        note_data: anki_note.note_data.clone(),
        custom_data,
    };
    render(parser, note_path.id(), &request).map(Some)
}

/// Whether Anki's note is what spares writes to Anki for the note `settings` and `data` describe
fn holds(
    parser: &dyn Parseable,
    id: Option<NoteId>,
    settings: &NoteSettings,
    data: &str,
    raw: &AnkiRawNote,
    anki_note: &AnkiNote,
) -> bool {
    let mut settings = settings.clone();
    if let Some(id) = id {
        settings
            .custom_data
            .insert(NOTE_ID_KEY.to_string(), Value::String(id.letters()));
    }
    let Ok(fields) = to_anki_fields(parser, &settings, data) else {
        return false;
    };
    let projected = AnkiRawNote {
        id: raw.id,
        flds: fields.joined(),
        tags: fields.tags.join(" "),
    }
    .to_note();
    // `SparesId` and `SparesParserName` say where a note goes, which the path already does: notes
    // added in Anki and matched by their Anki id have none.
    projected.note_data == anki_note.note_data
        && projected.keywords == anki_note.keywords
        && projected.custom_data == anki_note.custom_data
        && sorted_tags(&projected.tags) == sorted_tags(&anki_note.tags)
}

/// Anki splits tags at spaces, and sorts them
fn sorted_tags(tags: &[String]) -> Vec<&str> {
    let mut tags: Vec<&str> = tags
        .iter()
        .flat_map(|tag| tag.split(' '))
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort_unstable();
    tags.dedup();
    tags
}

fn parse_file(note_path: &NotePath, contents: &str) -> Option<(NoteSettings, String)> {
    match note_path {
        NotePath::Note(parser, id) => parse_note_file(parser.as_ref(), contents, *id, false)
            .ok()
            .map(|parsed| (parsed.settings, parsed.data)),
        NotePath::New(parser) => parse_new_note_file(parser.as_ref(), contents, false)
            .ok()
            .map(|parsed| (parsed.settings, parsed.data)),
    }
}

/// The keywords a file lists, which are those not embedded in its data
fn extra_keywords(parser: &dyn Parseable, settings: &NoteSettings, data: &str) -> Vec<String> {
    let embedded: Vec<&str> = parser
        .get_embedded_keywords(data)
        .map(|ranges| ranges.into_iter().map(|range| &data[range]).collect())
        .unwrap_or_default();
    settings
        .keywords
        .iter()
        .filter(|keyword| !embedded.contains(&keyword.as_str()))
        .cloned()
        .collect()
}

/// Rendered in place of a new note's id, which is then removed
const PLACEHOLDER_ID: NoteId = NoteId::UNSET;

/// A note's file. Without `id`, the file of a new note: without a note id.
pub(crate) fn render(
    parser: &dyn Parseable,
    id: Option<NoteId>,
    request: &GenerateNoteFilesRequest,
) -> Result<String, Error> {
    let (template, placeholder) =
        parser
            .get_template_data(TemplateType::Note)
            .map_err(|e| Error::Io {
                description: format!(
                    "Failed to read template for parser {}",
                    parser.get_parser_name()
                ),
                source: e,
            })?;
    let contents = construct_note_file_contents(parser, &template, &placeholder, request);
    if id.is_some() {
        return Ok(contents);
    }
    let keys = parser.note_settings_keys();
    let id_setting = parser.construct_setting(&format!(
        "{}{} {}",
        keys.note_id.get_write(),
        keys.settings_key_value_delim,
        request.note_id.letters()
    ));
    Ok(contents.replacen(&id_setting, "", 1))
}

fn read_tree(repo: &Path, commit: &str) -> Result<Vec<TreeEntry>, Error> {
    let out = git(repo, &["ls-tree", "-r", "-z", "--full-tree", commit], None)?;
    Ok(nul_split(&out)
        .filter_map(|line| {
            let line = String::from_utf8_lossy(line);
            let (meta, path) = line.split_once('\t')?;
            let mut meta = meta.split(' ');
            let mode = meta.next()?.to_string();
            let kind = meta.next()?;
            let sha = meta.next()?.to_string();
            (kind == "blob").then(|| TreeEntry {
                mode,
                sha,
                path: path.to_string(),
            })
        })
        .collect())
}

/// The contents of blobs that are UTF-8, by sha, read with one `git cat-file --batch`
fn read_blobs(repo: &Path, shas: &[&str]) -> Result<HashMap<String, String>, Error> {
    let mut blobs = HashMap::new();
    if shas.is_empty() {
        return Ok(blobs);
    }
    let input = shas.join("\n") + "\n";
    let out = git(repo, &["cat-file", "--batch"], Some(input.as_bytes()))?;
    let mut rest = out.as_slice();
    while let Some(newline) = rest.iter().position(|&b| b == b'\n') {
        let header = String::from_utf8_lossy(&rest[..newline]).into_owned();
        rest = &rest[newline + 1..];
        let mut fields = header.split(' ');
        let (Some(sha), Some(_kind), Some(size)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(size) = size.parse::<usize>() else {
            continue;
        };
        let contents = &rest[..size.min(rest.len())];
        if let Ok(contents) = std::str::from_utf8(contents) {
            blobs.insert(sha.to_string(), contents.to_string());
        }
        // The contents are followed by a newline
        rest = &rest[(size + 1).min(rest.len())..];
    }
    Ok(blobs)
}

fn hash_blob(repo: &Path, contents: &str) -> Result<String, Error> {
    let out = git(
        repo,
        &["hash-object", "-w", "--stdin"],
        Some(contents.as_bytes()),
    )?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Writes `tree` (path → mode and sha) with a temporary index, leaving the repository's index
/// alone.
fn write_tree(repo: &Path, tree: &BTreeMap<String, (String, String)>) -> Result<String, Error> {
    let index = repo
        .join(".git")
        .join(format!("spares-anki-index-{}", std::process::id()));
    let _ = std::fs::remove_file(&index);
    let result = (|| {
        let mut input = Vec::new();
        for (path, (mode, sha)) in tree {
            input.extend_from_slice(format!("{mode} {sha}\t{path}").as_bytes());
            input.push(0);
        }
        run_with_index(
            repo,
            &index,
            &["update-index", "--add", "-z", "--index-info"],
            &input,
        )?;
        run_with_index(repo, &index, &["write-tree"], &[])
    })();
    let _ = std::fs::remove_file(&index);
    result
}

fn run_with_index(
    repo: &Path,
    index: &PathBuf,
    args: &[&str],
    stdin: &[u8],
) -> Result<String, Error> {
    let mut command = git_command(repo);
    command
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| Error::NotesVcs(format!("Failed to run git: {e}")))?;
    let mut pipe = child.stdin.take().expect("stdin is piped");
    pipe.write_all(stdin)
        .map_err(|e| Error::NotesVcs(format!("Failed to write to git: {e}")))?;
    drop(pipe);
    let output = child
        .wait_with_output()
        .map_err(|e| Error::NotesVcs(format!("Failed to run git: {e}")))?;
    if !output.status.success() {
        return Err(super::git::git_error(args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A push to `anki`, from the notes repository's `update` hook. It runs after git received the
/// pushed objects and before it moves the ref.
#[derive(Debug, Deserialize, Serialize)]
pub struct AnkiReceiveRequest {
    /// The repository the hook runs in, which must be the server's
    pub repo: PathBuf,
    /// `anki` before the push
    pub old: String,
    /// The commit being pushed to `anki`
    pub new: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AnkiReceiveResponse {
    /// Whether git may move `anki`
    pub ok: bool,
    pub message: String,
    pub session: Option<String>,
}

/// From `post-receive`, once git moved `anki`
#[derive(Debug, Deserialize, Serialize)]
pub struct AnkiFinishRequest {
    pub session: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AnkiFinishResponse {
    pub message: String,
    /// Whether spares committed what Anki holds on top of the push, so there is something to fetch
    pub changed: bool,
}

/// An accepted push to `anki` that git has not moved the ref for yet. It holds the exclusive notes
/// lock, so a refresh cannot move `anki` between Anki being written and git moving the ref.
struct AnkiSession {
    id: String,
    old: String,
    new: String,
    /// Paths of the notes added from `new-*` files, by their new Anki id
    hints: HashMap<i64, String>,
    _guard: NotesGuard,
}

static ANKI_SESSION: Mutex<Option<AnkiSession>> = Mutex::new(None);

fn anki_session() -> MutexGuard<'static, Option<AnkiSession>> {
    ANKI_SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

/// The session id for `post-receive`, which runs in another process than the `update` hook.
pub fn anki_session_file_path() -> PathBuf {
    server_repo_dir().join(".git").join("spares-anki-session")
}

fn take_anki_session(id: &str) -> Option<AnkiSession> {
    let mut session = anki_session();
    if session.as_ref().is_some_and(|s| s.id != id) {
        return None;
    }
    session.take()
}

enum AnkiOutcome {
    Accepted {
        message: String,
        hints: HashMap<i64, String>,
    },
    Rejected(String),
}

/// Writes a push to `anki` into Anki. If it is accepted, the exclusive notes lock stays held until
/// [`finish`] (from `post-receive`) or the watchdog records what Anki then holds.
pub async fn receive(
    db: SqlitePool,
    backend: Arc<dyn AnkiBackend>,
    request: AnkiReceiveRequest,
) -> Result<AnkiReceiveResponse, Error> {
    let guard = exclusive_guard(None).await?;
    // Its own task, so a hook killed mid-request cannot stop writing to Anki halfway.
    let handle = tokio::spawn(async move {
        let outcome = receive_locked(&db, backend.as_ref(), &request)
            .await
            .unwrap_or_else(|e| {
                AnkiOutcome::Rejected(format!("spares failed to process the push to `anki`: {e}"))
            });
        match outcome {
            AnkiOutcome::Rejected(message) => AnkiReceiveResponse {
                ok: false,
                message,
                session: None,
            },
            AnkiOutcome::Accepted { message, hints } => {
                let id = format!(
                    "anki-{}-{}",
                    chrono::Utc::now().timestamp_millis(),
                    short(&request.new)
                );
                if let Err(e) = std::fs::write(anki_session_file_path(), &id) {
                    log::error!("Failed to write the `anki` push session: {e}");
                }
                *anki_session() = Some(AnkiSession {
                    id: id.clone(),
                    old: request.old.clone(),
                    new: request.new.clone(),
                    hints,
                    _guard: guard,
                });
                let watchdog_id = id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(WATCHDOG_DELAY).await;
                    if let Some(session) = take_anki_session(&watchdog_id) {
                        log::warn!(
                            "post-receive did not report the push of {} to `anki`; settling it",
                            session.new
                        );
                        let _ = settle(&db, backend.as_ref(), session).await;
                    }
                });
                AnkiReceiveResponse {
                    ok: true,
                    message,
                    session: Some(id),
                }
            }
        }
    });
    handle
        .await
        .map_err(|e| Error::NotesVcs(format!("The push task failed: {e}")))
}

/// Settles an accepted push to `anki` once git moved the ref: commits what Anki now holds on top,
/// if it differs from the push, and releases the lock.
pub async fn finish(
    db: SqlitePool,
    backend: Arc<dyn AnkiBackend>,
    request: AnkiFinishRequest,
) -> Result<AnkiFinishResponse, Error> {
    let Some(session) = take_anki_session(&request.session) else {
        return Ok(AnkiFinishResponse {
            message: "spares has already settled this push".to_string(),
            changed: false,
        });
    };
    let handle = tokio::spawn(async move { settle(&db, backend.as_ref(), session).await });
    handle
        .await
        .map_err(|e| Error::NotesVcs(format!("Settling the push failed: {e}")))?
}

async fn settle(
    db: &SqlitePool,
    backend: &dyn AnkiBackend,
    session: AnkiSession,
) -> Result<AnkiFinishResponse, Error> {
    let _ = std::fs::remove_file(anki_session_file_path());
    let repo = server_repo_dir();
    // git moved `anki` unless something went wrong after the hook; either way, record on top of
    // where it is what Anki now holds.
    let tip = anki_tip(&repo)?.unwrap_or(session.old.clone());
    let notes = backend.read_notes().await?;
    let committed = commit_snapshot(db, &repo, &tip, Some(&tip), &notes, &session.hints).await?;
    let message = match (&committed, tip == session.new) {
        (None, true) => "Anki holds what you pushed".to_string(),
        (Some(_), true) => "spares committed Anki's ids of the added notes (or how Anki stored them) on top of your push; fetch (`jj spares-anki-pull` / `spares vcs refresh anki && git fetch`) before merging again".to_string(),
        (_, false) => {
            let message = format!(
                "the push of {} did not reach `anki` after Anki was written; spares committed what Anki holds to `anki`, so fetch to see it",
                short(&session.new)
            );
            log::warn!("{message}");
            message
        }
    };
    Ok(AnkiFinishResponse {
        message,
        changed: committed.is_some(),
    })
}

/// What [`AnkiBackend::apply`] takes
type ApplyNotes = Vec<(NoteSettings, Option<String>)>;

/// A note the push changes, as Anki gets it
struct AnkiChange {
    path: String,
    /// The pushed file
    blob: String,
    parser: Box<dyn Parseable>,
    settings: NoteSettings,
    data: String,
    /// The spares note it is, for a canonical path
    spares_id: Option<NoteId>,
}

#[expect(clippy::too_many_lines)]
async fn receive_locked(
    db: &SqlitePool,
    backend: &dyn AnkiBackend,
    request: &AnkiReceiveRequest,
) -> Result<AnkiOutcome, Error> {
    // 1. The hook's repository must be the one this server writes to.
    if let Some(reason) = wrong_repo(&request.repo) {
        return Ok(AnkiOutcome::Rejected(reason));
    }
    let repo = server_repo_dir();
    let new = request.new.trim().to_ascii_lowercase();
    let old = request.old.trim().to_ascii_lowercase();
    for sha in [&old, &new] {
        if !(sha.len() == 40 || sha.len() == 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(AnkiOutcome::Rejected(format!("invalid commit id `{sha}`")));
        }
    }
    let Some(tip) = anki_tip(&repo)? else {
        return Ok(AnkiOutcome::Rejected(
            "`anki` does not exist yet; run `spares vcs refresh anki` (or `jj spares-anki-pull`), and push on top of it"
                .to_string(),
        ));
    };
    if tip != old {
        return Ok(AnkiOutcome::Rejected(
            "`anki` moved since your push started; fetch and push again".to_string(),
        ));
    }

    // 2. Anki must still hold what `old` describes.
    let notes = backend.read_notes().await?;
    let current = snapshot_tree(db, &repo, &old, &notes, &HashMap::new()).await?;
    if current != rev_parse(&repo, &format!("{old}^{{tree}}"))? {
        commit_tree(&repo, &current, &old, Some(&old))?;
        return Ok(AnkiOutcome::Rejected(
            "Anki has newer notes than your push is based on; spares committed them to `anki`. Fetch them (`jj spares-anki-pull`, or `spares vcs refresh anki && git fetch`), merge, and push again"
                .to_string(),
        ));
    }

    // 3. Fast-forward only
    let is_ancestor = git_output(&repo, &["merge-base", "--is-ancestor", &old, &new], None)?;
    match is_ancestor.status.code() {
        Some(0) => {}
        Some(1) => {
            return Ok(AnkiOutcome::Rejected(
                "your push to `anki` is not based on its current commit; fetch, merge or rebase onto `anki@origin`, and push again"
                    .to_string(),
            ));
        }
        _ => {
            return Err(git_error(
                &["merge-base", "--is-ancestor", &old, &new],
                &is_ancestor,
            ));
        }
    }

    // 4. Which paths may change, and the notes they are
    let raw = git(
        &repo,
        &[
            "diff",
            "--raw",
            "-z",
            "--no-renames",
            "--no-abbrev",
            &old,
            &new,
        ],
        None,
    )?;
    let mut errors: Vec<String> = Vec::new();
    let mut changes: Vec<AnkiChange> = Vec::new();
    let mut deletes: Vec<(String, i64)> = Vec::new();
    let mut tokens = nul_split(&raw);
    while let Some(header) = tokens.next() {
        let Some(path) = tokens.next() else { break };
        let path = String::from_utf8_lossy(path).into_owned();
        let header = String::from_utf8_lossy(header);
        let fields: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let [_, new_mode, old_sha, new_sha, status] = fields[..] else {
            errors.push(format!("{path}: unexpected diff entry `{header}`"));
            continue;
        };
        if path == README {
            continue;
        }
        if path == ".gitignore" || path == ".gitattributes" {
            errors.push(format!(
                "{path}: is managed by spares and cannot be changed"
            ));
            continue;
        }
        let Some(note_path) = NotePath::parse(&path) else {
            errors.push(format!(
                "{path}: is not a note file; only notes (`<parser>/<id>.<ext>`), new notes (`<parser>/new-<name>.<ext>`) and `{README}` can be pushed"
            ));
            continue;
        };
        if status == "D" {
            let blob = git(&repo, &["cat-file", "blob", old_sha], None)?;
            // A note that was never in Anki has nothing to delete there.
            if let Some(anki_id) = blob_anki_id(&String::from_utf8_lossy(&blob)) {
                deletes.push((path, anki_id));
            }
            continue;
        }
        if !matches!(status, "A" | "M") {
            errors.push(format!("{path}: has an unsupported change ({status})"));
            continue;
        }
        if new_mode != "100644" {
            errors.push(format!(
                "{path}: has mode {new_mode}; only regular files (100644) are allowed"
            ));
            continue;
        }
        let blob = git(&repo, &["cat-file", "blob", new_sha], None)?;
        let Ok(blob) = String::from_utf8(blob) else {
            errors.push(format!("{path}: is not valid UTF-8"));
            continue;
        };
        if blob.contains('\0') {
            errors.push(format!("{path}: contains a NUL byte"));
            continue;
        }
        if let Some(line) = conflict_marker_line(&blob) {
            errors.push(format!(
                "{path}: line {line} is a conflict marker; resolve the conflict first"
            ));
            continue;
        }
        let parsed = match &note_path {
            NotePath::Note(parser, id) => parse_note_file(parser.as_ref(), &blob, *id, false)
                .map(|parsed| (parsed.settings, parsed.data)),
            NotePath::New(parser) => parse_new_note_file(parser.as_ref(), &blob, false)
                .map(|parsed| (parsed.settings, parsed.data)),
        };
        let (settings, data) = match parsed {
            Ok(parsed) => parsed,
            Err(problems) => {
                errors.extend(problems.into_iter().map(|p| format!("{path}: {p}")));
                continue;
            }
        };
        let spares_id = note_path.id();
        let (NotePath::Note(parser, _) | NotePath::New(parser)) = note_path;
        changes.push(AnkiChange {
            path,
            blob,
            parser,
            settings,
            data,
            spares_id,
        });
    }
    // A copied note file keeps the Anki id of the note it was copied from, which would update
    // that Anki note instead of adding one.
    let new_entries = read_tree(&repo, &new)?;
    let note_shas: Vec<&str> = new_entries
        .iter()
        .filter(|entry| NotePath::parse(&entry.path).is_some())
        .map(|entry| entry.sha.as_str())
        .collect();
    let new_blobs = read_blobs(&repo, &note_shas)?;
    let mut paths_by_anki_id: BTreeMap<i64, Vec<&str>> = BTreeMap::new();
    for entry in &new_entries {
        if let Some(anki_id) = new_blobs
            .get(&entry.sha)
            .and_then(|blob| blob_anki_id(blob))
        {
            paths_by_anki_id
                .entry(anki_id)
                .or_default()
                .push(&entry.path);
        }
    }
    for change in &changes {
        let Some(anki_id) = change
            .settings
            .custom_data
            .get(ANKI_NOTE_ID_KEY)
            .and_then(anki_id_of)
        else {
            continue;
        };
        let others: Vec<&&str> = paths_by_anki_id
            .get(&anki_id)
            .into_iter()
            .flatten()
            .filter(|path| **path != change.path)
            .collect();
        if let Some(other) = others.first() {
            errors.push(format!(
                "{}: has the Anki id of `{other}` (anki-note-id {anki_id}); remove `anki-note-id` from the custom data of a copied note",
                change.path
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(AnkiOutcome::Rejected(format!(
            "spares rejected the push, so Anki was not changed:\n{}",
            errors
                .iter()
                .map(|e| format!("  - {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        )));
    }

    // 5. What Anki gets: a note it has is updated, any other added, by parser
    // By parser: the notes for Anki, and the position in `changes` of each note it adds or updates
    let mut by_parser: BTreeMap<String, (ApplyNotes, Vec<usize>)> = BTreeMap::new();
    let (mut added, mut updated) = (0, 0);
    // A note deleted in Anki whose file is restored on `anki` still has its old Anki id.
    let in_anki: std::collections::HashSet<i64> = notes.iter().map(|note| note.id).collect();
    for (i, change) in changes.iter().enumerate() {
        let mut settings = change.settings.clone();
        let anki_id = settings
            .custom_data
            .get(ANKI_NOTE_ID_KEY)
            .and_then(anki_id_of)
            .filter(|anki_id| in_anki.contains(anki_id));
        settings.action = if let Some(anki_id) = anki_id {
            updated += 1;
            NoteImportAction::Update(NoteId::from_packed(anki_id))
        } else {
            added += 1;
            NoteImportAction::Add
        };
        if let Some(id) = change.spares_id {
            settings
                .custom_data
                .insert(NOTE_ID_KEY.to_string(), Value::String(id.letters()));
        }
        let entry = by_parser
            .entry(change.parser.get_parser_name().to_string())
            .or_default();
        entry.0.push((settings, Some(change.data.clone())));
        entry.1.push(i);
    }
    for (path, anki_id) in &deletes {
        let Some(note_path) = NotePath::parse(path) else {
            continue;
        };
        let settings = NoteSettings {
            action: NoteImportAction::Delete(NoteId::from_packed(*anki_id)),
            ..NoteSettings::default()
        };
        by_parser
            .entry(note_path.parser().get_parser_name().to_string())
            .or_default()
            .0
            .push((settings, Some(String::new())));
    }

    // 6. Write Anki. It has no transactions, so a failure leaves some changes made.
    let mut added_ids: Vec<(usize, i64)> = Vec::new();
    for (parser_name, (notes, positions)) in by_parser {
        let parser = find_parser(&parser_name, &get_all_parsers())?;
        match backend.apply(parser.as_ref(), notes).await {
            Ok(added) => added_ids.extend(
                added
                    .into_iter()
                    .filter_map(|(i, anki_id)| Some((*positions.get(i)?, anki_id))),
            ),
            Err(e) => {
                let notes = backend.read_notes().await?;
                commit_snapshot(db, &repo, &old, Some(&old), &notes, &HashMap::new()).await?;
                return Ok(AnkiOutcome::Rejected(format!(
                    "writing to Anki failed partway ({e}), so some of the push is in Anki; spares committed what Anki now holds to `anki`. Fetch it (`jj spares-anki-pull`, or `spares vcs refresh anki && git fetch`), merge, and push again"
                )));
            }
        }
    }

    // 7. Record in spares the Anki ids of the notes it has, if their file is `main`'s, as after
    // merging `main` into `anki`: `main` then gets the same line as the snapshot writes on `anki`,
    // and both files stay the same. A file changed on `anki` gets its id there, and `main` gets
    // it by merging, since the same line added next to a change on one side would conflict.
    // Recording commits to `main`, which no push is importing, since this holds the exclusive lock.
    let mut hints = HashMap::new();
    let mut warnings = Vec::new();
    for (i, anki_id) in added_ids {
        let change = &changes[i];
        let mains = git_output(
            &repo,
            &["cat-file", "blob", &format!("HEAD:{}", change.path)],
            None,
        )?;
        let same_as_main = mains.status.success() && mains.stdout == change.blob.as_bytes();
        match change.spares_id {
            Some(_) if !same_as_main => {}
            Some(id) => {
                if let Err(e) = record_anki_id(db, id, anki_id).await {
                    warnings.push(format!(
                        "note {}: recording its Anki id failed ({e}); `spares vcs refresh anki` finds it by its SparesId",
                        id.letters()
                    ));
                }
            }
            None => {
                hints.insert(anki_id, change.path.clone());
            }
        }
    }
    let mut message = format!(
        "added {added}, updated {updated} and deleted {} Anki note(s)",
        deletes.len()
    );
    for warning in warnings {
        let _ = write!(message, "\n  - {warning}");
    }
    Ok(AnkiOutcome::Accepted { message, hints })
}

/// Adds the Anki note id to a spares note's custom data.
async fn record_anki_id(db: &SqlitePool, id: NoteId, anki_id: i64) -> Result<(), Error> {
    let mut custom_data = get_note(db, id).await?.custom_data;
    custom_data.insert(
        ANKI_NOTE_ID_KEY.to_string(),
        Value::String(anki_id.to_string()),
    );
    update_notes(
        db,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![id]),
            parser_id: None,
            data: None,
            keywords: None,
            tags: UpdateTags::None,
            custom_data: Some(custom_data),
        },
        chrono::Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await?;
    Ok(())
}

/// Sets the `SparesId` of Anki notes that a push to `main` created spares notes from, so Anki
/// knows them by their spares id too. Best effort: refreshes also find them by their Anki id.
pub async fn link_spares_ids(
    backend: &dyn AnkiBackend,
    ids: &[(i64, NoteId)],
) -> Result<(), Error> {
    if ids.is_empty() {
        return Ok(());
    }
    backend.set_spares_ids(ids).await
}

/// From `post-receive` after a push to `main`: the [`super::FinishResponse::anki_links`]
#[derive(Debug, Deserialize, Serialize)]
pub struct LinkSparesIdsRequest {
    /// `(Anki id, spares id)`
    pub ids: Vec<(i64, NoteId)>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LinkSparesIdsResponse {
    /// Whether Anki was open to set them
    pub linked: bool,
}
