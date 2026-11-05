//! Sync between two databases: the laptop (replica 0, client) and the server (replica 1),
//! reached in process.

use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use chrono::Duration;
use chrono::Utc;
use futures::future::BoxFuture;
use serde_json::Map;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

use super::client::SYNC_CONFLICT_TAG;
use super::protocol::FullUploadResponse;
use super::protocol::MetaResponse;
use super::protocol::PullRequest;
use super::protocol::PullResponse;
use super::protocol::PushRequest;
use super::protocol::PushResponse;
use super::records::CardRecord;
use super::records::NoteRecord;
use super::records::ReviewLogRecord;
use super::records::Selection;
use super::records::TagRecord;
use super::records::read_cards;
use super::records::read_notes;
use super::records::read_review_logs;
use super::records::read_tags;
use super::run::SyncMode;
use super::run::SyncRemote;
use super::run::SyncReport;
use super::run::sync;
use super::server;
use crate::Error;
use crate::api::card::update_cards;
use crate::api::note::create_notes;
use crate::api::note::delete_notes;
use crate::api::note::update_notes;
use crate::api::parser::create_parser;
use crate::api::review::submit_study_action;
use crate::config::Replica;
use crate::config::with_test_replica;
use crate::model::CardId;
use crate::model::NoteId;
use crate::parsers::get_all_parsers;
use crate::schema::card::CardsSelector;
use crate::schema::card::SpecialStateUpdate;
use crate::schema::card::UpdateCardsRequest;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::parser::CreateParserRequest;
use crate::schema::review::RatingSubmission;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;

const SERVER: Replica = Replica::new_unchecked(1);

/// Runs `f` as the server.
async fn on_server<F: std::future::Future>(f: F) -> F::Output {
    with_test_replica(SERVER, f).await
}

/// Enrolls a device with the server, which gives out numbers as the root.
async fn on_server_enroll(
    server: &SqlitePool,
    request: super::devices::EnrollRequest,
) -> Result<super::devices::EnrollResponse, Error> {
    on_server(super::devices::enroll(server, request)).await
}

struct Replicas {
    dir: PathBuf,
    laptop: SqlitePool,
    server: SqlitePool,
    remote: InProcess,
}

impl Drop for Replicas {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn open_db(path: &Path) -> SqlitePool {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .create_if_missing(true)
        .with_regexp();
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

impl Replicas {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("spares-sync-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let laptop = open_db(&dir.join("laptop.sqlite")).await;
        let server = open_db(&dir.join("server.sqlite")).await;
        // The laptop's host enrolled with the root, keeping its number.
        on_server_enroll(
            &server,
            super::devices::EnrollRequest {
                name: "laptop".to_string(),
                host: true,
                replica: Some(0),
            },
        )
        .await
        .unwrap();
        Self {
            remote: InProcess {
                pool: server.clone(),
                after_pull: Mutex::new(None),
                during_push: Mutex::new(None),
                lose_push_answer: AtomicBool::new(false),
                fail_push: AtomicBool::new(false),
            },
            dir,
            laptop,
            server,
        }
    }

    /// Two replicas that have done their first (full) sync, with a parser and `notes` notes.
    async fn synced(notes: &[&str]) -> (Self, Vec<NoteId>) {
        let replicas = Self::new().await;
        create_parser(
            &replicas.laptop,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            false,
        )
        .await
        .unwrap();
        let ids = create(&replicas.laptop, notes, &[]).await;
        replicas.sync_mode(SyncMode::FullUpload).await.unwrap();
        (replicas, ids)
    }

    async fn sync_mode(&self, mode: SyncMode) -> Result<SyncReport, Error> {
        sync(&self.laptop, &self.remote, mode, &self.dir).await
    }

    async fn sync(&self) -> SyncReport {
        self.sync_mode(SyncMode::Normal).await.unwrap()
    }

    /// Both databases hold the same rows.
    async fn assert_converged(&self) {
        let laptop = contents(&self.laptop).await;
        let server = contents(&self.server).await;
        assert_eq!(laptop.0, server.0, "notes differ");
        assert_eq!(laptop.1.len(), server.1.len(), "cards differ");
        for (l, s) in laptop.1.iter().zip(&server.1) {
            assert!(
                super::client::cards_equal(l, s),
                "cards differ: {l:?} {s:?}"
            );
        }
        assert_eq!(laptop.2, server.2, "review logs differ");
        assert_eq!(laptop.3, server.3, "tags differ");
        assert_eq!(
            saved_searches(&self.laptop).await,
            saved_searches(&self.server).await,
            "saved searches differ"
        );
    }
}

/// Each saved search as `(name, query)`, by name.
async fn saved_searches(pool: &SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT name, query FROM saved_search ORDER BY name")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn save_search(pool: &SqlitePool, name: &str, query: &str) {
    crate::api::saved_search::save_search(
        pool,
        crate::schema::saved_search::SavedSearch {
            name: name.to_string(),
            query: query.to_string(),
        },
    )
    .await
    .unwrap();
}

async fn exec(pool: &SqlitePool, statement: &str) {
    sqlx::query(AssertSqlSafe(statement))
        .execute(pool)
        .await
        .unwrap();
}

fn pairs(searches: &[(&str, &str)]) -> Vec<(String, String)> {
    searches
        .iter()
        .map(|(name, query)| ((*name).to_string(), (*query).to_string()))
        .collect()
}

struct InProcess {
    pool: SqlitePool,
    /// Runs on the server right after the next pull, as a phone edit during a sync would.
    after_pull: Mutex<Option<String>>,
    /// Runs on the client during the next push, before the server applies it.
    during_push: Mutex<Option<DuringPush>>,
    /// Loses the answer to the next push, after the server applied it.
    lose_push_answer: AtomicBool,
    /// Fails the next push before the server sees it.
    fail_push: AtomicBool,
}

type DuringPush = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

#[async_trait]
impl SyncRemote for InProcess {
    async fn meta(&self, replica: u8) -> Result<MetaResponse, Error> {
        on_server(server::meta(&self.pool, Some(replica))).await
    }

    async fn pull(&self, request: PullRequest) -> Result<PullResponse, Error> {
        let response = on_server(server::pull(&self.pool, request)).await?;
        let statement = self.after_pull.lock().unwrap().take();
        if let Some(statement) = statement {
            sqlx::query(AssertSqlSafe(statement))
                .execute(&self.pool)
                .await
                .unwrap();
        }
        Ok(response)
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, Error> {
        let during = self.during_push.lock().unwrap().take();
        if let Some(during) = during {
            during().await;
        }
        if self.fail_push.swap(false, Ordering::SeqCst) {
            return Err(Error::Sync("cannot reach the server".to_string()));
        }
        let response = on_server(server::push(&self.pool, request))
            .await
            .map(|(response, _)| response);
        if self.lose_push_answer.swap(false, Ordering::SeqCst) {
            response?;
            return Err(Error::Sync("the connection dropped".to_string()));
        }
        response
    }

    async fn download(&self, path: &Path) -> Result<(), Error> {
        on_server(server::full_download(&self.pool, path)).await
    }

    async fn upload(&self, path: &Path, replica: u8) -> Result<FullUploadResponse, Error> {
        on_server(server::full_upload(&self.pool, path, replica)).await
    }
}

async fn contents(
    pool: &SqlitePool,
) -> (
    Vec<NoteRecord>,
    Vec<CardRecord>,
    Vec<ReviewLogRecord>,
    Vec<TagRecord>,
) {
    let mut conn = pool.acquire().await.unwrap();
    let all = Selection::After(i64::MIN);
    (
        read_notes(&mut conn, all).await.unwrap(),
        read_cards(&mut conn, all).await.unwrap(),
        read_review_logs(&mut conn, all).await.unwrap(),
        read_tags(&mut conn, all).await.unwrap(),
    )
}

async fn parser_id(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT id FROM parser WHERE name = 'markdown'")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn create(pool: &SqlitePool, notes: &[&str], tags: &[&str]) -> Vec<NoteId> {
    if notes.is_empty() {
        return Vec::new();
    }
    let response = create_notes(
        pool,
        CreateNotesRequest {
            parser_id: parser_id(pool).await,
            requests: notes
                .iter()
                .map(|data| CreateNoteRequest {
                    data: (*data).to_string(),
                    keywords: vec![],
                    tags: tags.iter().map(|t| (*t).to_string()).collect(),
                    is_suspended: false,
                    custom_data: Map::new(),
                })
                .collect(),
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
    response.notes.into_iter().map(|n| n.id).collect()
}

async fn edit(pool: &SqlitePool, id: NoteId, data: &str) {
    update_notes(
        pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![id]),
            parser_id: None,
            data: Some(data.to_string()),
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_remove: None,
                tags_to_add: None,
            },
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
}

async fn set_tags(pool: &SqlitePool, id: NoteId, add: &[&str], remove: &[&str]) {
    update_notes(
        pool,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![id]),
            parser_id: None,
            data: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_remove: Some(remove.iter().map(|t| (*t).to_string()).collect()),
                tags_to_add: Some(add.iter().map(|t| (*t).to_string()).collect()),
            },
            custom_data: None,
        },
        Utc::now(),
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
}

async fn delete(pool: &SqlitePool, id: NoteId) {
    delete_notes(
        pool,
        DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![id]),
        },
        &get_all_parsers(),
        true,
    )
    .await
    .unwrap();
}

async fn data(pool: &SqlitePool, id: NoteId) -> Option<String> {
    sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn tags_of(pool: &SqlitePool, id: NoteId) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT t.name FROM note_tag nt JOIN tag t ON t.id = nt.tag_id WHERE nt.note_id = ?
         ORDER BY t.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn cards_of(pool: &SqlitePool, id: NoteId) -> Vec<CardId> {
    sqlx::query_scalar(r#"SELECT id FROM card WHERE note_id = ? ORDER BY "order""#)
        .bind(id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn rate(pool: &SqlitePool, card_id: CardId, rating: u32, at: chrono::DateTime<Utc>) {
    submit_study_action(
        pool,
        SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id,
                rating,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(2),
                tag_id: None,
            }),
        },
        at,
    )
    .await
    .unwrap();
}

const LINES: &str = "Line one\n\nThe {{ answer }} is here\n\nLine three\n\nLine four\n";

#[tokio::test]
async fn first_sync_needs_a_full_sync_then_normal_syncs_are_empty() {
    let replicas = Replicas::new().await;
    let err = replicas.sync_mode(SyncMode::Normal).await.unwrap_err();
    assert!(matches!(err, Error::FullSyncRequired(_)), "{err}");
    let (replicas, _) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    replicas.assert_converged().await;
    let report = replicas.sync().await;
    assert_eq!(report.mode, None, "{report:?}");
    assert_eq!((report.pulled, report.pushed), (0, 0));
}

#[tokio::test]
async fn edits_on_different_notes_converge() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    edit(&replicas.laptop, ids[0], "A {{ one }} laptop").await;
    on_server(edit(&replicas.server, ids[1], "B {{ two }} phone")).await;
    let new_on_server = on_server(create(&replicas.server, &["C {{ three }}"], &["web"])).await;
    let new_on_laptop = create(&replicas.laptop, &["D {{ four }}"], &[]).await;
    assert_ne!(new_on_server[0], new_on_laptop[0]);

    let report = replicas.sync().await;
    assert!(report.notes_changed.contains(&ids[1]));
    assert!(report.notes_changed.contains(&new_on_server[0]));
    replicas.assert_converged().await;
    assert!(
        data(&replicas.laptop, ids[1])
            .await
            .unwrap()
            .ends_with("phone")
    );
    assert!(
        data(&replicas.server, ids[0])
            .await
            .unwrap()
            .ends_with("laptop")
    );
    assert_eq!(tags_of(&replicas.laptop, new_on_server[0]).await, ["web"]);
    // Nothing is left to sync.
    let report = replicas.sync().await;
    assert_eq!((report.pulled, report.pushed), (0, 0), "{report:?}");
}

/// A note whose merge is the server's text, since the server made this side's change too, isn't
/// sent back as if changed here. A delete there would then be taken for an edit here, and the note
/// kept.
#[tokio::test]
async fn a_merge_that_is_the_servers_text_is_not_sent_back() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}\nB {{ two }}\nC {{ three }}"]).await;
    let text = data(&replicas.laptop, ids[0]).await.unwrap();
    let laptop = text.replacen("A ", "A alike ", 1);
    let server = laptop.replacen("C ", "C there ", 1);
    edit(&replicas.laptop, ids[0], &laptop).await;
    on_server(edit(&replicas.server, ids[0], &server)).await;
    let report = replicas.sync().await;
    assert_eq!(report.pushed, 0, "{report:?}");
    assert_eq!(data(&replicas.laptop, ids[0]).await.unwrap(), server);
    replicas.assert_converged().await;

    on_server(delete(&replicas.server, ids[0])).await;
    let report = replicas.sync().await;
    assert!(report.conflicts.is_empty(), "{report:?}");
    assert_eq!(data(&replicas.laptop, ids[0]).await, None);
    replicas.assert_converged().await;
}

#[tokio::test]
async fn old_unsynced_laptop_edit_survives_newer_phone_edit_to_another_line() {
    let (replicas, ids) = Replicas::synced(&[LINES]).await;
    edit(
        &replicas.laptop,
        ids[0],
        &LINES.replace("Line one", "Line ONE (laptop)"),
    )
    .await;
    // The phone's edit is newer.
    on_server(edit(
        &replicas.server,
        ids[0],
        &LINES.replace("Line four", "Line FOUR (phone)"),
    ))
    .await;
    let report = replicas.sync().await;
    assert!(report.conflicts.is_empty(), "{report:?}");
    let merged = data(&replicas.laptop, ids[0]).await.unwrap();
    assert!(merged.contains("Line ONE (laptop)"), "{merged}");
    assert!(merged.contains("Line FOUR (phone)"), "{merged}");
    replicas.assert_converged().await;
    // The card kept its id on both sides.
    assert_eq!(
        cards_of(&replicas.laptop, ids[0]).await,
        cards_of(&replicas.server, ids[0]).await
    );
}

#[tokio::test]
async fn edits_keeping_the_card_orders_merge_with_the_same_cards() {
    let (replicas, ids) = Replicas::synced(&[LINES]).await;
    let card = cards_of(&replicas.laptop, ids[0]).await;
    let synced = data(&replicas.laptop, ids[0]).await.unwrap();
    edit(
        &replicas.laptop,
        ids[0],
        &synced.replace("Line one", "Line ONE"),
    )
    .await;
    on_server(edit(
        &replicas.server,
        ids[0],
        &synced.replace("Line four", "Line FOUR"),
    ))
    .await;
    let report = replicas.sync().await;
    assert!(report.conflicts.is_empty(), "{report:?}");
    replicas.assert_converged().await;
    assert_eq!(
        data(&replicas.laptop, ids[0]).await.unwrap(),
        synced
            .replace("Line one", "Line ONE")
            .replace("Line four", "Line FOUR")
    );
    assert_eq!(cards_of(&replicas.laptop, ids[0]).await, card);
}

#[tokio::test]
async fn same_lines_edited_on_both_sides_is_a_conflict() {
    let (replicas, ids) = Replicas::synced(&[LINES]).await;
    let laptop_text = LINES.replace("Line three", "Line three (laptop)");
    let phone_text = LINES.replace("Line three", "Line three (phone)");
    edit(&replicas.laptop, ids[0], &laptop_text).await;
    on_server(edit(&replicas.server, ids[0], &phone_text)).await;
    let report = replicas.sync().await;
    assert_eq!(report.conflicts, [ids[0]]);
    // The local version stays live, the other is kept in `sync_conflict`.
    assert!(
        data(&replicas.laptop, ids[0])
            .await
            .unwrap()
            .contains("(laptop)")
    );
    let remote: String = sqlx::query_scalar("SELECT remote FROM sync_conflict WHERE note_id = ?")
        .bind(ids[0])
        .fetch_one(&replicas.laptop)
        .await
        .unwrap();
    assert!(remote.contains("(phone)"), "{remote}");
    assert_eq!(tags_of(&replicas.laptop, ids[0]).await, [SYNC_CONFLICT_TAG]);
    replicas.assert_converged().await;
}

#[tokio::test]
async fn tags_merge_as_sets() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    set_tags(&replicas.laptop, ids[0], &["x", "y"], &[]).await;
    replicas.sync().await;
    set_tags(&replicas.laptop, ids[0], &["laptop"], &["x"]).await;
    on_server(set_tags(&replicas.server, ids[0], &["phone"], &["y"])).await;
    replicas.sync().await;
    assert_eq!(tags_of(&replicas.laptop, ids[0]).await, ["laptop", "phone"]);
    replicas.assert_converged().await;
}

#[tokio::test]
async fn tag_reschedule_merges_with_other_changes() {
    let (replicas, _) = Replicas::synced(&[]).await;
    create(&replicas.laptop, &["A {{ one }}"], &["cram"]).await;
    replicas.sync().await;
    sqlx::query("UPDATE tag SET description = 'from the laptop' WHERE name = 'cram'")
        .execute(&replicas.laptop)
        .await
        .unwrap();
    sqlx::query("UPDATE tag SET reschedule = 0 WHERE name = 'cram'")
        .execute(&replicas.server)
        .await
        .unwrap();
    replicas.sync().await;
    replicas.assert_converged().await;
    let (description, reschedule): (String, bool) =
        sqlx::query_as("SELECT description, reschedule FROM tag WHERE name = 'cram'")
            .fetch_one(&replicas.laptop)
            .await
            .unwrap();
    assert_eq!(description, "from the laptop");
    assert!(!reschedule);
}

#[tokio::test]
async fn both_sides_creating_a_tag_with_one_name_gives_one_tag() {
    let (replicas, _) = Replicas::synced(&[]).await;
    let laptop_note = create(&replicas.laptop, &["A {{ one }}"], &["shared"]).await;
    let phone_note = on_server(create(&replicas.server, &["B {{ two }}"], &["shared"])).await;
    replicas.sync().await;
    replicas.assert_converged().await;
    let shared: Vec<i64> = sqlx::query_scalar("SELECT id FROM tag WHERE name = 'shared'")
        .fetch_all(&replicas.laptop)
        .await
        .unwrap();
    assert_eq!(shared.len(), 1);
    assert_eq!(tags_of(&replicas.laptop, laptop_note[0]).await, ["shared"]);
    assert_eq!(tags_of(&replicas.laptop, phone_note[0]).await, ["shared"]);
}

#[tokio::test]
async fn saved_searches_sync_both_ways() {
    let (replicas, _) = Replicas::synced(&[]).await;
    save_search(&replicas.laptop, "due", "c.due<=today").await;
    on_server(save_search(&replicas.server, "hard", "c.difficulty>7")).await;
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        saved_searches(&replicas.laptop).await,
        pairs(&[("due", "c.due<=today"), ("hard", "c.difficulty>7")])
    );

    // Edited on the phone, deleted on the laptop
    on_server(save_search(&replicas.server, "due", "c.due<=today+1d")).await;
    crate::api::saved_search::delete_saved_search(&replicas.laptop, "hard")
        .await
        .unwrap();
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        saved_searches(&replicas.server).await,
        pairs(&[("due", "c.due<=today+1d")])
    );

    // A full download keeps them.
    on_server(save_search(&replicas.server, "new", "c.state=new")).await;
    replicas.sync_mode(SyncMode::FullDownload).await.unwrap();
    replicas.assert_converged().await;
    assert_eq!(saved_searches(&replicas.laptop).await.len(), 2);
}

#[tokio::test]
async fn an_edit_to_a_saved_search_beats_a_delete() {
    let (replicas, _) = Replicas::synced(&[]).await;
    save_search(&replicas.laptop, "due", "c.due<=today").await;
    replicas.sync().await;
    on_server(crate::api::saved_search::delete_saved_search(
        &replicas.server,
        "due",
    ))
    .await
    .unwrap();
    save_search(&replicas.laptop, "due", "c.due<=today+1d").await;
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        saved_searches(&replicas.server).await,
        pairs(&[("due", "c.due<=today+1d")])
    );
}

#[tokio::test]
async fn both_sides_saving_one_name_keep_the_newer_query() {
    let (replicas, _) = Replicas::synced(&[]).await;
    save_search(&replicas.laptop, "due", "c.due<=today").await;
    on_server(save_search(&replicas.server, "due", "c.due<=today+1d")).await;
    exec(&replicas.laptop, "UPDATE saved_search SET updated_at = 100").await;
    exec(&replicas.server, "UPDATE saved_search SET updated_at = 200").await;
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        saved_searches(&replicas.laptop).await,
        pairs(&[("due", "c.due<=today+1d")])
    );

    // The other way around, with the laptop's newer
    save_search(&replicas.laptop, "hard", "c.difficulty>8").await;
    on_server(save_search(&replicas.server, "hard", "c.difficulty>7")).await;
    exec(
        &replicas.laptop,
        "UPDATE saved_search SET updated_at = 400 WHERE name = 'hard'",
    )
    .await;
    exec(
        &replicas.server,
        "UPDATE saved_search SET updated_at = 300 WHERE name = 'hard'",
    )
    .await;
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        saved_searches(&replicas.server).await,
        pairs(&[("due", "c.due<=today+1d"), ("hard", "c.difficulty>8")])
    );
}

/// Swaps the names of saved searches `a` and `b`, by way of a third, since names are unique.
async fn swap(pool: &SqlitePool) {
    exec(
        pool,
        "UPDATE saved_search SET name = 'swap' WHERE name = 'a'",
    )
    .await;
    exec(pool, "UPDATE saved_search SET name = 'a' WHERE name = 'b'").await;
    exec(
        pool,
        "UPDATE saved_search SET name = 'b' WHERE name = 'swap'",
    )
    .await;
}

#[tokio::test]
async fn saved_searches_can_swap_and_reuse_names() {
    let (replicas, _) = Replicas::synced(&[]).await;
    save_search(&replicas.laptop, "a", "tag=a").await;
    save_search(&replicas.laptop, "b", "tag=b").await;
    replicas.sync().await;
    // The phone swaps their names.
    swap(&replicas.server).await;
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        saved_searches(&replicas.laptop).await,
        pairs(&[("a", "tag=b"), ("b", "tag=a")])
    );
    // The laptop swaps them back, and deletes one to save another under its name.
    swap(&replicas.laptop).await;
    crate::api::saved_search::delete_saved_search(&replicas.laptop, "a")
        .await
        .unwrap();
    save_search(&replicas.laptop, "a", "tag=c").await;
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        saved_searches(&replicas.server).await,
        pairs(&[("a", "tag=c"), ("b", "tag=b")])
    );
}

#[tokio::test]
async fn reviews_are_unioned_and_replayed() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    let a = cards_of(&replicas.laptop, ids[0]).await[0];
    let b = cards_of(&replicas.laptop, ids[1]).await[0];
    let now = Utc::now();
    // Card A is reviewed on both sides, card B only on the phone.
    rate(&replicas.laptop, a, 3, now - Duration::minutes(30)).await;
    on_server(rate(&replicas.server, a, 4, now - Duration::minutes(10))).await;
    on_server(rate(&replicas.server, b, 3, now - Duration::minutes(5))).await;
    replicas.sync().await;
    replicas.assert_converged().await;
    let logs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log WHERE card_id = ?")
        .bind(a)
        .fetch_one(&replicas.laptop)
        .await
        .unwrap();
    assert_eq!(logs, 2);
    // B's scheduling comes from the phone, which reviewed it.
    let (state, stability): (i64, f64) =
        sqlx::query_as("SELECT state, stability FROM card WHERE id = ?")
            .bind(b)
            .fetch_one(&replicas.laptop)
            .await
            .unwrap();
    assert!(state != 0 && stability > 0.0);
}

#[tokio::test]
async fn card_fields_merge() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    let card = cards_of(&replicas.laptop, ids[0]).await[0];
    // The laptop suspends the card; the phone reviews it.
    update_cards(
        &replicas.laptop,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card]),
            desired_retention: None,
            special_state: Some(Some(SpecialStateUpdate::Suspended)),
            due: None,
        },
        Utc::now(),
        true,
    )
    .await
    .unwrap();
    on_server(rate(&replicas.server, card, 3, Utc::now())).await;
    replicas.sync().await;
    replicas.assert_converged().await;
    let (special_state, state): (Option<i64>, i64) =
        sqlx::query_as("SELECT special_state, state FROM card WHERE id = ?")
            .bind(card)
            .fetch_one(&replicas.server)
            .await
            .unwrap();
    assert!(special_state.is_some(), "the laptop's suspend is kept");
    assert_ne!(state, 0, "the phone's review is kept");
}

#[tokio::test]
async fn an_edit_beats_a_delete() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    // Edited on the laptop, deleted on the phone.
    edit(&replicas.laptop, ids[0], "A {{ one }} edited").await;
    on_server(delete(&replicas.server, ids[0])).await;
    // Deleted on the laptop, edited on the phone.
    delete(&replicas.laptop, ids[1]).await;
    on_server(edit(&replicas.server, ids[1], "B {{ two }} edited")).await;
    let report = replicas.sync().await;
    assert_eq!(report.conflicts.len(), 2, "{report:?}");
    replicas.assert_converged().await;
    assert!(
        data(&replicas.server, ids[0])
            .await
            .unwrap()
            .ends_with("edited")
    );
    assert_eq!(
        cards_of(&replicas.server, ids[0]).await,
        cards_of(&replicas.laptop, ids[0]).await
    );
    assert!(
        data(&replicas.laptop, ids[1])
            .await
            .unwrap()
            .ends_with("edited")
    );
    assert_eq!(tags_of(&replicas.laptop, ids[1]).await, [SYNC_CONFLICT_TAG]);
}

#[tokio::test]
async fn a_plain_delete_propagates() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    let a_card = cards_of(&replicas.laptop, ids[0]).await[0];
    on_server(rate(&replicas.server, a_card, 3, Utc::now())).await;
    replicas.sync().await;
    delete(&replicas.laptop, ids[0]).await;
    on_server(delete(&replicas.server, ids[1])).await;
    let report = replicas.sync().await;
    assert!(report.notes_deleted.contains(&ids[1]));
    assert_eq!(data(&replicas.server, ids[0]).await, None);
    assert_eq!(data(&replicas.laptop, ids[1]).await, None);
    replicas.assert_converged().await;
    // The review survives, unlinked.
    let orphans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log WHERE card_id IS NULL")
        .fetch_one(&replicas.server)
        .await
        .unwrap();
    assert_eq!(orphans, 1);
}

#[tokio::test]
async fn clozes_added_on_both_sides_keep_their_cards() {
    let text = "Q1 {{ a }}\n\nmiddle\n\nQ3 {{ c }}\n";
    let (replicas, ids) = Replicas::synced(&[text]).await;
    let before = cards_of(&replicas.laptop, ids[0]).await;
    assert_eq!(before.len(), 2);
    let laptop_text =
        data(&replicas.laptop, ids[0])
            .await
            .unwrap()
            .replacen("Q1", "Q0 {{ laptop }}\n\nQ1", 1);
    edit(&replicas.laptop, ids[0], &laptop_text).await;
    let server_text = data(&replicas.server, ids[0]).await.unwrap() + "\nQ4 {{ phone }}\n";
    on_server(edit(&replicas.server, ids[0], &server_text)).await;
    let phone_card = *on_server(cards_of(&replicas.server, ids[0]))
        .await
        .last()
        .unwrap();
    let phone_review_at = Utc::now();
    on_server(rate(&replicas.server, phone_card, 3, phone_review_at)).await;

    let report = replicas.sync().await;
    assert!(report.conflicts.is_empty(), "{report:?}");
    replicas.assert_converged().await;
    let merged = data(&replicas.laptop, ids[0]).await.unwrap();
    assert!(
        merged.contains("laptop") && merged.contains("phone"),
        "{merged}"
    );
    let after = cards_of(&replicas.laptop, ids[0]).await;
    assert_eq!(after.len(), 4, "{merged}");
    // The original cards and the phone's new card (with its review) are kept.
    for id in before.iter().chain([&phone_card]) {
        assert!(after.contains(id), "{id} lost: {after:?}");
    }
    let phone_logs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log WHERE card_id = ?")
        .bind(phone_card)
        .fetch_one(&replicas.laptop)
        .await
        .unwrap();
    assert_eq!(phone_logs, 1);
}

#[tokio::test]
async fn the_same_edit_on_both_sides_merges_with_one_card_per_cloze() {
    let (replicas, ids) = Replicas::synced(&[LINES]).await;
    let before = cards_of(&replicas.laptop, ids[0]).await;
    let synced = data(&replicas.laptop, ids[0]).await.unwrap();
    // A cloze without its order, which each side gives a card of its own
    let text = synced.replace("Line three", "Line three {{ new }}");
    edit(&replicas.laptop, ids[0], &text).await;
    on_server(edit(&replicas.server, ids[0], &text)).await;
    let report = replicas.sync().await;
    assert!(report.conflicts.is_empty(), "{report:?}");
    replicas.assert_converged().await;
    assert_eq!(
        tags_of(&replicas.laptop, ids[0]).await,
        Vec::<String>::new()
    );
    let after = cards_of(&replicas.laptop, ids[0]).await;
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0], before[0]);
    assert_eq!(
        data(&replicas.laptop, ids[0]).await,
        data(&replicas.server, ids[0]).await
    );
}

#[tokio::test]
async fn a_server_change_during_the_sync_restarts_it() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    edit(&replicas.laptop, ids[0], "A {{ one }} laptop").await;
    *replicas.remote.after_pull.lock().unwrap() = Some(format!(
        "UPDATE note SET data = 'B {{{{ two }}}} during' WHERE id = {}",
        ids[1].packed()
    ));
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        data(&replicas.laptop, ids[1]).await.unwrap(),
        "B {{ two }} during"
    );
}

#[tokio::test]
async fn full_download_replaces_the_laptop_but_keeps_its_id_counters() {
    let (replicas, _) = Replicas::synced(&["A {{ one }}"]).await;
    let local_only = create(&replicas.laptop, &["Local {{ only }}"], &[]).await;
    on_server(create(&replicas.server, &["Server {{ note }}"], &[])).await;
    let report = replicas.sync_mode(SyncMode::FullDownload).await.unwrap();
    assert!(report.notes_deleted.contains(&local_only[0]));
    replicas.assert_converged().await;
    // The laptop never hands out the id of the note it lost.
    let next = create(&replicas.laptop, &["Next {{ note }}"], &[]).await;
    assert!(next[0] > local_only[0]);
    // Normal syncs work afterwards.
    edit(&replicas.laptop, next[0], "Next {{ note }} edited").await;
    replicas.sync().await;
    replicas.assert_converged().await;
}

#[tokio::test]
async fn row_count_mismatch_needs_a_full_sync() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    // A row the laptop does not know of and never will: the replicas have drifted apart.
    let mut conn = replicas.server.acquire().await.unwrap();
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    super::set_applying(&mut tx, true).await.unwrap();
    sqlx::query(
        "INSERT INTO tag (id, name, description, auto_delete, usn) VALUES (9, 'drift', '', 0, 0)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    super::set_applying(&mut tx, false).await.unwrap();
    tx.commit().await.unwrap();
    edit(&replicas.laptop, ids[0], "A {{ one }} edited").await;
    let err = replicas.sync_mode(SyncMode::Normal).await.unwrap_err();
    assert!(matches!(err, Error::FullSyncRequired(_)), "{err}");
    // It is remembered. The push reached the server, so the note is stamped.
    let err = replicas.sync_mode(SyncMode::Normal).await.unwrap_err();
    assert!(matches!(err, Error::FullSyncRequired(_)), "{err}");
    let usn: i64 = sqlx::query_scalar("SELECT usn FROM note WHERE id = ?")
        .bind(ids[0])
        .fetch_one(&replicas.laptop)
        .await
        .unwrap();
    assert!(usn > 0, "{usn}");
    replicas.sync_mode(SyncMode::FullUpload).await.unwrap();
    replicas.sync().await;
}

#[tokio::test]
async fn a_schema_mismatch_is_refused() {
    let (replicas, _) = Replicas::synced(&[]).await;
    sqlx::query("UPDATE _sqlx_migrations SET version = version + 1 WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)")
        .execute(&replicas.server)
        .await
        .unwrap();
    let err = replicas.sync_mode(SyncMode::Normal).await.unwrap_err();
    assert!(
        matches!(err, Error::Sync(ref m) if m.contains("migration")),
        "{err}"
    );
}

async fn undo(pool: &SqlitePool) -> Result<(), Error> {
    crate::api::undo::undo_event(
        pool,
        crate::schema::undo::UndoEventRequest {
            event_id: None,
            undo_group: false,
        },
    )
    .await
    .map(|undone| assert!(undone.is_some(), "nothing to undo"))
}

async fn redo(pool: &SqlitePool) -> Result<(), Error> {
    crate::api::undo::redo_event(
        pool,
        crate::schema::undo::RedoEventRequest {
            event_id: None,
            redo_group: false,
        },
    )
    .await
    .map(|redone| assert!(redone.is_some(), "nothing to redo"))
}

async fn review_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM review_log")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn an_edit_can_be_undone_after_a_sync() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    edit(&replicas.laptop, ids[0], "A {{ one }} edited").await;
    replicas.sync().await;
    // The server changing another note doesn't stop it
    on_server(edit(&replicas.server, ids[1], "B {{ two }} there")).await;
    replicas.sync().await;
    undo(&replicas.laptop).await.unwrap();
    replicas.sync().await;
    assert_eq!(
        data(&replicas.server, ids[0]).await.unwrap(),
        "A {{[o:1] one }}"
    );
    replicas.assert_converged().await;
    redo(&replicas.laptop).await.unwrap();
    replicas.sync().await;
    assert_eq!(
        data(&replicas.server, ids[0]).await.unwrap(),
        "A {{[o:1] one }} edited"
    );
    replicas.assert_converged().await;
}

#[tokio::test]
async fn the_server_can_undo_its_own_edit_after_a_sync() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    let card = cards_of(&replicas.laptop, ids[0]).await[0];
    on_server(rate(&replicas.server, card, 3, Utc::now())).await;
    on_server(edit(&replicas.server, ids[0], "A {{ one }} there")).await;
    // The pull stamps both with its usn, which isn't a change from the laptop
    replicas.sync().await;
    on_server(undo(&replicas.server)).await.unwrap();
    on_server(undo(&replicas.server)).await.unwrap();
    replicas.sync().await;
    assert_eq!(
        data(&replicas.laptop, ids[0]).await.unwrap(),
        "A {{[o:1] one }}"
    );
    assert_eq!(review_count(&replicas.laptop).await, 0);
    replicas.assert_converged().await;
}

#[tokio::test]
async fn undo_stops_at_a_sync_that_changed_the_same_note() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}\n\nLine two\n"]).await;
    edit(&replicas.laptop, ids[0], "A {{ one }} here\n\nLine two\n").await;
    replicas.sync().await;
    on_server(edit(
        &replicas.server,
        ids[0],
        "A {{ one }} here\n\nLine two there\n",
    ))
    .await;
    replicas.sync().await;
    let err = undo(&replicas.laptop).await.unwrap_err();
    assert!(
        err.to_string().contains(&format!(
            "\"Edit note {}\" can no longer be undone, since a sync has changed the same notes or \
             cards since",
            ids[0].letters()
        )),
        "{err}"
    );
    // A change made after that sync can be undone.
    edit(
        &replicas.laptop,
        ids[0],
        "A {{ one }} after\n\nLine two there\n",
    )
    .await;
    undo(&replicas.laptop).await.unwrap();
    assert_eq!(
        data(&replicas.laptop, ids[0]).await.unwrap(),
        "A {{[o:1] one }} here\n\nLine two there\n"
    );
}

#[tokio::test]
async fn undoing_a_synced_review_deletes_it_everywhere() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    let card = cards_of(&replicas.laptop, ids[0]).await[0];
    rate(&replicas.laptop, card, 3, Utc::now()).await;
    replicas.sync().await;
    assert_eq!(review_count(&replicas.server).await, 1);
    undo(&replicas.laptop).await.unwrap();
    replicas.sync().await;
    assert_eq!(review_count(&replicas.server).await, 0);
    let state: i64 = sqlx::query_scalar("SELECT state FROM card WHERE id = ?")
        .bind(card)
        .fetch_one(&replicas.server)
        .await
        .unwrap();
    assert_eq!(state, 0, "the card is new again");
    replicas.assert_converged().await;
    // And redone: the review comes back under its id
    redo(&replicas.laptop).await.unwrap();
    replicas.sync().await;
    assert_eq!(review_count(&replicas.server).await, 1);
    replicas.assert_converged().await;
}

#[tokio::test]
async fn a_review_undone_on_the_server_goes_on_the_laptop() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    let card = cards_of(&replicas.laptop, ids[0]).await[0];
    on_server(rate(&replicas.server, card, 3, Utc::now())).await;
    replicas.sync().await;
    assert_eq!(review_count(&replicas.laptop).await, 1);
    on_server(undo(&replicas.server)).await.unwrap();
    replicas.sync().await;
    assert_eq!(review_count(&replicas.laptop).await, 0);
    replicas.assert_converged().await;
}

#[tokio::test]
async fn a_synced_delete_undone_brings_the_note_back_with_its_reviews() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    let card = cards_of(&replicas.laptop, ids[0]).await[0];
    rate(&replicas.laptop, card, 3, Utc::now()).await;
    replicas.sync().await;
    delete(&replicas.laptop, ids[0]).await;
    replicas.sync().await;
    assert_eq!(data(&replicas.server, ids[0]).await, None);
    undo(&replicas.laptop).await.unwrap();
    replicas.sync().await;
    assert_eq!(
        data(&replicas.server, ids[0]).await.unwrap(),
        "A {{[o:1] one }}"
    );
    let linked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log WHERE card_id = ?")
        .bind(card)
        .fetch_one(&replicas.server)
        .await
        .unwrap();
    assert_eq!(linked, 1, "the review has its card again");
    replicas.assert_converged().await;
}

#[tokio::test]
async fn conflicts_resolve_with_either_text_and_keep_their_cards() {
    use super::conflicts::ConflictKind;
    use super::conflicts::Resolution;
    use super::conflicts::list_conflicts;
    use super::conflicts::resolve_conflict;

    let (replicas, ids) = Replicas::synced(&[LINES, "B {{ two }}"]).await;
    let synced = data(&replicas.laptop, ids[0]).await.unwrap();
    let card = cards_of(&replicas.laptop, ids[0]).await;
    edit(
        &replicas.laptop,
        ids[0],
        &synced.replace("Line three", "Line three (laptop)"),
    )
    .await;
    on_server(edit(
        &replicas.server,
        ids[0],
        &synced.replace("Line three", "Line three (phone)"),
    ))
    .await;
    on_server(delete(&replicas.server, ids[1])).await;
    edit(&replicas.laptop, ids[1], "B {{ two }} kept").await;
    replicas.sync().await;

    let conflicts = list_conflicts(&replicas.laptop).await.unwrap();
    assert_eq!(conflicts.len(), 2);
    let text = conflicts.iter().find(|c| c.note_id == ids[0]).unwrap();
    assert_eq!(text.kind, ConflictKind::Text);
    let remote = text.remote.clone().unwrap();
    assert!(
        remote.contains("(phone)") && remote.contains("o:c"),
        "{remote}"
    );
    resolve_conflict(
        &replicas.laptop,
        text.id,
        Resolution::Text(remote),
        &get_all_parsers(),
    )
    .await
    .unwrap();
    let resolved = data(&replicas.laptop, ids[0]).await.unwrap();
    assert_eq!(resolved, synced.replace("Line three", "Line three (phone)"));
    assert_eq!(cards_of(&replicas.laptop, ids[0]).await, card);
    assert_eq!(
        tags_of(&replicas.laptop, ids[0]).await,
        Vec::<String>::new()
    );

    let deleted = conflicts.iter().find(|c| c.note_id == ids[1]).unwrap();
    assert_eq!(deleted.kind, ConflictKind::DeletedThere);
    resolve_conflict(
        &replicas.laptop,
        deleted.id,
        Resolution::Delete,
        &get_all_parsers(),
    )
    .await
    .unwrap();
    assert_eq!(list_conflicts(&replicas.laptop).await.unwrap().len(), 0);
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(data(&replicas.server, ids[1]).await, None);
    assert_eq!(data(&replicas.server, ids[0]).await.unwrap(), resolved);
}

#[tokio::test]
async fn the_note_change_number_moves_on_after_syncs() {
    let latest = |pool: SqlitePool| async move {
        crate::api::undo::get_latest_note_event_id(&pool)
            .await
            .unwrap()
    };
    let (replicas, _) = Replicas::synced(&["A {{ one }}"]).await;
    create(&replicas.laptop, &["B {{ two }}"], &[]).await;
    let before = latest(replicas.laptop.clone()).await;
    assert!(before > 0);
    // A sync changing notes writes no events, so `after_sync` moves the number on.
    let mut conn = replicas.laptop.acquire().await.unwrap();
    super::bump_notes_version(&mut conn).await.unwrap();
    drop(conn);
    let after_sync = latest(replicas.laptop.clone()).await;
    assert!(after_sync > before);
    // A full download clears the events, and the number still grows.
    on_server(create(&replicas.server, &["C {{ three }}"], &[])).await;
    replicas.sync_mode(SyncMode::FullDownload).await.unwrap();
    let after_full = latest(replicas.laptop.clone()).await;
    assert!(after_full > after_sync, "{after_full} <= {after_sync}");
    // New events keep it growing.
    create(&replicas.laptop, &["D {{ four }}"], &[]).await;
    assert!(latest(replicas.laptop.clone()).await > after_full);
}

// More replicas: a browser that keeps a copy of the collection (replica 2, a client), enrolled
// with the server, besides the laptop.

const DEVICE: Replica = Replica::new_unchecked(2);

/// Runs `f` as the device.
async fn on_device<F: std::future::Future>(f: F) -> F::Output {
    with_test_replica(DEVICE, f).await
}

impl Replicas {
    /// A third database, enrolled as replica 2, with a full download of the server's.
    async fn device(&self) -> SqlitePool {
        let enrolled = on_server_enroll(
            &self.server,
            super::devices::EnrollRequest {
                name: "phone".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(enrolled.replica, DEVICE.number());
        let device = open_db(&self.dir.join("device.sqlite")).await;
        self.sync_device(&device, SyncMode::FullDownload)
            .await
            .unwrap();
        device
    }

    async fn sync_device(&self, device: &SqlitePool, mode: SyncMode) -> Result<SyncReport, Error> {
        on_device(sync(device, &self.remote, mode, &self.dir)).await
    }
}

/// Each `render_asset` row as `(path, hash)`, and each `render_package` spec.
async fn render_assets(pool: &SqlitePool) -> (Vec<(String, String)>, Vec<String>) {
    let assets = sqlx::query_as("SELECT path, hash FROM render_asset ORDER BY path")
        .fetch_all(pool)
        .await
        .unwrap();
    let packages = sqlx::query_scalar("SELECT spec FROM render_package ORDER BY spec")
        .fetch_all(pool)
        .await
        .unwrap();
    (assets, packages)
}

/// A device gets the laptop's render assets from the server at each sync where they changed, and
/// never sends its own, which could be older than the laptop's.
#[tokio::test]
async fn a_device_follows_the_render_assets_and_never_sends_them() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    exec(
        &replicas.laptop,
        "INSERT INTO render_asset (path, hash, mtime, size) VALUES ('/t/template.typ', '1111111111111111111111111111111111111111111111111111111111111111', 1, 1);
         INSERT INTO render_package (spec) VALUES ('@preview/a:0.1.0')",
    )
    .await;
    replicas.sync().await;
    let device = replicas.device().await;
    let first = (
        vec![(
            "/t/template.typ".to_string(),
            "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        )],
        vec!["@preview/a:0.1.0".to_string()],
    );
    assert_eq!(render_assets(&device).await, first);

    // The template changed on the laptop, and moved
    exec(
        &replicas.laptop,
        "UPDATE render_asset SET hash = '2222222222222222222222222222222222222222222222222222222222222222', mtime = 2 WHERE path = '/t/template.typ';
         INSERT INTO render_asset (path, hash, mtime, size) VALUES ('/u/template.typ', '3333333333333333333333333333333333333333333333333333333333333333', 2, 1);
         INSERT INTO render_package (spec) VALUES ('@preview/b:0.2.0')",
    )
    .await;
    replicas.sync().await;
    on_device(edit(&device, ids[0], "A {{ one }} phone")).await;
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    let second = (
        vec![
            (
                "/t/template.typ".to_string(),
                "2222222222222222222222222222222222222222222222222222222222222222".to_string(),
            ),
            (
                "/u/template.typ".to_string(),
                "3333333333333333333333333333333333333333333333333333333333333333".to_string(),
            ),
        ],
        vec![
            "@preview/a:0.1.0".to_string(),
            "@preview/b:0.2.0".to_string(),
        ],
    );
    assert_eq!(render_assets(&device).await, second);
    assert_eq!(render_assets(&replicas.server).await, second);

    // Rows a device has, however old, stay there: the server keeps the laptop's
    exec(
        &device,
        "UPDATE render_asset SET hash = '1111111111111111111111111111111111111111111111111111111111111111', mtime = 1 WHERE path = '/t/template.typ'",
    )
    .await;
    on_device(edit(&device, ids[0], "A {{ one }} phone again")).await;
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    assert_eq!(render_assets(&replicas.server).await, second);
    // And the pull brought the server's back
    assert_eq!(render_assets(&device).await, second);

    // A client's older row doesn't replace a newer one either
    exec(
        &replicas.laptop,
        "UPDATE render_asset SET hash = 'h0', mtime = 0 WHERE path = '/t/template.typ'",
    )
    .await;
    replicas.sync().await;
    assert_eq!(render_assets(&replicas.server).await, second);
}

/// The laptop sends every render asset it has, so the server removes those it no longer sends,
/// unless another host syncs with it, whose notes may read them.
#[tokio::test]
async fn the_server_drops_render_assets_the_laptop_no_longer_has() {
    let (replicas, _) = Replicas::synced(&["A {{ one }}"]).await;
    exec(
        &replicas.laptop,
        "INSERT INTO render_asset (path, hash, mtime, size) VALUES ('/t/kept.typ', 'k', 1, 1), ('/t/gone.typ', 'g', 1, 1);
         INSERT INTO render_package (spec) VALUES ('@preview/kept:0.1.0'), ('@preview/gone:0.1.0')",
    )
    .await;
    replicas.sync().await;
    assert_eq!(render_assets(&replicas.server).await.0.len(), 2);

    exec(
        &replicas.laptop,
        "DELETE FROM render_asset WHERE path = '/t/gone.typ';
         DELETE FROM render_package WHERE spec = '@preview/gone:0.1.0'",
    )
    .await;
    // Another host that syncs with the server keeps them
    exec(
        &replicas.server,
        "INSERT INTO sync_client (replica, last_usn, last_sync_at) VALUES (5, 0, 0)",
    )
    .await;
    replicas.sync().await;
    assert_eq!(render_assets(&replicas.server).await.0.len(), 2);

    exec(
        &replicas.server,
        "DELETE FROM sync_client WHERE replica = 5",
    )
    .await;
    replicas.sync().await;
    assert_eq!(
        render_assets(&replicas.server).await,
        (
            vec![("/t/kept.typ".to_string(), "k".to_string())],
            vec!["@preview/kept:0.1.0".to_string()]
        )
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn a_device_has_to_enroll_and_its_number_is_its_own() {
    use super::devices::EnrollRequest;
    use super::devices::UnenrollRequest;
    use super::devices::list;
    use super::devices::unenroll;
    let (replicas, _) = Replicas::synced(&["A {{ one }}"]).await;
    let device = open_db(&replicas.dir.join("device.sqlite")).await;
    let err = replicas
        .sync_device(&device, SyncMode::FullDownload)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotEnrolled(2)), "{err}");

    let name = |name: &str| EnrollRequest {
        name: name.to_string(),
        ..Default::default()
    };
    assert!(on_server_enroll(&replicas.server, name(" ")).await.is_err());
    assert_eq!(
        on_server_enroll(&replicas.server, name("phone"))
            .await
            .unwrap()
            .replica,
        2
    );
    assert_eq!(
        on_server_enroll(&replicas.server, name("tablet"))
            .await
            .unwrap()
            .replica,
        3
    );
    replicas
        .sync_device(&device, SyncMode::FullDownload)
        .await
        .unwrap();
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    let devices = list(&replicas.server).await.unwrap();
    assert_eq!(
        devices
            .iter()
            .map(|d| (d.replica, d.name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "laptop"), (2, "phone"), (3, "tablet")]
    );
    let devices: Vec<_> = devices.into_iter().filter(|d| d.replica != 0).collect();

    // A host claims the number it has, unless another device has it, and gets a device's number
    // when the laptop's is taken.
    let host = |name: &str, replica: Option<u8>| EnrollRequest {
        name: name.to_string(),
        host: true,
        replica,
    };
    for taken in [0, 1, 2, 8] {
        assert!(
            on_server_enroll(&replicas.server, host("another host", Some(taken)))
                .await
                .is_err(),
            "{taken}"
        );
    }

    // A laptop full upload keeps the numbers taken, and when each device last synced, but the
    // device has to full sync.
    let last_sync_at = |devices: &[super::devices::Device]| -> Vec<Option<i64>> {
        devices.iter().map(|d| d.last_sync_at).collect()
    };
    let before = last_sync_at(&devices);
    assert!(before[0].is_some() && before[1].is_none(), "{before:?}");
    replicas.sync_mode(SyncMode::FullUpload).await.unwrap();
    let after: Vec<_> = list(&replicas.server)
        .await
        .unwrap()
        .into_iter()
        .filter(|d| d.replica != 0)
        .collect();
    assert_eq!(last_sync_at(&after), before);
    let err = replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::FullSyncRequired(_)), "{err}");
    assert_eq!(
        on_server_enroll(&replicas.server, host("laptop 2", None))
            .await
            .unwrap()
            .replica,
        4
    );

    // Unenrolled, the device can no longer sync, and its number goes to the next device.
    assert!(
        unenroll(&replicas.server, UnenrollRequest { replica: 2 })
            .await
            .unwrap()
    );
    let err = replicas
        .sync_device(&device, SyncMode::FullDownload)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotEnrolled(2)), "{err}");
    assert_eq!(
        on_server_enroll(&replicas.server, name("new phone"))
            .await
            .unwrap()
            .replica,
        2
    );
    // Unenrolled, the laptop's number goes to the next host that asks.
    unenroll(&replicas.server, UnenrollRequest { replica: 0 })
        .await
        .unwrap();
    assert_eq!(
        on_server_enroll(&replicas.server, host("new laptop", None))
            .await
            .unwrap()
            .replica,
        0
    );
}

#[tokio::test]
async fn a_device_under_a_freed_number_never_hands_out_its_ids_again() {
    use super::devices::EnrollRequest;
    use super::devices::UnenrollRequest;
    use super::devices::enroll;
    use super::devices::unenroll;
    let (replicas, _) = Replicas::synced(&["A {{ one }}"]).await;
    let device = replicas.device().await;
    let made = on_device(create(&device, &["B {{ phone }}", "C {{ phone }}"], &[])).await;
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    // Its newest note is gone everywhere, so no table remembers its id.
    on_device(delete(&device, made[1])).await;
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();

    unenroll(&replicas.server, UnenrollRequest { replica: 2 })
        .await
        .unwrap();
    let enrolled = enroll(
        &replicas.server,
        EnrollRequest {
            name: "new phone".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(enrolled.replica, DEVICE.number());
    let new_device = open_db(&replicas.dir.join("new-device.sqlite")).await;
    replicas
        .sync_device(&new_device, SyncMode::FullDownload)
        .await
        .unwrap();
    let next = on_device(create(&new_device, &["D {{ new phone }}"], &[])).await;
    assert_eq!(
        [made[1].letters(), next[0].letters()],
        ["aaai", "aaaj"],
        "the deleted note's id came back"
    );
}

#[tokio::test]
async fn the_laptop_server_and_a_device_converge() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}", "C {{ three }}"]).await;
    let device = replicas.device().await;
    // Each side edits a note and creates one, while out of sync.
    edit(&replicas.laptop, ids[0], "A {{ one }} laptop").await;
    on_server(edit(&replicas.server, ids[1], "B {{ two }} web")).await;
    on_device(edit(&device, ids[2], "C {{ three }} phone")).await;
    let on_laptop = create(&replicas.laptop, &["D {{ laptop }}"], &[]).await;
    let on_web = on_server(create(&replicas.server, &["E {{ web }}"], &[])).await;
    let on_phone = on_device(create(
        &device,
        &["F {{ phone }}", "G {{ phone }}"],
        &["phone"],
    ))
    .await;
    // Each follows the newest laptop note it has seen, `aac`: the web's and the phone's with letters
    // of their own, and the laptop's next note takes the next laptop id.
    assert_eq!(on_laptop[0].letters(), "aad");
    assert_eq!(on_web[0].letters(), "aaca");
    assert_eq!(
        on_phone.iter().map(|id| id.letters()).collect::<Vec<_>>(),
        ["aach", "aaci"]
    );
    let all: std::collections::BTreeSet<NoteId> = [&on_laptop[..], &on_web, &on_phone]
        .concat()
        .into_iter()
        .collect();
    assert_eq!(all.len(), 4, "ids collided: {all:?}");

    replicas.sync().await;
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(
        contents(&device).await.0,
        contents(&replicas.server).await.0
    );
    assert!(
        data(&replicas.laptop, ids[2])
            .await
            .unwrap()
            .ends_with("phone")
    );
    assert!(data(&device, ids[0]).await.unwrap().ends_with("laptop"));
    assert!(data(&device, ids[1]).await.unwrap().ends_with("web"));
    assert_eq!(tags_of(&replicas.laptop, on_phone[1]).await, ["phone"]);

    // Reviews on the device go in under its residue, and reach the laptop.
    let card = cards_of(&device, ids[0]).await[0];
    on_device(rate(&device, card, 3, Utc::now())).await;
    let log: i64 = sqlx::query_scalar("SELECT MAX(id) FROM review_log")
        .fetch_one(&device)
        .await
        .unwrap();
    assert_eq!(log % crate::ids::ID_STEP, 2);
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    replicas.sync().await;
    replicas.assert_converged().await;
}

#[tokio::test]
async fn the_laptop_and_a_device_editing_one_note_merge() {
    let (replicas, ids) = Replicas::synced(&[LINES]).await;
    let device = replicas.device().await;
    let laptop_edit = LINES.replacen("Line one", "Line one laptop", 1);
    edit(&replicas.laptop, ids[0], &laptop_edit).await;
    on_device(edit(
        &device,
        ids[0],
        &LINES.replacen("Line four", "Line four phone", 1),
    ))
    .await;
    // The device syncs first, then the laptop merges both.
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    replicas.sync().await;
    replicas
        .sync_device(&device, SyncMode::Normal)
        .await
        .unwrap();
    replicas.assert_converged().await;
    let merged = data(&device, ids[0]).await.unwrap();
    assert!(
        merged.contains("Line one laptop") && merged.contains("Line four phone"),
        "{merged}"
    );
}

#[test]
fn a_device_gets_the_config_and_every_template() {
    let config_dir = crate::config::get_config_dir();
    let template = config_dir.join("typst/templates/spares-test-device-config/card.typ");
    std::fs::create_dir_all(template.parent().unwrap()).unwrap();
    std::fs::write(&template, "#let card = 1").unwrap();
    std::fs::write(template.with_file_name("binary.png"), [0xff, 0xfe]).unwrap();
    let config = super::devices::device_config();
    std::fs::remove_dir_all(template.parent().unwrap()).unwrap();
    let config = config.unwrap();
    assert_eq!(
        config.files["typst/templates/spares-test-device-config/card.typ"],
        "#let card = 1"
    );
    assert!(!config.files.keys().any(|path| path.ends_with("binary.png")));
    let toml = &config.files["config.toml"];
    assert!(toml.contains("render_browser_parsers = false"), "{toml}");
    assert!(!toml.contains("[sync]"), "{toml}");
    assert_eq!(config.image_dir, crate::api::image::get_image_directory());
    assert_eq!(
        config.image_occlusion_dir,
        crate::parsers::image_occlusion::get_image_occlusion_directory()
    );
}

/// A browser's pool has one connection. A full download used to wait on a second one, which
/// never came, until the pool timed out.
#[tokio::test]
async fn a_full_download_works_on_a_single_connection() {
    let (replicas, _) = Replicas::synced(&["A {{ one }}"]).await;
    let path = replicas.dir.join("one-connection.sqlite");
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let report = sync(
        &pool,
        &replicas.remote,
        SyncMode::FullDownload,
        &replicas.dir,
    )
    .await
    .unwrap();
    assert_eq!(report.notes_changed.len(), 1);
}

/// A single connection, as a browser's copy has, that has done its first sync
async fn single_connection(replicas: &Replicas) -> SqlitePool {
    let path = replicas.dir.join("one-connection.sqlite");
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .create_if_missing(true)
        .with_regexp();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sync(
        &pool,
        &replicas.remote,
        SyncMode::FullDownload,
        &replicas.dir,
    )
    .await
    .unwrap();
    pool
}

/// How many rows are dirty or being pushed
async fn unsynced(pool: &SqlitePool) -> i64 {
    super::dirty_count(pool).await.unwrap()
}

/// The note's base, as `sync_base` keeps it
async fn base_data(pool: &SqlitePool, id: NoteId) -> Option<String> {
    sqlx::query_scalar(
        "SELECT json_extract(snapshot, '$.data') FROM sync_base WHERE kind = 'note' AND oid = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// The push holds no transaction, so the copy keeps answering while the server takes its time.
#[tokio::test]
async fn a_single_connection_answers_during_the_push() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    let pool = single_connection(&replicas).await;
    edit(&pool, ids[0], "A {{ one }} device").await;
    let during = pool.clone();
    *replicas.remote.during_push.lock().unwrap() = Some(Box::new(move || {
        Box::pin(async move {
            let notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note")
                .fetch_one(&during)
                .await
                .unwrap();
            assert_eq!(notes, 1);
        })
    }));
    let report = sync(&pool, &replicas.remote, SyncMode::Normal, &replicas.dir)
        .await
        .unwrap();
    assert!(report.pushed > 0, "{report:?}");
    assert_eq!(unsynced(&pool).await, 0);
    assert!(
        on_server(data(&replicas.server, ids[0]))
            .await
            .unwrap()
            .ends_with("device")
    );
}

/// A note edited while its earlier edit is pushed stays dirty, with the pushed text as its base,
/// so a server edit to another line afterwards merges without a conflict.
#[tokio::test]
async fn a_write_during_the_push_syncs_the_next_time() {
    let (replicas, ids) = Replicas::synced(&[LINES]).await;
    edit(
        &replicas.laptop,
        ids[0],
        &LINES.replacen("Line three", "Line three laptop", 1),
    )
    .await;
    // With the card's order, which the edit put in
    let pushed = data(&replicas.laptop, ids[0]).await.unwrap();
    let during = replicas.laptop.clone();
    let id = ids[0];
    let later = pushed.replacen("Line four", "Line four laptop", 1);
    let written = later.clone();
    *replicas.remote.during_push.lock().unwrap() = Some(Box::new(move || {
        Box::pin(async move { edit(&during, id, &written).await })
    }));
    replicas.sync().await;
    assert_eq!(
        on_server(data(&replicas.server, ids[0])).await.unwrap(),
        pushed
    );
    assert_eq!(base_data(&replicas.laptop, ids[0]).await.unwrap(), pushed);
    assert!(unsynced(&replicas.laptop).await > 0);

    let phone = pushed.replacen("Line one", "Line one phone", 1);
    on_server(edit(&replicas.server, ids[0], &phone)).await;
    let report = replicas.sync().await;
    assert!(report.conflicts.is_empty(), "{report:?}");
    replicas.assert_converged().await;
    assert_eq!(
        data(&replicas.laptop, ids[0]).await.unwrap(),
        later.replacen("Line one", "Line one phone", 1)
    );
}

/// A push the server applied but whose answer never came is finished by the next sync, which
/// would otherwise take the server for one restored from a backup.
#[tokio::test]
async fn a_lost_push_answer_is_finished_by_the_next_sync() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}", "B {{ two }}"]).await;
    edit(&replicas.laptop, ids[0], "A {{ one }} laptop").await;
    replicas
        .remote
        .lose_push_answer
        .store(true, Ordering::SeqCst);
    let err = replicas.sync_mode(SyncMode::Normal).await.unwrap_err();
    assert!(matches!(err, Error::Sync(_)), "{err}");
    assert!(
        on_server(data(&replicas.server, ids[0]))
            .await
            .unwrap()
            .ends_with("laptop")
    );
    // Changed again before the next sync
    edit(&replicas.laptop, ids[1], "B {{ two }} laptop").await;

    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(unsynced(&replicas.laptop).await, 0);
}

/// A push the server refused, or never received, is sent again by the next sync.
#[tokio::test]
async fn a_push_that_never_arrived_is_sent_again() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    edit(&replicas.laptop, ids[0], "A {{ one }} laptop").await;
    replicas.remote.fail_push.store(true, Ordering::SeqCst);
    replicas.sync_mode(SyncMode::Normal).await.unwrap_err();
    assert!(
        !on_server(data(&replicas.server, ids[0]))
            .await
            .unwrap()
            .ends_with("laptop")
    );
    replicas.sync().await;
    replicas.assert_converged().await;
    assert_eq!(unsynced(&replicas.laptop).await, 0);
}
