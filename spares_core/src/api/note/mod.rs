const BULK_REQUEST_THRESHOLD: usize = 25;
const AUTOMATIC_REBUILD: bool = false;

mod basic;
mod create;
mod delete;
pub mod export;
mod keyword_distance;
mod render;
mod search;
mod update;
pub use basic::*;
pub use create::*;
pub use delete::*;
pub use render::*;
pub use search::*;
pub use update::*;

#[cfg(test)]
pub(crate) mod tests {
    use chrono::Utc;
    use serde_json::Map;
    use sqlx::SqlitePool;

    pub use super::basic::*;
    use crate::api::note::create_notes;
    use crate::api::note::delete_notes;
    use crate::api::note::update_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::tag::create_tag;
    use crate::model::Tag;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::note::DeleteNotesRequest;
    use crate::schema::note::NotesSelector;
    use crate::schema::note::UpdateNotesRequest;
    use crate::schema::note::UpdateTags;
    use crate::schema::tag::CreateTagRequest;

    #[sqlx::test]
    async fn test_create_note_filtered_tag_error(pool: SqlitePool) -> () {
        // Create a filtered tag
        let request = CreateTagRequest {
            name: "test filtered tag".to_string(),
            description: String::new(),
            query: Some("tag=math".to_string()),
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request, false).await;
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
        let create_notes_res =
            create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
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
        let create_notes_res =
            create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Create a filtered tag
        let request = CreateTagRequest {
            name: "test filtered tag".to_string(),
            description: String::new(),
            query: Some("tag=math".to_string()),
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request, false).await;
        assert!(tag_res.is_ok());

        // Update note
        let note_id = create_notes_response.notes[0].id;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_remove: None,
                tags_to_add: Some(vec!["test filtered tag".to_string()]),
            },
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
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
        let create_notes_res =
            create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Create a filtered tag
        let request = CreateTagRequest {
            name: "test filtered tag".to_string(),
            description: String::new(),
            query: Some("tag=math".to_string()),
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request, false).await;
        assert!(tag_res.is_ok());

        // Update note
        let note_id = create_notes_response.notes[0].id;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_add: None,
                tags_to_remove: Some(vec!["test filtered tag".to_string()]),
            },
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(notes_res.is_err());
    }

    #[sqlx::test]
    async fn test_delete_note_unused_tags(pool: SqlitePool) -> () {
        // Create a tag
        let request = CreateTagRequest {
            name: "math".to_string(),
            description: String::new(),
            query: None,
            auto_delete: true,
        };
        let tag_res = create_tag(&pool, request, false).await;
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
        let create_notes_res =
            create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Delete note
        let request = DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![create_notes_response.notes[0].id]),
        };
        let delete_note_res = delete_notes(&pool, request, &get_all_parsers(), false).await;
        assert!(delete_note_res.is_ok());

        // Verify that tag is deleted
        let tags: Vec<Tag> = sqlx::query_as(r"SELECT * FROM tag")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(tags.len(), 0);
    }

    #[sqlx::test]
    async fn test_create_note_removes_ancestor_tags(pool: SqlitePool) -> () {
        let parser = create_parser_helper(&pool, "markdown").await;

        // Providing `a` and `a:1` — only `a:1` should be stored
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Test".to_string(),
                keywords: vec![],
                tags: vec!["a".to_string(), "a:1".to_string()],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        };
        let res = create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(res.is_ok());
        let mut tags = res.unwrap().notes[0].tags.clone();
        tags.sort();
        assert_eq!(tags, vec!["a:1"]);
    }

    #[sqlx::test]
    async fn test_create_note_removes_ancestor_tags_three_levels(pool: SqlitePool) -> () {
        let parser = create_parser_helper(&pool, "markdown").await;

        // `a`, `a:b`, `a:b:1` — only `a:b:1` should be stored
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "Test".to_string(),
                keywords: vec![],
                tags: vec!["a".to_string(), "a:b".to_string(), "a:b:1".to_string()],
                is_suspended: false,
                custom_data: Map::new(),
            }],
        };
        let res = create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(res.is_ok());
        let mut tags = res.unwrap().notes[0].tags.clone();
        tags.sort();
        assert_eq!(tags, vec!["a:b:1"]);
    }

    #[sqlx::test]
    async fn test_update_note_modify_tags_removes_ancestors(pool: SqlitePool) -> () {
        let parser = create_parser_helper(&pool, "markdown").await;

        let create_res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "Test".to_string(),
                    keywords: vec![],
                    tags: vec![],
                    is_suspended: false,
                    custom_data: Map::new(),
                }],
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await;
        assert!(create_res.is_ok());
        let note_id = create_res.unwrap().notes[0].id;

        // Add `a` and `a:1` via ModifyTags — only `a:1` should be stored
        let update_res = update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![note_id]),
                data: None,
                parser_id: None,
                keywords: None,
                tags: UpdateTags::ModifyTags {
                    tags_to_remove: None,
                    tags_to_add: Some(vec!["a".to_string(), "a:1".to_string()]),
                },
                custom_data: None,
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await;
        assert!(update_res.is_ok());
        let mut tags = update_res.unwrap().notes[0].tags.clone();
        tags.sort();
        assert_eq!(tags, vec!["a:1"]);
    }

    #[sqlx::test]
    async fn test_update_note_set_tags_removes_ancestors(pool: SqlitePool) -> () {
        let parser = create_parser_helper(&pool, "markdown").await;

        let create_res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "Test".to_string(),
                    keywords: vec![],
                    tags: vec![],
                    is_suspended: false,
                    custom_data: Map::new(),
                }],
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await;
        assert!(create_res.is_ok());
        let note_id = create_res.unwrap().notes[0].id;

        // SetTags with `a`, `a:b`, `x` — `a` is ancestor of `a:b`, so only `a:b` and `x` kept
        let update_res = update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![note_id]),
                data: None,
                parser_id: None,
                keywords: None,
                tags: UpdateTags::SetTags(vec![
                    "a".to_string(),
                    "a:b".to_string(),
                    "x".to_string(),
                ]),
                custom_data: None,
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await;
        assert!(update_res.is_ok());
        let mut tags = update_res.unwrap().notes[0].tags.clone();
        tags.sort();
        assert_eq!(tags, vec!["a:b", "x"]);
    }

    #[sqlx::test]
    async fn test_update_note_unused_tag(pool: SqlitePool) -> () {
        // Create a tag
        let request = CreateTagRequest {
            name: "math".to_string(),
            description: String::new(),
            query: None,
            auto_delete: true,
        };
        let tag_res = create_tag(&pool, request, false).await;
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
        let create_notes_res =
            create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response = create_notes_res.unwrap();

        // Update note
        let note_id = create_notes_response.notes[0].id;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            data: None,
            parser_id: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_add: None,
                tags_to_remove: Some(vec!["math".to_string()]),
            },
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(notes_res.is_ok());

        // Verify that tag is deleted
        let tags: Vec<Tag> = sqlx::query_as(r"SELECT * FROM tag")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(tags.len(), 0);
    }
}
