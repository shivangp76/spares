//! `POST /api/review/submit` with an `action_id`, through the server's router (with a login) and
//! through the router a browser runs on its own copy of the collection, whose database has one
//! connection as in a browser.
//!
//! A study action is carried out once however often it is sent, and either completely or not at
//! all: these count the review log rows, undo events and recorded actions it leaves.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use http_body_util::BodyExt;
use serde_json::Value;
use serde_json::json;
use spares_core::collection::Collection;
use spares_core::collection::CollectionPaths;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::note::CreateNoteRequest;
use spares_core::schema::note::CreateNotesRequest;
use spares_core::schema::parser::CreateParserRequest;
use tower::ServiceExt;

use crate::AppState;
use crate::host::open_collection;
use crate::host::tests::TempRoot;
use crate::host::tests::test_host;
use crate::route::create_local_router;
use crate::route::create_router;

#[derive(Clone, Copy, Debug)]
enum Route {
    /// The server's router, logged in with a token
    Native,
    /// A browser's own copy of the collection
    Device,
}

struct Setup {
    router: Router,
    collection: Arc<Collection>,
    token: Option<String>,
    card_id: i64,
    _root: TempRoot,
}

impl Setup {
    async fn new(route: Route) -> Self {
        let (router, collection, token, root) = match route {
            Route::Native => {
                let (host, root) = test_host().await;
                let collection = host.collection("me").await.unwrap();
                let router = create_router(Arc::new(AppState { host }), None);
                let request = Request::builder()
                    .method("POST")
                    .uri("/api/auth/token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "user": "me", "password": "password1", "name": "test" })
                            .to_string(),
                    ))
                    .unwrap();
                let (status, body) = respond(&router, request).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                let token = format!("Bearer {}", body["token"].as_str().unwrap());
                (router, collection, Some(token), root)
            }
            Route::Device => {
                let root = std::env::temp_dir().join(format!(
                    "spares-study-action-{}-{}",
                    std::process::id(),
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                ));
                // One connection, as a browser's copy has
                let collection = open_collection("device", CollectionPaths::in_dir(&root), 1)
                    .await
                    .unwrap();
                let router = create_local_router(Arc::clone(&collection));
                (router, collection, None, TempRoot(root))
            }
        };
        let card_id = collection
            .scope(async {
                let db = &collection.db;
                let parser = spares_core::api::parser::create_parser(
                    db,
                    CreateParserRequest {
                        name: "markdown".to_string(),
                    },
                    false,
                )
                .await
                .unwrap();
                let created = spares_core::api::note::create_notes(
                    db,
                    CreateNotesRequest {
                        parser_id: parser.id,
                        requests: vec![CreateNoteRequest {
                            data: "The capital of France is {{Paris}}.".to_string(),
                            keywords: Vec::new(),
                            tags: Vec::new(),
                            is_suspended: false,
                            custom_data: serde_json::Map::new(),
                        }],
                    },
                    chrono::Utc::now(),
                    &get_all_parsers(),
                    false,
                )
                .await
                .unwrap();
                let note_id = created.notes[0].id;
                sqlx::query_scalar::<_, i64>("SELECT id FROM card WHERE note_id = ?")
                    .bind(note_id)
                    .fetch_one(db)
                    .await
                    .unwrap()
            })
            .await;
        Self {
            router,
            collection,
            token,
            card_id,
            _root: root,
        }
    }

    async fn post(&self, uri: &str, body: &Value) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(token) = &self.token {
            request = request.header(AUTHORIZATION, token);
        }
        let request = request.body(Body::from(body.to_string())).unwrap();
        respond(&self.router, request).await
    }

    async fn submit(&self, body: &Value) -> (StatusCode, Value) {
        self.post("/api/review/submit", body).await
    }

    /// A filtered tag that doesn't reschedule its cards, with the card in it
    async fn cram_tag(&self) -> i64 {
        let tag = json!({ "name": "cram", "description": "", "query": "c.state=0",
            "auto_delete": false, "reschedule": false });
        let (status, body) = self.post("/api/tags", &tag).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tag WHERE name = 'cram'")
            .fetch_one(&self.collection.db)
            .await
            .unwrap();
        assert_eq!(
            self.count(&format!(
                "card_tag WHERE tag_id = {tag_id} AND card_id = {}",
                self.card_id
            ))
            .await,
            1
        );
        tag_id
    }

    async fn count(&self, table_and_filter: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table_and_filter}"
        )))
        .fetch_one(&self.collection.db)
        .await
        .unwrap()
    }

    /// What a study action can change: the card's row, its review log, its tags, the undo
    /// events and the recorded actions
    async fn snapshot(&self) -> (Value, i64, i64, i64, i64) {
        let card: String = sqlx::query_scalar(
            "SELECT json_array(due, stability, difficulty, state, special_state, custom_data)
             FROM card WHERE id = ?",
        )
        .bind(self.card_id)
        .fetch_one(&self.collection.db)
        .await
        .unwrap();
        (
            serde_json::from_str(&card).unwrap(),
            self.count("review_log").await,
            self.count("card_tag").await,
            self.count("event").await,
            self.count("study_action").await,
        )
    }
}

async fn respond(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

fn rating(card_id: i64, rating: u32, tag_id: Option<i64>, action_id: Option<&str>) -> Value {
    let mut body = json!({
        "scheduler_name": "fsrs",
        "action": { "Rate": {
            "card_id": card_id,
            "rating": rating,
            "recall_duration": 4,
            "rate_duration": 1,
            "tag_id": tag_id,
        }},
    });
    if let Some(id) = action_id {
        body["action_id"] = id.into();
    }
    body
}

const ROUTES: [Route; 2] = [Route::Native, Route::Device];

/// The response to a rating is lost after it committed, and the client sends it again: the
/// second attempt gets the first one's undo event, and writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_rating_sent_again_is_carried_out_once() {
    for route in ROUTES {
        let setup = Setup::new(route).await;
        let before = setup.snapshot().await;
        let body = rating(setup.card_id, 3, None, Some("rate-1"));
        let (status, first) = setup.submit(&body).await;
        assert_eq!(status, StatusCode::OK, "{route:?}: {first}");
        let event_id = first["event_id"].as_i64().unwrap();
        let after = setup.snapshot().await;
        assert_ne!(after.0, before.0, "{route:?}: the card was rescheduled");
        assert_eq!(after.1, before.1 + 1, "{route:?}: one review");
        assert_eq!(after.3, before.3 + 1, "{route:?}: one undo event");
        assert_eq!(after.4, 1, "{route:?}: the action is recorded");

        for _ in 0..2 {
            let (status, again) = setup.submit(&body).await;
            assert_eq!(status, StatusCode::OK, "{route:?}: {again}");
            assert_eq!(again["event_id"].as_i64(), Some(event_id), "{route:?}");
            assert_eq!(setup.snapshot().await, after, "{route:?}: nothing written");
        }

        // Undoing it by the replayed event id undoes the one review
        let (status, undone) = setup
            .post(
                "/api/undo",
                &json!({ "event_id": event_id, "undo_group": true }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{route:?}: {undone}");
        let undone_card = setup.snapshot().await;
        assert_eq!(undone_card.0, before.0, "{route:?}: the card is as it was");
        assert_eq!(undone_card.1, before.1, "{route:?}: the review is gone");
    }
}

/// A rating in a filtered tag that doesn't reschedule (a cram), sent again: one cram row, one
/// step through the tag.
#[tokio::test(flavor = "multi_thread")]
async fn a_cram_sent_again_is_carried_out_once() {
    for route in ROUTES {
        let setup = Setup::new(route).await;
        let tag_id = setup.cram_tag().await;
        let before = setup.snapshot().await;
        let body = rating(setup.card_id, 3, Some(tag_id), Some("cram-1"));
        let (status, first) = setup.submit(&body).await;
        assert_eq!(status, StatusCode::OK, "{route:?}: {first}");
        let after = setup.snapshot().await;
        assert_eq!(
            setup
                .count(&format!(
                    "review_log WHERE card_id = {} AND tag_id = {tag_id}",
                    setup.card_id
                ))
                .await,
            1,
            "{route:?}: one cram"
        );
        // The card's schedule stays; only its progress through the tag moved
        assert_eq!(after.0[0], before.0[0], "{route:?}: due");
        assert_ne!(
            after.0[5], before.0[5],
            "{route:?}: progress through the tag"
        );
        assert_eq!(after.3, before.3 + 1, "{route:?}: one undo event");

        let (status, again) = setup.submit(&body).await;
        assert_eq!(status, StatusCode::OK, "{route:?}: {again}");
        assert_eq!(again["event_id"], first["event_id"], "{route:?}");
        assert_eq!(setup.snapshot().await, after, "{route:?}: nothing written");
    }
}

/// The same id for a different action is refused, and changes nothing. Ids are refused when
/// malformed, and by actions that don't take one.
#[tokio::test(flavor = "multi_thread")]
async fn an_action_id_names_one_action() {
    for route in ROUTES {
        let setup = Setup::new(route).await;
        let (status, _) = setup
            .submit(&rating(setup.card_id, 3, None, Some("same")))
            .await;
        assert_eq!(status, StatusCode::OK, "{route:?}");
        let after = setup.snapshot().await;

        let (status, body) = setup
            .submit(&rating(setup.card_id, 1, None, Some("same")))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{route:?}: {body}");
        let bury = json!({ "scheduler_name": "fsrs", "action": { "Bury": { "card_id": setup.card_id } },
            "action_id": "same" });
        let (status, body) = setup.submit(&bury).await;
        assert_eq!(status, StatusCode::CONFLICT, "{route:?}: {body}");
        assert_eq!(setup.snapshot().await, after, "{route:?}");

        for id in ["", "has space", &"x".repeat(129)] {
            let (status, body) = setup
                .submit(&rating(setup.card_id, 3, None, Some(id)))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{route:?} {id:?}: {body}");
        }
        let postpone = json!({ "scheduler_name": "fsrs",
            "action": { "Postpone": { "count": 1, "query": null } }, "action_id": "p" });
        let (status, body) = setup.submit(&postpone).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route:?}: {body}");
        assert_eq!(setup.snapshot().await, after, "{route:?}");
    }
}

/// A bury sent again buries once, and its undo event is the first one's.
#[tokio::test(flavor = "multi_thread")]
async fn a_bury_sent_again_is_carried_out_once() {
    for route in ROUTES {
        let setup = Setup::new(route).await;
        let bury = json!({ "scheduler_name": "fsrs",
            "action": { "Bury": { "card_id": setup.card_id } }, "action_id": "bury-1" });
        let (status, first) = setup.submit(&bury).await;
        assert_eq!(status, StatusCode::OK, "{route:?}: {first}");
        let after = setup.snapshot().await;
        // Burying a buried card is refused, so a second bury without the id would fail
        let (status, again) = setup.submit(&bury).await;
        assert_eq!(status, StatusCode::OK, "{route:?}: {again}");
        assert_eq!(again["event_id"], first["event_id"], "{route:?}");
        assert_eq!(setup.snapshot().await, after, "{route:?}");
    }
}

/// A rating without an action id is refused, and changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn ratings_without_an_id_are_refused() {
    for route in ROUTES {
        let setup = Setup::new(route).await;
        let before = setup.snapshot().await;
        let (status, response) = setup.submit(&rating(setup.card_id, 3, None, None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route:?}: {response}");
        assert_eq!(setup.snapshot().await, before, "{route:?}");
    }
}

/// Attempts at one action that arrive together, such as a retry sent while the first attempt is
/// still running, are carried out once, and all get its undo event.
#[tokio::test(flavor = "multi_thread")]
async fn attempts_that_arrive_together_are_carried_out_once() {
    for route in ROUTES {
        let setup = Arc::new(Setup::new(route).await);
        let tag_id = setup.cram_tag().await;
        for (action_id, tag) in [("together", None), ("together-cram", Some(tag_id))] {
            let before = setup.snapshot().await;
            let body = rating(setup.card_id, 3, tag, Some(action_id));
            let attempts = (0..8).map(|_| {
                let setup = Arc::clone(&setup);
                let body = body.clone();
                tokio::spawn(async move { setup.submit(&body).await })
            });
            let mut event_ids = Vec::new();
            for attempt in attempts {
                let (status, response) = attempt.await.unwrap();
                assert_eq!(status, StatusCode::OK, "{route:?}: {response}");
                event_ids.push(response["event_id"].as_i64().unwrap());
            }
            event_ids.dedup();
            assert_eq!(event_ids.len(), 1, "{route:?} {action_id}: {event_ids:?}");
            let after = setup.snapshot().await;
            assert_eq!(after.1, before.1 + 1, "{route:?} {action_id}: one review");
            assert_eq!(
                after.3,
                before.3 + 1,
                "{route:?} {action_id}: one undo event"
            );
            assert_eq!(after.4, before.4 + 1, "{route:?} {action_id}: one record");
        }
    }
}

/// A write that fails partway through a rating (here the undo event, the last one) leaves
/// nothing of it: not the card's new schedule, its review, its step through the filtered tag, nor
/// the record of its id, so that sending it again carries it out.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_write_leaves_nothing_of_the_rating() {
    for route in ROUTES {
        let setup = Setup::new(route).await;
        let tag_id = setup.cram_tag().await;
        for (action_id, tag) in [("fails", None), ("fails-cram", Some(tag_id))] {
            sqlx::query(
                "CREATE TRIGGER fail_events BEFORE INSERT ON event
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END",
            )
            .execute(&setup.collection.db)
            .await
            .unwrap();
            let before = setup.snapshot().await;
            let body = rating(setup.card_id, 3, tag, Some(action_id));
            let (status, response) = setup.submit(&body).await;
            assert_eq!(
                status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{route:?}: {response}"
            );
            assert_eq!(setup.snapshot().await, before, "{route:?} {action_id}");

            sqlx::query("DROP TRIGGER fail_events")
                .execute(&setup.collection.db)
                .await
                .unwrap();
            let (status, response) = setup.submit(&body).await;
            assert_eq!(status, StatusCode::OK, "{route:?}: {response}");
            let after = setup.snapshot().await;
            assert_eq!(after.1, before.1 + 1, "{route:?} {action_id}: one review");
            assert_eq!(
                after.3,
                before.3 + 1,
                "{route:?} {action_id}: one undo event"
            );
        }
    }
}
