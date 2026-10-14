//! Sync between two databases: the laptop (replica 0, client) and the server (replica 1),
//! reached in process.

use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Duration;
use chrono::Utc;
use serde_json::Map;
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
use crate::config::SyncRole;
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
    with_test_replica(SERVER, SyncRole::Server, f).await
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
        Self {
            remote: InProcess {
                pool: server.clone(),
                after_pull: Mutex::new(None),
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
    }
}

struct InProcess {
    pool: SqlitePool,
    /// Runs on the server right after the next pull, as a phone edit during a sync would.
    after_pull: Mutex<Option<String>>,
}

#[async_trait]
impl SyncRemote for InProcess {
    async fn meta(&self, replica: u8) -> Result<MetaResponse, Error> {
        on_server(server::meta(&self.pool, Some(replica))).await
    }

    async fn pull(&self, request: PullRequest) -> Result<PullResponse, Error> {
        let response = on_server(server::pull(&self.pool, request)).await?;
        let statement = self.after_pull.lock().unwrap().take();
        if let Some(statement) = statement {
            sqlx::query(&statement).execute(&self.pool).await.unwrap();
        }
        Ok(response)
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, Error> {
        on_server(server::push(&self.pool, request))
            .await
            .map(|(response, _)| response)
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
    let all = Selection::After(-2);
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
    // It is remembered, and nothing was committed on the laptop.
    let err = replicas.sync_mode(SyncMode::Normal).await.unwrap_err();
    assert!(matches!(err, Error::FullSyncRequired(_)), "{err}");
    let dirty: i64 = sqlx::query_scalar("SELECT usn FROM note WHERE id = ?")
        .bind(ids[0])
        .fetch_one(&replicas.laptop)
        .await
        .unwrap();
    assert_eq!(dirty, -1);
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

#[tokio::test]
async fn undo_stops_at_the_last_sync() {
    let (replicas, ids) = Replicas::synced(&["A {{ one }}"]).await;
    edit(&replicas.laptop, ids[0], "A {{ one }} before").await;
    replicas.sync().await;
    let undo = || {
        crate::api::undo::undo_event(
            &replicas.laptop,
            crate::schema::undo::UndoEventRequest {
                event_id: None,
                undo_group: false,
            },
        )
    };
    let err = undo().await.unwrap_err();
    assert!(err.to_string().contains("before the last sync"), "{err}");
    // A change made after the sync can be undone.
    edit(&replicas.laptop, ids[0], "A {{ one }} after").await;
    undo().await.unwrap().unwrap();
    assert!(data(&replicas.laptop, ids[0]).await.unwrap().ends_with("before"));
}
