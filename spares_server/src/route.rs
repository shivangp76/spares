#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
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

#[cfg(not(target_family = "wasm"))]
use crate::AppState;
use crate::handlers::audio::AUDIO_BODY_LIMIT;
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
use crate::handlers::image::IMAGE_BODY_LIMIT;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::image::create_image_from_url_handler;
use crate::handlers::image::create_image_handler;
use crate::handlers::image_occlusion::IMAGE_OCCLUSION_BODY_LIMIT;
use crate::handlers::image_occlusion::create_image_occlusion_handler;
use crate::handlers::image_occlusion::get_image_occlusion_editor_config_handler;
use crate::handlers::image_occlusion::get_image_occlusion_file_handler;
use crate::handlers::image_occlusion::list_note_image_occlusions_handler;
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
#[cfg(not(target_family = "wasm"))]
use crate::handlers::note::list_live_notes_handler;
use crate::handlers::note::list_notes_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::note::relocate_live_cards_handler;
use crate::handlers::note::search_examples_handler;
use crate::handlers::note::search_explain_handler;
use crate::handlers::note::search_fields_handler;
use crate::handlers::note::search_keyword_handler;
use crate::handlers::note::search_notes_handler;
use crate::handlers::note::search_page_handler;
use crate::handlers::note::update_notes_batch_handler;
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
use crate::handlers::review::create_review_snapshot_handler;
use crate::handlers::review::get_review_card_by_id_handler;
use crate::handlers::review::get_review_card_handler;
use crate::handlers::review::get_review_config_handler;
use crate::handlers::review::get_review_history_handler;
use crate::handlers::review::get_statistics_graphs_handler;
use crate::handlers::review::get_statistics_handler;
use crate::handlers::review::get_steps_stats_handler;
use crate::handlers::review::get_unmasked_image_occlusion_handler;
use crate::handlers::review::next_review_card_available_at_handler;
use crate::handlers::review::queue_positions_handler;
use crate::handlers::review::submit_study_action_handler;
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
#[cfg(not(target_family = "wasm"))]
use crate::handlers::scheduler::reschedule_scheduler_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::MEDIA_BODY_LIMIT;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::SYNC_BODY_LIMIT;
use crate::handlers::sync::list_sync_conflicts_handler;
use crate::handlers::sync::resolve_sync_conflict_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_changed_files_handler;
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
use crate::handlers::sync::sync_media_download_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_media_handler;
#[cfg(not(target_family = "wasm"))]
use crate::handlers::sync::sync_media_list_handler;
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
use crate::handlers::undo::list_events_handler;
use crate::handlers::undo::redo_event_handler;
use crate::handlers::undo::undo_event_handler;

/// Files on the server that a browser's copy doesn't have
#[cfg(not(target_family = "wasm"))]
fn media_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        // Images dragged from a web page, which the server downloads
        .route("/api/images/url", post(create_image_from_url_handler))
        // Files read by sources that clients compile themselves
        .route("/api/render-assets", get(get_render_asset_handler))
        // Shared notes name files a browser's copy may not have
        .route(
            "/api/notes/share/summary",
            post(crate::handlers::share::share_summary_handler),
        )
        .route(
            "/api/notes/share/bundle",
            post(crate::handlers::share::share_bundle_handler),
        )
        .route(
            "/api/notes/share/import",
            post(crate::handlers::share::import_shared_handler).layer(DefaultBodyLimit::max(
                crate::handlers::share::IMPORT_BODY_LIMIT,
            )),
        )
        // Imports render the notes, and store the media that zips bundle
        .route(
            "/api/notes/import",
            post(crate::handlers::share::import_files_handler).layer(DefaultBodyLimit::max(
                crate::handlers::share::IMPORT_BODY_LIMIT,
            )),
        )
}

/// Images and audio added to notes, and image occlusions. A browser's copy stores the files it
/// adds through `spares_core::platform`, and uploads them before the push whose notes name them
/// (see `spares_core::sync::media`).
fn shared_media_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        // Images pasted or dropped into a note
        .route(
            "/api/images",
            post(create_image_handler).layer(DefaultBodyLimit::max(IMAGE_BODY_LIMIT)),
        )
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
            "/api/image-occlusions/editor-config",
            get(get_image_occlusion_editor_config_handler),
        )
        .route(
            "/api/notes/{id}/image-occlusions",
            get(list_note_image_occlusions_handler),
        )
        .route(
            "/api/notes/{id}/image-occlusions/{index}",
            put(update_note_image_occlusion_handler),
        )
}

/// Notes repository (`spares vcs init`, its push hooks, and the web UI's notice of saved
/// copies)
#[cfg(not(target_family = "wasm"))]
fn notes_vcs_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
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

/// Two-way sync (`spares_core::sync`): the host's side for its users' devices, and a collection's
/// own sync with its upstream (`run`). A browser syncs its copy through `spares_wasm` instead.
#[cfg(not(target_family = "wasm"))]
fn sync_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    use crate::handlers::relay::relay_to_upstream;
    use crate::handlers::relay::require_protocol;
    // What the root of the collection's sync chain answers, which a host with an upstream relays
    // there (see `handlers::relay`). A device must speak the host's version of the protocol
    // itself, which is checked before relaying.
    let protocol = Router::new()
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
        .layer(middleware::from_fn(relay_to_upstream))
        .layer(middleware::from_fn(require_protocol));
    // The web app asks for these too.
    let devices = Router::new()
        .route("/api/sync/unenroll", post(sync_unenroll_handler))
        .route("/api/sync/devices", get(sync_devices_handler))
        .layer(middleware::from_fn(relay_to_upstream));
    let relayed = protocol.merge(devices);
    Router::new()
        .merge(relayed)
        .route(
            "/api/sync/upstream",
            put(crate::handlers::sync::set_upstream_handler)
                .delete(crate::handlers::sync::remove_upstream_handler),
        )
        .route("/api/sync/config", get(sync_config_handler))
        // What `spares sync cloud` copies its files to. Not relayed: the files are for this host.
        .route(
            "/api/sync/push-target",
            get(crate::handlers::sync::sync_push_target_handler),
        )
        .route(
            "/api/sync/push-config",
            put(crate::handlers::sync::sync_push_config_handler),
        )
        .route(
            "/api/sync/render",
            post(crate::handlers::sync::sync_render_handler),
        )
        .route("/api/sync/files", get(sync_files_handler))
        .route("/api/sync/files/changed", post(sync_changed_files_handler))
        .route("/api/sync/media/{kind}", get(sync_media_list_handler))
        .route(
            "/api/sync/media/{kind}/{name}",
            get(sync_media_download_handler)
                .put(sync_media_handler)
                .layer(DefaultBodyLimit::max(MEDIA_BODY_LIMIT)),
        )
        .route("/api/sync/run", post(sync_run_handler))
        .layer(middleware::from_fn(crate::handlers::relay::refuse_loops))
}

/// The routes that need the server's files, processes or role: the rest of the API besides
/// [`shared_routes`].
#[cfg(not(target_family = "wasm"))]
fn native_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(
            "/api/notes/generate_files",
            post(generate_note_files_handler),
        )
        // Live notes are files on the laptop.
        .route("/api/notes/live", post(find_live_note_handler))
        .route("/api/notes/live/list", post(list_live_notes_handler))
        .route(
            "/api/notes/live/relocate-cards",
            post(relocate_live_cards_handler),
        )
        // Both use the scheduler settings in the collection's `config.toml`, which optimizing
        // writes, so a browser's copy sends them to the server and syncs the cards they changed.
        .route(
            "/api/scheduler/{name}/optimize",
            post(optimize_scheduler_handler),
        )
        .route(
            "/api/scheduler/{name}/reschedule",
            post(reschedule_scheduler_handler),
        )
        .merge(media_routes())
        .merge(notes_vcs_routes())
        .merge(sync_routes())
}

/// Review
fn review_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
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
        .route(
            "/api/review/unmasked-image-occlusion",
            get(get_unmasked_image_occlusion_handler),
        )
}

/// The routes that only need the database (and the files of `spares_core::platform`: config,
/// parser templates, images, audio and image occlusions): what a browser can answer from its own
/// copy.
#[allow(clippy::too_many_lines, reason = "one line per route")]
fn shared_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
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
        .route("/api/notes/batch", patch(update_notes_batch_handler))
        .route("/api/notes", delete(delete_notes_handler))
        .route("/api/notes", get(list_notes_handler))
        .route("/api/notes/count", get(count_notes_handler))
        .route("/api/notes/search", post(search_notes_handler))
        .route("/api/notes/search/page", post(search_page_handler))
        .route("/api/search/fields", get(search_fields_handler))
        .route("/api/search/examples", get(search_examples_handler))
        .route("/api/search/explain", post(search_explain_handler))
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
        // Undo
        .route("/api/undo", post(undo_event_handler))
        .route("/api/redo", post(redo_event_handler))
        .route("/api/undo/group", post(group_events_handler))
        .route("/api/events", get(list_events_handler))
        .merge(shared_media_routes())
        .route(
            "/api/render-assets/packages",
            get(list_render_packages_handler),
        )
        .route(
            "/api/render-assets/sources",
            get(list_render_sources_handler),
        )
        // A client's sync status and conflicts are its own.
        .route("/api/sync/status", get(sync_status_handler))
        .route("/api/sync/conflicts", get(list_sync_conflicts_handler))
        .route(
            "/api/sync/conflicts/{id}",
            post(resolve_sync_conflict_handler),
        )
}

/// The host's router: every route, the request's collection's rendered files at `/files`, and the
/// web app from `frontend_dir`.
#[cfg(not(target_family = "wasm"))]
pub fn create_router(app_state: Arc<AppState>, frontend_dir: Option<PathBuf>) -> Router {
    use crate::handlers::auth;
    crate::version::init();
    let served_frontend = frontend_dir.clone();
    let protected = shared_routes()
        .merge(native_routes())
        .route("/api/me", get(auth::me_handler))
        .route("/api/auth/devices", get(auth::list_devices_handler))
        .route(
            "/api/auth/devices/{id}",
            delete(auth::revoke_device_handler),
        )
        .route("/api/auth/password", post(auth::password_handler))
        .route("/api/account", delete(auth::delete_account_handler))
        .route("/api/account/export", get(auth::export_account_handler))
        .route("/api/metrics", get(auth::metrics_handler))
        // Rendered notes and cards
        .nest_service("/files", tower::service_fn(serve_collection_file))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth::authenticate,
        ));
    // Before logging in
    let public = Router::new()
        .route("/api/healthcheck", get(health_check_handler))
        .route(
            "/api/version",
            get(move || crate::version::version_handler(served_frontend)),
        )
        .route("/api/auth/login", post(auth::login_handler))
        .route("/api/auth/logout", post(auth::logout_handler))
        .route("/api/auth/session", get(auth::session_handler))
        .route("/api/auth/token", post(auth::token_handler))
        .route("/api/auth/signups", get(auth::signups_handler))
        .route("/api/auth/signup", post(auth::signup_handler))
        .route("/api/auth/verify", get(auth::verify_handler))
        .route("/api/auth/reset-request", post(auth::reset_request_handler))
        .route("/api/auth/reset", post(auth::reset_handler));

    let mut router = Router::new()
        .merge(public)
        .merge(protected)
        .layer(middleware::map_response(not_stored))
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

/// A file in the files directory of the request's collection (see `spares_core::collection`).
/// `ServeDir` refuses `..` and absolute paths, so another collection's files are out of reach.
#[cfg(not(target_family = "wasm"))]
async fn serve_collection_file(
    request: axum::extract::Request,
) -> Result<axum::response::Response, std::convert::Infallible> {
    use axum::response::IntoResponse;
    use tower::ServiceExt;
    let Some(collection) = request
        .extensions()
        .get::<Arc<spares_core::collection::Collection>>()
        .cloned()
    else {
        return Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response());
    };
    let response = ServeDir::new(&collection.paths.files)
        .oneshot(request)
        .await?;
    Ok(response.into_response())
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
/// with no login, since requests never leave the browser.
///
/// A few requests act on the server's files or devices rather than on the collection. They are
/// answered with [`needs_server`], so the web app sends them to the server instead. So are
/// optimizing and rescheduling, which use (and optimizing writes) the scheduler settings in the
/// server's `config.toml`: the cards they change reach the copy at its next sync. Every other
/// request is refused ([`not_on_device`]) rather than sent on, so a server route added later never
/// changes the server's database behind the copy's back. Of those, rendering and live notes stay
/// on the server.
pub fn create_local_router(collection: Arc<spares_core::collection::Collection>) -> Router {
    Router::<()>::new()
        .route("/api/healthcheck", get(health_check_handler))
        .merge(shared_routes())
        .route("/api/images/url", post(needs_server))
        .route("/api/render-assets", get(needs_server))
        .route("/api/notes/share/summary", post(needs_server))
        .route("/api/notes/share/bundle", post(needs_server))
        .route("/api/notes/share/import", post(needs_server))
        .route("/api/notes/import", post(needs_server))
        .route("/api/notes/vcs/status", get(needs_server))
        .route("/api/notes/vcs/saved-copies/remove", post(needs_server))
        .route("/api/sync/devices", get(needs_server))
        .route("/api/sync/unenroll", post(needs_server))
        .route("/api/scheduler/{name}/optimize", post(needs_server))
        .route("/api/scheduler/{name}/reschedule", post(needs_server))
        .fallback(not_on_device)
        // Also when a route has the path for another method: `POST /api/notes/live` matches
        // `GET /api/notes/{id}`.
        .method_not_allowed_fallback(not_on_device)
        .layer(middleware::from_fn(in_collection))
        .layer(axum::Extension(collection))
}

/// Runs the request in the collection the router's `Extension` gives it.
async fn in_collection(
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let collection = request
        .extensions()
        .get::<Arc<spares_core::collection::Collection>>()
        .cloned();
    match collection {
        Some(collection) => collection.scope(next.run(request)).await,
        None => next.run(request).await,
    }
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

    /// A browser's router answers from its database and files, sends requests for the server's
    /// files and devices to the server, and refuses the rest.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn the_local_router_answers_what_it_can(pool: SqlitePool) {
        let root = std::env::temp_dir().join(format!("spares-local-router-{}", std::process::id()));
        let collection = Arc::new(spares_core::collection::Collection::new(
            "device",
            pool,
            spares_core::collection::CollectionPaths::in_dir(&root),
        ));
        let router = create_local_router(collection);
        // No login: requests never leave the browser.
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
        // Explaining a query reads nothing, so the copy answers it
        let (status, explained) = call_json(
            &router,
            "POST",
            "/api/search/explain",
            serde_json::json!({ "query": "tag=a", "item": "Notes" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{explained}");
        assert!(
            explained["explanation"]
                .as_str()
                .unwrap()
                .contains("SELECT"),
            "{explained}"
        );
        // Images, audio and image occlusions are stored on the device (here, invalid requests)
        for (method, uri) in [
            ("POST", "/api/images"),
            ("POST", "/api/audio"),
            ("POST", "/api/image-occlusions"),
            ("GET", "/api/image-occlusions/file?path=/a.svg"),
            ("GET", "/api/notes/1/image-occlusions"),
            ("PUT", "/api/notes/1/image-occlusions/0"),
        ] {
            let status = call(&router, method, uri).await.0;
            assert!(
                status.is_client_error() || status.is_server_error(),
                "{method} {uri}: {status}"
            );
            assert!(
                ![StatusCode::SERVICE_UNAVAILABLE, StatusCode::NOT_IMPLEMENTED].contains(&status),
                "{method} {uri}: {status}"
            );
        }
        for (method, uri) in [
            ("POST", "/api/images/url"),
            ("GET", "/api/render-assets?path=/a.typ"),
            ("POST", "/api/notes/share/summary"),
            ("POST", "/api/notes/share/bundle"),
            ("POST", "/api/notes/share/import?name=a.zip"),
            ("POST", "/api/notes/import"),
            ("GET", "/api/notes/vcs/status"),
            ("POST", "/api/notes/vcs/saved-copies/remove"),
            ("GET", "/api/sync/devices"),
            ("POST", "/api/sync/unenroll"),
            ("POST", "/api/scheduler/fsrs/optimize"),
            ("POST", "/api/scheduler/fsrs/reschedule"),
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
            ("POST", "/api/notes/generate_files"),
            ("POST", "/api/notes/live"),
            ("POST", "/api/notes/live/list"),
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

    /// A card's image occlusion without masks is revalidated by its hash, and found only by its
    /// note and card.
    #[sqlx::test(migrations = "../spares_core/migrations")]
    async fn unmasked_image_occlusions_are_revalidated(pool: SqlitePool) {
        use axum::http::header::ETAG;
        use axum::http::header::IF_NONE_MATCH;

        let root = std::env::temp_dir().join(format!("spares-unmask-route-{}", std::process::id()));
        let collection = Arc::new(spares_core::collection::Collection::new(
            "device",
            pool,
            spares_core::collection::CollectionPaths::in_dir(&root),
        ));
        collection.set_replica(Some(spares_core::config::Replica::ROOT));
        let router = create_local_router(collection.clone());
        let (status, parser) = call_json(
            &router,
            "POST",
            "/api/parsers",
            serde_json::json!({ "name": "markdown" }),
        )
        .await;
        let parser_id = parser["id"]
            .as_i64()
            .unwrap_or_else(|| panic!("{status} {parser}"));
        // A transparent 1×1 PNG
        let image: Vec<u8> = [
            "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489",
            "0000000d49444154789c63000100000500010d0a2db40000000049454e44ae426082",
        ]
        .concat()
        .as_bytes()
        .chunks(2)
        .map(|hex| u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap())
        .collect();
        let clozes = r#"<svg width="1" height="1" xmlns="http://www.w3.org/2000/svg"><g class="layer" id="markup-group"><title>Markup</title></g><g class="layer" id="clozes-group"><title>Clozes</title><rect x="0" y="0" width="1" height="1"/><rect x="0" y="0" width="1" height="1"/></g></svg>"#;
        let db = collection.db.clone();
        let snippet = collection
            .scope(spares_core::api::image_occlusion::create_image_occlusion(
                &db,
                parser_id,
                &image,
                "route.png",
                clozes,
                &spares_core::parsers::get_all_parsers(),
            ))
            .await
            .unwrap()
            .snippet;
        let (status, created) = call_json(
            &router,
            "POST",
            "/api/notes",
            serde_json::json!({
                "parser_id": parser_id,
                "requests": [{ "data": snippet, "keywords": [], "tags": [], "is_suspended": false, "custom_data": {} }],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let note_id = created["notes"][0]["id"].as_i64().unwrap();
        let uri = |order: usize, part: &str| {
            format!(
                "/api/review/unmasked-image-occlusion?note_id={note_id}&card_order=1&order={order}&part={part}"
            )
        };
        let get = |uri: String, etag: Option<String>| {
            let router = router.clone();
            async move {
                let mut request = Request::builder().uri(uri);
                if let Some(etag) = etag {
                    request = request.header(IF_NONE_MATCH, etag);
                }
                let response = router
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let status = response.status();
                let headers = response.headers().clone();
                let body = response.into_body().collect().await.unwrap().to_bytes();
                (status, headers, body)
            }
        };

        let (status, headers, body) = get(uri(1, "image"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_ref(), image.as_slice());
        assert_eq!(headers[axum::http::header::CONTENT_TYPE], "image/png");
        assert_eq!(
            headers[axum::http::header::CACHE_CONTROL],
            "private, no-cache"
        );
        let etag = headers[ETAG].to_str().unwrap().to_string();
        let (status, _, body) = get(uri(1, "image"), Some(etag)).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert!(body.is_empty());
        let (status, _, _) = get(uri(1, "image"), Some("\"other\"".to_string())).await;
        assert_eq!(status, StatusCode::OK);
        // No markup
        assert_eq!(get(uri(1, "markup"), None).await.0, StatusCode::NO_CONTENT);
        assert_eq!(get(uri(2, "image"), None).await.0, StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Browsers don't keep API responses, which would answer later requests with old data.
    #[tokio::test]
    async fn api_responses_are_not_stored() {
        let (host, _root) = crate::host::tests::test_host().await;
        let router = create_router(Arc::new(AppState { host }), None);
        let request = Request::builder()
            .uri("/api/parsers")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
    }

    async fn send(
        router: &Router,
        request: axum::http::request::Builder,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
        let request = match body {
            Some(body) => request
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            headers,
            serde_json::from_slice(&body).unwrap_or_default(),
        )
    }

    /// `spares sync cloud` learns where to copy files with its token, and finds the check value
    /// the answer names in the file it names. The web app's session can't ask.
    #[tokio::test]
    #[allow(clippy::too_many_lines, reason = "one scenario")]
    async fn the_push_target_and_config_are_for_device_tokens() {
        use axum::http::header::AUTHORIZATION;
        use axum::http::header::COOKIE;
        use axum::http::header::SET_COOKIE;

        let (host, root) = crate::host::tests::test_host().await;
        let router = create_router(Arc::new(AppState { host }), None);
        let get = |uri: &str| Request::builder().uri(uri.to_string());
        let post = |uri: &str| Request::builder().method("POST").uri(uri.to_string());
        let credentials = serde_json::json!({ "user": "me", "password": "password1" });

        let (_, headers, _) =
            send(&router, post("/api/auth/login"), Some(credentials.clone())).await;
        let cookie = headers[SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let (status, _, _) = send(
            &router,
            get("/api/sync/push-target").header(COOKIE, &cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let mut request = credentials;
        request["name"] = "a CLI".into();
        let (_, _, body) = send(&router, post("/api/auth/token"), Some(request)).await;
        let token = format!("Bearer {}", body["token"].as_str().unwrap());
        let mut checks = Vec::new();
        for _ in 0..2 {
            let (status, _, target) = send(
                &router,
                get("/api/sync/push-target").header(AUTHORIZATION, &token),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{target}");
            let target: spares_core::sync::push::PushTarget =
                serde_json::from_value(target).unwrap();
            assert_eq!(target.root, root.0.join("users/me"));
            assert_eq!(target.config, root.0.join("users/me/config"));
            assert_eq!(target.files, root.0.join("cache/users/me/files"));
            assert_eq!(target.replica, 1);
            assert_eq!(
                std::fs::read_to_string(&target.check_file).unwrap(),
                target.check
            );
            checks.push(target.check);
        }
        assert_ne!(checks[0], checks[1]);

        // The pushed config.toml replaces the server's but for its [sync] table.
        let config_file = root.0.join("users/me/config/config.toml");
        std::fs::write(&config_file, "[sync]\nreplica = 1\n").unwrap();
        let laptop = "maximum_interval = 100\n\n[sync]\ntoken = \"spt_secret\"\nreplica = 0\n";
        let put_config = |auth: (axum::http::HeaderName, String), body: &str| {
            Request::builder()
                .method("PUT")
                .uri("/api/sync/push-config")
                .header(auth.0, auth.1)
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let response = router
            .clone()
            .oneshot(put_config((COOKIE, cookie.clone()), laptop))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        for changed in [true, false] {
            let response = router
                .clone()
                .oneshot(put_config((AUTHORIZATION, token.clone()), laptop))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["changed"], changed);
        }
        let written = std::fs::read_to_string(&config_file).unwrap();
        assert!(written.contains("maximum_interval = 100"), "{written}");
        assert!(
            written.contains("render_browser_parsers = false"),
            "{written}"
        );
        assert!(written.contains("replica = 1"), "{written}");
        assert!(!written.contains("spt_secret"), "{written}");
        let response = router
            .clone()
            .oneshot(put_config(
                (AUTHORIZATION, token.clone()),
                "maximum_interval = \"soon\"",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(std::fs::read_to_string(&config_file).unwrap(), written);

        // Rendering again after the push starts in the background.
        let (status, _, _) = send(
            &router,
            post("/api/sync/render").header(COOKIE, &cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = send(
            &router,
            post("/api/sync/render").header(AUTHORIZATION, &token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    /// Sessions need their CSRF token to change anything, tokens can be revoked and limited to
    /// sync, and each user only ever reaches their own collection.
    #[tokio::test]
    #[allow(clippy::too_many_lines, reason = "one scenario")]
    async fn requests_reach_only_their_users_collection() {
        use axum::http::header::AUTHORIZATION;
        use axum::http::header::COOKIE;
        use axum::http::header::SET_COOKIE;

        let (host, root) = crate::host::tests::test_host().await;
        host.create_user(crate::host::accounts::NewUser {
            id: Some("other"),
            name: "other",
            email: None,
            password: "password2",
            admin: false,
            email_verified: false,
        })
        .await
        .unwrap();
        std::fs::write(root.0.join("cache/users/me/files/mine.pdf"), "mine").unwrap();
        std::fs::write(root.0.join("cache/users/other/files/theirs.pdf"), "theirs").unwrap();
        let router = create_router(Arc::new(AppState { host }), None);
        let get = |uri: &str| Request::builder().uri(uri.to_string());
        let post = |uri: &str| Request::builder().method("POST").uri(uri.to_string());

        // Wrong passwords
        let (status, _, _) = send(
            &router,
            post("/api/auth/login"),
            Some(serde_json::json!({ "user": "me", "password": "password2" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // A session
        let (status, headers, body) = send(
            &router,
            post("/api/auth/login"),
            Some(serde_json::json!({ "user": "me", "password": "password1" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let set_cookie = headers[SET_COOKIE].to_str().unwrap().to_string();
        assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
        let cookie = set_cookie.split(';').next().unwrap().to_string();
        let csrf = body["csrf_token"].as_str().unwrap().to_string();
        let (status, _, me) = send(&router, get("/api/me").header(COOKIE, &cookie), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["user"]["id"], "me");
        let (status, _, _) = send(
            &router,
            get("/files/mine.pdf").header(COOKIE, &cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        for path in ["/files/theirs.pdf", "/files/../../other/files/theirs.pdf"] {
            let (status, _, _) = send(&router, get(path).header(COOKIE, &cookie), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
        // Changing something needs the CSRF token.
        let tag = serde_json::json!({ "name": "mine", "description": "", "parent_id": null,
            "query": null, "auto_delete": false });
        let (status, _, _) = send(
            &router,
            post("/api/tags").header(COOKIE, &cookie),
            Some(tag.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, body) = send(
            &router,
            post("/api/tags")
                .header(COOKIE, &cookie)
                .header("x-csrf-token", &csrf),
            Some(tag),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // A device token, for the other user
        let (status, _, body) = send(
            &router,
            post("/api/auth/token"),
            Some(
                serde_json::json!({ "user": "other", "password": "password2",
                "name": "a CLI" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let token = format!("Bearer {}", body["token"].as_str().unwrap());
        let device_id = body["device"]["id"].as_i64().unwrap();
        let (status, _, tags) = send(
            &router,
            get("/api/tags").header(AUTHORIZATION, &token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            tags,
            serde_json::json!([]),
            "the other user sees no tag of mine"
        );
        let (_, _, tags) = send(&router, get("/api/tags").header(COOKIE, &cookie), None).await;
        assert_eq!(tags.as_array().unwrap().len(), 1);
        let (status, _, _) = send(
            &router,
            get("/files/theirs.pdf").header(AUTHORIZATION, &token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Revoked
        let (status, _, _) = send(
            &router,
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/auth/devices/{device_id}"))
                .header(AUTHORIZATION, &token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = send(
            &router,
            get("/api/tags").header(AUTHORIZATION, &token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // A sync token only syncs.
        let (_, _, body) = send(
            &router,
            post("/api/auth/token"),
            Some(
                serde_json::json!({ "user": "other", "password": "password2",
                "name": "a host", "scope": "sync" }),
            ),
        )
        .await;
        let token = format!("Bearer {}", body["token"].as_str().unwrap());
        let (status, _, _) = send(
            &router,
            get("/api/tags").header(AUTHORIZATION, &token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, body) = send(
            &router,
            get("/api/sync/meta").header(AUTHORIZATION, &token),
            None,
        )
        .await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
        assert_ne!(status, StatusCode::UNAUTHORIZED, "{body}");

        // Logging out ends the session.
        let (status, _, _) = send(
            &router,
            post("/api/auth/logout").header(COOKIE, &cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = send(&router, get("/api/me").header(COOKIE, &cookie), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// A user's limits refuse requests past them: too many a minute, too large a body, past the
    /// storage quota, and too many failed logins.
    #[tokio::test]
    async fn limits_refuse_requests_past_them() {
        use axum::http::header::AUTHORIZATION;
        use axum::http::header::CONTENT_LENGTH;

        use crate::host::limits::LimitOverrides;

        let (host, _root) = crate::host::tests::test_host().await;
        crate::host::limits::set_overrides(
            &host.accounts,
            "me",
            &LimitOverrides {
                requests_per_minute: Some(5),
                media_body_mb: Some(1),
                storage_mb: Some(1),
                ..LimitOverrides::default()
            },
        )
        .await
        .unwrap();
        crate::host::limits::record_storage(&host.accounts, "me", 900 * 1024)
            .await
            .unwrap();
        let (_, token) = crate::host::auth::create_token(
            &host.accounts,
            "me",
            "test",
            crate::host::auth::Scope::All,
        )
        .await
        .unwrap();
        let bearer = format!("Bearer {token}");
        let router = create_router(Arc::new(AppState { host }), None);
        let put = |length: usize| {
            Request::builder()
                .method("PUT")
                .uri("/api/sync/media/image/a-0123456789ab.png")
                .header(AUTHORIZATION, &bearer)
                .header(CONTENT_LENGTH, length.to_string())
        };
        let (status, _, _) = send(&router, put(2 * 1024 * 1024), None).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        let (status, _, _) = send(&router, put(200 * 1024), None).await;
        assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
        let get = || {
            Request::builder()
                .uri("/api/tags")
                .header(AUTHORIZATION, &bearer)
        };
        for _ in 0..3 {
            assert_eq!(send(&router, get(), None).await.0, StatusCode::OK);
        }
        assert_eq!(
            send(&router, get(), None).await.0,
            StatusCode::TOO_MANY_REQUESTS
        );

        for _ in 0..10 {
            let (status, _, _) = send(
                &router,
                Request::builder().method("POST").uri("/api/auth/login"),
                Some(serde_json::json!({ "user": "me", "password": "wrong password" })),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        // Even the right password, for a while
        let (status, _, _) = send(
            &router,
            Request::builder().method("POST").uri("/api/auth/login"),
            Some(serde_json::json!({ "user": "me", "password": "password1" })),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }

    /// Invite signups need a code, used once; open ones an email address, verified by the link
    /// sent to it before the account can log in.
    #[tokio::test]
    #[allow(clippy::too_many_lines, reason = "one scenario")]
    async fn signups_follow_the_hosts_policy() {
        let (host, root) = crate::host::tests::test_host().await;
        let post = |uri: &str| Request::builder().method("POST").uri(uri.to_string());
        let signup = |name: &str, invite: Option<&str>| {
            serde_json::json!({ "name": name, "email": format!("{name}@example.com"),
                "password": "password9", "invite": invite })
        };
        let router = create_router(
            Arc::new(AppState {
                host: Arc::clone(&host),
            }),
            None,
        );
        let (status, _, _) = send(&router, post("/api/auth/signup"), Some(signup("a", None))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "closed");
        drop(router);
        host.close_all().await;

        // By invite
        std::fs::write(root.0.join("host.toml"), "signups = \"invite\"\n").unwrap();
        let host = crate::host::Host::open(&root.0).await.unwrap();
        let code = crate::host::signup::create_invite(&host.accounts, 1)
            .await
            .unwrap();
        let router = create_router(
            Arc::new(AppState {
                host: Arc::clone(&host),
            }),
            None,
        );
        let (status, _, _) = send(
            &router,
            post("/api/auth/signup"),
            Some(signup("b", Some("wrong"))),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, headers, body) = send(
            &router,
            post("/api/auth/signup"),
            Some(signup("b", Some(&code))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(headers.contains_key(axum::http::header::SET_COOKIE));
        let new_id = body["user"]["id"].as_str().unwrap().to_string();
        assert!(
            root.0
                .join("users")
                .join(&new_id)
                .join("collection.sqlite")
                .is_file()
        );
        let (status, _, _) = send(
            &router,
            post("/api/auth/signup"),
            Some(signup("c", Some(&code))),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "used up");
        drop(router);
        host.close_all().await;

        // Open, with email
        let mail = root.0.join("mail.txt");
        std::fs::write(
            root.0.join("host.toml"),
            format!(
                "signups = \"open\"\n[email]\ncommand = \"cat > {}\"\nfrom = \"s@example.com\"\n",
                mail.display()
            ),
        )
        .unwrap();
        let host = crate::host::Host::open(&root.0).await.unwrap();
        let router = create_router(
            Arc::new(AppState {
                host: Arc::clone(&host),
            }),
            None,
        );
        let (status, _, body) =
            send(&router, post("/api/auth/signup"), Some(signup("d", None))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["verify"], true);
        let login = serde_json::json!({ "user": "d", "password": "password9" });
        let (status, _, _) = send(&router, post("/api/auth/login"), Some(login.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "not verified yet");
        let sent = std::fs::read_to_string(&mail).unwrap();
        let link = sent
            .lines()
            .find(|line| line.contains("/api/auth/verify?token="))
            .unwrap();
        let path = &link[link.find("/api/").unwrap()..];
        let (status, headers, _) = send(&router, Request::builder().uri(path), None).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(headers[axum::http::header::LOCATION], "/login?verified=1");
        let (status, _, _) = send(&router, post("/api/auth/login"), Some(login)).await;
        assert_eq!(status, StatusCode::OK);

        // A forgotten password
        let (status, _, _) = send(
            &router,
            post("/api/auth/reset-request"),
            Some(serde_json::json!({ "email": "d@example.com" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sent = std::fs::read_to_string(&mail).unwrap();
        let link = sent
            .lines()
            .find(|line| line.contains("reset-password?token="))
            .unwrap();
        let token = &link[link.find("token=").unwrap() + "token=".len()..];
        let (status, _, _) = send(
            &router,
            post("/api/auth/reset"),
            Some(serde_json::json!({ "token": token, "password": "new password" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = send(
            &router,
            post("/api/auth/login"),
            Some(serde_json::json!({ "user": "d", "password": "new password" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        drop(router);
        host.close_all().await;
    }
}
