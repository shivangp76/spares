mod card;
mod note;
mod parser;
mod tag;

use chrono::Utc;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::api::note::create_notes;
use crate::api::parser::tests::create_parser_helper;
use crate::model::CardId;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;

/// Creates a single note with one cloze card and returns the card id.
pub(super) async fn create_card_helper(pool: &SqlitePool) -> CardId {
    let parser = create_parser_helper(pool, "markdown").await;
    let request = CreateNotesRequest {
        parser_id: parser.id,
        requests: vec![CreateNoteRequest {
            data: "Hello {{ world }}".to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        }],
    };
    let result = create_notes(pool, request, Utc::now(), &get_all_parsers(), false)
        .await
        .unwrap();
    let note_id = result.notes[0].id;
    let card: crate::model::Card = sqlx::query_as("SELECT * FROM card WHERE note_id = ? LIMIT 1")
        .bind(note_id)
        .fetch_one(pool)
        .await
        .unwrap();
    card.id
}
