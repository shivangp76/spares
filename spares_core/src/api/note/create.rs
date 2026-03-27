use super::{AUTOMATIC_REBUILD, BULK_REQUEST_THRESHOLD};
use crate::{
    CardErrorKind, Error, LibraryError, TagErrorKind,
    api::{
        card::create_card_tags,
        execute_batched_query, fetch_batched_query,
        note::basic::fetch_note_snapshot,
        parser::get_parser_name,
        placeholders, placeholders_2d,
        tag::{DEFAULT_TAG_AUTO_DELETE, create_tag},
        undo::{
            create_event_group, insert_events,
            payloads::{CreateNotesPayload, CreateTagPayload, NoteSnapshot},
        },
    },
    config::{read_external_config, read_internal_config, write_internal_config},
    helpers::{intersect, remove_ancestor_tags},
    model::{Card, CardId, EventType, Note, NoteId, NoteLink, SpecialState, TagId},
    parsers::{
        Parseable, ReadableCardIdentifier, add_order_to_note_data, extract_and_combine_keywords,
        find_parser,
        generate_files::{
            GenerateNoteFilesRequest, GenerateNoteFilesRequests, create_note_files_bulk,
        },
    },
    schema::{
        note::{self, CreateNoteRequest, CreateNotesRequest, NoteResponse, NotesResponse},
        tag::CreateTagRequest,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::SqlitePool;
use std::collections::HashMap;

async fn rebuild_filtered_tags_for_created_notes(
    db: &SqlitePool,
    note_responses: &[NoteResponse],
) -> Result<(), Error> {
    // Find all tags with queries
    let existing_filtered_tags: Vec<(TagId, String)> =
        sqlx::query_as(r"SELECT id, query FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    // Get card ids from the note.id here
    let created_card_ids: Vec<CardId> =
        fetch_batched_query(db, note_responses, async |db, chunk| {
            let query_str = format!(
                "SELECT id FROM cards WHERE note_id IN ({})",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_scalar(&query_str);
            for note in chunk {
                query = query.bind(note.id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
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
    Ok(())
}

pub async fn validate_tags(db: &SqlitePool, tags_by_note: Vec<&Vec<String>>) -> Result<(), Error> {
    let existing_filtered_tags_names: Vec<String> =
        sqlx::query_scalar(r"SELECT name FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    for tags in tags_by_note {
        if let Some(filtered_tag) = tags
            .iter()
            .find(|t| existing_filtered_tags_names.contains(t))
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

#[expect(clippy::too_many_lines)]
pub async fn create_notes(
    db: &SqlitePool,
    body: CreateNotesRequest,
    at: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
    log: bool,
) -> Result<NotesResponse, Error> {
    // Get parser
    let parser_name = get_parser_name(db, body.parser_id).await?;
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
    let mut note_keyword_entries = Vec::new();
    let mut note_link_entries = Vec::new();
    let mut note_tag_entries = Vec::new();
    let mut card_entries = Vec::new();
    // (dst_note_id, dst_order, src_note_id, src_order)
    let mut inherit_entries: Vec<(NoteId, u32, NoteId, usize)> = Vec::new();
    let mut new_tag_payloads: Vec<CreateTagPayload> = Vec::new();
    for create_note_request in &body.requests {
        let CreateNoteRequest {
            data,
            keywords: extra_keywords,
            tags,
            is_suspended,
            custom_data,
        } = create_note_request;
        let mut tags = remove_ancestor_tags(tags);
        tags.sort();
        let custom_data_str = Value::Object(custom_data.clone());
        let external_config = read_external_config().ok();
        let (note_data, card_datas) = add_order_to_note_data(
            parser.as_ref(),
            data,
            external_config.as_ref().map(|c| &c.overlapper),
        )?;
        // Create note
        // The RETURNING keyword is used instead of insert_result.last_insert_rowid() to prevent concurrency issues. If another writer writes in between the execution of the insert and the call of last_insert_rowid(), then the wrong id will be returned.
        let note_id: NoteId = sqlx::query_scalar(r"INSERT INTO note (data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?) RETURNING id")
            .bind(&note_data)
            .bind(at.timestamp())
            .bind(at.timestamp())
            .bind(body.parser_id)
            .bind(&custom_data_str)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Create note keywords
        let all_keywords =
            extract_and_combine_keywords(parser.as_ref(), note_data.as_str(), extra_keywords)
                .map_err(Error::Library)?;
        note_keyword_entries.push((note_id, all_keywords.clone()));

        // Note Links
        note_link_entries.extend(
            parser
                .get_linked_notes(&note_data)?
                .into_iter()
                .enumerate()
                .map(|(i, linked_note_range)| NoteLink {
                    parent_note_id: note_id,
                    linked_note_id: None,
                    order: i as u32,
                    searched_keyword: note_data[linked_note_range].to_string(),
                    matched_keyword: None,
                    score: None,
                }),
        );

        // Note Tags
        let (tag_ids, note_new_tag_payloads) = add_note_tags(db, &tags, &mut tag_map).await?;
        note_tag_entries.extend(tag_ids.into_iter().map(|tag_id| (note_id, tag_id)));
        new_tag_payloads.extend(note_new_tag_payloads);

        // Cards
        card_entries.extend(
            card_datas
                .iter()
                .enumerate()
                .map(|(i, card_data)| {
                    let order = (i + 1) as u32;
                    let mut card = Card::new(at);
                    card.note_id = note_id;
                    card.order = order;
                    card.back_type = card_data.back_type;
                    if *is_suspended {
                        card.special_state = Some(SpecialState::Suspended);
                    }
                    // Collect inheritance directives for post-creation update
                    if let Some(ReadableCardIdentifier {
                        note_id: src_note_id,
                        order: src_order,
                    }) = card_data.inherit
                    {
                        inherit_entries.push((note_id, order, src_note_id, src_order));
                    }
                    card
                })
                .collect::<Vec<_>>(),
        );

        let note = Note {
            id: note_id,
            data: note_data,
            created_at: at,
            updated_at: at,
            parser_id: body.parser_id,
            custom_data: custom_data_str,
        };
        note_responses.push(NoteResponse::new(
            &note,
            all_keywords
                .iter()
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>(),
            tags.clone(),
            None,
            card_datas.len(),
        ));

        // Parse note
        let generate_files_request = GenerateNoteFilesRequest {
            note_id: note.id,
            note_data: note.data.clone(),
            keywords: all_keywords
                .into_iter()
                .filter(|(_, embedded)| !embedded)
                .map(|(k, _)| k)
                .collect::<Vec<_>>(),
            linked_notes: None, // This is expensive so only done in `render_notes()`,
            custom_data: note.custom_data.as_object().unwrap().clone(),
            tags,
        };
        generate_files_requests.push(generate_files_request);
    }

    // Create all note_keywords at the very end, in bulk
    create_note_keywords(db, &note_keyword_entries).await?;

    // Create all note_links at the very end, in bulk
    create_note_links(db, &note_link_entries).await?;

    // Create all note_tags at the very end, in bulk
    create_note_tags(db, &note_tag_entries).await?;

    // Create all cards at the very end, in bulk
    create_cards(db, &card_entries).await?;

    apply_srs_inheritance(db, &inherit_entries).await?;

    if AUTOMATIC_REBUILD {
        // Add notes to matched filtered tags.
        // This must be done after creating other note tags and creating cards since that impacts if the note matches a query.
        rebuild_filtered_tags_for_created_notes(db, &note_responses).await?;
    }

    // Create card files, without compiling. (There is no point in compiling if the linked notes are not updated.)
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

    // Update config — only invalidate linked-note cache if any note has keywords or note links
    let has_keywords = note_keyword_entries.iter().any(|(_, ks)| !ks.is_empty());
    if has_keywords || !note_link_entries.is_empty() {
        let mut config = read_internal_config(db).await?;
        config.linked_notes_generated = false;
        write_internal_config(db, &config).await?;
    }

    // Log event
    if log {
        let mut snapshots = Vec::with_capacity(note_responses.len());
        for note_response in &note_responses {
            let snapshot = fetch_note_snapshot(
                db,
                note_response.id,
                &note_response.data,
                note_response.created_at,
                note_response.parser_id,
                &Value::Object(note_response.custom_data.clone()),
            )
            .await?;
            snapshots.push(snapshot);
        }
        let payload = CreateNotesPayload { notes: snapshots };
        let note_event = (
            EventType::CreateNotes,
            serde_json::to_value(&payload).unwrap(),
        );
        if new_tag_payloads.is_empty() {
            insert_events(db, &[note_event], at, None).await?;
        } else {
            let mut events: Vec<(EventType, Value)> = new_tag_payloads
                .into_iter()
                .map(|p| (EventType::CreateTag, serde_json::to_value(&p).unwrap()))
                .collect();
            events.push(note_event);
            create_event_group(db, events, at).await?;
        }
    }

    Ok(NotesResponse::new(note_responses))
}

/// Create notes from snapshots (used when applying the undo of a `DeleteNotes` event).
/// Inserts notes with their specific IDs, keywords, tags, and cards.
/// No file generation is performed.
#[expect(clippy::too_many_lines)]
pub(crate) async fn create_notes_event(
    db: &SqlitePool,
    payload: CreateNotesPayload,
    log: bool,
) -> Result<(), Error> {
    for snapshot in &payload.notes {
        let NoteSnapshot {
            id,
            data,
            created_at,
            parser_id,
            custom_data,
            keywords,
            tags,
            cards,
        } = snapshot;

        // Insert note with specific ID
        sqlx::query(
            r"INSERT INTO note (id, data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(data)
        .bind(created_at.timestamp())
        .bind(created_at.timestamp())
        .bind(parser_id)
        .bind(custom_data)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

        // Insert keywords (non-embedded only in snapshot; treat all as non-embedded)
        if !keywords.is_empty() {
            create_note_keywords(
                db,
                &[(*id, keywords.iter().map(|k| (k.clone(), false)).collect())],
            )
            .await?;
        }

        // Insert tags — create tags if they don't exist (do NOT log tag creation)
        let mut tag_ids: Vec<i64> = Vec::new();
        for tag_name in tags {
            let existing_tag_id: Option<i64> =
                sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ? LIMIT 1")
                    .bind(tag_name)
                    .fetch_optional(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
            let tag_id = if let Some(tag_id) = existing_tag_id {
                tag_id
            } else {
                let tag_response = create_tag(
                    db,
                    CreateTagRequest {
                        name: tag_name.clone(),
                        description: String::new(),
                        query: None,
                        auto_delete: DEFAULT_TAG_AUTO_DELETE,
                    },
                    false,
                )
                .await?;
                tag_response.id
            };
            tag_ids.push(tag_id);
        }
        if !tag_ids.is_empty() {
            create_note_tags(
                db,
                &tag_ids
                    .into_iter()
                    .map(|tid| (*id, tid))
                    .collect::<Vec<_>>(),
            )
            .await?;
        }

        // Insert cards with specific IDs
        if !cards.is_empty() {
            execute_batched_query(db, cards, async |db, chunk| {
                let query_str = format!(
                    "INSERT INTO card (id, note_id, \"order\", back_type, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data) VALUES {}",
                    placeholders_2d(chunk.len(), 12)
                );
                let mut query = sqlx::query(&query_str);
                for card in chunk {
                    query = query.bind(card.id);
                    query = query.bind(id);
                    query = query.bind(card.order);
                    query = query.bind(card.back_type);
                    query = query.bind(card.due.timestamp());
                    query = query.bind(card.due.timestamp());
                    query = query.bind(card.stability);
                    query = query.bind(card.difficulty);
                    query = query.bind(card.desired_retention);
                    query = query.bind(card.special_state);
                    query = query.bind(card.state);
                    query = query.bind(&card.custom_data);
                }
                query
                    .execute(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
                Ok(())
            })
            .await?;
        }
    }

    if log {
        insert_events(
            db,
            &[(
                crate::model::EventType::CreateNotes,
                serde_json::to_value(&payload).unwrap(),
            )],
            chrono::Utc::now(),
            None,
        )
        .await?;
    }

    Ok(())
}

pub async fn create_note_keywords(
    db: &SqlitePool,
    note_id_with_keywords: &[(NoteId, Vec<(String, bool)>)],
) -> Result<(), Error> {
    let rows = note_id_with_keywords
        .iter()
        .flat_map(|(note_id, ks)| ks.iter().map(|k| (*note_id, k)))
        .collect::<Vec<_>>();
    execute_batched_query(db, &rows, async |db, chunk| {
        let query_str = format!(
            "INSERT INTO note_keyword (note_id, keyword, embedded) VALUES {}",
            placeholders_2d(chunk.len(), 3)
        );
        let mut query = sqlx::query(&query_str);
        for (note_id, (keyword, embedded)) in chunk {
            query = query.bind(note_id);
            query = query.bind(keyword);
            query = query.bind(i32::from(*embedded));
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await
}

pub async fn create_note_links(
    db: &SqlitePool,
    note_link_entries: &[NoteLink],
) -> Result<(), Error> {
    execute_batched_query(db, note_link_entries, async |db, chunk| {
        let query_str = format!(
            "INSERT INTO note_link (parent_note_id, linked_note_id, \"order\", searched_keyword, matched_keyword, score) VALUES {}",
            placeholders_2d(chunk.len(), 6)
        );
        let mut query = sqlx::query(&query_str);
        for NoteLink {
            parent_note_id,
            linked_note_id,
            order,
            searched_keyword,
            matched_keyword,
            score,
        } in chunk
        {
            query = query.bind(parent_note_id);
            query = query.bind(linked_note_id);
            query = query.bind(order);
            query = query.bind(searched_keyword);
            query = query.bind(matched_keyword);
            query = query.bind(score);
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await
}

pub async fn create_note_tags(
    db: &SqlitePool,
    note_tag_entries: &[(NoteId, TagId)],
) -> Result<(), Error> {
    execute_batched_query(db, note_tag_entries, async |db, chunk| {
        let query_str = format!(
            "INSERT INTO note_tag (note_id, tag_id) VALUES {}",
            placeholders_2d(chunk.len(), 2)
        );
        let mut query = sqlx::query(query_str.as_str());
        for (note_id, tag_id) in chunk {
            query = query.bind(note_id);
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

/// After newly created cards have been inserted into the database, copy SRS fields from a source
/// card to each card that carried an `inh:NOTE_ID/ORDER` cloze setting.
///
/// `inherit_entries` is a list of `(dst_note_id, dst_order, src_note_id, src_order)`.
pub(super) async fn apply_srs_inheritance(
    db: &SqlitePool,
    inherit_entries: &[(NoteId, u32, NoteId, usize)],
) -> Result<(), Error> {
    for &(dst_note_id, dst_order, src_note_id, src_order) in inherit_entries {
        let src_card: Option<Card> =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? AND "order" = ?"#)
                .bind(src_note_id)
                .bind(src_order as u32)
                .fetch_optional(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

        let src_card = src_card.ok_or_else(|| {
            Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(format!(
                "`inh:` source card not found: note_id={src_note_id}, order={src_order}"
            ))))
        })?;

        sqlx::query(
            r#"UPDATE card SET stability = ?, difficulty = ?, desired_retention = ?,
               state = ?, due = ?, special_state = ?
               WHERE note_id = ? AND "order" = ?"#,
        )
        .bind(src_card.stability)
        .bind(src_card.difficulty)
        .bind(src_card.desired_retention)
        .bind(src_card.state)
        .bind(src_card.due.timestamp())
        .bind(src_card.special_state)
        .bind(dst_note_id)
        .bind(dst_order)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(())
}

pub async fn create_cards(db: &SqlitePool, card_entries: &[Card]) -> Result<(), Error> {
    execute_batched_query(db, card_entries, async |db, chunk| {
        let query_str = format!(
            "INSERT INTO card (note_id, \"order\", back_type, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data) VALUES {}",
            placeholders_2d(chunk.len(), 11)
        );
        let mut query = sqlx::query(query_str.as_str());
        for card in chunk {
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
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await
}

async fn add_note_tags(
    db: &SqlitePool,
    tags: &[String],
    tag_map: &mut Option<HashMap<String, i64>>,
) -> Result<(Vec<i64>, Vec<CreateTagPayload>), Error> {
    let mut tag_ids = Vec::new();
    let mut new_tag_payloads = Vec::new();
    for tag_name in tags {
        let tag_id_opt: Option<i64> = if let &mut Some(ref tag_mapping) = tag_map {
            let tag_id_res = tag_mapping.get(tag_name);
            tag_id_res.copied()
        } else {
            // Try to get tag_id
            let tag_opt: Option<i64> =
                sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ? LIMIT 1")
                    .bind(tag_name)
                    .fetch_optional(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
            tag_opt
        };
        let should_create_tag = tag_id_opt.is_none();
        if let Some(tag_id) = tag_id_opt {
            tag_ids.push(tag_id);
        }
        if should_create_tag {
            // Tag does not exist, so a new one should be created.
            let create_tag_request = CreateTagRequest {
                name: tag_name.clone(),
                description: String::new(),
                query: None,
                auto_delete: DEFAULT_TAG_AUTO_DELETE,
            };
            let tag_response = create_tag(db, create_tag_request, false).await?;
            tag_ids.push(tag_response.id);
            new_tag_payloads.push(CreateTagPayload {
                id: Some(tag_response.id),
                name: tag_name.clone(),
                description: String::new(),
                query: None,
                auto_delete: DEFAULT_TAG_AUTO_DELETE,
            });

            // Add to tag_map for following create note requests
            if let &mut Some(ref mut tag_mapping) = tag_map {
                tag_mapping.insert(tag_name.clone(), tag_response.id);
            }
        }
    }
    Ok((tag_ids, new_tag_payloads))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{
            note::create_notes, parser::tests::create_parser_helper, review::submit_study_action,
        },
        model::Card,
        parsers::get_all_parsers,
        schema::{
            note::{CreateNoteRequest, CreateNotesRequest},
            review::{RatingSubmission, StudyAction, SubmitStudyActionRequest},
        },
    };
    use chrono::Utc;
    use serde_json::Map;
    use sqlx::SqlitePool;

    /// Creates a note with a single cloze and returns `(note_id, card)`.
    async fn create_single_cloze_note(pool: &SqlitePool, data: &str) -> (NoteId, Card) {
        let parser = create_parser_helper(pool, "markdown").await;
        let res = create_notes(
            pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: data.to_string(),
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
        .await
        .unwrap();
        let note_id = res.notes[0].id;
        let card: Card = sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? AND "order" = 1"#)
            .bind(note_id)
            .fetch_one(pool)
            .await
            .unwrap();
        (note_id, card)
    }

    /// `inh:NOTE_ID/ORDER` on a newly created card copies all SRS fields from the source card.
    #[sqlx::test]
    async fn test_create_note_inherit_copies_srs_data(pool: SqlitePool) {
        // Step 1: Create the source note and give its card non-default SRS data by rating it.
        let (src_note_id, src_card) = create_single_cloze_note(&pool, "{{ source }}").await;
        submit_study_action(
            &pool,
            SubmitStudyActionRequest {
                scheduler_name: "fsrs".to_string(),
                action: StudyAction::Rate(RatingSubmission {
                    card_id: src_card.id,
                    rating: 4,
                    recall_duration: chrono::Duration::seconds(5),
                    rate_duration: chrono::Duration::seconds(2),
                    tag_id: None,
                }),
            },
            Utc::now(),
        )
        .await
        .unwrap();
        let src_card_after_review: Card = sqlx::query_as(r#"SELECT * FROM card WHERE id = ?"#)
            .bind(src_card.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        // Sanity-check: the review must have changed the SRS fields.
        assert_ne!(src_card_after_review.stability, src_card.stability);

        // Step 2: Create a new note whose card inherits from the source card.
        let parser = create_parser_helper(&pool, "markdown").await;
        let inherit_data = format!("{{{{[inh:{src_note_id}/1] destination }}}}");
        let res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: inherit_data,
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
        .await
        .unwrap();
        let dst_note_id = res.notes[0].id;

        // Step 3: Verify the destination card's SRS fields match the source card's.
        let dst_card: Card =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? AND "order" = 1"#)
                .bind(dst_note_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(dst_card.stability, src_card_after_review.stability);
        assert_eq!(dst_card.difficulty, src_card_after_review.difficulty);
        assert_eq!(
            dst_card.desired_retention,
            src_card_after_review.desired_retention
        );
        assert_eq!(dst_card.state, src_card_after_review.state);
        assert_eq!(
            dst_card.due.timestamp(),
            src_card_after_review.due.timestamp()
        );
        assert_eq!(dst_card.special_state, src_card_after_review.special_state);
    }

    /// `inh:` referencing a non-existent card returns an error.
    #[sqlx::test]
    async fn test_create_note_inherit_missing_source_returns_error(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "markdown").await;
        let res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "{{[inh:99999/1] destination }}".to_string(),
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
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("inh:"),
            "expected error mentioning `inh:`, got: {err}"
        );
    }
}
