use crate::{
    Error, LibraryError, TagErrorKind,
    api::card::create_card_tags,
    model::TagId,
    search::{evaluator::Evaluator, lexer::Lexer},
};
use sqlx::sqlite::SqlitePool;

pub async fn verify_filtered_tag_query(db: &SqlitePool, query: &str) -> Result<(), Error> {
    let mut lexer = Lexer::new(query);
    let tag_dependencies = lexer.extract_tag_dependencies().map_err(|e| {
        Error::Library(LibraryError::Tag(TagErrorKind::InvalidInput(e.to_string())))
    })?;
    let existing_filtered_tags: Vec<(String,)> =
        sqlx::query_as(r"SELECT name FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let existing_filtered_tags_names = existing_filtered_tags
        .iter()
        .map(|(x,)| x.as_str())
        .collect::<Vec<_>>();
    if tag_dependencies
        .iter()
        .any(|tag_name| existing_filtered_tags_names.contains(&tag_name.as_str()))
    {
        return Err(Error::Library(LibraryError::Tag(
            TagErrorKind::InvalidInput(
                "Cannot create a filtered tag that depends on another filtered tag.".to_string(),
            ),
        )));
    }
    Ok(())
}

pub async fn tag_cards_from_query(
    db: &SqlitePool,
    query: &str,
    tag_id: TagId,
) -> Result<(), Error> {
    let evaluator = Evaluator::new(query);
    let card_ids = evaluator.get_card_ids(db).await?;
    let card_tag_entries = card_ids
        .into_iter()
        .map(|card_id| (card_id, tag_id))
        .collect::<Vec<_>>();
    create_card_tags(db, &card_tag_entries).await?;
    Ok(())
}

pub async fn rebuild_tag(db: &SqlitePool, id: i64) -> Result<(), Error> {
    // NOTE: AUTOMATIC_REBUILD: If in the future we add a column to tags with `enum Rebuild { Manual, Automatic }` and the value is `Automatic`, then it fails since you can't rebuild a tag that is populated automatically. Automatic rebuilds were decided against since it would be expensive to compute if a lot of notes are created/ updated at once. This can also _not_ be made into a config value since we want this to be specific to each filtered tag.
    // Verify tag has a query, so it can be rebuilt
    let (tag,): (Option<String>,) = sqlx::query_as(r"SELECT query FROM tag WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    match tag {
        Some(query) => {
            // Delete existing card tags with this tag
            let _delete_card_tag_result = sqlx::query(r"DELETE FROM card_tag WHERE tag_id = ?")
                .bind(id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

            // Execute query and add tag to all notes that match query
            tag_cards_from_query(db, query.as_str(), id).await?;
        }
        None => {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput(
                    "Cannot rebuild a tag that does not have a query.".to_string(),
                ),
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{
            note::create_notes,
            parser::tests::create_parser_helper,
            tag::{create_tag, update_tag},
        },
        model::{Card, CardTag, Tag},
        parsers::get_all_parsers,
        schema::{
            note::{CreateNoteRequest, CreateNotesRequest},
            tag::{CreateTagRequest, UpdateTagRequest},
        },
    };
    use chrono::Utc;
    use serde_json::Map;

    #[sqlx::test]
    async fn test_create_filtered_tag(pool: SqlitePool) -> () {
        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note with a tag
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data {{1}}".to_string(),
            keywords: vec![],
            tags: vec!["math".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };

        // Create a note without a tag
        let create_note_request_2 = CreateNoteRequest {
            data: "Test data {{2}}".to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone(), create_note_request_2.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());

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

        // Verify note's cards are tagged with a filtered tag
        let filtered_tag_id = tag_res.unwrap().id;
        let card_tags: Vec<CardTag> = sqlx::query_as(r"SELECT * FROM card_tag WHERE tag_id = ?")
            .bind(filtered_tag_id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tags.len(), 1);
        let card_tag = &card_tags[0];
        let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(create_notes_res.unwrap().notes[0].id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tag.card_id, cards[0].id);
    }

    #[sqlx::test]
    async fn test_filtered_tag_dependence_error(pool: SqlitePool) -> () {
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

        // Create a filtered tag that depends on the previous filtered tag
        let request = CreateTagRequest {
            name: "test filtered tag 2".to_string(),
            description: String::new(),
            parent_id: None,
            query: Some("ball tag=\"test filtered tag\"".to_string()),
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_err());

        // Verify note is tagged with a filtered tag
        let tags: Vec<Tag> = sqlx::query_as(r"SELECT * FROM tag")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(tags.len(), 1);
    }

    #[sqlx::test]
    async fn test_update_filtered_tag(pool: SqlitePool) -> () {
        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note with a tag
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data {{1}}".to_string(),
            keywords: vec![],
            tags: vec!["math".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };

        // Create a note without a tag
        let create_note_request_2 = CreateNoteRequest {
            data: "Test data {{2}}".to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone(), create_note_request_2.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());
        let create_notes = create_notes_res.unwrap();

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
        let filtered_tag_id = tag_res.unwrap().id;

        // Verify note's cards are tagged with a filtered tag
        let card_tags: Vec<CardTag> = sqlx::query_as(r"SELECT * FROM card_tag WHERE tag_id = ?")
            .bind(filtered_tag_id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tags.len(), 1);
        let card_tag = &card_tags[0];
        let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(create_notes.notes[0].id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tag.card_id, cards[0].id);

        // Update filtered tag
        let request = UpdateTagRequest {
            name: None,
            description: None,
            parent_id: None,
            query: Some(Some("-tag=math".to_string())),
            auto_delete: None,
        };
        let tag_res = update_tag(&pool, request, filtered_tag_id).await;
        assert!(tag_res.is_ok());

        // Verify a different note is tagged with a filtered tag
        let card_tags: Vec<CardTag> = sqlx::query_as(r"SELECT * FROM card_tag WHERE tag_id = ?")
            .bind(filtered_tag_id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tags.len(), 1);
        let card_tag = &card_tags[0];
        let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(create_notes.notes[1].id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tag.card_id, cards[0].id);
    }

    #[sqlx::test]
    async fn test_rebuild_tag(pool: SqlitePool) -> () {
        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Create a note with a tag
        let create_note_request_1 = CreateNoteRequest {
            data: "Test data {{1}}".to_string(),
            keywords: vec![],
            tags: vec!["math".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };

        // Create a note without a tag
        let create_note_request_2 = CreateNoteRequest {
            data: "Test data {{2}}".to_string(),
            keywords: vec![],
            tags: vec![],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_1.clone(), create_note_request_2.clone()],
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
        let filtered_tag_id = tag_res.unwrap().id;

        // Verify note's cards are tagged with a filtered tag
        let card_tags: Vec<CardTag> = sqlx::query_as(r"SELECT * FROM card_tag WHERE tag_id = ?")
            .bind(filtered_tag_id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tags.len(), 1);
        let card_tag = &card_tags[0];
        let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(create_notes_response.notes[0].id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tag.card_id, cards[0].id);

        // Insert a note that matches the filtered tag query
        let create_note_request_3 = CreateNoteRequest {
            data: "Test data {{3}}".to_string(),
            keywords: vec![],
            tags: vec!["math".to_string()],
            is_suspended: false,
            custom_data: Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request_3.clone()],
        };
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(create_notes_res.is_ok());
        let create_notes_response_2 = create_notes_res.unwrap();
        let cards_2: Vec<Card> = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(create_notes_response_2.notes[0].id)
            .fetch_all(&pool)
            .await
            .unwrap();

        // Verify that the newly created note's cards are not tagged
        let card_tags: Vec<CardTag> = sqlx::query_as(r"SELECT * FROM card_tag WHERE tag_id = ?")
            .bind(filtered_tag_id)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_tags.len(), 1);

        // Rebuild the filtered tag
        let rebuild_res = rebuild_tag(&pool, filtered_tag_id).await;
        assert!(rebuild_res.is_ok());

        // Verify that the newly created note is tagged
        let card_tags: Vec<CardTag> =
            sqlx::query_as(r"SELECT * FROM card_tag WHERE tag_id = ? ORDER BY card_id ASC")
                .bind(filtered_tag_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(card_tags.len(), 2);
        assert_eq!((&card_tags[0]).card_id, cards[0].id);
        assert_eq!((&card_tags[1]).card_id, cards_2[0].id);
    }
}
