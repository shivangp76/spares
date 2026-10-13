//! The notes repository end to end: server writes, pushes, rejections, rollbacks and
//! reconciliation. In its own test binary, since it points the data directories at a temporary
//! directory through process-wide environment variables, and its tests share that directory, so
//! they run one at a time.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use chrono::Utc;
use serde_json::Map;
use spares_core::api::note::create_notes;
use spares_core::api::note::delete_notes;
use spares_core::api::note::get_note;
use spares_core::api::note::update_notes;
use spares_core::api::parser::create_parser;
use spares_core::api::tag::create_tag;
use spares_core::api::tag::update_tag;
use spares_core::api::undo::undo_event;
use spares_core::notes_vcs;
use spares_core::notes_vcs::FinishRequest;
use spares_core::notes_vcs::FinishResponse;
use spares_core::notes_vcs::ReceiveRequest;
use spares_core::notes_vcs::ReceiveResponse;
use spares_core::notes_vcs::ReconcileScope;
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
async fn init(pool: &SqlitePool) -> Vec<i64> {
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

fn note_path(dir: &Path, id: i64) -> PathBuf {
    dir.join("markdown").join(format!("{id:0>4}.md"))
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
fn commit_edit(clone: &Path, id: i64, edit: impl FnOnce(String) -> String) {
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

async fn note_data(pool: &SqlitePool, id: i64) -> String {
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
            "adds a note",
            Box::new(|c: &Path| {
                std::fs::copy(note_path(c, 1), c.join("markdown/0099.md")).unwrap();
            }),
        ),
        (
            "deletes a note",
            Box::new(move |c: &Path| {
                std::fs::remove_file(note_path(c, 1)).unwrap();
            }),
        ),
        (
            "only in case",
            Box::new(move |c: &Path| {
                // Through the index, since on a case-insensitive file system writing the file would
                // overwrite `markdown/0001.md` right here.
                let blob = git_stdin(c, &["hash-object", "-w", "--stdin"], "x\n");
                git(
                    c,
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("100644,{blob},Markdown/0001.md"),
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
                let path = note_path(c, 1);
                let s = read(&path).replace(
                    "First",
                    "<<<<<<< HEAD\nFirst\n=======\nOther\n>>>>>>> theirs\n",
                );
                std::fs::write(path, s).unwrap();
            }),
        ),
        (
            "names note 2",
            Box::new(move |c: &Path| {
                std::fs::copy(note_path(c, 2), note_path(c, 1)).unwrap();
            }),
        ),
        (
            "would delete the note",
            Box::new(move |c: &Path| {
                let path = note_path(c, 1);
                let s = read(&path).replace("# note-id: 1", "# action: delete; note-id: 1");
                std::fs::write(path, s).unwrap();
            }),
        ),
        (
            "holds 2 notes",
            Box::new(move |c: &Path| {
                let path = note_path(c, 1);
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
                std::fs::write(note_path(c, 1), "the LLM removed everything\n").unwrap();
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

#[sqlx::test]
async fn test_reconcile_dirty_quarantines(pool: SqlitePool) {
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
    assert_eq!(report.quarantined.len(), 3, "{report:?}");
    assert_eq!(status(&repo()), "");
    let saved: Vec<PathBuf> = walk(&notes_vcs::quarantine_dir());
    assert!(saved.iter().any(|p| read(p) == "a hand edit\n"));
    assert!(saved.iter().any(|p| read(p) == "stray\n"));
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
        "paths": [format!("markdown/{:0>4}.md", ids[0])],
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
