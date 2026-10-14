//! Importing a `git push` into the notes repository. The hooks in the repository only make HTTP
//! requests; everything here runs in the server while holding the exclusive notes lock, from
//! checking `HEAD` until git has moved `main` (or the push has been rolled back).

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;

use super::commit::NEEDS_RECONCILE;
use super::commit::commit_index;
use super::commit::commit_message;
use super::commit::git_index_lock;
use super::commit::start_deferring;
use super::commit::stop_deferring;
use super::git::git;
use super::git::git_error;
use super::git::git_output;
use super::git::nul_join;
use super::git::nul_split;
use super::git::rev_parse;
use super::lock::NotesGuard;
use super::lock::exclusive_guard;
use super::note_file::construct_db_note_file;
use super::note_file::db_note_fields;
use super::note_file::file_note_fields;
use super::note_file::parse_note_file;
use super::note_file::same_note;
use super::paths::enabled;
use super::paths::parse_note_rel_path;
use super::paths::server_repo_dir;
use super::reconcile::ReconcileScope;
use super::reconcile::reconcile;
use super::reconcile::write_db_note_files;
use crate::Error;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::api::note::update_notes;
use crate::api::undo::undo_event;
use crate::config::get_data_dir;
use crate::model::NoteId;
use crate::parsers::Parseable;
use crate::parsers::add_cloze_uid_to_note_data;
use crate::parsers::get_all_parsers;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::undo::UndoEventRequest;

/// The one file outside parser directories that pushes may change.
pub const README: &str = "README.md";

/// How long after a push was accepted the server waits for `post-receive` before settling it
/// itself, by checking whether git moved `main`.
pub const WATCHDOG_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize, Serialize)]
pub struct ReceiveRequest {
    /// The repository the hook runs in, which must be the server's
    pub repo: PathBuf,
    /// The commit being pushed to `main`
    pub new: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReceiveResponse {
    /// Whether git may update `main`
    pub ok: bool,
    pub message: String,
    pub session: Option<String>,
    pub imported: Vec<NoteId>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FinishRequest {
    pub session: String,
    pub new: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FinishResponse {
    pub message: String,
    /// Notes the push imported, to render
    pub imported: Vec<NoteId>,
    /// Whether spares committed normalized notes on top of the push, so there is something to
    /// fetch
    pub normalized: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StatusResponse {
    pub enabled: bool,
    /// Canonical data directory of the server, to tell spares instances apart
    pub data_dir: PathBuf,
    /// Canonical path of the repository the server writes to
    pub notes_dir: Option<PathBuf>,
    pub needs_reconcile: bool,
    pub receive_in_progress: bool,
    /// Outcome of the last push settled after its hook returned (rolled back, or committed as a
    /// partial import), until the next push starts
    pub last_incident: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
enum JournalState {
    Importing,
    AwaitingRef,
}

/// Written to `.git/spares-receive.json` while a push is being imported, so a server that dies
/// meanwhile settles it on startup.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Journal {
    session: String,
    old: String,
    new: String,
    ids: Vec<NoteId>,
    /// Events of the import, to undo if the push does not go through
    event_ids: Vec<i64>,
    /// Paths to commit once git moved `main`
    paths: Vec<PathBuf>,
    readme_changed: bool,
    state: JournalState,
}

struct Session {
    journal: Journal,
    _guard: NotesGuard,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static LAST_INCIDENT: Mutex<Option<String>> = Mutex::new(None);

fn session() -> MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_incident(incident: Option<String>) {
    *LAST_INCIDENT.lock().unwrap_or_else(|e| e.into_inner()) = incident;
}

fn journal_path() -> PathBuf {
    server_repo_dir().join(".git").join("spares-receive.json")
}

/// The session id for `post-receive`, which runs in another process than `push-to-checkout`.
pub fn session_file_path() -> PathBuf {
    server_repo_dir().join(".git").join("spares-session")
}

fn write_journal(journal: &Journal) -> Result<(), Error> {
    let path = journal_path();
    let contents = serde_json::to_vec_pretty(journal)
        .map_err(|e| Error::NotesVcs(format!("Failed to serialize the receive journal: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| Error::Io {
            description: format!("Failed to write {}", path.display()),
            source: e,
        })
}

fn remove_journal() {
    let _ = std::fs::remove_file(journal_path());
    let _ = std::fs::remove_file(session_file_path());
}

pub fn status() -> StatusResponse {
    let enabled = enabled();
    let data_dir = get_data_dir();
    StatusResponse {
        enabled,
        data_dir: data_dir.canonicalize().unwrap_or(data_dir),
        notes_dir: enabled
            .then(|| server_repo_dir().canonicalize().ok())
            .flatten(),
        needs_reconcile: NEEDS_RECONCILE.load(Ordering::SeqCst),
        receive_in_progress: session().is_some(),
        last_incident: LAST_INCIDENT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    }
}

enum Outcome {
    Accepted { journal: Journal, message: String },
    Rejected(String),
}

/// Validates and imports a push to `main`. If the push is accepted, the exclusive notes lock stays
/// held until [`finish`] (from `post-receive`) or the watchdog settles it.
pub async fn receive(db: SqlitePool, request: ReceiveRequest) -> Result<ReceiveResponse, Error> {
    let guard = exclusive_guard(None).await?;
    // Its own task, so a hook killed mid-request cannot stop the import halfway.
    let handle = tokio::spawn(async move {
        set_incident(None);
        let outcome = receive_locked(&db, &request).await.unwrap_or_else(|e| {
            Outcome::Rejected(format!("spares failed to process the push: {e}"))
        });
        match outcome {
            Outcome::Rejected(message) => ReceiveResponse {
                ok: false,
                message,
                session: None,
                imported: Vec::new(),
            },
            Outcome::Accepted { journal, message } => {
                let session_id = journal.session.clone();
                let imported = journal.ids.clone();
                *session() = Some(Session {
                    journal,
                    _guard: guard,
                });
                let watchdog_session = session_id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(WATCHDOG_DELAY).await;
                    if let Some(session) = take_session(Some(&watchdog_session)) {
                        log::warn!(
                            "post-receive did not report push {}; settling it",
                            session.journal.new
                        );
                        let _ = settle(&db, session).await;
                    }
                });
                ReceiveResponse {
                    ok: true,
                    message,
                    session: Some(session_id),
                    imported,
                }
            }
        }
    });
    handle
        .await
        .map_err(|e| Error::NotesVcs(format!("The push task failed: {e}")))
}

/// Settles an accepted push once git has updated `main`: commits the normalized notes and releases
/// the lock.
pub async fn finish(db: SqlitePool, request: FinishRequest) -> Result<FinishResponse, Error> {
    let Some(session) = take_session(Some(&request.session)) else {
        return Ok(FinishResponse {
            message: "spares has already settled this push".to_string(),
            imported: Vec::new(),
            normalized: false,
        });
    };
    let handle = tokio::spawn(async move { settle(&db, session).await });
    handle
        .await
        .map_err(|e| Error::NotesVcs(format!("Settling the push failed: {e}")))?
}

fn take_session(id: Option<&str>) -> Option<Session> {
    let mut session = session();
    if id.is_some_and(|id| session.as_ref().is_some_and(|s| s.journal.session != id)) {
        return None;
    }
    session.take()
}

/// Settles a push from a journal left by a server that stopped mid-receive. Called at startup.
pub async fn recover(db: &SqlitePool) -> Result<Option<String>, Error> {
    let Ok(contents) = std::fs::read(journal_path()) else {
        return Ok(None);
    };
    let journal: Journal = serde_json::from_slice(&contents)
        .map_err(|e| Error::NotesVcs(format!("Unreadable receive journal: {e}")))?;
    let guard = exclusive_guard(None).await?;
    let response = settle(
        db,
        Session {
            journal,
            _guard: guard,
        },
    )
    .await?;
    Ok(Some(response.message))
}

/// Decides by `HEAD` whether git moved `main` to the pushed commit. If it did, the normalized notes
/// are committed on top. Otherwise the import is rolled back, so the database matches `main` again.
async fn settle(db: &SqlitePool, session: Session) -> Result<FinishResponse, Error> {
    let journal = session.journal;
    let repo = server_repo_dir();
    let head = rev_parse(&repo, "HEAD")?;
    let mut paths = stop_deferring();
    if head == journal.new {
        paths.extend(journal.paths.iter().cloned());
        let normalized = {
            let _index = git_index_lock();
            add_existing(&repo, &paths)?;
            commit_index(&repo, &commit_message("normalize pushed", &journal.ids))?
        };
        remove_journal();
        let message = if normalized {
            "spares committed the notes as it writes them on top of your push".to_string()
        } else {
            "spares stored the notes exactly as pushed".to_string()
        };
        return Ok(FinishResponse {
            message,
            imported: journal.ids,
            normalized,
        });
    }

    // git did not move `main`, so the import must not stay in the database.
    {
        let _index = git_index_lock();
        git(&repo, &["read-tree", "HEAD"], None)?;
        if journal.readme_changed {
            let _ = git(&repo, &["checkout", "HEAD", "--", README], None);
        }
    }
    let message = format!(
        "the push of {} did not reach `main` after it was imported: {}",
        short(&journal.new),
        rollback(db, &journal).await
    );
    log::warn!("{message}");
    set_incident(Some(message.clone()));
    remove_journal();
    Ok(FinishResponse {
        message,
        imported: Vec::new(),
        normalized: false,
    })
}

/// `git add` of the paths that exist or are tracked, since a path that is neither fails it.
fn add_existing(repo: &Path, paths: &[PathBuf]) -> Result<Vec<u8>, Error> {
    let tracked = super::reconcile::tracked_paths(repo)?;
    let paths: Vec<&PathBuf> = paths
        .iter()
        .filter(|p| repo.join(p).exists() || tracked.contains(*p))
        .collect();
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    git(
        repo,
        &[
            "add",
            "-A",
            "-f",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        Some(&nul_join(&paths)),
    )
}

/// Undoes the import's events, newest first, then regenerates its notes from the database. Returns
/// a description of the outcome for the user.
async fn rollback(db: &SqlitePool, journal: &Journal) -> String {
    let mut failure = None;
    for event_id in journal.event_ids.iter().rev() {
        if let Err(e) = undo_event(
            db,
            UndoEventRequest {
                event_id: Some(*event_id),
                undo_group: true,
            },
        )
        .await
        {
            failure = Some(e);
            break;
        }
    }
    match failure {
        None => {
            let result = reconcile(db, ReconcileScope::Notes(journal.ids.clone()), None).await;
            if let Err(e) = result {
                NEEDS_RECONCILE.store(true, Ordering::SeqCst);
                log::error!("Failed to regenerate notes after rolling back: {e}");
            }
            "it was rolled back, so nothing was imported".to_string()
        }
        Some(e) => {
            let message = format!(
                "spares: partial import of rejected push {} ({})",
                short(&journal.new),
                commit_message("import", &journal.ids).trim_start_matches("spares: import ")
            );
            let result = reconcile(
                db,
                ReconcileScope::Notes(journal.ids.clone()),
                Some(&message),
            )
            .await;
            if let Err(e) = result {
                NEEDS_RECONCILE.store(true, Ordering::SeqCst);
                log::error!("Failed to commit a partial import: {e}");
            }
            format!(
                "rolling it back failed ({e}), so some of {} are imported in spares; fetch to see them",
                describe_ids(&journal.ids)
            )
        }
    }
}

struct PushedNote {
    rel: PathBuf,
    parser: Box<dyn Parseable>,
    id: NoteId,
    old_blob: Vec<u8>,
    new_blob: String,
}

#[expect(clippy::too_many_lines)]
async fn receive_locked(db: &SqlitePool, request: &ReceiveRequest) -> Result<Outcome, Error> {
    // 1. The hook's repository must be the one this server writes to.
    if !enabled() {
        return Ok(Outcome::Rejected(
            "this spares server does not keep notes in a git repository (run `spares note init-vcs`)"
                .to_string(),
        ));
    }
    let repo = server_repo_dir();
    let ours = repo.canonicalize().unwrap_or_else(|_| repo.clone());
    let theirs = request
        .repo
        .canonicalize()
        .unwrap_or_else(|_| request.repo.clone());
    if ours != theirs {
        return Ok(Outcome::Rejected(format!(
            "this repository ({}) belongs to a different spares instance than the server reached, which writes to {}. Run `spares note init-vcs --repair-hooks` with that instance's settings",
            theirs.display(),
            ours.display()
        )));
    }

    // 2. Anything left uncommitted (a killed server, a failed commit) is brought in line first.
    if let Err(e) = reconcile(db, ReconcileScope::Dirty, None).await {
        return Ok(Outcome::Rejected(format!(
            "spares could not bring its repository up to date, so it cannot accept pushes: {e}"
        )));
    }

    // 3. Fast-forward only
    let new = request.new.trim().to_ascii_lowercase();
    if !(new.len() == 40 || new.len() == 64) || !new.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(Outcome::Rejected(format!("invalid commit id `{new}`")));
    }
    let old = rev_parse(&repo, "HEAD")?;
    let is_ancestor = git_output(&repo, &["merge-base", "--is-ancestor", &old, &new], None)?;
    match is_ancestor.status.code() {
        Some(0) => {}
        Some(1) => {
            return Ok(Outcome::Rejected(
                "spares has newer notes than your push is based on; fetch and rebase (`jj spares-pull` / `git pull --rebase`), then push again"
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

    // 4. Which paths may change
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
    let head_paths: Vec<String> = nul_split(&git(
        &repo,
        &["ls-tree", "-r", "-z", "--name-only", &old],
        None,
    )?)
    .map(|p| String::from_utf8_lossy(p).into_owned())
    .collect();
    let head_dirs: BTreeSet<&str> = head_paths
        .iter()
        .filter_map(|p| p.split_once('/').map(|(dir, _)| dir))
        .collect();
    let mut errors: Vec<String> = Vec::new();
    let mut pushed: Vec<PushedNote> = Vec::new();
    let mut readme_changed = false;
    let mut tokens = nul_split(&raw);
    while let Some(header) = tokens.next() {
        let Some(path) = tokens.next() else { break };
        let path = String::from_utf8_lossy(path).into_owned();
        let header = String::from_utf8_lossy(header);
        let fields: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let [old_mode, new_mode, old_sha, new_sha, status] = fields[..] else {
            errors.push(format!("{path}: unexpected diff entry `{header}`"));
            continue;
        };
        if path == README && matches!(status, "M" | "A" | "D") {
            readme_changed = true;
            continue;
        }
        if let Some(existing) = head_paths
            .iter()
            .find(|p| **p != path && p.to_lowercase() == path.to_lowercase())
        {
            errors.push(format!(
                "{path}: differs from `{existing}` only in case; on a case-insensitive file system it would overwrite it"
            ));
            continue;
        }
        let note = parse_note_rel_path(Path::new(&path));
        let reason = match (status, &note) {
            (_, _) if path == ".gitignore" || path == ".gitattributes" => {
                Some("is managed by spares and cannot be changed".to_string())
            }
            ("A", Some(_)) => Some(
                "adds a note; create notes with `spares note add` or `spares import`".to_string(),
            ),
            ("D", Some(_)) => {
                Some("deletes a note; delete notes with `spares note delete`".to_string())
            }
            ("T", _) => Some("changes the file type".to_string()),
            ("M", Some((parser, _))) if !head_dirs.contains(parser.get_parser_name()) => {
                Some("is in a parser directory spares has not created".to_string())
            }
            ("M", Some(_)) if old_mode != "100644" || new_mode != "100644" => Some(format!(
                "has mode {new_mode}; only regular files (100644) are allowed"
            )),
            ("M", Some(_)) => None,
            (_, None) => Some(format!(
                "is not a note file; only existing notes (`<parser>/<id>.<ext>`, e.g. `typst/0042.typ`) and `{README}` can be pushed"
            )),
            (status, Some(_)) => Some(format!("has an unsupported change ({status})")),
        };
        if let Some(reason) = reason {
            errors.push(format!("{path}: {reason}"));
            continue;
        }
        let Some((parser, id)) = note else { continue };

        // 5. Content and parse rules
        let new_blob = git(&repo, &["cat-file", "blob", new_sha], None)?;
        let old_blob = git(&repo, &["cat-file", "blob", old_sha], None)?;
        let Ok(new_blob) = String::from_utf8(new_blob) else {
            errors.push(format!("{path}: is not valid UTF-8"));
            continue;
        };
        if new_blob.contains('\0') {
            errors.push(format!("{path}: contains a NUL byte"));
            continue;
        }
        if let Some(line) = conflict_marker_line(&new_blob) {
            errors.push(format!(
                "{path}: line {line} is a conflict marker; resolve the conflict first"
            ));
            continue;
        }
        if let Err(problems) = parse_note_file(parser.as_ref(), &new_blob, id, false) {
            for problem in problems {
                errors.push(format!("{path}: {problem}"));
            }
            continue;
        }
        pushed.push(PushedNote {
            rel: PathBuf::from(&path),
            parser,
            id,
            old_blob,
            new_blob,
        });
    }
    if !errors.is_empty() {
        return Ok(Outcome::Rejected(format!(
            "spares rejected the push, so nothing was imported:\n{}",
            errors
                .iter()
                .map(|e| format!("  - {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        )));
    }

    // 6. Compare-and-swap: the database must still hold what the push is based on.
    let ids: Vec<NoteId> = pushed.iter().map(|n| n.id).collect();
    if !ids.is_empty() {
        let rows = get_render_note_data(db, Some(ids.clone())).await?;
        let links = get_note_links_map(db, &ids).await?;
        let mut missing = Vec::new();
        let mut changed = Vec::new();
        for note in &pushed {
            let Some(row) = rows.iter().find(|r| r.note_id == note.id) else {
                missing.push(note.id);
                continue;
            };
            if row.parser_name != note.parser.get_parser_name() {
                changed.push(note.id);
                continue;
            }
            let db_file = construct_db_note_file(note.parser.as_ref(), row, &links)?;
            if db_file.as_bytes() == note.old_blob.as_slice() {
                continue;
            }
            let base_fields = std::str::from_utf8(&note.old_blob)
                .ok()
                .and_then(|old| file_note_fields(note.parser.as_ref(), old, note.id));
            let unchanged = base_fields.is_some_and(|base| {
                same_note(note.parser.as_ref(), note.id, &base, &db_note_fields(row))
            });
            if !unchanged {
                changed.push(note.id);
            }
        }
        if !missing.is_empty() || !changed.is_empty() {
            let mut reasons = Vec::new();
            if !changed.is_empty() {
                reconcile(
                    db,
                    ReconcileScope::Notes(changed.clone()),
                    Some(&commit_message("update changed", &changed)),
                )
                .await?;
                reasons.push(format!(
                    "  - notes {} changed in spares since the commit your push is based on (an edit, undo or tag change); spares committed their current version",
                    join_ids(&changed)
                ));
            }
            if !missing.is_empty() {
                reconcile(db, ReconcileScope::Notes(missing.clone()), None).await?;
                reasons.push(format!(
                    "  - notes {} no longer exist in spares",
                    join_ids(&missing)
                ));
            }
            return Ok(Outcome::Rejected(format!(
                "spares rejected the push, so nothing was imported:\n{}\nFetch and rebase (`jj spares-pull` / `git pull --rebase`), resolve any conflicts, then push again.",
                reasons.join("\n")
            )));
        }
    }

    // 7. Journal, so a server that dies from here on settles the push on startup
    let mut journal = Journal {
        session: format!("{}-{}", Utc::now().timestamp_millis(), short(&new)),
        old: old.clone(),
        new: new.clone(),
        ids: ids.clone(),
        event_ids: Vec::new(),
        paths: pushed.iter().map(|n| n.rel.clone()).collect(),
        readme_changed,
        state: JournalState::Importing,
    };
    write_journal(&journal)?;
    start_deferring();

    // 8. Import
    if let Err(e) = import(db, &pushed, &mut journal).await {
        stop_deferring();
        let outcome = rollback(db, &journal).await;
        remove_journal();
        return Ok(Outcome::Rejected(format!(
            "spares failed to import the push ({e}); {outcome}"
        )));
    }

    // 9. Hand the result to git: the index holds the pushed commit, and the working tree what the
    // database now holds. Committing the difference waits until git has moved `main`.
    let prepared: Result<(), Error> = async {
        {
            let _index = git_index_lock();
            git(&repo, &["read-tree", &new], None)?;
            if readme_changed {
                let new_readme = format!("{new}:{README}");
                if git_output(&repo, &["cat-file", "-e", &new_readme], None)?
                    .status
                    .success()
                {
                    git(&repo, &["checkout", &new, "--", README], None)?;
                } else {
                    let _ = std::fs::remove_file(repo.join(README));
                }
            }
        }
        write_db_note_files(db, &ids).await?;
        if readme_changed {
            journal.paths.push(PathBuf::from(README));
        }
        journal.state = JournalState::AwaitingRef;
        write_journal(&journal)?;
        std::fs::write(session_file_path(), &journal.session).map_err(|e| Error::Io {
            description: "Failed to write the push session".to_string(),
            source: e,
        })
    }
    .await;
    if let Err(e) = prepared {
        stop_deferring();
        {
            let _index = git_index_lock();
            let _ = git(&repo, &["read-tree", "HEAD"], None);
            if readme_changed {
                let _ = git(&repo, &["checkout", "HEAD", "--", README], None);
            }
        }
        let outcome = rollback(db, &journal).await;
        remove_journal();
        return Ok(Outcome::Rejected(format!(
            "spares failed to prepare the push ({e}); {outcome}"
        )));
    }

    let message = if ids.is_empty() {
        "no notes changed".to_string()
    } else {
        format!("imported {} into spares", describe_ids(&ids))
    };
    Ok(Outcome::Accepted { journal, message })
}

async fn import(
    db: &SqlitePool,
    pushed: &[PushedNote],
    journal: &mut Journal,
) -> Result<(), Error> {
    let parsers = get_all_parsers();
    for note in pushed {
        let parser_id: i64 = sqlx::query_scalar(r"SELECT id FROM parser WHERE name = ?")
            .bind(note.parser.get_parser_name())
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let mut contents = note.new_blob.clone();
        let validated = parse_note_file(note.parser.as_ref(), &contents, note.id, false)
            .map_err(|problems| Error::NotesVcs(problems.join("; ")))?;
        // Live notes get ids for new clozes, as `spares import` gives them.
        if validated
            .settings
            .custom_data
            .contains_key("live_sync_name")
        {
            let (enriched, minted) = add_cloze_uid_to_note_data(note.parser.as_ref(), &contents)?;
            if !minted.is_empty() {
                contents = enriched;
            }
        }
        let parsed = parse_note_file(note.parser.as_ref(), &contents, note.id, true)
            .map_err(|problems| Error::NotesVcs(problems.join("; ")))?;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note.id]),
            parser_id: Some(parser_id),
            data: Some(parsed.data),
            keywords: Some(parsed.settings.keywords),
            tags: UpdateTags::SetTags(parsed.settings.tags),
            custom_data: Some(parsed.settings.custom_data),
        };
        let response = update_notes(db, request, Utc::now(), &parsers, true).await?;
        if let Some(event_id) = response.event_id {
            journal.event_ids.push(event_id);
            write_journal(journal)?;
        }
    }
    Ok(())
}

/// The 1-based line number of the first git or jj conflict marker in `contents`.
pub fn conflict_marker_line(contents: &str) -> Option<usize> {
    contents.lines().position(is_conflict_marker).map(|i| i + 1)
}

fn is_conflict_marker(line: &str) -> bool {
    for marker in ['<', '>', '|'] {
        let run = line.chars().take_while(|c| *c == marker).count();
        if run >= 7 {
            let rest = &line[run..];
            if rest.is_empty() || rest.starts_with(' ') {
                return true;
            }
        }
    }
    // jj's diff-style markers: `%%%%%%% diff from ...` and `\\\\\\\        to: ...`
    for marker in ['%', '\\'] {
        if line.chars().take_while(|c| *c == marker).count() >= 7 {
            return true;
        }
    }
    false
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// `note 3` or `notes 3, 5`
fn describe_ids(ids: &[NoteId]) -> String {
    format!(
        "note{} {}",
        if ids.len() == 1 { "" } else { "s" },
        join_ids(ids)
    )
}

fn join_ids(ids: &[NoteId]) -> String {
    ids.iter()
        .map(|id| id.letters())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::conflict_marker_line;

    #[test]
    fn test_conflict_markers() {
        assert_eq!(conflict_marker_line("a\n<<<<<<< HEAD\nb"), Some(2));
        assert_eq!(
            conflict_marker_line(">>>>>>> conflict 1 of 1 ends"),
            Some(1)
        );
        assert_eq!(conflict_marker_line("x\n|||||||"), Some(2));
        assert_eq!(conflict_marker_line("%%%%%%% diff from: a"), Some(1));
        assert_eq!(conflict_marker_line(r"\\\\\\\        to: b"), Some(1));
        // Markdown headings and rules, and text that merely contains the characters
        assert_eq!(conflict_marker_line("Title\n=======\n---\n> quote"), None);
        assert_eq!(conflict_marker_line("<<<<<<<x"), None);
        assert_eq!(conflict_marker_line("a <<<<<<< b"), None);
    }
}
