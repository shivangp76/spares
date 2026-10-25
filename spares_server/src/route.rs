#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
#[cfg(not(target_family = "wasm"))]
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
#[cfg(not(target_family = "wasm"))]
use axum::middleware;
use axum::routing::delete;
use axum::routing::get;
use axum::routing::patch;
use axum::routing::post;
use axum::routing::put;
#[cfg(not(target_family = "wasm"))]
use tower_http::services::ServeDir;
#[cfg(not(target_family = "wasm"))]
use tower_http::services::ServeFile;

use crate::AppState;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::audio::AUDIO_BODY_LIMIT;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::audio::create_audio_handler;
use crate::handlers::card::count_cards_handler;
use crate::handlers::card::forget_card_handler;
use crate::handlers::card::get_card_handler;
use crate::handlers::card::get_card_reviews_handler;
use crate::handlers::card::get_cards_handler;
use crate::handlers::card::get_leeches_handler;
use crate::handlers::card::list_cards_handler;
use crate::handlers::card::unbury_cards_handler;
use crate::handlers::card::update_cards_handler;
use crate::handlers::health_check_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image::IMAGE_BODY_LIMIT;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image::create_image_from_url_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image::create_image_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image_occlusion::IMAGE_OCCLUSION_BODY_LIMIT;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image_occlusion::create_image_occlusion_handler;
use crate::handlers::image_occlusion::get_image_occlusion_editor_config_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image_occlusion::get_image_occlusion_file_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image_occlusion::list_note_image_occlusions_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image_occlusion::update_note_image_occlusion_handler;
use crate::handlers::note::count_notes_handler;
use crate::handlers::note::create_notes_handler;
use crate::handlers::note::delete_notes_handler;
use crate::handlers::note::export_notes_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::note::find_live_note_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::note::generate_note_files_handler;
use crate::handlers::note::get_duplicate_keywords_handler;
use crate::handlers::note::get_keywords_handler;
use crate::handlers::note::get_note_graph_handler;
use crate::handlers::note::get_note_handler;
use crate::handlers::note::get_note_links_handler;
use crate::handlers::note::get_note_render_handler;
use crate::handlers::note::get_unmatched_keywords_handler;
use crate::handlers::note::list_notes_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::note::relocate_live_cards_handler;
use crate::handlers::note::search_examples_handler;
use crate::handlers::note::search_fields_handler;
use crate::handlers::note::search_keyword_handler;
use crate::handlers::note::search_notes_handler;
use crate::handlers::note::search_page_handler;
use crate::handlers::note::update_notes_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_anki_finish_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_anki_link_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_anki_receive_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_anki_refresh_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_check_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_finish_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_init_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_receive_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_remove_saved_copies_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::notes_vcs::notes_vcs_status_handler;
use crate::handlers::parser::create_parser_handler;
use crate::handlers::parser::delete_parser_handler;
use crate::handlers::parser::get_parser_handler;
use crate::handlers::parser::get_parser_syntax_handler;
use crate::handlers::parser::list_known_parsers_handler;
use crate::handlers::parser::list_parsers_handler;
use crate::handlers::parser::update_parser_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::render_asset::get_render_asset_handler;
use crate::handlers::render_asset::list_render_packages_handler;
use crate::handlers::render_asset::list_render_sources_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::require_api_key;
use crate::handlers::review::create_review_snapshot_handler;
use crate::handlers::review::get_review_card_by_id_handler;
use crate::handlers::review::get_review_card_handler;
use crate::handlers::review::get_review_config_handler;
use crate::handlers::review::get_review_history_handler;
use crate::handlers::review::get_review_queue_handler;
use crate::handlers::review::get_statistics_graphs_handler;
use crate::handlers::review::get_statistics_handler;
use crate::handlers::review::get_steps_stats_handler;
use crate::handlers::review::next_review_card_available_at_handler;
use crate::handlers::review::queue_positions_handler;
use crate::handlers::review::submit_study_action_handler;
use crate::handlers::review::sync_offline_actions_handler;
use crate::handlers::saved_search::delete_saved_search_handler;
use crate::handlers::saved_search::get_saved_search_handler;
use crate::handlers::saved_search::list_saved_searches_handler;
use crate::handlers::saved_search::save_search_handler;
use crate::handlers::scheduler::get_rating_from_score_handler;
use crate::handlers::scheduler::get_scheduler_ratings_handler;
use crate::handlers::scheduler::get_scheduler_states_handler;
use crate::handlers::scheduler::list_schedulers_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::scheduler::optimize_scheduler_handler;
use crate::handlers::scheduler::reschedule_scheduler_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::MEDIA_BODY_LIMIT;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::SYNC_BODY_LIMIT;
use crate::handlers::sync::list_sync_conflicts_handler;
use crate::handlers::sync::resolve_sync_conflict_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_config_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_devices_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_enroll_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_files_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_full_download_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_full_upload_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_media_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_meta_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_pull_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_push_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_run_handler;
use crate::handlers::sync::sync_status_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_unenroll_handler;
use crate::handlers::tag::create_tag_handler;
use crate::handlers::tag::delete_tag_handler;
use crate::handlers::tag::get_tag_by_name_handler;
use crate::handlers::tag::get_tag_handler;
use crate::handlers::tag::list_tags_handler;
use crate::handlers::tag::rebuild_tag_handler;
use crate::handlers::tag::rename_tags_handler;
use crate::handlers::tag::update_tag_handler;
use crate::handlers::undo::get_latest_note_event_id_handler;
use crate::handlers::undo::group_events_handler;
use crate::handlers::undo::redo_event_handler;
use crate::handlers::undo::undo_event_handler;

/// Images and audio added to notes, and image occlusions, which are files on the server
#[cfg(not(target_family = "wasm"))]
fn media_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Images pasted or dropped into a note
        .route(
            "/api/images",
            post(create_image_handler).layer(DefaultBodyLimit::max(IMAGE_BODY_LIMIT)),
        )
        .route("/api/images/url", post(create_image_from_url_handler))
        // Audio clips recorded into a note
        .route(
            "/api/audio",
            post(create_audio_handler).layer(DefaultBodyLimit::max(AUDIO_BODY_LIMIT)),
        )
        .route(
            "/api/image-occlusions",
            post(create_image_occlusion_handler)
                .layer(DefaultBodyLimit::max(IMAGE_OCCLUSION_BODY_LIMIT)),
        )
        .route(
            "/api/image-occlusions/file",
            get(get_image_occlusion_file_handler),
        )
        .route(
            "/api/notes/{id}/image-occlusions",
            get(list_note_image_occlusions_handler),
        )
        .route(
            "/api/notes/{id}/image-occlusions/{index}",
            put(update_note_image_occlusion_handler),
        )
        // Files read by sources that clients compile themselves
        .route("/api/render-assets", get(get_render_asset_handler))
}

/// Notes repository (`spares vcs init`, its push hooks, and the web UI's notice of saved
/// copies)
#[cfg(not(target_family = "wasm"))]
fn notes_vcs_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/notes/vcs/status", get(notes_vcs_status_handler))
        .route("/api/notes/vcs/receive", post(notes_vcs_receive_handler))
        .route("/api/notes/vcs/finish", post(notes_vcs_finish_handler))
        .route("/api/notes/vcs/init", post(notes_vcs_init_handler))
        .route("/api/notes/vcs/check", post(notes_vcs_check_handler))
        .route(
            "/api/notes/vcs/anki/refresh",
            post(notes_vcs_anki_refresh_handler),
        )
        .route(
            "/api/notes/vcs/anki/receive",
            post(notes_vcs_anki_receive_handler),
        )
        .route(
            "/api/notes/vcs/anki/finish",
            post(notes_vcs_anki_finish_handler),
        )
        .route(
            "/api/notes/vcs/anki/link",
            post(notes_vcs_anki_link_handler),
        )
        .route(
            "/api/notes/vcs/saved-copies/remove",
            post(notes_vcs_remove_saved_copies_handler),
        )
}

/// Two-way sync (`spares_core::sync`), the server's side and a client server's own sync. A browser
/// syncs its copy through `spares_wasm` instead.
#[cfg(not(target_family = "wasm"))]
fn sync_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sync/meta", get(sync_meta_handler))
        .route("/api/sync/pull", post(sync_pull_handler))
        .route(
            "/api/sync/push",
            post(sync_push_handler).layer(DefaultBodyLimit::max(SYNC_BODY_LIMIT)),
        )
        .route(
            "/api/sync/full",
            get(sync_full_download_handler)
                .put(sync_full_upload_handler)
                .layer(DefaultBodyLimit::max(SYNC_BODY_LIMIT)),
        )
        .route("/api/sync/enroll", post(sync_enroll_handler))
        .route("/api/sync/unenroll", post(sync_unenroll_handler))
        .route("/api/sync/devices", get(sync_devices_handler))
        .route("/api/sync/config", get(sync_config_handler))
        .route("/api/sync/files", get(sync_files_handler))
        .route(
            "/api/sync/media/{kind}/{name}",
            put(sync_media_handler).layer(DefaultBodyLimit::max(MEDIA_BODY_LIMIT)),
        )
        .route("/api/sync/run", post(sync_run_handler))
}

/// The routes that need the server's files, processes or role: the rest of the API besides
/// [`shared_routes`].
#[cfg(not(target_family = "wasm"))]
fn native_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/notes/generate_files",
            post(generate_note_files_handler),
        )
        // Live notes are files on the laptop.
        .route("/api/notes/live", post(find_live_note_handler))
        .route(
            "/api/notes/live/relocate-cards",
            post(relocate_live_cards_handler),
        )
        .route(
            "/api/scheduler/{name}/optimize",
            post(optimize_scheduler_handler),
        )
        .merge(media_routes())
        .merge(notes_vcs_routes())
        .merge(sync_routes())
}

/// Review
fn review_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/review", post(get_review_card_handler))
        .route(
            "/api/review/next-available",
            post(next_review_card_available_at_handler),
        )
        .route("/api/review/card/{id}", post(get_review_card_by_id_handler))
        .route("/api/review/submit", post(submit_study_action_handler))
        .route("/api/review/snapshot", post(create_review_snapshot_handler))
        .route("/api/review/queue/position", post(queue_positions_handler))
        .route("/api/review/statistics", post(get_statistics_handler))
        .route("/api/review/history", get(get_review_history_handler))
        .route(
            "/api/statistics/graphs",
            post(get_statistics_graphs_handler),
        )
        .route("/api/statistics/steps", post(get_steps_stats_handler))
        .route("/api/review/config", get(get_review_config_handler))
}

/// Offline review on web clients
fn offline_review_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/review/queue", post(get_review_queue_handler))
        .route("/api/review/sync", post(sync_offline_actions_handler))
}

/// The routes that only need the database (and the config and parser templates, see
/// `spares_core::platform`): what a browser can answer from its own copy.
#[allow(clippy::too_many_lines, reason = "one line per route")]
fn shared_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Parser
        .route("/api/parsers", post(create_parser_handler))
        .route("/api/parsers/{id}", get(get_parser_handler))
        .route("/api/parsers/{id}/syntax", get(get_parser_syntax_handler))
        .route("/api/parsers/{id}", patch(update_parser_handler))
        .route("/api/parsers/{id}", delete(delete_parser_handler))
        .route("/api/parsers", get(list_parsers_handler))
        .route("/api/parsers/known", get(list_known_parsers_handler))
        // Tag
        .route("/api/tags", post(create_tag_handler))
        .route("/api/tags/{id}", get(get_tag_handler))
        .route("/api/tags/name/{id}", get(get_tag_by_name_handler))
        .route("/api/tags", patch(update_tag_handler))
        .route("/api/tags/rename", post(rename_tags_handler))
        .route("/api/tags/{id}", delete(delete_tag_handler))
        .route("/api/tags", get(list_tags_handler))
        .route("/api/tags/{id}/rebuild", get(rebuild_tag_handler))
        // Note
        .route("/api/notes", post(create_notes_handler))
        .route("/api/notes/{id}", get(get_note_handler))
        .route("/api/notes/{id}/render", get(get_note_render_handler))
        .route("/api/notes", patch(update_notes_handler)) // the request body contains note_ids: Vec<i64>
        .route("/api/notes", delete(delete_notes_handler))
        .route("/api/notes", get(list_notes_handler))
        .route("/api/notes/count", get(count_notes_handler))
        .route("/api/notes/search", post(search_notes_handler))
        .route("/api/notes/search/page", post(search_page_handler))
        .route("/api/search/fields", get(search_fields_handler))
        .route("/api/search/examples", get(search_examples_handler))
        .route("/api/saved-searches", get(list_saved_searches_handler))
        .route("/api/saved-searches", put(save_search_handler))
        .route("/api/saved-searches/{name}", get(get_saved_search_handler))
        .route(
            "/api/saved-searches/{name}",
            delete(delete_saved_search_handler),
        )
        .route("/api/notes/search/keyword", post(search_keyword_handler))
        .route(
            "/api/notes/unmatched-keywords",
            get(get_unmatched_keywords_handler),
        )
        .route(
            "/api/notes/duplicate-keywords",
            get(get_duplicate_keywords_handler),
        )
        .route("/api/notes/keywords", get(get_keywords_handler))
        .route("/api/notes/search/note-links", post(get_note_links_handler))
        .route("/api/notes/graph", post(get_note_graph_handler))
        .route("/api/notes/export", post(export_notes_handler))
        .route(
            "/api/notes/latest-event-id",
            get(get_latest_note_event_id_handler),
        )
        // Card
        .route("/api/cards", get(list_cards_handler))
        .route("/api/cards/count", get(count_cards_handler))
        .route("/api/cards/{id}", get(get_card_handler))
        .route("/api/cards/note_id/{id}", get(get_cards_handler))
        .route("/api/cards/leeches", post(get_leeches_handler))
        .route("/api/cards", patch(update_cards_handler))
        .route("/api/cards/{id}/forget", post(forget_card_handler))
        .route("/api/cards/{id}/reviews", get(get_card_reviews_handler))
        .route("/api/cards/unbury", post(unbury_cards_handler))
        .merge(review_routes())
        // Scheduler
        .route("/api/schedulers", get(list_schedulers_handler))
        .route(
            "/api/scheduler/{name}/ratings",
            get(get_scheduler_ratings_handler),
        )
        .route(
            "/api/scheduler/{name}/states",
            get(get_scheduler_states_handler),
        )
        .route(
            "/api/scheduler/{name}/rating",
            get(get_rating_from_score_handler),
        )
        .route(
            "/api/scheduler/{name}/reschedule",
            post(reschedule_scheduler_handler),
        )
        // Undo
        .route("/api/undo", post(undo_event_handler))
        .route("/api/redo", post(redo_event_handler))
        .route("/api/undo/group", post(group_events_handler))
        .route(
            "/api/image-occlusions/editor-config",
            get(get_image_occlusion_editor_config_handler),
        )
        .route(
            "/api/render-assets/packages",
            get(list_render_packages_handler),
        )
        .route(
            "/api/render-assets/sources",
            get(list_render_sources_handler),
        )
        .merge(offline_review_routes())
        // A client's sync status and conflicts are its own.
        .route("/api/sync/status", get(sync_status_handler))
        .route("/api/sync/conflicts", get(list_sync_conflicts_handler))
        .route(
            "/api/sync/conflicts/{id}",
            post(resolve_sync_conflict_handler),
        )
}

/// The server's router: every route, rendered files at `/files`, and the web app from
/// `frontend_dir`.
#[cfg(not(target_family = "wasm"))]
pub fn create_router(
    app_state: Arc<AppState>,
    files_dir: PathBuf,
    frontend_dir: Option<PathBuf>,
) -> Router {
    crate::version::init();
    let served_frontend = frontend_dir.clone();
    let protected = shared_routes()
        .merge(native_routes())
        .layer(middleware::map_response(not_stored))
        // Rendered notes and cards
        .nest_service("/files", ServeDir::new(files_dir))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            require_api_key,
        ));

    let mut router = Router::new()
        .route("/api/healthcheck", get(health_check_handler))
        .route(
            "/api/version",
            get(move || crate::version::version_handler(served_frontend)),
        )
        .merge(protected)
        .with_state(app_state);

    if let Some(dir) = frontend_dir {
        // Serve the SPA: known static files are served directly, all other paths
        // fall back to index.html so React Router handles client-side navigation.
        // The build writes brotli and gzip copies of the files, which are sent instead to
        // browsers that accept them.
        let index = ServeFile::new(dir.join("index.html"))
            .precompressed_br()
            .precompressed_gzip();
        router = router.fallback_service(
            ServeDir::new(&dir)
                .precompressed_br()
                .precompressed_gzip()
                .fallback(index),
        );
    }

    router
}

/// `Cache-Control: no-store` for an API response that doesn't say otherwise. Without it, Firefox
/// keeps responses such as `GET /api/sync/meta` and answers later requests with them, so a
/// browser's copy would sync against the server as it was.
#[cfg(not(target_family = "wasm"))]
async fn not_stored(mut response: axum::response::Response) -> axum::response::Response {
    response
        .headers_mut()
        .entry(axum::http::header::CACHE_CONTROL)
        .or_insert(axum::http::HeaderValue::from_static("no-store"));
    response
}

/// The router a browser runs on its own copy of the collection (`spares_wasm`): [`shared_routes`],
/// with no API key, since requests never leave the browser.
///
/// A few requests act on the server's files or devices rather than on the collection. They are
/// answered with [`needs_server`], so the web app sends them to the server instead. Every other
/// request is refused ([`not_on_device`]) rather than sent on, so a server route added later never
/// changes the server's database behind the copy's back. Of those, image occlusions, rendering,
/// live notes and optimizing a scheduler (which writes `config.toml` and reschedules the server's
/// cards) stay on the server.
pub fn create_local_router(app_state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/healthcheck", get(health_check_handler))
        .merge(shared_routes())
        .route("/api/images", post(needs_server))
        .route("/api/images/url", post(needs_server))
        .route("/api/audio", post(needs_server))
        .route("/api/render-assets", get(needs_server))
        .route("/api/notes/vcs/status", get(needs_server))
        .route("/api/notes/vcs/saved-copies/remove", post(needs_server))
        .route("/api/sync/devices", get(needs_server))
        .route("/api/sync/unenroll", post(needs_server))
        .fallback(not_on_device)
        // Also when a route has the path for another method: `POST /api/notes/live` matches
        // `GET /api/notes/{id}`.
        .method_not_allowed_fallback(not_on_device)
        .with_state(app_state)
}

/// `503 {"needs_server": true}`: a request for the server's files or devices, which a browser's
/// own copy sends on.
async fn needs_server() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "message": "This needs the server",
            "needs_server": true,
        })),
    )
}

/// `501`: a request that would act on the server's collection, not this device's copy.
async fn not_on_device() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": "This isn't available on a device that keeps a copy of the collection. \
                        Use the laptop, or a browser that doesn't keep a copy.",
        })),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    use super::*;

    async fn call(router: &Router, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or_default())
    }

    async fn call_json(
        router: &Router,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or_default())
    }

    /// A browser's router answers from its database, sends requests for the server's files and
    /// devices to the server, and refuses the rest.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn the_local_router_answers_what_it_can(pool: SqlitePool) {
        let router = create_local_router(Arc::new(AppState {
            db: pool,
            api_key: Some("unused".to_string()),
        }));
        // No API key: requests never leave the browser.
        let (status, parsers) = call(&router, "GET", "/api/parsers").await;
        assert_eq!(status, StatusCode::OK, "{parsers}");
        assert_eq!(parsers, serde_json::json!([]));
        assert_eq!(
            call(&router, "GET", "/api/sync/conflicts").await.0,
            StatusCode::OK
        );
        // Templates are in memory in a browser, and empty in tests
        let (status, exported) = call_json(
            &router,
            "POST",
            "/api/notes/export",
            serde_json::json!({ "query": "" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{exported}");
        for (method, uri) in [
            ("POST", "/api/images"),
            ("POST", "/api/images/url"),
            ("POST", "/api/audio"),
            ("GET", "/api/render-assets?path=/a.typ"),
            ("GET", "/api/notes/vcs/status"),
            ("POST", "/api/notes/vcs/saved-copies/remove"),
            ("GET", "/api/sync/devices"),
            ("POST", "/api/sync/unenroll"),
        ] {
            let (status, body) = call(&router, method, uri).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{method} {uri}");
            assert_eq!(body["needs_server"], true, "{method} {uri}");
        }
        for (method, uri) in [
            ("GET", "/files/abc-1-front.pdf"),
            ("GET", "/api/sync/meta?replica=2"),
            ("POST", "/api/sync/push"),
            ("POST", "/api/sync/run"),
            ("POST", "/api/image-occlusions"),
            ("GET", "/api/image-occlusions/file?path=/a.svg"),
            ("GET", "/api/notes/1/image-occlusions"),
            ("PUT", "/api/notes/1/image-occlusions/0"),
            ("POST", "/api/scheduler/fsrs/optimize"),
            ("POST", "/api/notes/generate_files"),
            ("POST", "/api/notes/live"),
            ("POST", "/api/notes/live/relocate-cards"),
            ("POST", "/api/notes/vcs/receive"),
        ] {
            let (status, body) = call(&router, method, uri).await;
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
            assert_eq!(
                body["needs_server"],
                serde_json::Value::Null,
                "{method} {uri}"
            );
        }
    }

    /// Browsers don't keep API responses, which would answer later requests with old data.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn api_responses_are_not_stored(pool: SqlitePool) {
        let files = std::env::temp_dir();
        let router = create_router(
            Arc::new(AppState {
                db: pool,
                api_key: None,
            }),
            files,
            None,
        );
        let request = Request::builder()
            .uri("/api/parsers")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
    }
}
