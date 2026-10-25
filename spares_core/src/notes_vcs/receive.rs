//! Importing a `git push` into the notes repository. The hooks in the repository only make HTTP
//! requests; everything here runs in the server while holding the exclusive notes lock, from
//! checking `HEAD` until git has moved `main` (or the push has been rolled back).

use std::collections::BTreeSet;
use std::fmt::Write as _;
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
use super::note_file::ParsedNoteFile;
use super::note_file::construct_db_note_file;
use super::note_file::db_note_fields;
use super::note_file::file_note_fields;
use super::note_file::is_warning;
use super::note_file::parse_new_note_file;
use super::note_file::parse_note_file;
use super::note_file::same_note;
use super::paths::enabled;
use super::paths::note_rel_paths_all_parsers;
use super::paths::parse_new_note_rel_path;
use super::paths::parse_note_rel_path;
use super::paths::server_repo_dir;
use super::reconcile::ReconcileScope;
use super::reconcile::SavedCopy;
use super::reconcile::overwritten_dir;
use super::reconcile::reconcile;
use super::reconcile::saved_copies;
use super::reconcile::write_db_note_files;
use crate::Error;
use crate::api::note::create_notes;
use crate::api::note::delete_notes;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::api::note::update_notes;
use crate::api::undo::is_reverted;
use crate::api::undo::join_event_group;
use crate::api::undo::undo_event;
use crate::config::get_data_dir;
use crate::model::NoteId;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::DeleteNotesRequest;
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
    /// Where [`saved_copies`] are
    pub overwritten_dir: Option<PathBuf>,
    /// Files spares overwrote or removed without having written them, which may hold edits that
    /// are now nowhere else
    pub saved_copies: Vec<SavedCopy>,
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
    /// Every note the import changes, created and deleted ones included, to regenerate if it is
    /// rolled back
    ids: Vec<NoteId>,
    /// Notes the push created, as they are created
    #[serde(default)]
    created: Vec<NoteId>,
    /// Notes the push deleted
    #[serde(default)]
    deleted: Vec<NoteId>,
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
        overwritten_dir: enabled.then(overwritten_dir),
        saved_copies: if enabled { saved_copies() } else { Vec::new() },
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
                let imported = imported_ids(&journal);
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
            imported: imported_ids(&journal),
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
///
/// The import's events form one group, so the first undo reverts them all. Journals written before
/// imports were grouped hold one ungrouped event per note, which the later iterations revert.
async fn rollback(db: &SqlitePool, journal: &Journal) -> String {
    // Undoing reconciles the notes it changes, which would save created files as written by
    // someone else. If the undo fails, reconciling writes them again.
    remove_created_files(journal);
    let mut failure = None;
    for event_id in journal.event_ids.iter().rev() {
        match is_reverted(db, *event_id).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
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

/// Removes the files of notes the import created that `HEAD` does not have, so reconciling does
/// not save them as files spares did not write.
fn remove_created_files(journal: &Journal) {
    let repo = server_repo_dir();
    for id in &journal.created {
        for rel in note_rel_paths_all_parsers(*id) {
            let head_has_it = git_output(
                &repo,
                &["cat-file", "-e", &format!("HEAD:{}", rel.display())],
                None,
            )
            .is_ok_and(|out| out.status.success());
            if !head_has_it {
                let _ = std::fs::remove_file(repo.join(&rel));
            }
        }
    }
}

struct PushedNote {
    rel: PathBuf,
    parser: Box<dyn Parseable>,
    change: PushedChange,
}

enum PushedChange {
    Update {
        id: NoteId,
        old_blob: Vec<u8>,
        new_blob: String,
    },
    Delete {
        id: NoteId,
        old_blob: Vec<u8>,
    },
    /// A `new-*` file
    Create {
        new_blob: String,
    },
}

impl PushedNote {
    /// The note the push changes, and the file it is based on, unless it creates one
    fn existing(&self) -> Option<(NoteId, &[u8])> {
        match &self.change {
            PushedChange::Update { id, old_blob, .. } | PushedChange::Delete { id, old_blob } => {
                Some((*id, old_blob))
            }
            PushedChange::Create { .. } => None,
        }
    }
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
    let mut warnings: Vec<String> = Vec::new();
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
        let new_note = parse_new_note_rel_path(Path::new(&path));
        let reason = match (status, &note, &new_note) {
            (_, _, _) if path == ".gitignore" || path == ".gitattributes" => {
                Some("is managed by spares and cannot be changed".to_string())
            }
            ("T", _, _) => Some("changes the file type".to_string()),
            ("A" | "M", Some(_), _) | ("A", _, Some(_)) if new_mode != "100644" => Some(format!(
                "has mode {new_mode}; only regular files (100644) are allowed"
            )),
            ("M", Some((parser, _)), _) if !head_dirs.contains(parser.get_parser_name()) => {
                Some("is in a parser directory spares has not created".to_string())
            }
            ("M", Some(_), _) if old_mode != "100644" => Some(format!(
                "has mode {old_mode}; only regular files (100644) are allowed"
            )),
            ("A", Some((_, id)), _) => Some(if note_exists(db, *id).await? {
                format!(
                    "adds a second file for note {}, which has one under another parser",
                    id.letters()
                )
            } else {
                format!(
                    "adds note {}, which does not exist in spares; name a new note's file `new-<name>`, e.g. `{}/new-{}.{}`, and spares gives it an id",
                    id.letters(),
                    note.as_ref()
                        .map(|(p, _)| p.get_parser_name())
                        .unwrap_or_default(),
                    id.letters(),
                    note.as_ref()
                        .map(|(p, _)| p.file_extension())
                        .unwrap_or_default(),
                )
            }),
            ("A", None, Some(parser))
                if parser_id(db, parser.get_parser_name()).await?.is_none() =>
            {
                Some(format!(
                    "creates a note with parser `{}`, which spares has no notes of; create the first one with `spares note add`",
                    parser.get_parser_name()
                ))
            }
            ("M" | "D", Some(_), _) | ("A", None, Some(_)) => None,
            (_, None, None) => Some(format!(
                "is not a note file; only notes (`<parser>/<id>.<ext>`, e.g. `typst/gir.typ`), new notes (`<parser>/new-<name>.<ext>`) and `{README}` can be pushed"
            )),
            (status, _, _) => Some(format!("has an unsupported change ({status})")),
        };
        if let Some(reason) = reason {
            errors.push(format!("{path}: {reason}"));
            continue;
        }
        let old_blob = || git(&repo, &["cat-file", "blob", old_sha], None);
        let (parser, change) = match (status, note, new_note) {
            ("D", Some((parser, id)), _) => (
                parser,
                PushedChange::Delete {
                    id,
                    old_blob: old_blob()?,
                },
            ),
            ("M", Some((parser, id)), _) => (
                parser,
                PushedChange::Update {
                    id,
                    old_blob: old_blob()?,
                    new_blob: String::new(),
                },
            ),
            (_, _, Some(parser)) => (
                parser,
                PushedChange::Create {
                    new_blob: String::new(),
                },
            ),
            _ => continue,
        };

        // 5. Content and parse rules
        let mut change = change;
        if let PushedChange::Update { new_blob, .. } | PushedChange::Create { new_blob } =
            &mut change
        {
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
            *new_blob = blob;
        }
        let parsed_settings = match &change {
            PushedChange::Update {
                id,
                old_blob,
                new_blob,
            } => match parse_note_file(parser.as_ref(), new_blob, *id, false) {
                Ok(parsed) => {
                    if let Some(reason) = live_note_edit(parser.as_ref(), *id, old_blob, &parsed) {
                        errors.push(format!("{path}: {reason}"));
                        continue;
                    }
                    Ok(parsed.settings)
                }
                Err(problems) => Err(problems),
            },
            PushedChange::Create { new_blob } => {
                parse_new_note_file(parser.as_ref(), new_blob, false).map(|parsed| parsed.settings)
            }
            PushedChange::Delete { .. } => Ok(NoteSettings::default()),
        };
        match parsed_settings {
            Ok(settings) => warnings.extend(
                settings
                    .errors_and_warnings
                    .iter()
                    .filter(|e| is_warning(e))
                    .map(|e| format!("{path}: {e}")),
            ),
            Err(problems) => {
                for problem in problems {
                    errors.push(format!("{path}: {problem}"));
                }
                continue;
            }
        }
        pushed.push(PushedNote {
            rel: PathBuf::from(&path),
            parser,
            change,
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
    let existing: Vec<(&PushedNote, NoteId, &[u8])> = pushed
        .iter()
        .filter_map(|note| note.existing().map(|(id, old_blob)| (note, id, old_blob)))
        .collect();
    let based_on: Vec<NoteId> = existing.iter().map(|(_, id, _)| *id).collect();
    if !based_on.is_empty() {
        let rows = get_render_note_data(db, Some(based_on.clone())).await?;
        let links = get_note_links_map(db, &based_on).await?;
        let mut missing = Vec::new();
        let mut changed = Vec::new();
        for (note, id, old_blob) in &existing {
            let Some(row) = rows.iter().find(|r| r.note_id == *id) else {
                missing.push(*id);
                continue;
            };
            if row.parser_name != note.parser.get_parser_name() {
                changed.push(*id);
                continue;
            }
            let db_file = construct_db_note_file(note.parser.as_ref(), row, &links)?;
            if db_file.as_bytes() == *old_blob {
                continue;
            }
            let base_fields = std::str::from_utf8(old_blob)
                .ok()
                .and_then(|old| file_note_fields(note.parser.as_ref(), old, *id));
            let unchanged = base_fields.is_some_and(|base| {
                same_note(note.parser.as_ref(), *id, &base, &db_note_fields(row))
            });
            if !unchanged {
                changed.push(*id);
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
    let deleted: Vec<NoteId> = pushed
        .iter()
        .filter_map(|note| match note.change {
            PushedChange::Delete { id, .. } => Some(id),
            _ => None,
        })
        .collect();

    // 7. Journal, so a server that dies from here on settles the push on startup
    let mut journal = Journal {
        session: format!("{}-{}", Utc::now().timestamp_millis(), short(&new)),
        old: old.clone(),
        new: new.clone(),
        ids: based_on.clone(),
        created: Vec::new(),
        deleted: deleted.clone(),
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
        write_db_note_files(db, &journal.ids).await?;
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

    let mut message = describe_import(&journal);
    if !warnings.is_empty() {
        message.push_str(" with warnings:");
        for warning in &warnings {
            let _ = write!(message, "\n  - {warning}");
        }
    }
    Ok(Outcome::Accepted { journal, message })
}

/// Imports the push: deletes first, then updates, then creates, so a note moved to another parser
/// (deleted, and created from a `new-*` file) and keywords of linked notes stay consistent. Its
/// events form one group, so one undo reverts it all.
async fn import(
    db: &SqlitePool,
    pushed: &[PushedNote],
    journal: &mut Journal,
) -> Result<(), Error> {
    let parsers = get_all_parsers();
    let at = Utc::now();
    // The first event of the import, whose group the later ones join
    let mut leader = None;
    if !journal.deleted.is_empty() {
        let response = delete_notes(
            db,
            DeleteNotesRequest {
                selector: NotesSelector::Ids(journal.deleted.clone()),
            },
            &parsers,
            true,
        )
        .await?;
        join_import_group(db, &mut leader, response.event_id, journal).await?;
        write_journal(journal)?;
    }

    for note in pushed {
        let PushedChange::Update { id, new_blob, .. } = &note.change else {
            continue;
        };
        let parser_id = parser_id(db, note.parser.get_parser_name())
            .await?
            .ok_or_else(|| {
                Error::NotesVcs(format!("no parser `{}`", note.parser.get_parser_name()))
            })?;
        let parsed = parse_note_file(note.parser.as_ref(), new_blob, *id, true)
            .map_err(|problems| Error::NotesVcs(problems.join("; ")))?;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![*id]),
            parser_id: Some(parser_id),
            data: Some(parsed.data),
            keywords: Some(parsed.settings.keywords),
            tags: UpdateTags::SetTags(parsed.settings.tags),
            custom_data: Some(parsed.settings.custom_data),
        };
        let response = update_notes(db, request, at, &parsers, true).await?;
        join_import_group(db, &mut leader, response.event_id, journal).await?;
        write_journal(journal)?;
    }

    for note in pushed {
        let PushedChange::Create { new_blob } = &note.change else {
            continue;
        };
        let parser_id = parser_id(db, note.parser.get_parser_name())
            .await?
            .ok_or_else(|| {
                Error::NotesVcs(format!("no parser `{}`", note.parser.get_parser_name()))
            })?;
        let parsed = parse_new_note_file(note.parser.as_ref(), new_blob, true)
            .map_err(|problems| Error::NotesVcs(problems.join("; ")))?;
        let response = create_notes(
            db,
            CreateNotesRequest {
                parser_id,
                requests: vec![CreateNoteRequest {
                    data: parsed.data,
                    keywords: parsed.settings.keywords,
                    tags: parsed.settings.tags,
                    is_suspended: parsed.settings.is_suspended,
                    custom_data: parsed.settings.custom_data,
                }],
            },
            at,
            &parsers,
            true,
        )
        .await?;
        for created in &response.notes {
            journal.created.push(created.id);
            journal.ids.push(created.id);
        }
        // Before the event, so a failure from here on still reconciles the created files away
        write_journal(journal)?;
        join_import_group(db, &mut leader, response.event_id, journal).await?;
        write_journal(journal)?;
    }
    Ok(())
}

/// Records an event of the import, in the group of its first event.
async fn join_import_group(
    db: &SqlitePool,
    leader: &mut Option<i64>,
    event_id: Option<i64>,
    journal: &mut Journal,
) -> Result<(), Error> {
    let Some(event_id) = event_id else {
        return Ok(());
    };
    match *leader {
        None => *leader = Some(event_id),
        Some(leader) => join_event_group(db, leader, event_id).await?,
    }
    journal.event_ids.push(event_id);
    Ok(())
}

async fn parser_id(db: &SqlitePool, parser_name: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(r"SELECT id FROM parser WHERE name = ?")
        .bind(parser_name)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

async fn note_exists(db: &SqlitePool, id: NoteId) -> Result<bool, Error> {
    let found: Option<i64> = sqlx::query_scalar(r"SELECT 1 FROM note WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(found.is_some())
}

/// What an accepted push did, for the pusher
fn describe_import(journal: &Journal) -> String {
    let updated = imported_ids(journal);
    let updated: Vec<NoteId> = updated
        .into_iter()
        .filter(|id| !journal.created.contains(id))
        .collect();
    let mut parts = Vec::new();
    if !updated.is_empty() {
        parts.push(format!("imported {} into spares", describe_ids(&updated)));
    }
    if !journal.created.is_empty() {
        parts.push(format!("created {}", describe_ids(&journal.created)));
    }
    if !journal.deleted.is_empty() {
        parts.push(format!(
            "DELETED {} with their review history (`spares undo` restores them; notes linking to them update on their next render)",
            describe_ids(&journal.deleted)
        ));
    }
    if parts.is_empty() {
        "no notes changed".to_string()
    } else {
        parts.join("; ")
    }
}

/// Notes the push imported or created, which exist and can be rendered
fn imported_ids(journal: &Journal) -> Vec<NoteId> {
    journal
        .ids
        .iter()
        .filter(|id| !journal.deleted.contains(id))
        .copied()
        .collect()
}

/// Why a pushed change to a note is refused because the note is live, or would become live. A
/// live note's data and custom data come from its source file, so an edit here would be overwritten
/// by the next `spares import` of that file. Its tags and keywords stay editable, as on the web.
fn live_note_edit(
    parser: &dyn Parseable,
    id: NoteId,
    old_blob: &[u8],
    new: &ParsedNoteFile,
) -> Option<String> {
    let old = std::str::from_utf8(old_blob)
        .ok()
        .and_then(|old| file_note_fields(parser, old, id))?;
    let mut new = new.fields(parser);
    let live_sync_name = [&old, &new]
        .into_iter()
        .find_map(|fields| fields.custom_data.get("live_sync_name")?.as_str())?
        .to_string();
    new.tags.clone_from(&old.tags);
    new.keywords.clone_from(&old.keywords);
    if same_note(parser, id, &old, &new) {
        return None;
    }
    Some(format!(
        "note {id} is a live note (live-sync-name `{live_sync_name}`), so only its tags and keywords can be changed here; edit it in its source file and run `spares import`"
    ))
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
