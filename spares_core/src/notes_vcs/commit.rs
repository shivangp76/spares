use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use super::git::git;
use super::git::git_output;
use super::git::nul_join;
use super::paths::enabled;
use super::paths::note_rel_path;
use super::paths::server_repo_dir;
use crate::Error;
use crate::model::NoteId;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;

/// Set when committing failed, so files may differ from `HEAD`. The next push reconciles first.
pub static NEEDS_RECONCILE: AtomicBool = AtomicBool::new(false);

/// Serializes git commands that change the index. Requests holding the shared notes lock run
/// concurrently, but their commits must not.
static GIT_INDEX: Mutex<()> = Mutex::new(());

pub(crate) fn git_index_lock() -> MutexGuard<'static, ()> {
    GIT_INDEX.lock().unwrap_or_else(|e| e.into_inner())
}

/// While a push is imported and until git has moved `main`, commits must not move `HEAD`, or git's
/// ref update would fail after the database already changed. Paths to commit are collected here
/// instead and committed when the push settles.
static DEFERRED: Mutex<Option<BTreeSet<PathBuf>>> = Mutex::new(None);

fn deferred() -> MutexGuard<'static, Option<BTreeSet<PathBuf>>> {
    DEFERRED.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn start_deferring() {
    *deferred() = Some(BTreeSet::new());
}

/// Stops deferring commits and returns the paths collected meanwhile.
pub(crate) fn stop_deferring() -> Vec<PathBuf> {
    deferred()
        .take()
        .map(|paths| paths.into_iter().collect())
        .unwrap_or_default()
}

/// `spares: <verb> notes 12, 15`, or `spares: <verb> 4120 notes` for many notes.
pub fn commit_message(verb: &str, ids: &[NoteId]) -> String {
    let ids: BTreeSet<NoteId> = ids.iter().copied().collect();
    match ids.len() {
        0 => format!("spares: {verb} notes"),
        1..=10 => format!(
            "spares: {verb} note{} {}",
            if ids.len() == 1 { "" } else { "s" },
            ids.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        n => format!("spares: {verb} {n} notes"),
    }
}

/// Relative paths of the raw files of `ids`, which belong to `parser_name`.
pub fn note_rel_paths(parser_name: &str, ids: &[NoteId]) -> Vec<PathBuf> {
    match find_parser(parser_name, &get_all_parsers()) {
        Ok(parser) => ids
            .iter()
            .map(|id| note_rel_path(parser.as_ref(), *id))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Commits the server's changes to `paths` (relative to the repository, or absolute inside it).
/// Does nothing without VCS. Errors are logged rather than returned, since the database has already
/// changed by the time files are committed; [`NEEDS_RECONCILE`] makes the next push repair it.
pub fn commit_note_files<P: AsRef<Path>>(paths: &[P], message: &str) {
    if !enabled() {
        return;
    }
    if let Err(e) = try_commit(paths, message) {
        NEEDS_RECONCILE.store(true, Ordering::SeqCst);
        log::error!("Failed to commit notes ({message}): {e}");
    }
}

/// Like [`commit_note_files`], but returns whether a commit was made, and errors.
pub(crate) fn try_commit<P: AsRef<Path>>(paths: &[P], message: &str) -> Result<bool, Error> {
    let repo = server_repo_dir();
    let rel_paths: BTreeSet<PathBuf> = paths
        .iter()
        .map(|p| {
            let p = p.as_ref();
            p.strip_prefix(&repo).unwrap_or(p).to_path_buf()
        })
        .collect();
    if rel_paths.is_empty() {
        return Ok(false);
    }
    if let Some(pending) = deferred().as_mut() {
        pending.extend(rel_paths);
        return Ok(false);
    }
    let rel_paths: Vec<PathBuf> = rel_paths.into_iter().collect();

    let _index = git_index_lock();
    // `git add` fails entirely on a path that neither exists nor is tracked.
    let tracked = super::reconcile::tracked_paths(&repo)?;
    let rel_paths: Vec<PathBuf> = rel_paths
        .into_iter()
        .filter(|p| repo.join(p).exists() || tracked.contains(p))
        .collect();
    if rel_paths.is_empty() {
        return Ok(false);
    }
    git(
        &repo,
        &[
            "add",
            "-A",
            "-f",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        Some(&nul_join(&rel_paths)),
    )?;
    commit_index(&repo, message)
}

/// Commits the index if it differs from `HEAD`. The caller holds [`git_index_lock`].
pub(crate) fn commit_index(repo: &Path, message: &str) -> Result<bool, Error> {
    let args = ["diff", "--cached", "--quiet"];
    let diff = git_output(repo, &args, None)?;
    match diff.status.code() {
        Some(0) => return Ok(false),
        Some(1) => {}
        _ => return Err(super::git::git_error(&args, &diff)),
    }
    git(repo, &["commit", "-q", "--no-verify", "-m", message], None)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::commit_message;

    #[test]
    fn test_commit_message() {
        assert_eq!(
            commit_message("update", &[15, 12]),
            "spares: update notes 12, 15"
        );
        assert_eq!(commit_message("delete", &[3]), "spares: delete note 3");
        let many: Vec<i64> = (1..=11).collect();
        assert_eq!(commit_message("render", &many), "spares: render 11 notes");
    }
}
