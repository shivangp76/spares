use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::delete;
use axum::routing::get;
use axum::routing::patch;
use axum::routing::post;
use axum::routing::put;
use tower_http::services::ServeDir;
use tower_http::services::ServeFile;

use crate::AppState;
use crate::handlers::audio::AUDIO_BODY_LIMIT;
use crate::handlers::audio::create_audio_handler;
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
use crate::handlers::image::create_image_from_url_handler;
use crate::handlers::image::create_image_handler;
use crate::handlers::image_occlusion::IMAGE_OCCLUSION_BODY_LIMIT;
use crate::handlers::image_occlusion::create_image_occlusion_handler;
use crate::handlers::image_occlusion::get_image_occlusion_editor_config_handler;
use crate::handlers::image_occlusion::get_image_occlusion_file_handler;
use crate::handlers::image_occlusion::list_note_image_occlusions_handler;
use crate::handlers::image_occlusion::update_note_image_occlusion_handler;
use crate::handlers::note::create_notes_handler;
use crate::handlers::note::delete_notes_handler;
use crate::handlers::note::export_notes_handler;
use crate::handlers::note::find_live_note_handler;
use crate::handlers::note::generate_note_files_handler;
use crate::handlers::note::get_duplicate_keywords_handler;
use crate::handlers::note::get_keywords_handler;
use crate::handlers::note::get_note_graph_handler;
use crate::handlers::note::get_note_handler;
use crate::handlers::note::get_note_links_handler;
use crate::handlers::note::get_note_render_handler;
use crate::handlers::note::get_unmatched_keywords_handler;
use crate::handlers::note::list_notes_handler;
use crate::handlers::note::relocate_live_cards_handler;
use crate::handlers::note::search_examples_handler;
use crate::handlers::note::search_fields_handler;
use crate::handlers::note::search_keyword_handler;
use crate::handlers::note::search_notes_handler;
use crate::handlers::note::search_page_handler;
use crate::handlers::note::update_notes_handler;
use crate::handlers::notes_vcs::notes_vcs_anki_finish_handler;
use crate::handlers::notes_vcs::notes_vcs_anki_link_handler;
use crate::handlers::notes_vcs::notes_vcs_anki_receive_handler;
use crate::handlers::notes_vcs::notes_vcs_anki_refresh_handler;
use crate::handlers::notes_vcs::notes_vcs_check_handler;
use crate::handlers::notes_vcs::notes_vcs_finish_handler;
use crate::handlers::notes_vcs::notes_vcs_init_handler;
use crate::handlers::notes_vcs::notes_vcs_receive_handler;
use crate::handlers::notes_vcs::notes_vcs_remove_saved_copies_handler;
use crate::handlers::notes_vcs::notes_vcs_status_handler;
use crate::handlers::parser::create_parser_handler;
use crate::handlers::parser::delete_parser_handler;
use crate::handlers::parser::get_parser_handler;
use crate::handlers::parser::get_parser_syntax_handler;
use crate::handlers::parser::list_known_parsers_handler;
use crate::handlers::parser::list_parsers_handler;
use crate::handlers::parser::update_parser_handler;
use crate::handlers::render_asset::get_render_asset_handler;
use crate::handlers::render_asset::list_render_packages_handler;
use crate::handlers::render_asset::list_render_sources_handler;
use crate::handlers::require_api_key;
use crate::handlers::review::create_review_snapshot_handler;
use crate::handlers::review::get_review_card_by_id_handler;
use crate::handlers::review::get_review_card_handler;
use crate::handlers::review::get_review_config_handler;
use crate::handlers::review::get_review_history_handler;
use crate::handlers::review::get_review_queue_handler;
use crate::handlers::review::get_statistics_graphs_handler;
use crate::handlers::review::get_statistics_handler;
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
use crate::handlers::scheduler::optimize_scheduler_handler;
use crate::handlers::scheduler::reschedule_scheduler_handler;
use crate::handlers::sync::SYNC_BODY_LIMIT;
use crate::handlers::sync::list_sync_conflicts_handler;
use crate::handlers::sync::resolve_sync_conflict_handler;
use crate::handlers::sync::sync_full_download_handler;
use crate::handlers::sync::sync_full_upload_handler;
use crate::handlers::sync::sync_meta_handler;
use crate::handlers::sync::sync_pull_handler;
use crate::handlers::sync::sync_push_handler;
use crate::handlers::sync::sync_run_handler;
use crate::handlers::sync::sync_status_handler;
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

fn image_occlusion_routes() -> Router<Arc<AppState>> {
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
            "/api/image-occlusions/editor-config",
            get(get_image_occlusion_editor_config_handler),
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
}

/// Notes repository (`spares note init-vcs`, its push hooks, and the web UI's notice of saved
/// copies)
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

/// Two-way sync (`spares_core::sync`)
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
        .route("/api/sync/run", post(sync_run_handler))
        .route("/api/sync/status", get(sync_status_handler))
        .route("/api/sync/conflicts", get(list_sync_conflicts_handler))
        .route(
            "/api/sync/conflicts/{id}",
            post(resolve_sync_conflict_handler),
        )
}

/// Files read by sources that clients compile themselves
fn render_asset_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/render-assets", get(get_render_asset_handler))
        .route(
            "/api/render-assets/packages",
            get(list_render_packages_handler),
        )
        .route(
            "/api/render-assets/sources",
            get(list_render_sources_handler),
        )
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
        .route("/api/review/config", get(get_review_config_handler))
}

/// Offline review on web clients
fn offline_review_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/review/queue", post(get_review_queue_handler))
        .route("/api/review/sync", post(sync_offline_actions_handler))
}

#[allow(clippy::too_many_lines, reason = "one line per route")]
pub(crate) fn create_router(
    app_state: Arc<AppState>,
    files_dir: PathBuf,
    frontend_dir: Option<PathBuf>,
) -> Router {
    let protected = Router::new()
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
        .route(
            "/api/notes/generate_files",
            post(generate_note_files_handler),
        )
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
        .route("/api/notes/export", post(export_notes_handler))
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
        .route("/api/notes/live", post(find_live_note_handler))
        .route(
            "/api/notes/live/relocate-cards",
            post(relocate_live_cards_handler),
        )
        .route(
            "/api/notes/latest-event-id",
            get(get_latest_note_event_id_handler),
        )
        // Card
        .route("/api/cards", get(list_cards_handler))
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
            "/api/scheduler/{name}/optimize",
            post(optimize_scheduler_handler),
        )
        .route(
            "/api/scheduler/{name}/reschedule",
            post(reschedule_scheduler_handler),
        )
        // Undo
        .route("/api/undo", post(undo_event_handler))
        .route("/api/redo", post(redo_event_handler))
        .route("/api/undo/group", post(group_events_handler))
        .merge(image_occlusion_routes())
        .merge(render_asset_routes())
        .merge(offline_review_routes())
        .merge(notes_vcs_routes())
        .merge(sync_routes())
        // Rendered notes and cards
        .nest_service("/files", ServeDir::new(files_dir))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            require_api_key,
        ));

    let mut router = Router::new()
        .route("/api/healthcheck", get(health_check_handler))
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
