use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;

use super::commit::NEEDS_RECONCILE;
use super::commit::commit_message;
use super::commit::git_index_lock;
use super::commit::try_commit;
use super::git::git;
use super::git::git_output;
use super::git::nul_split;
use super::note_file::construct_db_note_file;
use super::note_file::db_note_fields;
use super::note_file::file_note_fields;
use super::note_file::same_note;
use super::paths::enabled;
use super::paths::note_rel_path;
use super::paths::note_rel_paths_all_parsers;
use super::paths::parse_note_rel_path;
use super::paths::server_repo_dir;
use crate::Error;
use crate::api::note::RenderNoteData;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::model::NoteId;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;

/// Which notes [`reconcile`] regenerates from the database.
#[derive(Clone, Debug)]
pub enum ReconcileScope {
    /// Paths that differ from `HEAD`, e.g. written by a server that was killed before committing
    Dirty,
    /// Notes the database changed without writing their files (undo, tag and parser changes)
    Notes(Vec<NoteId>),
    /// Every note, comparing semantically so only real differences are rewritten
    All,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ReconcileReport {
    /// Note files written from the database (or that would be, for a dry run)
    pub rewritten: Vec<PathBuf>,
    /// Files of notes that no longer exist, or now belong to another parser
    pub removed: Vec<PathBuf>,
    /// Other tracked files restored to `HEAD`
    pub restored: Vec<PathBuf>,
    /// Files spares did not write, copied to [`overwritten_dir`] before being overwritten or
    /// removed
    pub saved: Vec<PathBuf>,
    pub committed: bool,
}

impl ReconcileReport {
    pub fn is_empty(&self) -> bool {
        self.rewritten.is_empty()
            && self.removed.is_empty()
            && self.restored.is_empty()
            && self.saved.is_empty()
    }
}

/// Where files spares did not write are saved before it overwrites or removes them, in a directory
/// per reconcile named by its time. Nothing reads them back or removes them.
pub fn overwritten_dir() -> PathBuf {
    server_repo_dir().join(".git").join("spares-overwritten")
}

/// Moves copies saved under the directory's former name, `spares-quarantine`, to
/// [`overwritten_dir`]. Runs at startup.
pub(crate) fn rename_quarantine_dir() {
    let old = server_repo_dir().join(".git").join("spares-quarantine");
    let new = overwritten_dir();
    if old.exists()
        && !new.exists()
        && let Err(e) = std::fs::rename(&old, &new)
    {
        log::error!(
            "Failed to rename {} to {}: {e}",
            old.display(),
            new.display()
        );
    }
}

/// Makes the notes repository describe the database again for `scope`, and commits the result.
///
/// The database is the source of truth: note files are regenerated from it, files of notes that no
/// longer exist are removed, and other modified tracked files are restored to `HEAD`. Anything
/// overwritten or removed that differs from `HEAD` (so was not committed by spares) is first saved
/// to [`overwritten_dir`]. Callers hold the exclusive notes lock.
pub async fn reconcile(
    db: &SqlitePool,
    scope: ReconcileScope,
    message: Option<&str>,
) -> Result<ReconcileReport, Error> {
    reconcile_inner(db, scope, message, false).await
}

/// What [`reconcile`] would change, without changing anything.
pub async fn reconcile_dry_run(
    db: &SqlitePool,
    scope: ReconcileScope,
) -> Result<ReconcileReport, Error> {
    reconcile_inner(db, scope, None, true).await
}

/// Logs instead of failing: for operations whose database change already happened.
pub async fn reconcile_after(db: &SqlitePool, note_ids: Vec<NoteId>, what: &str) {
    if !enabled() || note_ids.is_empty() {
        return;
    }
    let message = commit_message(what, &note_ids);
    if let Err(e) = reconcile(db, ReconcileScope::Notes(note_ids), Some(&message)).await {
        NEEDS_RECONCILE.store(true, Ordering::SeqCst);
        log::error!("Failed to update note files after `{what}`: {e}");
    }
}

#[expect(clippy::too_many_lines)]
async fn reconcile_inner(
    db: &SqlitePool,
    scope: ReconcileScope,
    message: Option<&str>,
    dry_run: bool,
) -> Result<ReconcileReport, Error> {
    let mut report = ReconcileReport::default();
    if !enabled() {
        return Ok(report);
    }
    let repo = server_repo_dir();
    if !dry_run {
        remove_stale_index_lock(&repo);
    }

    let tracked = tracked_paths(&repo)?;
    let (note_ids, other_dirty, semantic) = match &scope {
        ReconcileScope::Dirty => {
            let mut note_ids = BTreeSet::new();
            let mut others = Vec::new();
            for (path, is_tracked) in dirty_paths(&repo)? {
                match parse_note_rel_path(&path) {
                    Some((_, id)) => {
                        note_ids.insert(id);
                    }
                    None => others.push((path, is_tracked)),
                }
            }
            (note_ids, others, false)
        }
        ReconcileScope::Notes(ids) => (ids.iter().copied().collect(), Vec::new(), false),
        ReconcileScope::All => {
            let mut note_ids: BTreeSet<NoteId> = get_render_note_data(db, None)
                .await?
                .iter()
                .map(|row| row.note_id)
                .collect();
            note_ids.extend(
                tracked
                    .iter()
                    .filter_map(|p| parse_note_rel_path(p).map(|(_, id)| id)),
            );
            (note_ids, Vec::new(), true)
        }
    };

    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ").to_string();
    let mut commit_paths: Vec<PathBuf> = Vec::new();

    if !note_ids.is_empty() {
        let ids: Vec<NoteId> = note_ids.iter().copied().collect();
        let rows: HashMap<NoteId, RenderNoteData> = get_render_note_data(db, Some(ids.clone()))
            .await?
            .into_iter()
            .map(|row| (row.note_id, row))
            .collect();
        let links = get_note_links_map(db, &ids).await?;
        let parsers = get_all_parsers();
        for id in ids {
            let row = rows.get(&id);
            let expected = row.and_then(|row| {
                find_parser(&row.parser_name, &parsers)
                    .ok()
                    .map(|parser| (note_rel_path(parser.as_ref(), id), parser, row))
            });
            if row.is_some() && expected.is_none() {
                log::warn!(
                    "Note {} uses parser `{}`, which is not installed, so its file is left alone",
                    id.letters(),
                    row.map(|r| r.parser_name.as_str()).unwrap_or_default()
                );
                continue;
            }
            // Files of the note under other parsers, or of a note that no longer exists
            for candidate in note_rel_paths_all_parsers(id) {
                if expected.as_ref().is_some_and(|(rel, ..)| *rel == candidate) {
                    continue;
                }
                let absolute = repo.join(&candidate);
                if !absolute.exists() && !tracked.contains(&candidate) {
                    continue;
                }
                report.removed.push(candidate.clone());
                commit_paths.push(candidate.clone());
                if dry_run {
                    continue;
                }
                if absolute.exists() {
                    if differs_from_head(&repo, &candidate, None)? {
                        save_overwritten(&repo, &candidate, &stamp, false)?;
                        report.saved.push(candidate.clone());
                    }
                    std::fs::remove_file(&absolute).map_err(|e| io_error(&absolute, e))?;
                }
            }
            let Some((rel, parser, row)) = expected else {
                continue;
            };
            let contents = construct_db_note_file(parser.as_ref(), row, &links)?;
            let absolute = repo.join(&rel);
            let current = std::fs::read(&absolute).ok();
            commit_paths.push(rel.clone());
            if current.as_deref() == Some(contents.as_bytes()) {
                continue;
            }
            if semantic
                && let Some(current) = current.as_deref()
                && let Ok(current) = std::str::from_utf8(current)
                && file_note_fields(parser.as_ref(), current, id)
                    .is_some_and(|f| same_note(parser.as_ref(), id, &f, &db_note_fields(row)))
            {
                continue;
            }
            report.rewritten.push(rel.clone());
            if dry_run {
                continue;
            }
            if current.is_some() && differs_from_head(&repo, &rel, None)? {
                save_overwritten(&repo, &rel, &stamp, false)?;
                report.saved.push(rel.clone());
            }
            write_file(&absolute, contents.as_bytes())?;
        }
    }

    for (rel, is_tracked) in other_dirty {
        let absolute = repo.join(&rel);
        if is_tracked {
            report.restored.push(rel.clone());
            if dry_run {
                continue;
            }
            if absolute.exists() {
                save_overwritten(&repo, &rel, &stamp, false)?;
                report.saved.push(rel.clone());
            }
            let _index = git_index_lock();
            git(
                &repo,
                &[
                    Path::new("checkout"),
                    Path::new("HEAD"),
                    Path::new("--"),
                    &rel,
                ],
                None,
            )?;
        } else {
            report.removed.push(rel.clone());
            if dry_run {
                continue;
            }
            save_overwritten(&repo, &rel, &stamp, true)?;
            report.saved.push(rel.clone());
        }
    }

    if dry_run {
        return Ok(report);
    }
    let ids: Vec<NoteId> = note_ids.into_iter().collect();
    let default_message = commit_message("reconcile", &ids);
    report.committed = try_commit(&commit_paths, message.unwrap_or(&default_message))?;
    if matches!(scope, ReconcileScope::Dirty | ReconcileScope::All) {
        NEEDS_RECONCILE.store(false, Ordering::SeqCst);
    }
    if !report.is_empty() {
        log::info!("Reconciled the notes repository: {report:?}");
    }
    Ok(report)
}

/// Writes the files of `ids` from the database into the repository, without committing.
pub(crate) async fn write_db_note_files(
    db: &SqlitePool,
    ids: &[NoteId],
) -> Result<Vec<PathBuf>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let repo = server_repo_dir();
    let rows = get_render_note_data(db, Some(ids.to_vec())).await?;
    let links = get_note_links_map(db, ids).await?;
    let parsers = get_all_parsers();
    let mut written = Vec::new();
    for row in &rows {
        let parser = find_parser(&row.parser_name, &parsers)?;
        let rel = note_rel_path(parser.as_ref(), row.note_id);
        let contents = construct_db_note_file(parser.as_ref(), row, &links)?;
        write_file(&repo.join(&rel), contents.as_bytes())?;
        written.push(rel);
    }
    Ok(written)
}

pub(crate) fn tracked_paths(repo: &Path) -> Result<BTreeSet<PathBuf>, Error> {
    Ok(nul_split(&git(repo, &["ls-files", "-z"], None)?)
        .map(|p| PathBuf::from(String::from_utf8_lossy(p).into_owned()))
        .collect())
}

/// Paths that differ from `HEAD` in the index or working tree, with whether they are tracked.
pub(crate) fn dirty_paths(repo: &Path) -> Result<Vec<(PathBuf, bool)>, Error> {
    let out = git(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        None,
    )?;
    Ok(nul_split(&out)
        .filter(|entry| entry.len() > 3)
        .map(|entry| {
            let path = PathBuf::from(String::from_utf8_lossy(&entry[3..]).into_owned());
            (path, &entry[..2] != b"??")
        })
        .collect())
}

/// Whether the working-tree file at `rel` (or `contents`, if given) differs from `HEAD`, so was not
/// committed by spares. Files absent from `HEAD` always differ.
pub(crate) fn differs_from_head(
    repo: &Path,
    rel: &Path,
    contents: Option<&[u8]>,
) -> Result<bool, Error> {
    let mut spec = std::ffi::OsString::from("HEAD:");
    spec.push(rel);
    let output = git_output(
        repo,
        &[std::ffi::OsStr::new("cat-file"), "blob".as_ref(), &spec],
        None,
    )?;
    if !output.status.success() {
        return Ok(true);
    }
    let current = match contents {
        Some(contents) => contents.to_vec(),
        None => match std::fs::read(repo.join(rel)) {
            Ok(current) => current,
            Err(_) => return Ok(false),
        },
    };
    Ok(output.stdout != current)
}

/// Saves `rel` under [`overwritten_dir`]: moved if `move_file`, copied otherwise.
pub(crate) fn save_overwritten(
    repo: &Path,
    rel: &Path,
    stamp: &str,
    move_file: bool,
) -> Result<(), Error> {
    let source = repo.join(rel);
    let target = overwritten_dir().join(stamp).join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    let result = if move_file {
        std::fs::rename(&source, &target)
    } else {
        std::fs::copy(&source, &target).map(|_| ())
    };
    result.map_err(|e| io_error(&source, e))?;
    log::warn!(
        "Saved a copy of {} to {} before overwriting it, since spares did not write it",
        source.display(),
        target.display()
    );
    Ok(())
}

/// Removes an `index.lock` left behind by a git process that was killed. Only spares runs git
/// commands that change this repository's index, and they hold [`git_index_lock`].
fn remove_stale_index_lock(repo: &Path) {
    let _index = git_index_lock();
    let lock = repo.join(".git").join("index.lock");
    let Ok(metadata) = std::fs::metadata(&lock) else {
        return;
    };
    let age = metadata
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .unwrap_or_default();
    if age >= Duration::from_secs(10) && std::fs::remove_file(&lock).is_ok() {
        log::warn!("Removed a stale {}", lock.display());
    }
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    std::fs::write(path, contents).map_err(|e| io_error(path, e))
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        description: format!("Failed to access {}", path.display()),
        source,
    }
}

/// Directories under [`overwritten_dir`], oldest first.
pub fn overwritten_entries() -> Vec<PathBuf> {
    let Ok(dirs) = std::fs::read_dir(overwritten_dir()) else {
        return Vec::new();
    };
    let mut entries: Vec<PathBuf> = dirs.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    entries
}
