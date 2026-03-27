use crate::{
    Error,
    api::{
        execute_batched_query, fetch_batched_query,
        parser::get_parser,
        placeholders,
        undo::{
            insert_events,
            payloads::{CardSnapshot, DeleteNotesPayload, NoteSnapshot},
        },
    },
    config::{read_internal_config, write_internal_config},
    helpers::value_to_string_vec,
    model::{Card, EventType, Note, NoteId, NoteLink, TagId},
    parsers::{
        Parseable, RenderOutputDirectoryType, find_parser,
        generate_files::{CardSide, RenderOutputType},
        get_output_raw_dir,
        image_occlusion::{
            get_image_occlusion_card_filepath, get_image_occlusion_rendered_directory,
            parse_image_occlusion_data,
        },
    },
    schema::{
        FilterOptions,
        note::{DeleteNotesRequest, LinkedNote, NoteResponse, NotesSelector},
    },
    search::evaluator::Evaluator,
};
use chrono::DateTime;
use chrono::Utc;
use itertools::Itertools;
use serde_json::Value;
use sqlx::sqlite::SqlitePool;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub async fn get_note(db: &SqlitePool, note_id: NoteId) -> Result<NoteResponse, Error> {
    // Get note
    let note: Note = sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    let config = read_internal_config(db).await?;
    enrich_note(db, &note, config.linked_notes_generated).await
}

pub async fn enrich_note(
    db: &SqlitePool,
    note: &Note,
    linked_notes_generated: bool,
) -> Result<NoteResponse, Error> {
    // Get tags for note
    // NOTE: Filtered tags (from `card_tags`) are not returned here, since they are specific to cards, not notes.
    let tags: Vec<String> = sqlx::query_scalar(r"SELECT t.name FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? ORDER BY name ASC")
        .bind(note.id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    // Get linked notes
    let note_links: Vec<NoteLink> =
        sqlx::query_as(r"SELECT * FROM note_link WHERE parent_note_id = ?")
            .bind(note.id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let linked_notes_arg = if linked_notes_generated {
        Some(
            note_links
                .into_iter()
                .map(LinkedNote::new)
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    // Get card count
    let card_count: u32 = sqlx::query_scalar(r"SELECT COUNT(*) FROM card WHERE note_id = ?")
        .bind(note.id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    // Get keywords
    let keywords: Vec<String> = sqlx::query_scalar(
        r"SELECT keyword FROM note_keyword WHERE note_id = ? ORDER BY keyword ASC",
    )
    .bind(note.id)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    Ok(NoteResponse::new(
        note,
        keywords,
        tags,
        linked_notes_arg,
        card_count as usize,
    ))
}

pub(crate) async fn fetch_note_snapshot(
    db: &SqlitePool,
    note_id: NoteId,
    data: &str,
    created_at: DateTime<Utc>,
    parser_id: i64,
    custom_data: &Value,
) -> Result<NoteSnapshot, Error> {
    // Fetch non-embedded keywords
    let keywords: Vec<String> = sqlx::query_scalar(
        r"SELECT keyword FROM note_keyword WHERE note_id = ? AND embedded = 0 ORDER BY keyword ASC",
    )
    .bind(note_id)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    // Fetch non-filtered tag names
    let tags: Vec<String> = sqlx::query_scalar(
        r"SELECT t.name FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.query IS NULL ORDER BY t.name ASC",
    )
    .bind(note_id)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    // Fetch cards
    let cards: Vec<Card> =
        sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER BY "order" ASC"#)
            .bind(note_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    let card_snapshots = cards.iter().map(CardSnapshot::from_card).collect();

    Ok(NoteSnapshot {
        id: note_id,
        data: data.to_string(),
        created_at,
        parser_id,
        custom_data: custom_data.clone(),
        keywords,
        tags,
        cards: card_snapshots,
    })
}

pub async fn list_notes(db: &SqlitePool, opts: FilterOptions) -> Result<Vec<NoteResponse>, Error> {
    #[derive(sqlx::FromRow)]
    struct ListNotesRow {
        #[sqlx(flatten)]
        note: Note,
        keywords_value: Value,
        tags_value: Value,
        card_count: u32,
    }
    // Single query to fetch notes with their tags, card counts, and note links in one go
    let limit = opts.limit.unwrap_or(10);
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    // NOTE: Filtered tags (from `card_tags`) are not returned here, since they are specific to cards, not notes.
    let notes_data: Vec<ListNotesRow> = sqlx::query_as(
        r"SELECT
           n.*,
           COALESCE((SELECT JSON_GROUP_ARRAY(nk.keyword)
            FROM note_keyword nk
            WHERE nk.note_id = n.id AND nk.embedded = 0), '[]') as keywords_value,
           COALESCE(JSON_GROUP_ARRAY(t.name), '[]') AS tags_value,
           (SELECT COUNT(*) FROM card WHERE note_id = n.id) AS card_count
         FROM note n
         LEFT JOIN note_tag nt ON n.id = nt.note_id
         LEFT JOIN tag t ON nt.tag_id = t.id
         GROUP BY n.id
         ORDER BY n.id
         LIMIT ? OFFSET ?
        ",
    )
    .bind(limit as u32)
    .bind(offset as u32)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    let config = read_internal_config(db).await?;
    let mut responses = Vec::new();
    for ListNotesRow {
        note,
        keywords_value,
        tags_value,
        card_count,
    } in notes_data
    {
        // Get linked_notes
        let linked_notes_arg = if config.linked_notes_generated {
            let note_links: Vec<NoteLink> =
                sqlx::query_as(r"SELECT * FROM note_link WHERE parent_note_id = ?")
                    .bind(note.id)
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
            Some(
                note_links
                    .into_iter()
                    .map(LinkedNote::new)
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        let keywords: Vec<String> = value_to_string_vec(&keywords_value);
        let mut tags: Vec<String> = value_to_string_vec(&tags_value);
        tags.sort();
        responses.push(NoteResponse::new(
            &note,
            keywords,
            tags,
            linked_notes_arg,
            card_count as usize,
        ));
    }
    Ok(responses)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::note::{create_notes, delete_notes, update_notes};
    use crate::api::parser::tests::create_parser_helper;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::{NotesSelector, UpdateNotesResponse, UpdateTags};
    use crate::{
        model::NoteTag,
        schema::note::{CreateNoteRequest, CreateNotesRequest, UpdateNotesRequest},
    };
    use chrono::Utc;
    use serde_json::Map;

    fn contain_same_elements<T>(vec1: &[T], vec2: &[T]) -> bool
    where
        T: PartialEq,
    {
        vec1.iter().all(|item| vec2.contains(item))
    }

    pub async fn create_note_helper(pool: &SqlitePool) -> Vec<NoteResponse> {
        let parser = create_parser_helper(pool, "markdown").await;

        let notes: Vec<(&str, &str, &[&str], &[&str], usize)> = vec![
            (
                r"First {{ Cloze here }}",
                r"First {{[o:1] Cloze here }}",
                &["tag 1", "tag 3"],
                &["another keyword"],
                0,
            ),
            (
                r"Second {{ Cloze }}",
                r"Second {{[o:1] Cloze }}",
                &["tag 1", "tag 2"],
                &["keyword 1", "keyword 2"],
                0,
            ),
            (
                r"Third {{ Cloze here, linking to [keyword 1][li], [keywords 1][li], and [keyword 2][li] }}",
                r"Third {{[o:1] Cloze here, linking to [keyword 1][li], [keywords 1][li], and [keyword 2][li] }}",
                &[],
                &[],
                3,
            ),
        ];

        // Create notes
        let mut all_notes: Vec<NoteResponse> = Vec::new();
        for (insertion_data, data, tags, keywords, note_links_count) in notes {
            let tags: Vec<String> = tags.iter().map(|x| (*x).to_string()).collect();
            let create_note_request = CreateNoteRequest {
                data: insertion_data.to_string(),
                keywords: keywords
                    .iter()
                    .copied()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>(),
                tags,
                is_suspended: false,
                custom_data: Map::new(),
            };
            let request = CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![create_note_request.clone()],
            };
            let create_notes_res =
                create_notes(pool, request, Utc::now(), &get_all_parsers(), false).await;
            assert!(create_notes_res.is_ok());
            if let Ok(notes_response) = create_notes_res {
                let note = notes_response.notes.into_iter().next().unwrap();
                assert_eq!(note.data, data);
                assert_eq!(note.parser_id, parser.id);
                assert_eq!(note.keywords, create_note_request.keywords);
                assert_eq!(note.tags, create_note_request.tags);

                // Check database and verify item with id exists
                let note_res: Result<Note, sqlx::Error> =
                    sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
                        .bind(note.id)
                        .fetch_one(pool)
                        .await;
                assert!(note_res.is_ok());
                let db_note = note_res.unwrap();
                assert_eq!(db_note.data, data);
                assert_eq!(db_note.parser_id, parser.id);

                // Verify note_keywords in database
                let note_keywords_res: Result<Vec<(String, bool)>, sqlx::Error> =
                    sqlx::query_as(r"SELECT keyword, embedded FROM note_keyword WHERE note_id = ? ORDER BY keyword")
                        .bind(note.id)
                        .fetch_all(pool)
                        .await;
                assert!(note_keywords_res.is_ok());
                let db_keywords = note_keywords_res.unwrap();
                assert!(db_keywords.iter().all(|(_, embedded)| !embedded));
                assert_eq!(
                    db_keywords
                        .iter()
                        .map(|(k, _)| k.clone())
                        .collect::<Vec<_>>(),
                    create_note_request.keywords
                );

                // Verify note_tags in database
                let note_tag_res: Result<Vec<NoteTag>, sqlx::Error> =
                    sqlx::query_as(r"SELECT * FROM note_tag WHERE note_id = ?")
                        .bind(note.id)
                        .fetch_all(pool)
                        .await;
                assert!(note_tag_res.is_ok());
                let note_tags = note_tag_res.unwrap();
                assert_eq!(note_tags.len(), create_note_request.tags.len());

                // Verify linked_notes in database
                // NOTE: Linked notes are only matched after calling the render endpoint, but the searched_keyword should be inserted
                let note_link_res: Result<Vec<NoteLink>, sqlx::Error> =
                    sqlx::query_as(r#"SELECT * FROM note_link WHERE parent_note_id = ?"#)
                        .bind(note.id)
                        .fetch_all(pool)
                        .await;
                assert!(note_link_res.is_ok());
                let note_links = note_link_res.unwrap();
                assert_eq!(note_links.len(), note_links_count);

                all_notes.push(note);
            }
        }
        all_notes
    }

    #[sqlx::test]
    async fn test_create_note(pool: SqlitePool) -> () {
        // Create note
        let _ = create_note_helper(&pool).await;
    }

    #[sqlx::test]
    async fn test_get_note(pool: SqlitePool) -> () {
        // Create note
        let created_notes = create_note_helper(&pool).await;
        let last_note = created_notes.last().unwrap();

        // Get note
        let note_res = get_note(&pool, last_note.id).await;
        assert!(note_res.is_ok());
        if let Ok(note) = note_res {
            assert_eq!(note.data, last_note.data);
            assert_eq!(note.parser_id, last_note.parser_id);
            assert_eq!(note.keywords, last_note.keywords);
            assert!(contain_same_elements(&note.tags, &last_note.tags));
        }
    }

    #[sqlx::test]
    async fn test_update_note(pool: SqlitePool) -> () {
        // Create note
        let created_notes = create_note_helper(&pool).await;
        let last_note = created_notes.last().unwrap();

        // Update note
        let id = last_note.id;
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![id]),
            data: Some(created_notes[1].data.to_string()),
            parser_id: None,
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(notes_res.is_ok());
        if let Ok(UpdateNotesResponse { notes, .. }) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert_eq!(note.data, created_notes[1].data);
            assert_eq!(note.parser_id, last_note.parser_id);
            assert_eq!(note.keywords, last_note.keywords);
            assert_eq!(note.tags, last_note.tags);

            // Check database and verify item with id has the new property
            let note_res: Result<Note, sqlx::Error> =
                sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
                    .bind(note.id)
                    .fetch_one(&pool)
                    .await;
            assert!(note_res.is_ok());
            if let Ok(db_note) = note_res {
                assert_eq!(db_note.data, created_notes[1].data);
                assert_eq!(db_note.parser_id, last_note.parser_id);
            }

            // Verify keywords are unchanged
            let note_keywords_res: Result<Vec<String>, sqlx::Error> = sqlx::query_scalar(
                r"SELECT keyword FROM note_keyword WHERE note_id = ? ORDER BY keyword",
            )
            .bind(note.id)
            .fetch_all(&pool)
            .await;
            assert!(note_keywords_res.is_ok());
            let db_keywords: Vec<String> = note_keywords_res.unwrap();
            assert_eq!(db_keywords, last_note.keywords);

            // let cards_res: Result<Vec<Card>, sqlx::Error> =
            //     sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            //         .bind(note.id)
            //         .fetch_all(&pool)
            //         .await;
            // assert!(cards_res.is_ok());
            // if let Ok(cards) = cards_res {
            //     assert!(cards
            //         .iter()
            //         .all(|card| card.special_state == Some(SpecialState::Suspended)));
            // }
        }
    }

    #[sqlx::test]
    async fn test_delete_note(pool: SqlitePool) -> () {
        // Create note so it can be deleted
        let created_notes = create_note_helper(&pool).await;
        let last_note = created_notes.last().unwrap();

        // Delete note
        let request = DeleteNotesRequest {
            selector: NotesSelector::Ids(vec![last_note.id]),
        };
        let delete_note_res = delete_notes(&pool, request, &get_all_parsers(), false).await;
        assert!(delete_note_res.is_ok());

        // Check database and verify item with id does not exist
        let note_res: Result<Note, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
                .bind(last_note.id)
                .fetch_one(&pool)
                .await;
        assert!(note_res.is_err());
        // Workaround since sqlx::Error does not derive PartialEq
        assert_eq!(
            format!("{:?}", note_res.unwrap_err()),
            format!("{:?}", sqlx::Error::RowNotFound)
        );

        // Verify note_tags for that note are deleted
        let note_tag_res: Result<Vec<NoteTag>, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM note_tag WHERE note_id = ?")
                .bind(last_note.id)
                .fetch_all(&pool)
                .await;
        assert!(note_tag_res.is_ok());
        let note_tags = note_tag_res.unwrap();
        assert_eq!(note_tags.len(), 0);
    }

    #[sqlx::test]
    async fn test_list_notes(pool: SqlitePool) -> () {
        // Create notes
        let created_notes = create_note_helper(&pool).await;

        // List notes
        let notes_res = list_notes(
            &pool,
            FilterOptions {
                limit: None,
                page: None,
            },
        )
        .await;
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 3);
            assert_eq!(notes.first().unwrap().data, created_notes[0].data);
            assert_eq!(notes.last().unwrap().data, created_notes[2].data);

            // Render notes was not called, so linked notes should be empty
            assert_eq!(notes.first().unwrap().linked_notes, None);
        }
    }
}
