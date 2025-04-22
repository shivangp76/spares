const BULK_REQUEST_THRESHOLD: usize = 25;
const MAX_CARDS_SINGLE_INSERTION: usize = 25;
const AUTOMATIC_REBUILD: bool = false;

mod basic;
mod create;
mod render;
mod search;
mod update;
pub use basic::*;
pub use create::*;
pub use render::*;
pub use search::*;
pub use update::*;

#[cfg(test)]
pub(crate) mod tests {
    use crate::api::note::{create_notes, update_notes};
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::tag::create_tag;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::{
        CreateNoteRequest, CreateNotesRequest, NotesSelector, UpdateNotesRequest,
    };
    use crate::schema::tag::CreateTagRequest;
    use chrono::Utc;
    use serde_json::Map;
    use sqlx::SqlitePool;

    pub use super::basic::*;
    use crate::model::Tag;

    #[sqlx::test]
    async fn test_create_note_filtered_tag_error(pool: SqlitePool) -> () {
        // Create a filtered tag
        let request = CreateTagRequest {
            name: "test filtered tag".to_string(),
            description: String::new(),
            parent_id: None,
            query: Some("tag=math".to_string()),
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_ok());

        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note with a tag
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data 1".to_string(),
            keywords: vec![],
            tags: vec!["test filtered tag".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_err());
    }

    #[sqlx::test]
    async fn test_update_note_add_filtered_tag_error(pool: SqlitePool) -> () {
        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data 1".to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Create a filtered tag
        let request = CreateTagRequest {
            name: "test filtered tag".to_string(),
            description: String::new(),
            parent_id: None,
            query: Some("tag=math".to_string()),
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_ok());

        // Update note
        let note_id = create_notes_response.notes[0].id;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags_to_add: Some(vec!["test filtered tag".to_string()]),
            tags_to_remove: None,
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(notes_res.is_err());
    }

    #[sqlx::test]
    async fn test_update_note_remove_filtered_tag_error(pool: SqlitePool) -> () {
        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data 1".to_string(),
            keywords: vec![],
            tags: vec!["math".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Create a filtered tag
        let request = CreateTagRequest {
            name: "test filtered tag".to_string(),
            description: String::new(),
            parent_id: None,
            query: Some("tag=math".to_string()),
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_ok());

        // Update note
        let note_id = create_notes_response.notes[0].id;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags_to_add: None,
            tags_to_remove: Some(vec!["test filtered tag".to_string()]),
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(notes_res.is_err());
    }

    #[sqlx::test]
    async fn test_delete_note_unused_tags(pool: SqlitePool) -> () {
        // Create a tag
        let request = CreateTagRequest {
            name: "math".to_string(),
            description: String::new(),
            parent_id: None,
            query: None,
            auto_delete: true,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_ok());

        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data 1".to_string(),
            keywords: vec![],
            tags: vec!["math".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Delete note
        let delete_note_res =
            delete_note(&pool, create_notes_response.notes[0].id, &get_all_parsers()).await;
        assert!(delete_note_res.is_ok());

        // Verify that tag is deleted
        let tags: Vec<Tag> = sqlx::query_as(r"SELECT * FROM tag")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(tags.len(), 0);
    }

    #[sqlx::test]
    async fn test_update_note_unused_tag(pool: SqlitePool) -> () {
        // Create a tag
        let request = CreateTagRequest {
            name: "math".to_string(),
            description: String::new(),
            parent_id: None,
            query: None,
            auto_delete: true,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_ok());

        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data 1".to_string(),
            keywords: vec![],
            tags: vec!["math".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Update note
        let note_id = create_notes_response.notes[0].id;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags_to_add: None,
            tags_to_remove: Some(vec!["math".to_string()]),
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(notes_res.is_ok());

        // Verify that tag is deleted
        let tags: Vec<Tag> = sqlx::query_as(r"SELECT * FROM tag")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(tags.len(), 0);
    }
}
