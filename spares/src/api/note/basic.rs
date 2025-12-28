use crate::{
    Error,
    api::{execute_batched_query, fetch_batched_query, parser::get_parser, placeholders},
    config::{read_internal_config, write_internal_config},
    helpers::value_to_string_vec,
    model::{Note, NoteId, NoteLink, TagId},
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

    let config = read_internal_config()?;
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

fn delete_file(file_path: &Path) -> Result<(), Error> {
    if cfg!(test) {
        // Don't clutter the Trash with testing files
        std::fs::remove_file(file_path).map_err(|e| Error::Io {
            source: e,
            description: String::new(),
        })
    } else {
        trash::delete(file_path).map_err(Error::Trash)
    }
}

pub fn delete_note_files(
    parser: &dyn Parseable,
    note_id: NoteId,
    card_orders: &[usize],
    note_data: &str,
) -> Result<(), Error> {
    // NOTE: aux files are not recorded, so they cannot be deleted
    // Delete the following, if they exist:
    // - Note raw file
    // - Note rendered file
    // - All card raw files
    // - All card rendered files
    // - All image occlusion rendered files
    // Do NOT delete all image occlusion raw files, in case they are used elsewhere

    // Note raw path
    let mut note_raw_path =
        get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
    note_raw_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    note_raw_path.set_extension(parser.file_extension());
    if note_raw_path.exists() {
        delete_file(&note_raw_path)?;
    }

    // Note rendered path
    let mut note_rendered_path = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    note_rendered_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    if note_rendered_path.exists() {
        delete_file(&note_rendered_path)?;
    }

    let image_occlusion_clozes = parse_image_occlusion_data(note_data, parser, false, &mut 0)?;

    for current_card_order in card_orders {
        // Card front raw path
        let mut card_front_raw_path = get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(*current_card_order, CardSide::Front),
            None,
        );
        card_front_raw_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Front),
            note_id,
        ));
        card_front_raw_path.set_extension(parser.file_extension());
        if card_front_raw_path.exists() {
            delete_file(&card_front_raw_path)?;
        }

        // Card front rendered path
        let mut card_front_rendered_path =
            parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
        card_front_rendered_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Front),
            note_id,
        ));
        if card_front_rendered_path.exists() {
            delete_file(&card_front_rendered_path)?;
        }

        // Card back raw path
        let mut card_back_raw_path = get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(*current_card_order, CardSide::Back),
            None,
        );
        card_back_raw_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Back),
            note_id,
        ));
        card_back_raw_path.set_extension(parser.file_extension());
        if card_back_raw_path.exists() {
            delete_file(&card_back_raw_path)?;
        }

        // Card back rendered path
        let mut card_back_rendered_path =
            parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
        card_back_rendered_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Back),
            note_id,
        ));
        if card_back_rendered_path.exists() {
            delete_file(&card_back_rendered_path)?;
        }

        // Image occlusion rendered paths
        for (i, _image_occlusion_cloze) in image_occlusion_clozes.iter().enumerate() {
            for side in [CardSide::Front, CardSide::Back] {
                let mut output_rendered_filepath = get_image_occlusion_rendered_directory();
                output_rendered_filepath.push(parser.get_output_filename(
                    RenderOutputType::Card(*current_card_order, side),
                    note_id,
                ));
                let image_occlusion_order_in_card = i + 1;
                let image_occlusion_card_filepath = get_image_occlusion_card_filepath(
                    &output_rendered_filepath,
                    side,
                    image_occlusion_order_in_card,
                );
                if image_occlusion_card_filepath.exists() {
                    delete_file(&image_occlusion_card_filepath)?;
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn delete_notes(
    db: &SqlitePool,
    body: DeleteNotesRequest,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(), Error> {
    let DeleteNotesRequest { selector } = body;

    // Resolve selector to get note ids
    let note_ids = selector.to_note_ids(db).await?;
    if note_ids.is_empty() {
        return Ok(());
    }

    // Get all note data before deletion (batched query)
    let note_data_rows: Vec<(NoteId, i64, String)> =
        fetch_batched_query(db, &note_ids, async |db, chunk| {
            let query_str = format!(
                r"SELECT id, parser_id, data FROM note WHERE id IN ({})",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as(&query_str);
            for note_id in chunk {
                query = query.bind(note_id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;

    // Get all card orders for all notes (batched query)
    let card_orders_rows: Vec<(NoteId, u32)> =
        fetch_batched_query(db, &note_ids, async |db, chunk| {
            let query_str = format!(
                r#"SELECT note_id, "order" FROM card WHERE note_id IN ({})"#,
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as(&query_str);
            for note_id in chunk {
                query = query.bind(note_id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;

    // Group card orders by note_id
    let mut card_orders_map: HashMap<NoteId, Vec<usize>> = HashMap::new();
    for (note_id, card_order) in card_orders_rows {
        card_orders_map
            .entry(note_id)
            .or_default()
            .push(card_order as usize);
    }

    // Get all tags with `auto_delete` enabled for all notes (batched query)
    // NOTE: AUTOMATIC REBUILD: If `Automatic` rebuild is enabled in the future, then a check would be added to ensure `auto_delete` is false. In other words, `auto_delete` as true and rebuild as `Automatic` conflict since once the tag has 0 notes left, it will be deleted so that means notes are not automatically added to it anymore.
    let tags_rows: Vec<TagId> = fetch_batched_query(db, &note_ids, async |db, chunk| {
        let query_str = format!(
            "SELECT DISTINCT t.id
            FROM tag t
            LEFT JOIN note_tag nt ON t.id = nt.tag_id
            LEFT JOIN card_tag ct ON t.id = ct.tag_id
            LEFT JOIN card c ON ct.card_id = c.id
            WHERE
                (nt.note_id IN ({}) OR c.note_id IN ({}))
                AND t.auto_delete = 1",
            placeholders(chunk.len()),
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_scalar(&query_str);
        for note_id in chunk {
            query = query.bind(note_id);
        }
        for note_id in chunk {
            query = query.bind(note_id);
        }
        query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })
    })
    .await?;

    // Delete all notes in batched queries
    execute_batched_query(db, &note_ids, async |db, chunk| {
        let query_str = format!(
            "DELETE FROM note WHERE id IN ({})",
            placeholders(chunk.len()),
        );
        let mut query = sqlx::query(&query_str);
        for note_id in chunk {
            query = query.bind(note_id);
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await?;

    // Delete tags with no more notes
    let all_tag_ids = tags_rows
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    delete_empty_tags(db, &all_tag_ids).await?;

    // Delete files for all notes
    let grouped_note_data = note_data_rows
        .into_iter()
        .map(|(nid, pid, nd)| (pid, (nid, nd)))
        .into_group_map();
    for (parser_id, notes) in grouped_note_data {
        let parser_response = get_parser(db, parser_id).await?;
        let parser = find_parser(parser_response.name.as_str(), all_parsers)?;
        for (note_id, note_data) in notes {
            let card_orders = card_orders_map.remove(&note_id).unwrap_or_default();
            delete_note_files(parser.as_ref(), note_id, &card_orders, &note_data)?;
        }
    }

    // Update config
    let mut config = read_internal_config()?;
    config.linked_notes_generated = false;
    write_internal_config(&config)?;

    Ok(())
}

pub async fn delete_empty_tags(db: &SqlitePool, tag_ids: &[TagId]) -> Result<(), Error> {
    execute_batched_query(db, tag_ids, async |db, chunk| {
        let query_str = format!(
            r"DELETE FROM tag
            WHERE id IN ({})
            AND auto_delete = 1
            AND NOT EXISTS (
                SELECT 1 FROM note_tag WHERE note_tag.tag_id = tag.id
            )
            AND NOT EXISTS (
                SELECT 1 FROM card_tag WHERE card_tag.tag_id = tag.id
            )",
            placeholders(chunk.len()),
        );
        let mut query = sqlx::query(&query_str);
        for tag_id in chunk {
            query = query.bind(tag_id);
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await
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

    let config = read_internal_config()?;
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
    use crate::api::note::{create_notes, update_notes};
    use crate::api::parser::tests::create_parser_helper;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::{NotesSelector, UpdateTags};
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
                r"Third {{ Cloze here, linking to [keyword 1][li], [keyword 1.5][li], and [keyword 2][li] }}",
                r"Third {{[o:1] Cloze here, linking to [keyword 1][li], [keyword 1.5][li], and [keyword 2][li] }}",
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
                create_notes(pool, request, Utc::now(), &get_all_parsers()).await;
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
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
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
        let delete_note_res = delete_notes(&pool, request, &get_all_parsers()).await;
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
