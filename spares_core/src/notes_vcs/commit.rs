use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

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

/// Serializes git commands that change the index. Requests holding the shared notes lock run
/// concurrently, but their commits must not. Held until the guard is dropped.
pub(crate) fn git_index_lock() -> GitIndexGuard {
    let state = crate::collection::state();
    {
        let lock = &state.vcs.git_index;
        let mut held = crate::collection::lock(&lock.held);
        while *held {
            held = lock
                .freed
                .wait(held)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *held = true;
    }
    GitIndexGuard(state)
}

/// The collection's git index lock (see [`git_index_lock`]): a mutex whose guard owns what it
/// locks, so it can be taken from the collection's state.
#[derive(Default)]
pub(crate) struct GitIndexLock {
    held: Mutex<bool>,
    freed: std::sync::Condvar,
}

pub(crate) struct GitIndexGuard(std::sync::Arc<crate::collection::CollectionState>);

impl Drop for GitIndexGuard {
    fn drop(&mut self) {
        let lock = &self.0.vcs.git_index;
        *crate::collection::lock(&lock.held) = false;
        lock.freed.notify_one();
    }
}

/// While a push is imported and until git has moved `main`, commits must not move `HEAD`, or git's
/// ref update would fail after the database already changed. Paths to commit are collected in the
/// collection's state instead and committed when the push settles.
fn with_deferred<T>(f: impl FnOnce(&mut Option<BTreeSet<PathBuf>>) -> T) -> T {
    let state = crate::collection::state();
    let mut deferred = crate::collection::lock(&state.vcs.deferred);
    f(&mut deferred)
}

pub(crate) fn start_deferring() {
    with_deferred(|deferred| *deferred = Some(BTreeSet::new()));
}

/// Stops deferring commits and returns the paths collected meanwhile.
pub(crate) fn stop_deferring() -> Vec<PathBuf> {
    with_deferred(|deferred| {
        deferred
            .take()
            .map(|paths| paths.into_iter().collect())
            .unwrap_or_default()
    })
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
                .map(|id| id.letters())
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
/// changed by the time files are committed; `VcsState::needs_reconcile` makes the next push repair it.
pub fn commit_note_files<P: AsRef<Path>>(paths: &[P], message: &str) {
    if !enabled() {
        return;
    }
    if let Err(e) = try_commit(paths, message) {
        super::set_needs_reconcile(true);
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
    let deferred = with_deferred(|deferred| match deferred.as_mut() {
        Some(pending) => {
            pending.extend(rel_paths.iter().cloned());
            true
        }
        None => false,
    });
    if deferred {
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
    use crate::model::NoteId;

    fn ids(ids: &[&str]) -> Vec<NoteId> {
        ids.iter().map(|id| id.parse().unwrap()).collect()
    }

    #[test]
    fn test_commit_message() {
        assert_eq!(
            commit_message("update", &ids(&["gja", "giza"])),
            "spares: update notes giza, gja"
        );
        assert_eq!(
            commit_message("delete", &ids(&["aac"])),
            "spares: delete note aac"
        );
        let many: Vec<NoteId> = (0..11u8)
            .map(|i| format!("aa{}", char::from(b'a' + i)).parse().unwrap())
            .collect();
        assert_eq!(commit_message("render", &many), "spares: render 11 notes");
    }
}
