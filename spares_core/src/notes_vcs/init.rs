use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;

use super::commit::commit_index;
use super::commit::git_index_lock;
use super::git::git;
use super::note_file::construct_db_note_file;
use super::paths::enabled;
use super::paths::note_rel_path;
use super::paths::server_repo_dir;
use super::paths::write_render_hash;
use super::receive::StatusResponse;
use super::receive::recover;
use super::receive::status;
use super::reconcile::ReconcileReport;
use super::reconcile::ReconcileScope;
use super::reconcile::dirty_paths;
use super::reconcile::overwritten_dir;
use super::reconcile::reconcile;
use super::reconcile::reconcile_dry_run;
use super::reconcile::rename_quarantine_dir;
use super::reconcile::saved_copies;
use crate::Error;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::api::note::render_notes;
use crate::config::get_data_dir;
use crate::model::NoteId;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::get_all_parsers;
use crate::schema::note::NotesSelector;
use crate::schema::note::RenderNotesRequest;

/// The repository's `.gitignore`. Spares owns the file, so pushes cannot change it; the server
/// writes this at startup (or removes the file if this is empty) and commits the change.
pub const GITIGNORE: &str = "";

/// Where the repository is assembled before it is activated by a rename.
pub fn init_staging_dir() -> PathBuf {
    get_data_dir().join(".notes-spares-init")
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InitResponse {
    pub notes_dir: PathBuf,
    pub note_count: usize,
    /// Notes whose existing rendered file stays valid, so they are not compiled again
    pub render_hashes_kept: usize,
}

/// Creates the server copy of the notes repository from the database and activates it. The caller
/// holds the exclusive notes lock, so no change can land between the snapshot and the activation.
pub async fn init_server_repo(db: &SqlitePool) -> Result<InitResponse, Error> {
    let repo = server_repo_dir();
    if repo.exists() {
        return Err(Error::NotesVcs(format!(
            "{} already exists",
            repo.display()
        )));
    }
    let staging = init_staging_dir();
    if staging.exists() {
        // Left by an init that stopped before activating, so nothing uses it.
        std::fs::remove_dir_all(&staging).map_err(|e| io_error(&staging, e))?;
    }
    let staging_repo = staging.join("notes");
    std::fs::create_dir_all(&staging_repo).map_err(|e| io_error(&staging_repo, e))?;

    // Match linked notes to keywords, as a render does, without writing any files.
    render_notes(
        db,
        RenderNotesRequest {
            selector: NotesSelector::Ids(Vec::new()),
            immutable_note_ids: None,
            overridden_output_raw_dir: None,
            include_linked_notes: true,
            include_cards: false,
            generate_rendered: false,
            force_generate_rendered: false,
        },
        &get_all_parsers(),
    )
    .await?;

    let rows = get_render_note_data(db, None).await?;
    let ids: Vec<NoteId> = rows.iter().map(|row| row.note_id).collect();
    let links = if ids.is_empty() {
        HashMap::default()
    } else {
        get_note_links_map(db, &ids).await?
    };
    let parsers = get_all_parsers();
    let legacy_dir = get_data_dir().join("notes");
    let mut render_hashes_kept = 0;
    for row in &rows {
        let parser = find_parser(&row.parser_name, &parsers)?;
        let rel = note_rel_path(parser.as_ref(), row.note_id);
        let contents = construct_db_note_file(parser.as_ref(), row, &links)?;
        let path = staging_repo.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        std::fs::write(&path, &contents).map_err(|e| io_error(&path, e))?;

        // Keep the rendered file valid if it was compiled from exactly this content, which the
        // legacy file's hash line records.
        let hash = blake3::hash(contents.as_bytes()).to_hex().to_string();
        let legacy_hash = std::fs::read_to_string(legacy_dir.join(&rel))
            .ok()
            .and_then(|legacy| {
                legacy
                    .lines()
                    .rev()
                    .find(|line| !line.is_empty())
                    .and_then(|line| {
                        parser
                            .extract_comment(line)
                            .strip_prefix("hash: ")
                            .map(str::to_string)
                    })
            });
        let mut rendered = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
        rendered.push(parser.get_output_filename(RenderOutputType::Note, row.note_id));
        if legacy_hash.as_deref() == Some(hash.as_str()) && rendered.exists() {
            write_render_hash(parser.get_parser_name(), row.note_id, &hash);
            render_hashes_kept += 1;
        }
    }

    if !GITIGNORE.is_empty() {
        std::fs::write(staging_repo.join(".gitignore"), GITIGNORE)
            .map_err(|e| io_error(&staging_repo, e))?;
    }
    git(&staging_repo, &["init", "-q", "-b", "main"], None)?;
    git(&staging_repo, &["add", "-A"], None)?;
    git(
        &staging_repo,
        &[
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "spares: initial import",
        ],
        None,
    )?;

    std::fs::rename(&staging_repo, &repo).map_err(|e| io_error(&repo, e))?;
    let _ = std::fs::remove_dir_all(&staging);
    log::info!(
        "Notes are now kept in the git repository {}",
        repo.display()
    );
    Ok(InitResponse {
        notes_dir: repo,
        note_count: rows.len(),
        render_hashes_kept,
    })
}

/// Run when the server starts, before it accepts requests: settles a push the server stopped in
/// the middle of, then commits or reverts anything left uncommitted.
pub async fn startup(db: &SqlitePool) {
    if !enabled() {
        return;
    }
    rename_quarantine_dir();
    match recover(db).await {
        Ok(Some(message)) => log::warn!("Settled an interrupted push: {message}"),
        Ok(None) => {}
        Err(e) => log::error!("Failed to settle an interrupted push: {e}"),
    }
    if let Err(e) = reconcile(db, ReconcileScope::Dirty, None).await {
        log::error!("Failed to reconcile the notes repository: {e}");
    }
    warn_of_saved_copies();
    match sync_gitignore() {
        Ok(true) => log::info!("Updated the notes repository's .gitignore"),
        Ok(false) => {}
        Err(e) => log::error!("Failed to update the notes repository's .gitignore: {e}"),
    }
    check_hooks();
}

/// Saved copies may hold edits that are nowhere else, and are kept until removed.
fn warn_of_saved_copies() {
    let copies = saved_copies();
    if copies.is_empty() {
        return;
    }
    let differing = copies.iter().filter(|c| !c.matches_current).count();
    log::warn!(
        "{} copies of files spares overwrote without having written them are saved in {}, and \
         {differing} differ from the current file. Check in the web UI that nothing in them is \
         lost, then delete them there.",
        copies.len(),
        overwritten_dir().display()
    );
}

/// Makes the repository's `.gitignore` match [`GITIGNORE`], removing it if that is empty, and
/// commits the change. Returns whether anything changed. Runs at startup, before requests.
pub fn sync_gitignore() -> Result<bool, Error> {
    let repo = server_repo_dir();
    let path = repo.join(".gitignore");
    let current = std::fs::read_to_string(&path).ok();
    let wanted = (!GITIGNORE.is_empty()).then_some(GITIGNORE);
    if current.as_deref() == wanted {
        return Ok(false);
    }
    match wanted {
        Some(contents) => std::fs::write(&path, contents).map_err(|e| io_error(&path, e))?,
        None => std::fs::remove_file(&path).map_err(|e| io_error(&path, e))?,
    }
    let _index = git_index_lock();
    git(&repo, &["add", "-A", "-f", "--", ".gitignore"], None)?;
    commit_index(&repo, "spares: update .gitignore")
}

/// Warns if git would not run the push hooks, in which case a push would not be imported.
fn check_hooks() {
    let repo = server_repo_dir();
    let _index = git_index_lock();
    let hooks = git(&repo, &["rev-parse", "--git-path", "hooks"], None)
        .map(|out| repo.join(String::from_utf8_lossy(&out).trim()));
    let expected = repo.join(".git").join("hooks");
    match hooks {
        Ok(hooks) if same_path(&hooks, &expected) => {
            for hook in ["pre-receive", "push-to-checkout", "post-receive"] {
                if !expected.join(hook).exists() {
                    log::warn!(
                        "The `{hook}` hook is missing from {}; run `spares note init-vcs --repair-hooks`",
                        expected.display()
                    );
                }
            }
        }
        Ok(hooks) => log::warn!(
            "git runs hooks from {} instead of {}; run `spares note init-vcs --repair-hooks`",
            hooks.display(),
            expected.display()
        ),
        Err(e) => log::warn!("Failed to check the push hooks: {e}"),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CheckRequest {
    pub fix: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CheckResponse {
    pub status: StatusResponse,
    /// Paths that differ from `HEAD`
    pub dirty: Vec<PathBuf>,
    /// What does not match the database (or what was fixed, with `fix`)
    pub report: ReconcileReport,
}

/// Compares every note file with the database. With `fix`, rewrites those that differ. The caller
/// holds the exclusive notes lock.
pub async fn check(db: &SqlitePool, request: CheckRequest) -> Result<CheckResponse, Error> {
    if !enabled() {
        return Ok(CheckResponse {
            status: status(),
            dirty: Vec::new(),
            report: ReconcileReport::default(),
        });
    }
    let dirty = dirty_paths(&server_repo_dir())?
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    let report = if request.fix {
        let mut report = reconcile(db, ReconcileScope::Dirty, None).await?;
        let all = reconcile(db, ReconcileScope::All, None).await?;
        report.rewritten.extend(all.rewritten);
        report.removed.extend(all.removed);
        report.restored.extend(all.restored);
        report.saved.extend(all.saved);
        report.committed |= all.committed;
        report
    } else {
        reconcile_dry_run(db, ReconcileScope::All).await?
    };
    Ok(CheckResponse {
        status: status(),
        dirty,
        report,
    })
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        description: format!("Failed to access {}", path.display()),
        source,
    }
}
