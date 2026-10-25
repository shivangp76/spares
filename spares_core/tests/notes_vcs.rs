//! The notes repository end to end: server writes, pushes, rejections, rollbacks and
//! reconciliation. In its own test binary, since it points the data directories at a temporary
//! directory through process-wide environment variables, and its tests share that directory, so
//! they run one at a time.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::OnceLock;

use chrono::Utc;
use serde_json::Map;
use serde_json::json;
use spares_core::adapters::impls::anki::backend::AnkiBackend;
use spares_core::adapters::impls::anki::backend::AnkiRawNote;
use spares_core::adapters::impls::anki::backend::FakeAnki;
use spares_core::adapters::impls::anki::backend::FakeAnkiCall;
use spares_core::api::note::create_notes;
use spares_core::api::note::delete_notes;
use spares_core::api::note::get_note;
use spares_core::api::note::update_notes;
use spares_core::api::parser::create_parser;
use spares_core::api::tag::create_tag;
use spares_core::api::tag::rename_tags;
use spares_core::api::tag::update_tag;
use spares_core::api::undo::undo_event;
use spares_core::model::NoteId;
use spares_core::notes_vcs;
use spares_core::notes_vcs::FinishRequest;
use spares_core::notes_vcs::FinishResponse;
use spares_core::notes_vcs::ReceiveRequest;
use spares_core::notes_vcs::ReceiveResponse;
use spares_core::notes_vcs::ReconcileScope;
use spares_core::notes_vcs::RemoveSavedCopiesRequest;
use spares_core::parsers::find_parser;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::note::CreateNoteRequest;
use spares_core::schema::note::CreateNotesRequest;
use spares_core::schema::note::DeleteNotesRequest;
use spares_core::schema::note::NotesSelector;
use spares_core::schema::note::UpdateNotesRequest;
use spares_core::schema::note::UpdateTags;
use spares_core::schema::parser::CreateParserRequest;
use spares_core::schema::tag::CreateTagRequest;
use spares_core::schema::tag::TagSelector;
use spares_core::schema::tag::UpdateTagRequest;
use spares_core::schema::undo::UndoEventRequest;
use sqlx::SqlitePool;

/// Changes a clone to make a push that must be rejected
type Change = dyn Fn(&Path);

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static BASE: OnceLock<PathBuf> = OnceLock::new();

/// Points spares at a fresh temporary directory and returns a guard that keeps other tests out.
async fn setup() -> tokio::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().await;
    let base = BASE.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("spares-notes-vcs-{}", std::process::id()));
        // SAFETY: set before any test reads them, while holding `SERIAL`
        unsafe {
            std::env::set_var("SPARES_DATA_DIR", base.join("data"));
            std::env::set_var("SPARES_CACHE_DIR", base.join("cache"));
            std::env::set_var("SPARES_CONFIG_DIR", base.join("config"));
            std::env::set_var("SPARES_FILES_DIR", base.join("cache"));
            std::env::set_var("SPARES_REPLICA", "0");
            for variable in [
                "SPARES_TEST_MODE",
                "MARKDOWN_OUT_DIR",
                "LATEX_OUT_DIR",
                "TYPST_OUT_DIR",
            ] {
                std::env::remove_var(variable);
            }
        }
        base
    });
    for dir in ["data", "cache", "clone"] {
        let _ = std::fs::remove_dir_all(base.join(dir));
    }
    std::fs::create_dir_all(base.join("data")).unwrap();
    guard
}

fn base() -> &'static Path {
    BASE.get().unwrap()
}

fn repo() -> PathBuf {
    notes_vcs::server_repo_dir()
}

fn clone_dir() -> PathBuf {
    base().join("clone")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.name=Tester", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_stdin(dir: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"])
}

fn last_message(dir: &Path) -> String {
    git(dir, &["log", "-1", "--format=%s"])
}

fn commit_count(dir: &Path) -> usize {
    git(dir, &["rev-list", "--count", "HEAD"]).parse().unwrap()
}

fn status(dir: &Path) -> String {
    git(dir, &["status", "--porcelain", "--untracked-files=all"])
}

/// Creates the markdown parser and three notes, then the repository. Returns the note ids.
async fn init(pool: &SqlitePool) -> Vec<NoteId> {
    let parser = create_parser(
        pool,
        CreateParserRequest {
            name: "markdown".to_string(),
        },
        true,
    )
    .await
    .unwrap();
    let requests = [
        (
            "First {{ Cloze here }}",
            vec!["tag 1"],
            vec!["first keyword"],
        ),
        (
            "Second {{ Cloze }}",
            vec!["tag 1", "tag 2"],
            vec!["second keyword"],
        ),
        (
            "Third {{ Cloze, linking to [first keyword][li] }}",
            vec![],
            vec![],
        ),
    ]
    .into_iter()
    .map(|(data, tags, keywords)| CreateNoteRequest {
        data: data.to_string(),
        keywords: keywords.into_iter().map(str::to_string).collect(),
        tags: tags.into_iter().map(str::to_string).collect(),
        is_suspended: false,
        custom_data: Map::new(),
    })
    .collect();
    let notes = create_notes(
        pool,
        CreateNotesRequest {
            parser_id: parser.id,
            requests,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    assert!(!notes_vcs::enabled());
    notes_vcs::init_server_repo(pool).await.unwrap();
    assert!(notes_vcs::enabled());
    notes.notes.iter().map(|n| n.id).collect()
}

/// The id of the first note [`init`] creates.
fn first() -> NoteId {
    "aaa".parse().unwrap()
}

/// The id of the second note [`init`] creates.
fn second() -> NoteId {
    "aab".parse().unwrap()
}

fn note_path(dir: &Path, id: NoteId) -> PathBuf {
    dir.join("markdown").join(format!("{id}.md"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn clone() -> PathBuf {
    let clone = clone_dir();
    git(
        base(),
        &[
            "clone",
            "-q",
            repo().to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    clone
}

/// Edits a note file in the clone and commits it.
fn commit_edit(clone: &Path, id: NoteId, edit: impl FnOnce(String) -> String) {
    let path = note_path(clone, id);
    std::fs::write(&path, edit(read(&path))).unwrap();
    git(clone, &["commit", "-qam", "edit"]);
}

struct Pushed {
    receive: ReceiveResponse,
    finish: Option<FinishResponse>,
}

/// Does what `git push` and the hooks do: transfers the clone's `HEAD`, asks the server to receive
/// it, moves `main` if it was accepted (unless `update_ref` is false), and finishes.
async fn push(pool: &SqlitePool, clone: &Path, update_ref: bool) -> Pushed {
    let new = head(clone);
    git(
        &repo(),
        &[
            "fetch",
            "-q",
            clone.to_str().unwrap(),
            "+HEAD:refs/test/incoming",
        ],
    );
    let old = head(&repo());
    let receive = notes_vcs::receive(
        pool.clone(),
        ReceiveRequest {
            repo: repo(),
            new: new.clone(),
        },
    )
    .await
    .unwrap();
    let finish = if receive.ok {
        if update_ref {
            git(&repo(), &["update-ref", "refs/heads/main", &new, &old]);
        }
        Some(
            notes_vcs::finish(
                pool.clone(),
                FinishRequest {
                    session: receive.session.clone().unwrap(),
                    new,
                },
            )
            .await
            .unwrap(),
        )
    } else {
        None
    };
    Pushed { receive, finish }
}

async fn note_data(pool: &SqlitePool, id: NoteId) -> String {
    get_note(pool, id).await.unwrap().data
}

fn reset_clone(clone: &Path) {
    git(clone, &["fetch", "-q", "origin"]);
    git(clone, &["reset", "-q", "--hard", "origin/main"]);
    git(clone, &["clean", "-qfd"]);
}

#[sqlx::test]
async fn test_init_and_server_writes_commit(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    assert_eq!(commit_count(&repo()), 1);
    assert_eq!(last_message(&repo()), "spares: initial import");
    assert_gitignore_matches();
    for id in &ids {
        let contents = read(&note_path(&repo(), *id));
        assert!(contents.contains(&format!("note-id: {id}")));
        // The hash line would change on every render and conflict with users' edits.
        assert!(!contents.contains("hash:"), "{contents}");
    }
    // The links the third note's file shows point at users' clone, not the server copy.
    let linking = read(&note_path(&repo(), ids[2]));
    let user_path = note_path(&notes_vcs_user_dir(), ids[0]);
    assert!(linking.contains(user_path.to_str().unwrap()), "{linking}");

    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![ids[0]]),
            parser_id: None,
            data: Some("First {{ Changed cloze }}".to_string()),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        last_message(&repo()),
        format!("spares: update note {}", ids[0])
    );
    assert!(read(&note_path(&repo(), ids[0])).contains("Changed cloze"));
    assert_eq!(status(&repo()), "");

    delete_notes(
        &pool,
        DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![ids[1]]),
        },
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        last_message(&repo()),
        format!("spares: delete note {}", ids[1])
    );
    assert!(!note_path(&repo(), ids[1]).exists());
    assert_eq!(status(&repo()), "");
}

fn notes_vcs_user_dir() -> PathBuf {
    base().join("data").join("notes")
}

#[sqlx::test]
async fn test_push_imports(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    commit_edit(&clone, ids[0], |s| {
        s.replace(
            "First {{[o:1] Cloze here }}",
            "First {{[o:1] Pushed cloze }}",
        )
        .replace("tags: tag 1", "tags: tag 1, zz pushed")
    });
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    assert_eq!(pushed.receive.imported, vec![ids[0]]);
    let finish = pushed.finish.unwrap();
    assert_eq!(finish.imported, vec![ids[0]]);

    let note = get_note(&pool, ids[0]).await.unwrap();
    assert_eq!(note.data, "First {{[o:1] Pushed cloze }}");
    assert!(
        note.tags.contains(&"zz pushed".to_string()),
        "{:?}",
        note.tags
    );
    // `main` holds the push (and possibly spares' normalization of it), and nothing is left over.
    git(
        &repo(),
        &["merge-base", "--is-ancestor", &head(&clone), "HEAD"],
    );
    assert_eq!(status(&repo()), "");
    // The pushed content was already canonical, so there is nothing to fetch.
    assert!(!finish.normalized, "{}", finish.message);
    assert_eq!(head(&repo()), head(&clone));

    // Pushing again without fetching works, since `main` did not move.
    commit_edit(&clone, ids[1], |s| s.replace("Second", "Second, edited"));
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    assert!(note_data(&pool, ids[1]).await.starts_with("Second, edited"));
}

#[sqlx::test]
async fn test_push_note_without_cards_warns(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    commit_edit(&clone, ids[0], |s| {
        s.replace("First {{[o:1] Cloze here }}", "First, without clozes")
    });
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    assert!(
        pushed.receive.message.contains("No cards found."),
        "{}",
        pushed.receive.message
    );
    assert_eq!(pushed.receive.imported, vec![ids[0]]);
    assert_eq!(note_data(&pool, ids[0]).await, "First, without clozes");
}

#[sqlx::test]
async fn test_push_normalizes(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    // A cloze without its order: spares adds it, so the stored file differs from the push.
    commit_edit(&clone, ids[0], |s| {
        s.replace(
            "First {{[o:1] Cloze here }}",
            "First {{ Cloze here }} and {{ another }}",
        )
    });
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    let finish = pushed.finish.unwrap();
    assert!(finish.normalized, "{}", finish.message);
    assert_eq!(
        last_message(&repo()),
        format!("spares: normalize pushed note {}", ids[0])
    );
    assert_eq!(status(&repo()), "");
    let data = note_data(&pool, ids[0]).await;
    assert!(read(&note_path(&repo(), ids[0])).contains(&data));
}

#[sqlx::test]
async fn test_push_rejections(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    let original_head = head(&repo());
    let original = note_data(&pool, ids[0]).await;

    let cases: Vec<(&str, Box<Change>)> = vec![
        (
            "does not exist in spares; name a new note's file `new-<name>`",
            Box::new(|c: &Path| {
                std::fs::copy(note_path(c, first()), c.join("markdown/zzz.md")).unwrap();
            }),
        ),
        (
            "a new note's file must not have a note id",
            Box::new(|c: &Path| {
                std::fs::copy(note_path(c, first()), c.join("markdown/new-copy.md")).unwrap();
            }),
        ),
        (
            "has no notes of",
            Box::new(|c: &Path| {
                std::fs::create_dir_all(c.join("typst")).unwrap();
                std::fs::write(c.join("typst/new-x.typ"), "x").unwrap();
            }),
        ),
        (
            "only in case",
            Box::new(move |c: &Path| {
                // Through the index, since on a case-insensitive file system writing the file would
                // overwrite `markdown/aaa.md` right here.
                let blob = git_stdin(c, &["hash-object", "-w", "--stdin"], "x\n");
                git(
                    c,
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("100644,{blob},Markdown/aaa.md"),
                    ],
                );
            }),
        ),
        (
            "not a note file",
            Box::new(|c: &Path| {
                std::fs::write(c.join("markdown/42.md"), "x").unwrap();
            }),
        ),
        (
            "managed by spares",
            Box::new(|c: &Path| {
                std::fs::write(c.join(".gitignore"), "*.md\n").unwrap();
            }),
        ),
        (
            "conflict marker",
            Box::new(move |c: &Path| {
                let path = note_path(c, first());
                let s = read(&path).replace(
                    "First",
                    "<<<<<<< HEAD\nFirst\n=======\nOther\n>>>>>>> theirs\n",
                );
                std::fs::write(path, s).unwrap();
            }),
        ),
        (
            "names note aab",
            Box::new(move |c: &Path| {
                std::fs::copy(note_path(c, second()), note_path(c, first())).unwrap();
            }),
        ),
        (
            "would delete the note",
            Box::new(move |c: &Path| {
                let path = note_path(c, first());
                let s = read(&path).replace("# note-id: aaa", "# action: delete; note-id: aaa");
                std::fs::write(path, s).unwrap();
            }),
        ),
        (
            "holds 2 notes",
            Box::new(move |c: &Path| {
                let path = note_path(c, first());
                let s = read(&path).replace(
                "<!--- spares: end --->",
                "<!--- spares: note start --->\nSmuggled {{ new note }}\n<!--- spares: note end --->\n<!--- spares: end --->",
            );
                std::fs::write(path, s).unwrap();
            }),
        ),
        (
            "holds no note",
            Box::new(move |c: &Path| {
                std::fs::write(note_path(c, first()), "the LLM removed everything\n").unwrap();
            }),
        ),
    ];
    for (expected, change) in cases {
        reset_clone(&clone);
        change(&clone);
        // The case variant is already staged, and `git add` refuses it on a case-insensitive
        // file system.
        if expected != "only in case" {
            git(&clone, &["add", "-A"]);
        }
        git(&clone, &["commit", "-qm", expected]);
        let pushed = push(&pool, &clone, true).await;
        assert!(!pushed.receive.ok, "{expected}: was accepted");
        assert!(
            pushed.receive.message.contains(expected),
            "{expected}: {}",
            pushed.receive.message
        );
        assert_eq!(head(&repo()), original_head, "{expected}");
        assert_eq!(status(&repo()), "", "{expected}");
        assert_eq!(note_data(&pool, ids[0]).await, original, "{expected}");
    }
}

#[sqlx::test]
async fn test_push_must_fast_forward(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    // A web edit lands after the clone
    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![ids[0]]),
            parser_id: None,
            data: Some("First {{ Web edit }}".to_string()),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    commit_edit(&clone, ids[0], |s| s.replace("Cloze here", "Clone edit"));
    let pushed = push(&pool, &clone, true).await;
    assert!(!pushed.receive.ok);
    assert!(
        pushed.receive.message.contains("newer notes"),
        "{}",
        pushed.receive.message
    );
    assert_eq!(note_data(&pool, ids[0]).await, "First {{[o:1] Web edit }}");
}

#[sqlx::test]
async fn test_push_detects_database_only_changes(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    // A change that does not write the file, as undo and tag renames did before.
    sqlx::query("UPDATE note SET data = ? WHERE id = ?")
        .bind("First {{[o:1] Changed behind git's back }}")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    let before = head(&repo());
    commit_edit(&clone, ids[0], |s| s.replace("Cloze here", "Clone edit"));
    let pushed = push(&pool, &clone, true).await;
    assert!(!pushed.receive.ok);
    assert!(
        pushed.receive.message.contains("changed in spares"),
        "{}",
        pushed.receive.message
    );
    // The database keeps its version, and spares committed it so the user can rebase onto it.
    assert_eq!(
        note_data(&pool, ids[0]).await,
        "First {{[o:1] Changed behind git's back }}"
    );
    assert_ne!(head(&repo()), before);
    assert!(read(&note_path(&repo(), ids[0])).contains("Changed behind git's back"));
    assert_eq!(status(&repo()), "");
}

#[sqlx::test]
async fn test_failed_import_rolls_back(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    create_tag(
        &pool,
        CreateTagRequest {
            name: "filtered".to_string(),
            description: String::new(),
            query: Some(r#"tag="tag 2""#.to_string()),
            auto_delete: false,
            reschedule: true,
        },
        true,
    )
    .await
    .unwrap();
    let clone = clone();
    let first = note_data(&pool, ids[0]).await;
    let before = head(&repo());
    // The first note imports fine; the second fails, since filtered tags cannot be assigned.
    let path = note_path(&clone, ids[0]);
    std::fs::write(
        &path,
        read(&path).replace("Cloze here", "Imported then undone"),
    )
    .unwrap();
    let path = note_path(&clone, ids[1]);
    std::fs::write(
        &path,
        read(&path).replace("tags: tag 1, tag 2", "tags: tag 1, tag 2, filtered"),
    )
    .unwrap();
    git(&clone, &["commit", "-qam", "edit"]);
    let pushed = push(&pool, &clone, true).await;
    assert!(!pushed.receive.ok);
    assert!(
        pushed.receive.message.contains("rolled back"),
        "{}",
        pushed.receive.message
    );
    assert_eq!(note_data(&pool, ids[0]).await, first);
    assert_eq!(head(&repo()), before);
    assert_eq!(status(&repo()), "");
}

#[sqlx::test]
async fn test_push_whose_ref_update_fails_is_rolled_back(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    let original = note_data(&pool, ids[0]).await;
    let before = head(&repo());
    commit_edit(&clone, ids[0], |s| {
        s.replace("Cloze here", "Never reaches main")
    });
    let pushed = push(&pool, &clone, false).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    let finish = pushed.finish.unwrap();
    assert!(finish.message.contains("rolled back"), "{}", finish.message);
    assert_eq!(note_data(&pool, ids[0]).await, original);
    assert_eq!(head(&repo()), before);
    assert_eq!(status(&repo()), "");
    assert!(notes_vcs::status().last_incident.is_some());
}

/// The events of the push's notes: `(id, group_id)`, oldest first
async fn events_after(pool: &SqlitePool, id: i64) -> Vec<(i64, Option<i64>)> {
    sqlx::query_as("SELECT id, group_id FROM event WHERE id > ? ORDER BY id")
        .bind(id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn max_event_id(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM event")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn test_push_of_several_notes_is_one_group(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    let originals = [
        note_data(&pool, ids[0]).await,
        note_data(&pool, ids[1]).await,
    ];
    let before = max_event_id(&pool).await;
    let before_head = head(&repo());
    // The first note gets a new tag, so its own events are a `[CreateTag, UpdateNotes]` group,
    // which the push's group takes in.
    let path = note_path(&clone, ids[0]);
    std::fs::write(
        &path,
        read(&path)
            .replace("Cloze here", "Grouped")
            .replace("tags: tag 1", "tags: tag 1, zz grouped"),
    )
    .unwrap();
    let path = note_path(&clone, ids[1]);
    std::fs::write(&path, read(&path).replace("Second", "Second, grouped")).unwrap();
    git(&clone, &["commit", "-qam", "edit"]);
    // The ref update fails, so the push is rolled back after it was imported.
    let pushed = push(&pool, &clone, false).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);

    let events = events_after(&pool, before).await;
    let pushed_events: Vec<_> = events.iter().take(3).collect();
    let leader = pushed_events[0].0;
    assert!(
        pushed_events
            .iter()
            .all(|(_, group)| *group == Some(leader)),
        "{events:?}"
    );

    let finish = pushed.finish.unwrap();
    assert!(finish.message.contains("rolled back"), "{}", finish.message);
    assert!(!finish.message.contains("partial"), "{}", finish.message);
    assert_eq!(note_data(&pool, ids[0]).await, originals[0]);
    assert_eq!(note_data(&pool, ids[1]).await, originals[1]);
    assert_eq!(head(&repo()), before_head);
}

#[sqlx::test]
async fn test_reconcile_dirty_saves_overwritten(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let note = note_path(&repo(), ids[0]);
    let committed = read(&note);
    std::fs::write(&note, "a hand edit\n").unwrap();
    std::fs::write(repo().join("stray.txt"), "stray\n").unwrap();
    std::fs::write(repo().join(".gitignore"), "changed\n").unwrap();
    let report = notes_vcs::reconcile(&pool, ReconcileScope::Dirty, None)
        .await
        .unwrap();
    assert_eq!(read(&note), committed);
    assert!(!repo().join("stray.txt").exists());
    assert_gitignore_matches();
    assert_eq!(report.saved.len(), 3, "{report:?}");
    assert_eq!(status(&repo()), "");
    let saved: Vec<PathBuf> = walk(&notes_vcs::overwritten_dir());
    assert!(saved.iter().any(|p| read(p) == "a hand edit\n"));
    assert!(saved.iter().any(|p| read(p) == "stray\n"));

    let copies = notes_vcs::saved_copies();
    assert_eq!(copies.len(), 3, "{copies:?}");
    assert!(copies.iter().all(|c| !c.matches_current), "{copies:?}");
    // The same edit pushed later leaves nothing in the copy that is lost
    std::fs::write(&note, "a hand edit\n").unwrap();
    assert_eq!(
        notes_vcs::saved_copies()
            .iter()
            .filter(|c| c.matches_current)
            .count(),
        1
    );
    std::fs::write(&note, &committed).unwrap();

    let escape = RemoveSavedCopiesRequest {
        paths: vec![PathBuf::from("../config")],
    };
    assert!(notes_vcs::remove_saved_copies(&escape).is_err());
    let all = RemoveSavedCopiesRequest {
        paths: copies.into_iter().map(|c| c.path).collect(),
    };
    notes_vcs::remove_saved_copies(&all).unwrap();
    assert!(notes_vcs::saved_copies().is_empty());
    assert_eq!(
        std::fs::read_dir(notes_vcs::overwritten_dir())
            .unwrap()
            .count(),
        0
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[sqlx::test]
async fn test_undo_and_tag_rename_commit(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let response = update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![ids[0]]),
            parser_id: None,
            data: Some("First {{ To be undone }}".to_string()),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    assert!(read(&note_path(&repo(), ids[0])).contains("To be undone"));
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: response.event_id,
            undo_group: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        last_message(&repo()),
        format!("spares: undo note {}", ids[0])
    );
    assert!(read(&note_path(&repo(), ids[0])).contains("Cloze here"));
    assert_eq!(status(&repo()), "");

    update_tag(
        &pool,
        UpdateTagRequest {
            tag_to_modify: TagSelector::Name("tag 1".to_string()),
            name: Some("renamed".to_string()),
            description: None,
            query: None,
            auto_delete: None,
            reschedule: None,
        },
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        last_message(&repo()),
        format!("spares: retag notes {}, {}", ids[0], ids[1])
    );
    assert!(read(&note_path(&repo(), ids[0])).contains("tags: renamed"));
    assert_eq!(status(&repo()), "");

    // Renaming an ancestor rewrites the notes of the tags under it.
    rename_tags(&pool, "renamed", "group:renamed", true)
        .await
        .unwrap();
    rename_tags(&pool, "group", "set", true).await.unwrap();
    assert!(read(&note_path(&repo(), ids[0])).contains("tags: set:renamed"));
    assert_eq!(status(&repo()), "");
}

#[sqlx::test]
async fn test_startup_rolls_back_an_interrupted_push(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let original = note_data(&pool, ids[0]).await;
    // What a server that died mid-import leaves: the import's change in the database, and the
    // journal naming a commit that never reached `main`.
    let response = update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![ids[0]]),
            parser_id: None,
            data: Some("First {{ Half imported }}".to_string()),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    let head = head(&repo());
    let journal = serde_json::json!({
        "session": "test",
        "old": head,
        "new": "0".repeat(40),
        "ids": [ids[0]],
        "event_ids": [response.event_id.unwrap()],
        "paths": [format!("markdown/{}.md", ids[0])],
        "readme_changed": false,
        "state": "Importing",
    });
    std::fs::write(
        repo().join(".git").join("spares-receive.json"),
        serde_json::to_vec(&journal).unwrap(),
    )
    .unwrap();

    notes_vcs::startup(&pool).await;

    assert_eq!(note_data(&pool, ids[0]).await, original);
    assert!(!repo().join(".git").join("spares-receive.json").exists());
    assert!(read(&note_path(&repo(), ids[0])).contains("Cloze here"));
    assert_eq!(status(&repo()), "");
    assert!(notes_vcs::status().last_incident.is_some());
}

#[sqlx::test]
async fn test_push_note_with_linked_notes(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    // The third note's file shows its linked notes, so its link reference is numbered.
    commit_edit(&clone, ids[2], |s| s.replace("Third", "Third, edited"));
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    assert!(note_data(&pool, ids[2]).await.starts_with("Third, edited"));
    assert_eq!(status(&repo()), "");
    // Pushing again from the same base still passes the base check.
    commit_edit(&clone, ids[2], |s| s.replace("edited", "edited twice"));
    reset_clone_keep(&clone);
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
}

/// Rebases the clone's commits onto spares' `main`, as `git pull --rebase` does.
fn reset_clone_keep(clone: &Path) {
    git(clone, &["pull", "-q", "--rebase"]);
}

fn assert_gitignore_matches() {
    let path = repo().join(".gitignore");
    if notes_vcs::GITIGNORE.is_empty() {
        assert!(!path.exists());
    } else {
        assert_eq!(read(&path), notes_vcs::GITIGNORE);
    }
}

#[sqlx::test]
async fn test_startup_updates_gitignore(pool: SqlitePool) {
    let _guard = setup().await;
    init(&pool).await;
    // A repository created with another `.gitignore`
    std::fs::write(repo().join(".gitignore"), "old pattern\n").unwrap();
    git(&repo(), &["add", "-f", ".gitignore"]);
    git(&repo(), &["commit", "-qm", "old gitignore"]);

    notes_vcs::startup(&pool).await;

    assert_gitignore_matches();
    assert_eq!(last_message(&repo()), "spares: update .gitignore");
    assert_eq!(status(&repo()), "");
    // Nothing more to do the next time
    assert!(!notes_vcs::sync_gitignore().unwrap());
}

/// A live note's data and custom data come from its source file, so a push may only change its
/// tags and keywords.
#[sqlx::test]
async fn test_push_live_note(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let mut custom_data = Map::new();
    custom_data.insert("live_sync_name".to_string(), "lecture".into());
    custom_data.insert("live_block_order".to_string(), 0.into());
    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![ids[0]]),
            parser_id: None,
            data: None,
            keywords: None,
            tags: UpdateTags::None,
            custom_data: Some(custom_data),
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    let clone = clone();
    let original = note_data(&pool, ids[0]).await;
    let original_head = head(&repo());

    let cases: Vec<(&str, Box<Change>)> = vec![
        (
            "edits the data",
            Box::new(move |c: &Path| {
                let path = note_path(c, first());
                std::fs::write(&path, read(&path).replace("Cloze here", "Edited")).unwrap();
            }),
        ),
        (
            "drops liveness",
            Box::new(move |c: &Path| {
                let path = note_path(c, first());
                let s = read(&path).replace(
                    "<!--- # custom-data: {\"live_sync_name\":\"lecture\",\"live_block_order\":0} --->\n",
                    "",
                );
                std::fs::write(&path, s).unwrap();
            }),
        ),
        (
            "makes a note live",
            Box::new(move |c: &Path| {
                let path = note_path(c, second());
                let s = read(&path).replace(
                    "<!--- # tags: tag 1, tag 2 --->\n",
                    "<!--- # tags: tag 1, tag 2 --->\n<!--- # custom-data: {\"live_sync_name\":\"lecture\",\"live_block_order\":1} --->\n",
                );
                std::fs::write(&path, s).unwrap();
            }),
        ),
    ];
    for (case, change) in cases {
        reset_clone(&clone);
        change(&clone);
        git(&clone, &["commit", "-qam", case]);
        let pushed = push(&pool, &clone, true).await;
        assert!(!pushed.receive.ok, "{case}: was accepted");
        assert!(
            pushed.receive.message.contains("is a live note"),
            "{case}: {}",
            pushed.receive.message
        );
        assert_eq!(head(&repo()), original_head, "{case}");
        assert_eq!(note_data(&pool, ids[0]).await, original, "{case}");
    }

    // Tags and keywords stay editable.
    reset_clone(&clone);
    commit_edit(&clone, ids[0], |s| {
        s.replace("tags: tag 1", "tags: tag 1, zz pushed").replace(
            "keywords: first keyword",
            "keywords: first keyword, pushed keyword",
        )
    });
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    let note = get_note(&pool, ids[0]).await.unwrap();
    assert_eq!(note.data, original);
    assert!(
        note.tags.contains(&"zz pushed".to_string()),
        "{:?}",
        note.tags
    );
    assert!(
        note.keywords.contains(&"pushed keyword".to_string()),
        "{:?}",
        note.keywords
    );
    assert_eq!(
        note.custom_data.get("live_sync_name"),
        Some(&"lecture".into())
    );
}

/// A new note's file made from the first note's: without its note id, with other data
fn new_note_file(clone: &Path, data: &str) -> String {
    let file = read(&note_path(clone, first()));
    let without_id = file.replace(&format!("<!--- # note-id: {} --->\n", first()), "");
    assert_ne!(without_id, file);
    let new_file = without_id.replace("First {{[o:1] Cloze here }}", data);
    assert_ne!(new_file, without_id);
    new_file
}

async fn note_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM note")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn test_push_creates_and_deletes_notes(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    let new_file = new_note_file(&clone, "Fourth {{ New cloze }}");
    std::fs::write(clone.join("markdown/new-fourth.md"), new_file).unwrap();
    std::fs::remove_file(note_path(&clone, ids[1])).unwrap();
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-qm", "create and delete"]);
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    let message = &pushed.receive.message;
    assert!(message.contains("created note"), "{message}");
    assert!(
        message.contains(&format!("DELETED note {}", ids[1])),
        "{message}"
    );
    assert!(get_note(&pool, ids[1]).await.is_err());
    let created: NoteId = sqlx::query_scalar("SELECT id FROM note WHERE data LIKE 'Fourth%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    // The deleted note is not rendered, the created one is.
    let finish = pushed.finish.unwrap();
    assert!(finish.imported.contains(&created), "{:?}", finish.imported);
    assert!(!finish.imported.contains(&ids[1]), "{:?}", finish.imported);

    // spares renamed the new file to the note's id on top of the push.
    assert!(finish.normalized, "{}", finish.message);
    assert!(!repo().join("markdown/new-fourth.md").exists());
    assert!(read(&note_path(&repo(), created)).contains("Fourth {{[o:1] New cloze }}"));
    let changed = git(
        &repo(),
        &["diff", "--name-status", "--no-renames", "HEAD~", "HEAD"],
    );
    assert!(changed.contains("D\tmarkdown/new-fourth.md"), "{changed}");
    assert!(
        changed.contains(&format!("A\tmarkdown/{created}.md")),
        "{changed}"
    );
    assert_eq!(status(&repo()), "");

    // One undo restores the deleted note and removes the created one.
    undo_event(
        &pool,
        UndoEventRequest {
            event_id: None,
            undo_group: true,
        },
    )
    .await
    .unwrap();
    assert!(get_note(&pool, ids[1]).await.is_ok());
    assert!(get_note(&pool, created).await.is_err());
}

#[sqlx::test]
async fn test_push_creating_and_deleting_rolls_back(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    let before = head(&repo());
    let count = note_count(&pool).await;
    std::fs::write(
        clone.join("markdown/new-fourth.md"),
        new_note_file(&clone, "Fourth {{ Never reaches main }}"),
    )
    .unwrap();
    std::fs::remove_file(note_path(&clone, ids[1])).unwrap();
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-qm", "create and delete"]);
    let pushed = push(&pool, &clone, false).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    let finish = pushed.finish.unwrap();
    assert!(finish.message.contains("rolled back"), "{}", finish.message);
    assert!(!finish.message.contains("partial"), "{}", finish.message);
    assert_eq!(note_count(&pool).await, count);
    assert!(get_note(&pool, ids[1]).await.is_ok());
    assert!(note_path(&repo(), ids[1]).exists());
    assert_eq!(head(&repo()), before);
    assert_eq!(status(&repo()), "");
    assert!(
        notes_vcs::status().saved_copies.is_empty(),
        "{:?}",
        notes_vcs::status().saved_copies
    );
}

#[sqlx::test]
async fn test_push_deleting_every_note_of_a_parser(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let clone = clone();
    git(&clone, &["rm", "-qr", "markdown"]);
    git(&clone, &["commit", "-qm", "delete all"]);
    let pushed = push(&pool, &clone, true).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    for id in ids {
        assert!(get_note(&pool, id).await.is_err());
    }
    assert_eq!(status(&repo()), "");
}

// The `anki` branch

fn anki_ref(dir: &Path) -> String {
    git(dir, &["rev-parse", "refs/heads/anki"])
}

/// What spares writes to Anki for the note in `contents`
fn stored_in_anki(id: NoteId, contents: &str) -> (String, String) {
    let parser = find_parser("markdown", &get_all_parsers()).unwrap();
    let mut parsed = notes_vcs::parse_note_file(parser.as_ref(), contents, id, false).unwrap();
    parsed
        .settings
        .custom_data
        .insert("note-id".to_string(), json!(id.letters()));
    FakeAnki::stored(parser.as_ref(), &parsed.settings, &parsed.data).unwrap()
}

/// The Anki id [`link_to_anki`] gives the `i`th note
fn anki_id(i: usize) -> i64 {
    1_000 + i64::try_from(i).unwrap()
}

/// Puts `ids` in Anki as spares writes them, and records their Anki ids in spares.
async fn link_to_anki(pool: &SqlitePool, anki: &FakeAnki, ids: &[NoteId]) {
    for (i, id) in ids.iter().enumerate() {
        let mut custom_data = get_note(pool, *id).await.unwrap().custom_data;
        custom_data.insert("anki-note-id".to_string(), json!(anki_id(i).to_string()));
        update_notes(
            pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![*id]),
                parser_id: None,
                data: None,
                keywords: None,
                tags: UpdateTags::None,
                custom_data: Some(custom_data),
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();
        let (flds, tags) = stored_in_anki(*id, &read(&note_path(&repo(), *id)));
        anki.put(AnkiRawNote {
            id: anki_id(i),
            flds,
            tags,
        });
    }
}

async fn refresh(pool: &SqlitePool, anki: &Arc<FakeAnki>) -> notes_vcs::anki::RefreshResponse {
    notes_vcs::anki::refresh(pool.clone(), Arc::clone(anki) as Arc<dyn AnkiBackend>)
        .await
        .unwrap()
}

fn files_at(dir: &Path, rev: &str) -> Vec<String> {
    git(dir, &["ls-tree", "-r", "--name-only", rev])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Edits Anki's front field of a note
fn edit_in_anki(anki: &FakeAnki, anki_id: i64, edit: impl FnOnce(&str) -> String) {
    let mut note = anki.note(anki_id).unwrap();
    let mut fields: Vec<String> = note.flds.split('\u{1f}').map(str::to_string).collect();
    fields[0] = edit(&fields[0]);
    note.flds = fields.join("\u{1f}");
    anki.put(note);
}

#[sqlx::test]
async fn test_anki_refresh_of_round_tripped_notes_is_empty(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    // Anki holds only the first card, so a note of several cards loses data there.
    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![ids[0]]),
            parser_id: None,
            data: Some("First {{ one }} and {{ two }}".to_string()),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        false,
    )
    .await
    .unwrap();
    let anki = Arc::new(FakeAnki::new());
    link_to_anki(&pool, &anki, &ids).await;
    let main = head(&repo());

    let refreshed = refresh(&pool, &anki).await;
    assert!(refreshed.changed);
    assert_eq!(refreshed.old, None);
    assert_eq!(anki_ref(&repo()), main);
    // Refreshing again with nothing changed in Anki commits nothing.
    let again = refresh(&pool, &anki).await;
    assert!(!again.changed);
    assert_eq!(anki_ref(&repo()), main);
    assert_eq!(head(&repo()), main);
    assert_eq!(status(&repo()), "");
}

#[sqlx::test]
async fn test_anki_refresh_keeps_notes_never_in_anki(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let anki = Arc::new(FakeAnki::new());
    link_to_anki(&pool, &anki, &ids[..1]).await;
    // A note of another note type is not read, and one without a spares id is new.
    edit_in_anki(&anki, anki_id(0), |front| format!("{front} changed"));
    let refreshed = refresh(&pool, &anki).await;
    assert!(refreshed.changed);
    let files = files_at(&repo(), "refs/heads/anki");
    for id in &ids {
        assert!(files.contains(&format!("markdown/{id}.md")), "{files:?}");
    }
    // Only the note Anki changed differs.
    let diff = git(&repo(), &["diff", "--name-only", "main", "anki"]);
    assert_eq!(diff, format!("markdown/{}.md", ids[0]));
    let file = git(&repo(), &["show", &format!("anki:markdown/{}.md", ids[0])]);
    assert!(file.contains("changed"), "{file}");
    assert!(file.contains("anki-note-id"), "{file}");
}

#[sqlx::test]
async fn test_anki_refresh_drops_notes_deleted_in_anki(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let anki = Arc::new(FakeAnki::new());
    link_to_anki(&pool, &anki, &ids[..2]).await;
    refresh(&pool, &anki).await;
    anki.remove(anki_id(1));
    let refreshed = refresh(&pool, &anki).await;
    assert!(refreshed.changed);
    let files = files_at(&repo(), "refs/heads/anki");
    assert!(
        !files.contains(&format!("markdown/{}.md", ids[1])),
        "{files:?}"
    );
    assert!(
        files.contains(&format!("markdown/{}.md", ids[0])),
        "{files:?}"
    );
    // The third note was never in Anki, so it stays.
    assert!(
        files.contains(&format!("markdown/{}.md", ids[2])),
        "{files:?}"
    );
}

/// Merges `anki` into `main` in the clone and pushes it
async fn merge_anki_and_push(pool: &SqlitePool, clone: &Path) -> Pushed {
    git(
        clone,
        &[
            "fetch",
            "-q",
            "origin",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    git(clone, &["reset", "-q", "--hard", "origin/main"]);
    git(clone, &["merge", "-q", "--no-edit", "origin/anki"]);
    push(pool, clone, true).await
}

#[sqlx::test]
async fn test_anki_edits_and_new_notes_merge_into_spares(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let anki = Arc::new(FakeAnki::new());
    link_to_anki(&pool, &anki, &ids).await;
    edit_in_anki(&anki, anki_id(0), |front| {
        front.replace("First", "First, edited in Anki")
    });
    // A note made in Anki, without a spares id
    let new_anki_id = 5_000;
    anki.put(AnkiRawNote {
        id: new_anki_id,
        flds: ["Made in Anki ", "{{[o:1] cloze }}", "", "", "markdown"].join("\u{1f}"),
        tags: "anki".to_string(),
    });
    refresh(&pool, &anki).await;
    let new_path = format!("markdown/new-anki-{new_anki_id}.md");
    assert!(files_at(&repo(), "refs/heads/anki").contains(&new_path));

    let clone = clone();
    let pushed = merge_anki_and_push(&pool, &clone).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    assert!(
        note_data(&pool, ids[0])
            .await
            .starts_with("First, edited in Anki"),
        "{}",
        note_data(&pool, ids[0]).await
    );
    let created: NoteId = sqlx::query_scalar("SELECT id FROM note WHERE data LIKE 'Made in Anki%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let custom_data = get_note(&pool, created).await.unwrap().custom_data;
    assert_eq!(
        custom_data.get("anki-note-id"),
        Some(&json!(new_anki_id.to_string()))
    );
    assert_eq!(status(&repo()), "");

    // The Anki note still has no spares id, but is not duplicated: its Anki id finds the note.
    refresh(&pool, &anki).await;
    let files = files_at(&repo(), "refs/heads/anki");
    assert!(!files.iter().any(|f| f.contains("new-anki")), "{files:?}");
    assert!(
        files.contains(&format!("markdown/{created}.md")),
        "{files:?}"
    );
    assert_eq!(
        git(&repo(), &["show", &format!("anki:markdown/{created}.md")]),
        git(&repo(), &["show", &format!("main:markdown/{created}.md")]),
    );
    // So merging `anki` again is clean and changes nothing.
    let before = note_data(&pool, ids[0]).await;
    let pushed = merge_anki_and_push(&pool, &clone).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    assert!(
        pushed.receive.imported.is_empty(),
        "{}",
        pushed.receive.message
    );
    assert_eq!(note_data(&pool, ids[0]).await, before);
}

struct AnkiPushed {
    receive: notes_vcs::anki::AnkiReceiveResponse,
    finish: Option<notes_vcs::anki::AnkiFinishResponse>,
}

/// Does what `git push origin HEAD:anki` and the hooks do
async fn push_anki(pool: &SqlitePool, anki: &Arc<FakeAnki>, clone: &Path) -> AnkiPushed {
    let new = head(clone);
    git(
        &repo(),
        &[
            "fetch",
            "-q",
            clone.to_str().unwrap(),
            "+HEAD:refs/test/incoming",
        ],
    );
    let old = anki_ref(&repo());
    let backend = Arc::clone(anki) as Arc<dyn AnkiBackend>;
    let receive = notes_vcs::anki::receive(
        pool.clone(),
        Arc::clone(&backend),
        notes_vcs::anki::AnkiReceiveRequest {
            repo: repo(),
            old: old.clone(),
            new: new.clone(),
        },
    )
    .await
    .unwrap();
    let finish = if receive.ok {
        git(&repo(), &["update-ref", "refs/heads/anki", &new, &old]);
        Some(
            notes_vcs::anki::finish(
                pool.clone(),
                backend,
                notes_vcs::anki::AnkiFinishRequest {
                    session: receive.session.clone().unwrap(),
                },
            )
            .await
            .unwrap(),
        )
    } else {
        None
    };
    AnkiPushed { receive, finish }
}

/// Checks out `anki` in the clone
fn checkout_anki(clone: &Path) {
    git(
        clone,
        &[
            "fetch",
            "-q",
            "origin",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    git(clone, &["checkout", "-q", "-B", "work", "origin/anki"]);
}

#[sqlx::test]
async fn test_anki_push_writes_anki(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let parser_id: i64 = sqlx::query_scalar("SELECT id FROM parser WHERE name = 'markdown'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let fourth = create_notes(
        &pool,
        CreateNotesRequest {
            parser_id,
            requests: vec![CreateNoteRequest {
                data: "Fourth {{ cloze }}".to_string(),
                keywords: Vec::new(),
                tags: Vec::new(),
                is_suspended: false,
                custom_data: Map::new(),
            }],
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap()
    .notes[0]
        .id;
    let anki = Arc::new(FakeAnki::new());
    link_to_anki(&pool, &anki, &ids[..2]).await;
    refresh(&pool, &anki).await;
    // Changed in spares, then sent by merging `main` into `anki`
    update_notes(
        &pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![ids[2]]),
            parser_id: None,
            data: Some("Third, changed in spares {{ Cloze }}".to_string()),
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    let clone = clone();
    checkout_anki(&clone);
    git(&clone, &["merge", "-q", "--no-edit", "origin/main"]);
    std::fs::write(
        clone.join("markdown/new-fresh.md"),
        new_note_file(&clone, "Fresh {{ note }}"),
    )
    .unwrap();
    let path = note_path(&clone, ids[0]);
    std::fs::write(&path, read(&path).replace("Cloze here", "Edited for Anki")).unwrap();
    // Never in Anki, and changed on `anki` to send it
    let path = note_path(&clone, fourth);
    std::fs::write(&path, read(&path).replace("Fourth", "Fourth, sent")).unwrap();
    std::fs::remove_file(note_path(&clone, ids[1])).unwrap();
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-qm", "for Anki"]);
    // The new note is a copy of the first, Anki id included.
    let pushed = push_anki(&pool, &anki, &clone).await;
    assert!(!pushed.receive.ok);
    assert!(
        pushed
            .receive
            .message
            .contains("new-fresh.md: has the Anki id of"),
        "{}",
        pushed.receive.message
    );
    assert_eq!(anki.take_calls(), []);
    let fresh = clone.join("markdown/new-fresh.md");
    let without_id: String = read(&fresh)
        .split_inclusive('\n')
        .filter(|line| !line.contains("custom-data"))
        .collect();
    std::fs::write(&fresh, without_id).unwrap();
    git(
        &clone,
        &["commit", "-qam", "for Anki, without the copied id"],
    );
    let pushed = push_anki(&pool, &anki, &clone).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    assert!(
        pushed
            .receive
            .message
            .contains("added 3, updated 1 and deleted 1"),
        "{}",
        pushed.receive.message
    );

    let calls = anki.take_calls();
    assert!(
        calls.contains(&FakeAnkiCall::Update {
            anki_id: anki_id(0),
            spares_id: Some(ids[0].letters()),
        }),
        "{calls:?}"
    );
    assert!(
        calls.contains(&FakeAnkiCall::Delete {
            anki_id: anki_id(1)
        }),
        "{calls:?}"
    );
    let added: Vec<(i64, Option<String>)> = calls
        .iter()
        .filter_map(|call| match call {
            FakeAnkiCall::Add { anki_id, spares_id } => Some((*anki_id, spares_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(added.len(), 3, "{calls:?}");
    let added_as = |id: NoteId| {
        added
            .iter()
            .find(|(_, spares_id)| spares_id.as_deref() == Some(id.letters().as_str()))
            .unwrap()
            .0
    };
    assert!(
        added.iter().any(|(_, spares_id)| spares_id.is_none()),
        "{calls:?}"
    );
    // spares records the Anki id of the note whose file is `main`'s, so both get the same line.
    let custom_data = get_note(&pool, ids[2]).await.unwrap().custom_data;
    assert_eq!(
        custom_data.get("anki-note-id"),
        Some(&json!(added_as(ids[2]).to_string()))
    );
    // The note changed on `anki` gets its id there, and in spares by merging `anki`.
    let fourth_anki_id = added_as(fourth);
    assert!(
        !get_note(&pool, fourth)
            .await
            .unwrap()
            .custom_data
            .contains_key("anki-note-id")
    );
    // The push only changed Anki, not spares' notes.
    assert!(!note_data(&pool, ids[0]).await.contains("Edited for Anki"));

    // Anki's ids are committed on top, so the next merge of `main` into `anki` is clean.
    let finish = pushed.finish.unwrap();
    assert!(finish.changed, "{}", finish.message);
    let files = files_at(&repo(), "refs/heads/anki");
    assert!(
        files.contains(&"markdown/new-fresh.md".to_string()),
        "{files:?}"
    );
    assert!(!files.iter().any(|f| f.contains("new-anki")), "{files:?}");
    assert!(git(&repo(), &["show", "anki:markdown/new-fresh.md"]).contains("anki-note-id"));
    let custom_data_line = |rev: &str| {
        git(&repo(), &["show", &format!("{rev}:markdown/{}.md", ids[2])])
            .lines()
            .find(|line| line.contains("custom-data"))
            .map(str::to_string)
    };
    assert!(custom_data_line("anki").is_some());
    assert_eq!(custom_data_line("anki"), custom_data_line("main"));
    checkout_anki(&clone);
    git(&clone, &["merge", "-q", "--no-edit", "origin/main"]);
    // A refresh right after finds Anki as the branch describes it.
    assert!(!refresh(&pool, &anki).await.changed);

    // Merging `anki` into `main` creates the new note, with its Anki id.
    let pushed = merge_anki_and_push(&pool, &clone).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    let created: NoteId = sqlx::query_scalar("SELECT id FROM note WHERE data LIKE 'Fresh%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let fourth_note = get_note(&pool, fourth).await.unwrap();
    assert!(
        fourth_note.data.starts_with("Fourth, sent"),
        "{}",
        fourth_note.data
    );
    assert_eq!(
        fourth_note.custom_data.get("anki-note-id"),
        Some(&json!(fourth_anki_id.to_string()))
    );
    let anki_links = pushed.finish.unwrap().anki_links;
    assert_eq!(anki_links.len(), 1, "{anki_links:?}");
    assert_eq!(anki_links[0].1, created);
}

#[sqlx::test]
async fn test_anki_push_with_stale_base_is_rejected(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let anki = Arc::new(FakeAnki::new());
    link_to_anki(&pool, &anki, &ids[..1]).await;
    refresh(&pool, &anki).await;
    let before = anki_ref(&repo());
    let clone = clone();
    checkout_anki(&clone);
    commit_edit(&clone, ids[0], |s| {
        s.replace("Cloze here", "From the clone")
    });
    edit_in_anki(&anki, anki_id(0), |front| format!("{front} from Anki"));
    let pushed = push_anki(&pool, &anki, &clone).await;
    assert!(!pushed.receive.ok);
    assert!(
        pushed.receive.message.contains("Anki has newer notes"),
        "{}",
        pushed.receive.message
    );
    assert_eq!(anki.take_calls(), []);
    // What Anki holds is committed, ready to fetch and merge.
    assert_ne!(anki_ref(&repo()), before);
    assert!(git(&repo(), &["show", &format!("anki:markdown/{}.md", ids[0])]).contains("from Anki"));
}

#[sqlx::test]
async fn test_anki_push_failing_partway_records_what_anki_holds(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let anki = Arc::new(FakeAnki::new());
    refresh(&pool, &anki).await;
    let before = anki_ref(&repo());
    let clone = clone();
    checkout_anki(&clone);
    let first = note_path(&clone, ids[0]);
    std::fs::write(&first, read(&first).replace("First", "First, sent")).unwrap();
    let second = note_path(&clone, ids[1]);
    std::fs::write(&second, read(&second).replace("Second", "Second, sent")).unwrap();
    git(&clone, &["commit", "-qam", "send two"]);
    anki.fail_after(1);
    let pushed = push_anki(&pool, &anki, &clone).await;
    assert!(!pushed.receive.ok);
    assert!(
        pushed.receive.message.contains("failed partway"),
        "{}",
        pushed.receive.message
    );
    assert_eq!(anki.notes().len(), 1);
    // `anki` now shows the note Anki got.
    let tip = anki_ref(&repo());
    assert_ne!(tip, before);
    assert_eq!(
        git(
            &repo(),
            &["rev-list", "--count", &format!("{before}..{tip}")]
        ),
        "1"
    );
    let changed = git(&repo(), &["diff", "--name-only", &before, &tip]);
    assert_eq!(changed.lines().count(), 1, "{changed}");
}

#[sqlx::test]
async fn test_anki_push_adds_back_a_note_deleted_in_anki(pool: SqlitePool) {
    let _guard = setup().await;
    let ids = init(&pool).await;
    let anki = Arc::new(FakeAnki::new());
    link_to_anki(&pool, &anki, &ids[..1]).await;
    anki.remove(anki_id(0));
    refresh(&pool, &anki).await;
    let path = format!("markdown/{}.md", ids[0]);
    assert!(!files_at(&repo(), "refs/heads/anki").contains(&path));
    // Restoring spares' version on `anki` sends it to Anki again, under a new id.
    let clone = clone();
    checkout_anki(&clone);
    git(&clone, &["checkout", "origin/main", "--", &path]);
    git(&clone, &["commit", "-qm", "send it again"]);
    let pushed = push_anki(&pool, &anki, &clone).await;
    assert!(pushed.receive.ok, "{}", pushed.receive.message);
    let calls = anki.take_calls();
    let [
        FakeAnkiCall::Add {
            anki_id: new_id, ..
        },
    ] = calls[..]
    else {
        panic!("{calls:?}");
    };
    assert_ne!(new_id, anki_id(0));
    let custom_data = get_note(&pool, ids[0]).await.unwrap().custom_data;
    assert_eq!(
        custom_data.get("anki-note-id"),
        Some(&json!(new_id.to_string()))
    );
    assert!(!refresh(&pool, &anki).await.changed);
}
