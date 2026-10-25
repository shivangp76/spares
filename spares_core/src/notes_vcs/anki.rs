//! The notes repository's `anki` branch, which describes Anki's notes. Fetching it shows Anki's
//! state, and pushing to it writes to Anki, so moving notes between Anki and spares is a merge
//! between `anki` and `main`.
//!
//! git cannot run anything on fetch, so the branch is brought up to date explicitly by [`refresh`]
//! (`spares note anki-refresh`). It is never checked out: it is written with a temporary index, so
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
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;

use super::git::git;
use super::git::git_command;
use super::git::git_output;
use super::git::nul_split;
use super::git::rev_parse;
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
use crate::Error;
use crate::adapters::impls::anki::AnkiNote;
use crate::adapters::impls::anki::backend::AnkiBackend;
use crate::adapters::impls::anki::backend::AnkiRawNote;
use crate::adapters::impls::anki::to_anki_fields;
use crate::api::note::RenderNoteData;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::helpers::get_or_compile_regex;
use crate::model::CustomData;
use crate::model::NOTE_ID_KEY;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;
use crate::parsers::TemplateType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::generate_files::construct_note_file_contents;
use crate::parsers::get_all_parsers;

/// The branch describing Anki
pub const ANKI_REF: &str = "refs/heads/anki";

/// Where an Anki note's id is kept in a spares note's custom data
pub const ANKI_NOTE_ID_KEY: &str = "anki-note-id";

/// Parser of Anki notes whose `SparesParserName` is empty or not installed, if spares has no notes
/// to take the most used parser from
const FALLBACK_PARSER: &str = "markdown";

/// `spares note anki-refresh`
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
            "this spares server does not keep notes in a git repository (run `spares note init-vcs`)"
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
    let new = match commit_snapshot(db, &repo, &parent, old.as_deref(), &notes).await? {
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
) -> Result<Option<String>, Error> {
    let tree = snapshot_tree(db, repo, parent, notes).await?;
    let parent_tree = rev_parse(repo, &format!("{parent}^{{tree}}"))?;
    if tree == parent_tree {
        return Ok(None);
    }
    let commit = String::from_utf8_lossy(&git(
        repo,
        &[
            "commit-tree",
            &tree,
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
async fn snapshot_tree(
    db: &SqlitePool,
    repo: &Path,
    parent: &str,
    notes: &[AnkiRawNote],
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
const PLACEHOLDER_ID: NoteId = NoteId::from_packed(0);

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
