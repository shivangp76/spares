use super::{AUTOMATIC_REBUILD, BULK_REQUEST_THRESHOLD, MAX_CARDS_SINGLE_INSERTION};
use crate::{
    Error, LibraryError, TagErrorKind,
    api::{
        card::create_card_tags,
        tag::{DEFAULT_TAG_AUTO_DELETE, create_tag},
    },
    config::{read_internal_config, write_internal_config},
    helpers::{intersect, parse_list},
    model::{Card, CardId, Note, NoteId, SpecialState, TagId},
    parsers::{
        Parseable, add_order_to_note_data, find_parser,
        generate_files::{
            GenerateNoteFilesRequest, GenerateNoteFilesRequests, create_note_files_bulk,
        },
    },
    schema::{
        note::{CreateNoteRequest, CreateNotesRequest, NoteResponse, NotesResponse},
        tag::CreateTagRequest,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::SqlitePool;
use std::collections::HashMap;

pub async fn validate_tags(db: &SqlitePool, tags_by_note: Vec<&Vec<String>>) -> Result<(), Error> {
    let existing_filtered_tags: Vec<(String,)> =
        sqlx::query_as(r"SELECT name FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let existing_filtered_tags_names = existing_filtered_tags
        .iter()
        .map(|(x,)| x.as_str())
        .collect::<Vec<_>>();
    for tags in tags_by_note {
        if let Some(filtered_tag) = tags
            .iter()
            .find(|t| existing_filtered_tags_names.contains(&t.as_str()))
        {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput(format!(
                    "Cannot create a note with a filtered tag `{}`. Filtered tags cannot be assigned to manually.",
                    filtered_tag
                )),
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn create_notes(
    db: &SqlitePool,
    body: CreateNotesRequest,
    at: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<NotesResponse, Error> {
    // Get parser
    let (parser_name,): (String,) = sqlx::query_as(r"SELECT name FROM parser WHERE id = ?")
        .bind(body.parser_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let parser = find_parser(parser_name.as_str(), all_parsers)?;

    // Validate tags do not contain filtered tags
    let tags_by_note = body
        .requests
        .iter()
        .map(|create_note_request| &create_note_request.tags)
        .collect::<Vec<_>>();
    validate_tags(db, tags_by_note).await?;

    let mut note_responses = Vec::new();
    let mut generate_files_requests = Vec::new();
    let mut tag_map: Option<HashMap<String, i64>> = if body.requests.len() > BULK_REQUEST_THRESHOLD
    {
        let tags: Vec<(String, i64)> = sqlx::query_as(r"SELECT name, id FROM tag")
            .bind(body.parser_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Some(tags.into_iter().collect::<HashMap<_, _>>())
    } else {
        None
    };
    let mut note_tag_entries = Vec::new();
    let mut card_entries = Vec::new();
    for create_note_request in &body.requests {
        let CreateNoteRequest {
            data,
            keywords,
            tags,
            is_suspended,
            custom_data,
        } = create_note_request;
        let mut tags = tags.clone();
        tags.sort();
        let keywords_str = &keywords.join(",");
        let custom_data_str = Value::Object(custom_data.clone());
        let (note_data, cards_count) = add_order_to_note_data(parser.as_ref(), data)?;
        // Create note
        // The RETURNING keyword is used instead of insert_result.last_insert_rowid() to prevent concurrency issues. If another writer writes in between the execution of the insert and the call of last_insert_rowid(), then the wrong id will be returned.
        let (note_id,): (NoteId,) = sqlx::query_as(r"INSERT INTO note (data, keywords, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?, ?) RETURNING id")
            .bind(&note_data)
            .bind(keywords_str)
            .bind(at.timestamp())
            .bind(at.timestamp())
            .bind(body.parser_id)
            .bind(&custom_data_str)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let tag_ids = add_note_tags(db, &tags, &mut tag_map).await?;
        note_tag_entries.extend(tag_ids.into_iter().map(|tag_id| (note_id, tag_id)));
        card_entries.extend(
            (1..=cards_count)
                .map(|i| {
                    let mut card = Card::new(at);
                    card.note_id = note_id;
                    card.order = i as u32;
                    if *is_suspended {
                        card.special_state = Some(SpecialState::Suspended);
                    }
                    card
                })
                .collect::<Vec<_>>(),
        );
        let note = Note {
            id: note_id,
            data: note_data,
            keywords: keywords_str.clone(),
            created_at: at,
            updated_at: at,
            parser_id: body.parser_id,
            custom_data: custom_data_str,
        };
        note_responses.push(NoteResponse::new(&note, tags.clone(), None, cards_count));

        // Parse note
        let generate_files_request = GenerateNoteFilesRequest {
            note_id: note.id,
            note_data: note.data.to_string(),
            keywords: parse_list(note.keywords.as_str()),
            linked_notes: None, // This is expensive so only done in `render_notes()`,
            custom_data: note.custom_data.as_object().unwrap().clone(),
            tags,
        };
        generate_files_requests.push(generate_files_request);
    }

    // Create all note_tags at the very end, in bulk
    create_note_tags(db, &note_tag_entries).await?;

    // Create all cards at the very end, in bulk
    create_cards(db, &card_entries).await?;

    if AUTOMATIC_REBUILD {
        // Add notes to matched filtered tags
        // This must be done after creating other note tags and creating cards since that impacts if the note matches a query.
        // Find all tags with queries
        let existing_filtered_tags: Vec<(TagId, String)> =
            sqlx::query_as(r"SELECT id, query FROM tag WHERE query IS NOT NULL")
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        // Get card ids from the note.id here
        let query_str = format!(
            "SELECT id FROM cards WHERE note_id IN ({})",
            vec!["?"; note_responses.len()].join(", ")
        );
        let mut query = sqlx::query_as(query_str.as_str());
        for note in &note_responses {
            query = query.bind(note.id);
        }
        let card_id_tuples: Vec<(CardId,)> = query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let created_card_ids = card_id_tuples.into_iter().map(|(x,)| x).collect::<Vec<_>>();
        let mut card_filtered_tag_entries = Vec::new();
        for (tag_id, query) in existing_filtered_tags {
            // Reexecute query to see if this card matches
            let evaluator = Evaluator::new(query.as_str());
            let card_ids = evaluator.get_card_ids(db).await?;
            let card_ids_to_tag = intersect(&card_ids, &created_card_ids);
            let card_tags = card_ids_to_tag
                .into_iter()
                .map(|card_id| (card_id, tag_id))
                .collect::<Vec<_>>();
            card_filtered_tag_entries.extend(card_tags);
        }
        create_card_tags(db, &card_filtered_tag_entries).await?;
    }

    // Create card files, without compiling
    let parse_notes_request = GenerateNoteFilesRequests {
        requests: generate_files_requests,
        overridden_output_raw_dir: None,
        include_cards: true,
        render: false,
        force_render: false,
    };
    let _card_paths = create_note_files_bulk(parser.as_ref(), &parse_notes_request)?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    // Update config
    let mut config = read_internal_config()?;
    config.linked_notes_generated = false;
    write_internal_config(&config)?;

    Ok(NotesResponse::new(note_responses))
}

pub async fn create_note_tags(
    db: &SqlitePool,
    note_tag_entries: &[(NoteId, TagId)],
) -> Result<(), Error> {
    if !note_tag_entries.is_empty() {
        let insert_note_tag_query_str = format!(
            "INSERT INTO note_tag (note_id, tag_id) VALUES {}",
            vec!["(?, ?)"; note_tag_entries.len()].join(", ")
        );
        let mut query = sqlx::query(insert_note_tag_query_str.as_str());
        for (note_id, tag_id) in note_tag_entries {
            query = query.bind(note_id);
            query = query.bind(tag_id);
        }
        let _insert_result = query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(())
}

pub async fn create_cards(db: &SqlitePool, card_entries: &[Card]) -> Result<(), Error> {
    // We chunk up the insertions to avoid "too many SQL variables error" caused by too many bind statements.
    let card_entries_chunks = card_entries
        .chunks(MAX_CARDS_SINGLE_INSERTION)
        .collect::<Vec<_>>();
    for card_entries_chunk in card_entries_chunks {
        let create_cards_query_str = format!(
            "INSERT INTO card (note_id, \"order\", back_type, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data) VALUES {}",
            vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; card_entries_chunk.len()].join(", ")
        );
        let mut query = sqlx::query(create_cards_query_str.as_str());
        for card in card_entries_chunk {
            query = query.bind(card.note_id);
            query = query.bind(card.order);
            query = query.bind(card.back_type);
            query = query.bind(card.updated_at.timestamp());
            query = query.bind(card.due.timestamp());
            query = query.bind(card.stability);
            query = query.bind(card.difficulty);
            query = query.bind(card.desired_retention);
            query = query.bind(card.special_state);
            query = query.bind(card.state);
            query = query.bind(&card.custom_data);
        }
        let _insert_result = query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(())
}

async fn add_note_tags(
    db: &SqlitePool,
    tags: &[String],
    tag_map: &mut Option<HashMap<String, i64>>,
) -> Result<Vec<i64>, Error> {
    let mut tag_ids = Vec::new();
    for tag_name in tags {
        let tag_id_opt: Option<i64> = if let &mut Some(ref tag_mapping) = tag_map {
            let tag_id_res = tag_mapping.get(tag_name);
            tag_id_res.copied()
        } else {
            // Try to get tag_id
            let tag_opt: Option<(i64,)> =
                sqlx::query_as(r"SELECT id FROM tag WHERE name = ? LIMIT 1")
                    .bind(tag_name)
                    .fetch_optional(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
            tag_opt.map(|x| x.0)
        };
        let should_create_tag = tag_id_opt.is_none();
        if let Some(tag_id) = tag_id_opt {
            tag_ids.push(tag_id);
        }
        if should_create_tag {
            // Tag does not exist, so a new one should be created.
            let create_tag_request = CreateTagRequest {
                name: tag_name.to_string(),
                description: String::new(),
                parent_id: None,
                query: None,
                auto_delete: DEFAULT_TAG_AUTO_DELETE,
            };
            let tag_response = create_tag(db, create_tag_request).await?;
            tag_ids.push(tag_response.id);

            // Add to tag_map for following create note requests
            if let &mut Some(ref mut tag_mapping) = tag_map {
                tag_mapping.insert(tag_name.to_string(), tag_response.id);
            }
        }
    }
    Ok(tag_ids)
}
