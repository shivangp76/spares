use crate::{
    AppState,
    handlers::{
        card::{get_card_handler, get_cards_handler, get_leeches_handler, update_card_handler},
        health_check_handler,
        note::{
            create_notes_handler, delete_note_handler, generate_note_files_handler,
            get_note_handler, list_notes_handler, search_keyword_handler, search_notes_handler,
            update_notes_handler,
        },
        parser::{
            create_parser_handler, delete_parser_handler, get_parser_handler, list_parsers_handler,
            update_parser_handler,
        },
        review::{get_review_card_handler, get_statistics_handler, submit_study_action_handler},
        scheduler::get_scheduler_ratings_handler,
        tag::{
            create_tag_handler, delete_tag_handler, get_tag_by_name_handler, get_tag_handler,
            list_tags_handler, rebuild_tag_handler, update_tag_handler,
        },
    },
};
use axum::{
    Router,
    routing::{delete, get, patch, post},
};
use std::sync::Arc;

pub fn create_router(app_state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/healthcheck", get(health_check_handler))
        // Parser
        .route("/api/parsers", post(create_parser_handler))
        .route("/api/parsers/:id", get(get_parser_handler))
        .route("/api/parsers/:id", patch(update_parser_handler))
        .route("/api/parsers/:id", delete(delete_parser_handler))
        .route("/api/parsers", get(list_parsers_handler))
        // Tag
        .route("/api/tags", post(create_tag_handler))
        .route("/api/tags/:id", get(get_tag_handler))
        .route("/api/tags/name/:id", get(get_tag_by_name_handler))
        .route("/api/tags/:id", patch(update_tag_handler))
        .route("/api/tags/:id", delete(delete_tag_handler))
        .route("/api/tags", get(list_tags_handler))
        .route("/api/tags/:id/rebuild", get(rebuild_tag_handler))
        // Note
        .route("/api/notes", post(create_notes_handler))
        .route("/api/notes/:id", get(get_note_handler))
        .route("/api/notes", patch(update_notes_handler)) // the request body contains note_ids: Vec<i64>
        .route("/api/notes/:id", delete(delete_note_handler))
        .route("/api/notes", get(list_notes_handler))
        .route(
            "/api/notes/generate_files",
            post(generate_note_files_handler),
        )
        .route("/api/notes/search", post(search_notes_handler))
        .route("/api/notes/search/keyword", post(search_keyword_handler))
        // Card
        .route("/api/cards/:id", get(get_card_handler))
        .route("/api/cards/note_id/:id", get(get_cards_handler))
        .route("/api/cards/leeches", post(get_leeches_handler))
        .route("/api/cards", patch(update_card_handler))
        // Review
        .route("/api/review", post(get_review_card_handler))
        .route("/api/review/submit", post(submit_study_action_handler))
        .route("/api/review/statistics", post(get_statistics_handler))
        // Scheduler
        .route(
            "/api/scheduler/:name/ratings",
            get(get_scheduler_ratings_handler),
        )
        .with_state(app_state)
}
