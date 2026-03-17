use super::{
    AUTOMATIC_REBUILD, create_cards, create_note_keywords, create_note_links, create_note_tags,
    delete_empty_tags, delete_note_files,
};
use crate::{
    Error, LibraryError, ParserErrorKind, TagErrorKind,
    api::{
        card::{create_card_tags, delete_card_tags},
        execute_batched_query, fetch_batched_query,
        note::basic::fetch_note_snapshot,
        parser::get_parser_name,
        placeholders, placeholders_2d,
        tag::{DEFAULT_TAG_AUTO_DELETE, create_tag},
        undo::{
            create_event_group, insert_events,
            payloads::{
                CreateTagPayload, NoteSnapshot, Transition, UpdateNotePayload, UpdateNotesPayload,
            },
        },
    },
    config::{read_internal_config, write_internal_config},
    helpers::remove_ancestor_tags,
    model::{Card, CardId, EventType, Note, NoteId, NoteLink, SpecialState, TagId},
    parsers::{
        CardData, MatchCardsResult, Parseable, add_order_to_note_data,
        extract_and_combine_keywords, find_parser,
        generate_files::{
            GenerateNoteFilesRequest, GenerateNoteFilesRequests, create_note_files_bulk,
        },
        get_cards, match_cards,
    },
    schema::{
        note::{NoteResponse, NotesSelector, UpdateNotesRequest, UpdateNotesResponse, UpdateTags},
        tag::CreateTagRequest,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Utc};
use futures::future::try_join_all;
use itertools::Itertools;
use serde_json::Value;
use sqlx::sqlite::SqlitePool;
use std::collections::{HashMap, HashSet};

async fn update_cards(
    db: &SqlitePool,
    old_cards: &[CardData],
    new_cards: &[CardData],
    note_id: NoteId,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    // Line up cards
    // The card's id in the database cannot change since they are referred to in `review_log`.
    let old_cards_orders = old_cards.iter().map(|x| x.order).collect::<Vec<_>>();
    let new_cards_orders = new_cards.iter().map(|x| x.order).collect::<Vec<_>>();
    let match_cards_result = match_cards(&old_cards_orders, &new_cards_orders)?;
    let MatchCardsResult {
        move_card_indices,
        delete_card_indices,
        create_card_indices,
        same_indices,
    } = match_cards_result;

    // TODO: Only cards in `same_indices` which had their `back_type` or `special_state` updated should be updated below. Most of the time these field won't change, so this is wasteful. These field can be known by comparing the output of `get_cards()` for the old and new note data.

    // Update moved cards (or cards with the same index since their `back_type` or `special_state` might have changed)
    let indices = move_card_indices
        .iter()
        .map(|(x, _)| *x)
        .chain(same_indices.clone())
        .collect::<Vec<_>>();
    let mut moved_cards: Vec<Card> = fetch_batched_query(db, &indices, async |db, chunk| {
        let query_str = format!(
            "SELECT * FROM card WHERE note_id = ? AND \"order\" IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_as(&query_str);
        query = query.bind(note_id);
        for index in chunk {
            query = query.bind(*index as u32);
        }
        query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })
    })
    .await?;

    let move_card_indices_map = move_card_indices
        .into_iter()
        .chain(same_indices.into_iter().map(|i| (i, i)))
        .collect::<HashMap<usize, usize>>();
    for moved_card in &mut moved_cards {
        let to_card_index = move_card_indices_map
            .get(&(moved_card.order as usize))
            .unwrap();
        moved_card.order = *to_card_index as u32;
        let new_card = new_cards.get(to_card_index - 1).unwrap();
        // NOTE: Suspending overwrites a buried card
        if let Some(is_suspended) = new_card.is_suspended {
            if is_suspended {
                moved_card.special_state = Some(SpecialState::Suspended);
            } else if matches!(moved_card.special_state, Some(SpecialState::Suspended)) {
                moved_card.special_state = None;
            }
        }
        moved_card.back_type = new_card.back_type;
        moved_card.updated_at = at;
        let _update_card_result =
            sqlx::query(r#"UPDATE card SET "order" = ?, back_type = ?, special_state = ?, updated_at = ? WHERE id = ?"#)
                .bind(moved_card.order)
                .bind(moved_card.back_type)
                .bind(moved_card.special_state)
                .bind(moved_card.updated_at.timestamp())
                .bind(moved_card.id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
    }

    // Delete cards
    execute_batched_query(db, &delete_card_indices, async |db, chunk| {
        let query_str = format!(
            "DELETE FROM card WHERE note_id = ? AND \"order\" IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query(query_str.as_str());
        query = query.bind(note_id);
        for card_index in chunk {
            query = query.bind(*card_index as u32);
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await;

    // Create new cards
    let new_cards = create_card_indices
        .into_iter()
        .map(|i| {
            let new_card = new_cards.get(i - 1).unwrap();
            let mut card = Card::new(at);
            card.note_id = note_id;
            card.order = i as u32;
            if new_card.is_suspended.unwrap_or(false) {
                card.special_state = Some(SpecialState::Suspended);
            }
            card.back_type = new_card.back_type;
            card
        })
        .collect::<Vec<_>>();
    create_cards(db, &new_cards).await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn update_tags(
    db: &SqlitePool,
    tags: &UpdateTags,
    note_id: NoteId,
) -> Result<Vec<CreateTagPayload>, Error> {
    let mut new_tag_payloads: Vec<CreateTagPayload> = Vec::new();
    // Validate tags do not contain filtered tags
    let existing_filtered_tags_names: Vec<String> =
        sqlx::query_scalar(r"SELECT name FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    let remove_all_tags = matches!(tags, UpdateTags::SetTags(_));
    let (tags_to_remove, tags_to_add) = match tags {
        UpdateTags::ModifyTags {
            tags_to_remove,
            tags_to_add,
        } => (tags_to_remove, tags_to_add),
        UpdateTags::SetTags(items) => (&None, &Some(items.clone())),
        UpdateTags::None => (&None, &None),
    };

    if let Some(tags_to_remove) = tags_to_remove
        && let Some(filtered_tag) = tags_to_remove
            .iter()
            .find(|t| existing_filtered_tags_names.contains(t))
    {
        return Err(Error::Library(LibraryError::Tag(
            TagErrorKind::InvalidInput(format!(
                "Cannot manually remove filtered tag `{}`. Filtered tags are dynamically assigned.",
                filtered_tag
            )),
        )));
    }
    // Remove tags
    let mut tags_to_check = Vec::new();
    if remove_all_tags {
        // Get tags for the note that have `auto_delete` enabled
        let tag_ids: Vec<TagId> = sqlx::query_scalar(r"SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.auto_delete = 1")
                .bind(note_id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        tags_to_check.extend(tag_ids);

        // Remove all tags
        let _delete_note_tag_result = sqlx::query(r"DELETE FROM note_tag WHERE note_id = ?")
            .bind(note_id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    } else if let Some(tags_to_remove) = tags_to_remove
        && !tags_to_remove.is_empty()
    {
        // Get tags for the note that have `auto_delete` enabled
        let tags: Vec<TagId> =
        fetch_batched_query(db, tags_to_remove, async |db, chunk| {
            let query_str = format!(
                "SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.name in ({}) AND t.auto_delete = 1",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_scalar(&query_str);
            query = query.bind(note_id);
            for tag_name in chunk {
                query = query.bind(tag_name);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
        tags_to_check.extend(tags);

        execute_batched_query(db, tags_to_remove, async |db, chunk| {
            let query_str = format!(
                "DELETE FROM note_tag WHERE tag_id IN (SELECT id FROM tag WHERE name IN ({}))",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query(query_str.as_str());
            for tag_name in chunk {
                query = query.bind(tag_name);
            }
            query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            Ok(())
        })
        .await?;
    }
    // Delete tags with no more notes
    delete_empty_tags(db, &tags_to_check).await?;

    if let Some(tags_to_add) = tags_to_add {
        let tags_to_add = &remove_ancestor_tags(tags_to_add);
        if let Some(filtered_tag) = tags_to_add
            .iter()
            .find(|t| existing_filtered_tags_names.contains(t))
        {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput(format!(
                    "Cannot manually add filtered tag `{}`. Filtered tags are dynamically assigned.",
                    filtered_tag
                )),
            )));
        }

        // Add tags
        // Determine new tags
        let tags_info: Vec<(TagId, String)> =
            fetch_batched_query(db, tags_to_add, async |db, chunk| {
                let query_str = format!(
                    "SELECT id, name FROM tag WHERE name IN ({})",
                    placeholders(chunk.len())
                );
                let mut query = sqlx::query_as(query_str.as_str());
                for tag_name in chunk {
                    query = query.bind(tag_name);
                }
                query
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })
            })
            .await?;
        let mut new_tag_ids: Vec<i64> = tags_info.iter().map(|(x, _)| *x).collect::<Vec<_>>();
        let existing_tag_names = tags_info
            .iter()
            .map(|x| x.1.clone())
            .collect::<HashSet<_>>();

        let new_tags = tags_to_add
            .iter()
            .filter(|tag_name| !existing_tag_names.contains(tag_name.as_str()))
            .collect::<Vec<_>>();

        // Create new tags
        let new_tag_names: Vec<String> = new_tags.iter().map(|s| (*s).clone()).collect();
        let tag_responses = try_join_all(
            new_tags
                .into_iter()
                .map(|tag| {
                    create_tag(
                        db,
                        CreateTagRequest {
                            name: (*tag).clone(),
                            description: String::new(),
                            query: None,
                            auto_delete: DEFAULT_TAG_AUTO_DELETE,
                        },
                        false,
                    )
                })
                .collect::<Vec<_>>(),
        )
        .await?;
        new_tag_payloads.extend(new_tag_names.into_iter().zip(tag_responses.iter()).map(
            |(name, resp)| CreateTagPayload {
                id: Some(resp.id),
                name,
                description: String::new(),
                query: None,
                auto_delete: DEFAULT_TAG_AUTO_DELETE,
            },
        ));
        new_tag_ids.extend(tag_responses.into_iter().map(|r| r.id).collect::<Vec<_>>());

        // Add these tags
        execute_batched_query(db, &new_tag_ids, async |db, chunk| {
            let query_str = format!(
                "INSERT INTO note_tag (note_id, tag_id) VALUES {}",
                placeholders_2d(chunk.len(), 2)
            );
            let mut query = sqlx::query(query_str.as_str());
            for tag_id in chunk {
                query = query.bind(note_id);
                query = query.bind(tag_id);
            }
            query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            Ok(())
        })
        .await?;
    }
    Ok(new_tag_payloads)
}

async fn update_note_links(
    db: &SqlitePool,
    note_id: NoteId,
    new_parser: &dyn Parseable,
    new_data: &str,
) -> Result<(), Error> {
    // Get old linked notes from note_link table
    let old_note_links: Vec<NoteLink> =
        sqlx::query_as(r#"SELECT * FROM note_link WHERE parent_note_id = ? ORDER BY "order""#)
            .bind(note_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    // Reparse linked notes from new note data
    let new_linked_note_ranges = new_parser
        .get_linked_notes(new_data)
        .map_err(Error::Library)?;

    // Create a hashmap from old note links: searched_keyword -> (matched_keyword, linked_note_id, score)
    let old_note_links_map: HashMap<String, NoteLink> = old_note_links
        .iter()
        .map(|nl| (nl.searched_keyword.clone(), nl.clone()))
        .collect();

    // Check if old and new linked notes match up exactly by (order, searched_keyword)
    let mut links_match_exactly = old_note_links.len() == new_linked_note_ranges.len();
    if links_match_exactly {
        for (i, range) in new_linked_note_ranges.iter().enumerate() {
            let new_searched_keyword = new_data[range.clone()].to_string();
            if let Some(old_link) = old_note_links.get(i) {
                if old_link.searched_keyword != new_searched_keyword {
                    links_match_exactly = false;
                    break;
                }
            } else {
                links_match_exactly = false;
                break;
            }
        }
    }

    if !links_match_exactly {
        // Delete all linked notes for this note
        let _delete_result = sqlx::query(r"DELETE FROM note_link WHERE parent_note_id = ?")
            .bind(note_id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Create new note links, preserving matched info where possible
        let new_note_links: Vec<NoteLink> = new_linked_note_ranges
            .into_iter()
            .enumerate()
            .map(|(i, range)| {
                let searched_keyword = new_data[range].to_string();
                // Try to find matching info from old note links by searched_keyword
                let nl_opt = old_note_links_map.get(&searched_keyword);

                NoteLink {
                    parent_note_id: note_id,
                    linked_note_id: nl_opt.map(|nl| nl.linked_note_id).unwrap_or_default(),
                    order: i as u32,
                    searched_keyword,
                    matched_keyword: nl_opt
                        .map(|nl| nl.matched_keyword.clone())
                        .unwrap_or_default(),
                    score: nl_opt.map(|nl| nl.score).unwrap_or_default(),
                }
            })
            .collect();

        // Insert all new linked note ids
        if !new_note_links.is_empty() {
            create_note_links(db, &new_note_links).await?;
        }
    }

    Ok(())
}

fn get_parser_and_cards(
    parser_rows: &[(i64, String)],
    parser_id: i64,
    note_data: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(Box<dyn Parseable>, Vec<CardData>), Error> {
    let (_, parser_name) =
        parser_rows
            .iter()
            .find(|row| row.0 == parser_id)
            .ok_or(Error::Library(LibraryError::Parser(
                ParserErrorKind::NotFound(String::new()),
            )))?;
    let parser = find_parser(parser_name.as_str(), all_parsers)?;
    let cards = get_cards(parser.as_ref(), None, note_data, false, true)?;
    Ok((parser, cards))
}

#[allow(clippy::too_many_lines)]
pub async fn update_notes(
    db: &SqlitePool,
    body: UpdateNotesRequest,
    at: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
    log: bool,
) -> Result<UpdateNotesResponse, Error> {
    let mut note_responses = Vec::new();
    // Destructuring is used so if the struct is ever updated, the compiler will warn us to make the appropriate changes here.
    let UpdateNotesRequest {
        selector,
        parser_id,
        data,
        keywords,
        tags,
        custom_data,
    } = body;

    let mut parse_note_requests = Vec::new();
    let mut update_note_payloads: Vec<UpdateNotePayload> = Vec::new();
    let mut new_tag_payloads: Vec<CreateTagPayload> = Vec::new();
    let note_ids = selector.to_note_ids(db).await?;
    for note_id in &note_ids {
        let existing_note: Note = sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
            .bind(note_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Capture before-snapshot for undo logging
        let before_snapshot: Option<NoteSnapshot> = if log {
            let snap = fetch_note_snapshot(
                db,
                *note_id,
                &existing_note.data,
                existing_note.created_at,
                existing_note.parser_id,
                &existing_note.custom_data,
            )
            .await?;
            Some(snap)
        } else {
            None
        };
        // Get new values (if empty, use old value)
        let submitted_new_data = data.as_ref().unwrap_or(&existing_note.data).clone();
        let new_parser_id = parser_id.unwrap_or(existing_note.parser_id);
        let new_custom_data = custom_data
            .clone()
            .map(Value::Object)
            .unwrap_or(existing_note.custom_data);

        // Get existing extra keywords if not updating
        let new_extra_keywords: Vec<String> = if let Some(ref ks) = keywords {
            ks.clone()
        } else {
            let keywords: Vec<String> = sqlx::query_scalar(
                r"SELECT keyword FROM note_keyword WHERE note_id = ? AND embedded = 0",
            )
            .bind(note_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
            keywords
        };

        // Get parsers and cards
        let parser_rows: Vec<(i64, String)> =
            sqlx::query_as(r"SELECT id, name FROM parser WHERE id IN (?, ?)")
                .bind(existing_note.parser_id)
                .bind(new_parser_id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        let (old_parser, old_cards) = get_parser_and_cards(
            &parser_rows,
            existing_note.parser_id,
            existing_note.data.as_str(),
            all_parsers,
        )?;
        let (new_parser, new_cards) = get_parser_and_cards(
            &parser_rows,
            new_parser_id,
            submitted_new_data.as_str(),
            all_parsers,
        )?;

        // TODO: PERF: `add_order_to_note_data()` calls `get_cards()` and so does `get_parser_and_cards()`. There should be a way to modify the `get_cards()` function itself to return the old indices while also updating the note data with the new indices
        // Update note, adding orders sequentially
        let (new_data, _) =
            add_order_to_note_data(new_parser.as_ref(), submitted_new_data.as_str())?;
        let created_at: i64 =
        sqlx::query_scalar(r"UPDATE note SET data = ?, parser_id = ?, custom_data = ?, updated_at = ? WHERE id = ? RETURNING created_at")
            .bind(&new_data)
            .bind(new_parser_id)
            .bind(&new_custom_data)
            .bind(at.timestamp())
            .bind(note_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Update keywords
        let all_keywords = extract_and_combine_keywords(
            new_parser.as_ref(),
            new_data.as_str(),
            &new_extra_keywords,
        )
        .map_err(Error::Library)?;
        // NOTE: All keywords must be updated. Suppose a note had an extra keyword A and no embedded keywords. Suppose this note is updated with no new extra keywords, but an extra embedded keyword of A. Then, the extra keyword of A should be deleted and converted to an embedded keyword.
        sqlx::query(r"DELETE FROM note_keyword WHERE note_id = ?")
            .bind(note_id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Insert new keywords
        create_note_keywords(db, &[(*note_id, all_keywords.clone())]).await?;

        let created_at = DateTime::from_timestamp(created_at, 0).unwrap();
        let updated_at = at;
        let updated_note = Note {
            id: *note_id,
            data: new_data.clone(),
            created_at,
            updated_at,
            parser_id: new_parser_id,
            custom_data: new_custom_data.clone(),
        };
        update_cards(db, &old_cards, &new_cards, *note_id, at).await?;

        new_tag_payloads.extend(update_tags(db, &tags, *note_id).await?);

        // Update note links
        update_note_links(db, *note_id, new_parser.as_ref(), new_data.as_str()).await?;

        let tags: Vec<String> = sqlx::query_scalar(r"SELECT name FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.query IS NULL ORDER BY name ASC")
            .bind(note_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        note_responses.push(NoteResponse::new(
            &updated_note,
            all_keywords
                .iter()
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>(),
            tags.clone(),
            None,
            new_cards.len(),
        ));

        // Delete old generated files, if the parser changed
        if parser_id.is_some() && existing_note.parser_id != updated_note.parser_id {
            let card_orders = old_cards
                .iter()
                .map(|card| card.order.unwrap())
                .collect::<Vec<_>>();
            delete_note_files(
                old_parser.as_ref(),
                *note_id,
                &card_orders,
                existing_note.data.as_str(),
            )?;
        }

        // Parse note
        let parse_note_request = GenerateNoteFilesRequest {
            note_id: updated_note.id,
            note_data: updated_note.data.clone(),
            keywords: all_keywords
                .into_iter()
                .filter(|(_, embedded)| !embedded)
                .map(|(k, _)| k)
                .collect::<Vec<_>>(),
            linked_notes: None, // This is expensive so only done in `render_notes()`,
            custom_data: updated_note.custom_data.as_object().unwrap().clone(),
            tags,
        };
        parse_note_requests.push((updated_note.parser_id, parse_note_request));

        // Build UpdateNotePayload for undo logging
        if log && let Some(before) = before_snapshot {
            let after_snapshot = fetch_note_snapshot(
                db,
                *note_id,
                &updated_note.data,
                updated_note.created_at,
                updated_note.parser_id,
                &updated_note.custom_data,
            )
            .await?;

            let data_transition = if before.data == after_snapshot.data {
                None
            } else {
                Some(Transition {
                    before: before.data.clone(),
                    after: after_snapshot.data.clone(),
                })
            };
            let parser_id_transition = if before.parser_id == after_snapshot.parser_id {
                None
            } else {
                Some(Transition {
                    before: before.parser_id,
                    after: after_snapshot.parser_id,
                })
            };
            let keywords_transition = if before.keywords == after_snapshot.keywords {
                None
            } else {
                Some(Transition {
                    before: before.keywords.clone(),
                    after: after_snapshot.keywords.clone(),
                })
            };
            let tags_transition = if before.tags == after_snapshot.tags {
                None
            } else {
                Some(Transition {
                    before: before.tags.clone(),
                    after: after_snapshot.tags.clone(),
                })
            };
            let custom_data_transition = if before.custom_data == after_snapshot.custom_data {
                None
            } else {
                Some(Transition {
                    before: before.custom_data.clone(),
                    after: after_snapshot.custom_data.clone(),
                })
            };
            let cards_transition = if before.cards == after_snapshot.cards {
                None
            } else {
                Some(Transition {
                    before: before.cards.clone(),
                    after: after_snapshot.cards.clone(),
                })
            };

            update_note_payloads.push(UpdateNotePayload {
                id: *note_id,
                data: data_transition,
                parser_id: parser_id_transition,
                keywords: keywords_transition,
                tags: tags_transition,
                custom_data: custom_data_transition,
                cards: cards_transition,
            });
        }
    }

    if AUTOMATIC_REBUILD {
        // Add/ Remove notes from matched filtered tags
        // This must be done after creating other note tags and creating cards since that impacts if the note matches a query.
        // Find all tags with queries
        let existing_filtered_tags: Vec<(TagId, String)> =
            sqlx::query_as(r"SELECT id, query FROM tag WHERE query IS NOT NULL")
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        // Get card ids from the note.id
        let created_card_ids: Vec<CardId> =
            fetch_batched_query(db, &note_responses, async |db, chunk| {
                let query_str = format!(
                    "SELECT id FROM cards WHERE note_id IN ({})",
                    placeholders(chunk.len())
                );
                let mut query = sqlx::query_scalar(query_str.as_str());
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
        let mut delete_card_tag_entries = Vec::new();
        for (tag_id, query) in existing_filtered_tags {
            // Reexecute query to see if this card matches
            let evaluator = Evaluator::new(query.as_str());
            let search_card_ids = evaluator.get_card_ids(db).await?;
            let (card_ids_to_add_tag, card_ids_to_remove_tag): (Vec<_>, Vec<_>) = created_card_ids
                .iter()
                .map(|card_id| (*card_id, tag_id))
                .partition(|(card_id, _)| search_card_ids.contains(card_id));
            // Check for existing card-tag relationships to avoid duplicates
            let existing_card_tags: Vec<(CardId, TagId)> =
                fetch_batched_query(db, &created_card_ids, async |db, chunk| {
                    let query_str = format!(
                        "SELECT card_id, tag_id FROM card_tag WHERE card_id IN ({}) AND tag_id = ?",
                        placeholders(chunk.len())
                    );
                    let mut query = sqlx::query_as(query_str.as_str());
                    for card_id in chunk {
                        query = query.bind(card_id);
                    }
                    query
                        .bind(tag_id)
                        .fetch_all(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })
                })
                .await?;
            let existing_card_tags_set: HashSet<(CardId, TagId)> =
                existing_card_tags.into_iter().collect();
            let card_ids_to_add_tag: Vec<(CardId, TagId)> = card_ids_to_add_tag
                .into_iter()
                .filter(|entry| !existing_card_tags_set.contains(entry))
                .collect();
            card_filtered_tag_entries.extend(card_ids_to_add_tag);
            delete_card_tag_entries.extend(card_ids_to_remove_tag);
        }
        create_card_tags(db, &card_filtered_tag_entries).await?;
        delete_card_tags(db, &delete_card_tag_entries).await?;
    }

    // Get parser
    for (parser_id, requests) in parse_note_requests.into_iter().into_group_map() {
        let parser_name = get_parser_name(db, parser_id).await?;
        let parser = find_parser(parser_name.as_str(), all_parsers)?;

        // Update note and card files, without compiling
        // This will also ensure that updated notes will have their clozes renumbered sequentially so the note is ready to be edited again.
        let parse_notes_request = GenerateNoteFilesRequests {
            requests,
            overridden_output_raw_dir: None,
            include_cards: true,
            render: false,
            force_render: false,
        };
        let _card_paths = create_note_files_bulk(parser.as_ref(), &parse_notes_request)?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
    }

    // Update config
    let mut config = read_internal_config()?;
    config.linked_notes_generated = false;
    write_internal_config(&config)?;

    // Log event
    let event_id = if log && !update_note_payloads.is_empty() {
        let payload = UpdateNotesPayload {
            notes: update_note_payloads,
        };
        let note_event = (
            EventType::UpdateNotes,
            serde_json::to_value(&payload).unwrap(),
        );
        if new_tag_payloads.is_empty() {
            let event_ids = insert_events(db, &[note_event], at, None).await?;
            Some(*event_ids.first().unwrap())
        } else {
            let mut events: Vec<(EventType, Value)> = new_tag_payloads
                .into_iter()
                .map(|p| (EventType::CreateTag, serde_json::to_value(&p).unwrap()))
                .collect();
            events.push(note_event);
            let group_id = create_event_group(db, events, at).await?;
            Some(group_id)
        }
    } else {
        None
    };

    Ok(UpdateNotesResponse {
        notes: note_responses,
        event_id,
    })
}

/// Apply an `UpdateNotes` event payload (used when undoing `UpdateNotes`).
/// For each note, applies the `after` field values from each transition.
#[allow(clippy::too_many_lines)]
pub(crate) async fn update_notes_event(
    db: &SqlitePool,
    payload: UpdateNotesPayload,
    log: bool,
) -> Result<(), Error> {
    let at = chrono::Utc::now();
    for note_payload in &payload.notes {
        let UpdateNotePayload {
            id,
            data,
            parser_id,
            keywords,
            tags,
            custom_data,
            cards,
        } = note_payload;

        // Build UPDATE note SQL for scalar fields if any changed
        let new_data: Option<&str> = data.as_ref().map(|t| t.after.as_str());
        let new_parser_id: Option<i64> = parser_id.as_ref().map(|t| t.after);
        let new_custom_data: Option<&Value> = custom_data.as_ref().map(|t| &t.after);

        if new_data.is_some() || new_parser_id.is_some() || new_custom_data.is_some() {
            // Fetch existing note to fill in unchanged fields
            let existing_note: Note = sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
                .bind(id)
                .fetch_one(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let data_to_set = new_data.unwrap_or(existing_note.data.as_str());
            let parser_id_to_set = new_parser_id.unwrap_or(existing_note.parser_id);
            let custom_data_to_set = new_custom_data.unwrap_or(&existing_note.custom_data);
            sqlx::query(
                r"UPDATE note SET data = ?, parser_id = ?, custom_data = ?, updated_at = ? WHERE id = ?",
            )
            .bind(data_to_set)
            .bind(parser_id_to_set)
            .bind(custom_data_to_set)
            .bind(at.timestamp())
            .bind(id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        }

        // Update keywords
        if let Some(kw_transition) = keywords {
            sqlx::query(r"DELETE FROM note_keyword WHERE note_id = ?")
                .bind(id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let kw_entries: Vec<(NoteId, Vec<(String, bool)>)> = vec![(
                *id,
                kw_transition
                    .after
                    .iter()
                    .map(|k| (k.clone(), false))
                    .collect(),
            )];
            create_note_keywords(db, &kw_entries).await?;
        }

        // Update tags
        if let Some(tags_transition) = tags {
            // Get tags with auto_delete before removing them
            let tag_ids_to_check: Vec<TagId> =
                sqlx::query_scalar(r"SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.auto_delete = 1")
                    .bind(id)
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;

            // Remove all note_tags
            sqlx::query(r"DELETE FROM note_tag WHERE note_id = ?")
                .bind(id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

            // Delete empty auto_delete tags
            delete_empty_tags(db, &tag_ids_to_check).await?;

            // Re-add tags from the `after` list (create if needed, do NOT log)
            let mut new_tag_ids: Vec<i64> = Vec::new();
            for tag_name in &tags_transition.after {
                let existing_tag_id: Option<i64> =
                    sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ? LIMIT 1")
                        .bind(tag_name)
                        .fetch_optional(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                let tag_id = if let Some(tid) = existing_tag_id {
                    tid
                } else {
                    let tag_response = create_tag(
                        db,
                        crate::schema::tag::CreateTagRequest {
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
                new_tag_ids.push(tag_id);
            }
            create_note_tags(
                db,
                &new_tag_ids
                    .into_iter()
                    .map(|tid| (*id, tid))
                    .collect::<Vec<_>>(),
            )
            .await?;
        }

        // Restore cards
        if let Some(cards_transition) = cards {
            // Delete all existing cards
            sqlx::query(r"DELETE FROM card WHERE note_id = ?")
                .bind(id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

            // Re-insert cards from `after` with specific IDs
            let card_snapshots = &cards_transition.after;
            if !card_snapshots.is_empty() {
                execute_batched_query(db, card_snapshots, async |db, chunk| {
                    let query_str = format!(
                        "INSERT INTO card (id, note_id, \"order\", back_type, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data) VALUES {}",
                        crate::api::placeholders_2d(chunk.len(), 12)
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
    }

    if log {
        insert_events(
            db,
            &[(
                crate::model::EventType::UpdateNotes,
                serde_json::to_value(&payload).unwrap(),
            )],
            at,
            None,
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        api::{
            note::{create_notes, update_notes},
            parser::tests::create_parser_helper,
            review::submit_study_action,
        },
        model::{Card, SpecialState},
        parsers::{BackType, get_all_parsers},
        schema::{
            note::{
                CreateNoteRequest, CreateNotesRequest, NotesSelector, UpdateNotesRequest,
                UpdateNotesResponse, UpdateTags,
            },
            review::{RatingSubmission, StudyAction, SubmitStudyActionRequest},
        },
    };
    use chrono::{Duration, Utc};
    use indoc::indoc;
    use serde_json::{Map, Value};
    use sqlx::SqlitePool;

    #[sqlx::test]
    async fn test_update_note_match_cards(pool: SqlitePool) -> () {
        // Tests that:
        // - Cards are updated correctly when the orders are changed/added/removed
        // - Supending, unsuspending, or changing the `back_type` of a card whose order was changed/added updates properly
        //
        // Create note
        let original_note_data: &str = r"
        {{[o:1] First cloze }}
        {{[o:2] Second cloze }}
        {{[o:3;s:] Third cloze }}
        {{[o:4] Fourth cloze }}";
        let create_note_request = CreateNoteRequest {
            data: original_note_data.to_string(),
            keywords: Vec::new(),
            tags: Vec::new(),
            is_suspended: false,
            custom_data: Map::new(),
        };
        let parser = create_parser_helper(&pool, "markdown").await;
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request.clone()],
        };
        let at = Utc::now();
        let create_notes_res = create_notes(&pool, request, at, &get_all_parsers(), false).await;
        assert!(create_notes_res.is_ok());
        let created_notes = create_notes_res.unwrap();
        assert_eq!(created_notes.notes.len(), 1);
        let created_note = created_notes.notes.first().unwrap();

        let cards_res: Result<Vec<Card>, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
                .bind(created_note.id)
                .fetch_all(&pool)
                .await;
        assert!(cards_res.is_ok());
        let cards = cards_res.unwrap();
        assert_eq!(cards.len(), 4);
        // Update the cards after the note is created and copy their index to the `custom_data` field. That way the card can easily be tracked after the note is updated.
        for card in cards {
            let mut custom_data_map = Map::new();
            custom_data_map.insert(
                "original-order".to_string(),
                Value::Number(card.order.into()),
            );
            let custom_data = Value::Object(custom_data_map);
            let _update_card_result =
                sqlx::query(r"UPDATE card SET custom_data = ?, updated_at = ? WHERE id = ?")
                    .bind(custom_data)
                    .bind(at.timestamp())
                    .bind(card.id)
                    .execute(&pool)
                    .await;
        }

        // Update note
        let id = created_note.id;
        let new_note_data: &str = indoc! {r"
        {{[o:1] First cloze }}
        {{[o:3;s:n;f:all;b:a] Third cloze }}
        {{[s:;f:all;b:a] New cloze 1 }}
        {{ New cloze 2 }}
        {{ New cloze 3 }}
        {{[o:2] Second cloze }}"
        };
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![id]),
            data: Some(new_note_data.to_string()),
            parser_id: None,
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(notes_res.is_ok());
        let UpdateNotesResponse { notes, .. } = notes_res.unwrap();
        assert_eq!(notes.len(), 1);
        let note = notes.first().unwrap();
        let new_note_data_with_order: &str = indoc! {r"
        {{[o:1] First cloze }}
        {{[o:2;f:all;b:a] Third cloze }}
        {{[o:3;f:all;b:a] New cloze 1 }}
        {{[o:4] New cloze 2 }}
        {{[o:5] New cloze 3 }}
        {{[o:6] Second cloze }}"
        };
        assert_eq!(note.data, new_note_data_with_order);

        let cards_res: Result<Vec<Card>, sqlx::Error> =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER BY "order""#)
                .bind(note.id)
                .fetch_all(&pool)
                .await;
        assert!(cards_res.is_ok());
        let cards = cards_res.unwrap();
        assert_eq!(cards.len(), 6);

        // Verify the first card is suspended and has its `back_type` updated
        let card = cards.get(1).unwrap();
        assert_eq!(card.special_state, None);
        assert_eq!(card.back_type, BackType::CardFilePath);

        // Verify the second card is suspended and has its `back_type` updated
        let card = cards.get(2).unwrap();
        assert_eq!(card.special_state, Some(SpecialState::Suspended));
        assert_eq!(card.back_type, BackType::CardFilePath);

        let mapping = [
            (1, Some(1)),
            (6, Some(2)),
            (2, Some(3)),
            (3, None),
            (4, None),
            (5, None),
        ];
        for (card_order, original_order_opt) in mapping {
            let card = cards.iter().find(|card| card.order == card_order).unwrap();
            if let Some(original_order) = original_order_opt {
                assert!(card.custom_data.get("original-order").is_some());
                assert_eq!(
                    card.custom_data.get("original-order").unwrap(),
                    &Value::Number(original_order.into())
                );
            } else {
                assert!(card.custom_data.get("original-order").is_none());
            }
        }
    }

    async fn update_note_change_sides_helper(
        pool: &SqlitePool,
        new_settings_string: &str,
    ) -> (Vec<Card>, Vec<Card>, String) {
        // Create note
        let original_note_data: &str = r"Data {{ First cloze }}";
        let create_note_request = CreateNoteRequest {
            data: original_note_data.to_string(),
            keywords: Vec::new(),
            tags: Vec::new(),
            is_suspended: false,
            custom_data: Map::new(),
        };
        let parser = create_parser_helper(&pool, "markdown").await;
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![create_note_request.clone()],
        };
        let create_notes_res =
            create_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(create_notes_res.is_ok());
        let created_notes = create_notes_res.unwrap();
        assert_eq!(created_notes.notes.len(), 1);
        let created_note = created_notes.notes.first().unwrap();

        // Submit review for a card
        let cards_res: Result<Vec<Card>, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE note_id = ? ORDER BY due ASC")
                .bind(&created_note.id)
                .fetch_all(pool)
                .await;
        assert!(cards_res.is_ok());
        let old_cards = cards_res.unwrap();
        let card_to_review = old_cards[0].clone();
        let request = SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id: card_to_review.id,
                rating: 4,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(5),
                tag_id: None,
            }),
        };
        let submit_review_res = submit_study_action(&pool, request, Utc::now()).await;
        assert!(submit_review_res.is_ok());

        // Get cards
        let cards_res: Result<Vec<Card>, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE note_id = ? ORDER BY due ASC")
                .bind(&created_note.id)
                .fetch_all(pool)
                .await;
        assert!(cards_res.is_ok());
        let old_cards = cards_res.unwrap();

        // Check database and verify card is now due later
        let new_card_res: Result<Card, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
                .bind(card_to_review.id)
                .fetch_one(pool)
                .await;
        assert!(new_card_res.is_ok());
        let reviewed_card = new_card_res.unwrap();
        assert!(reviewed_card.due > card_to_review.due);

        // Update note
        let id = created_note.id;
        let new_note_data = format!("Data {{{{[{}] First cloze }}}}", new_settings_string);
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![id]),
            data: Some(new_note_data.clone()),
            parser_id: None,
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers(), false).await;
        assert!(notes_res.is_ok());
        let UpdateNotesResponse { notes, .. } = notes_res.unwrap();
        assert_eq!(notes.len(), 1);
        let note = notes.first().unwrap();

        // Ensure previous card was deleted and a new card was created
        // Get cards
        let cards_res: Result<Vec<Card>, sqlx::Error> =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER BY "order""#)
                .bind(&created_note.id)
                .fetch_all(pool)
                .await;
        assert!(cards_res.is_ok());
        let new_cards = cards_res.unwrap();

        (old_cards, new_cards, note.data.clone())
    }

    #[sqlx::test]
    #[ignore]
    async fn test_update_note_change_to_reverse_only(pool: SqlitePool) -> () {
        let (old_cards, new_cards, _updated_note) =
            update_note_change_sides_helper(&pool, "o:1;ro:").await;
        assert_eq!(old_cards.len(), 1);
        assert_eq!(new_cards.len(), 1);

        // Since the card was changed to reverse only, a new card should be created since these cards aren't correlated.
        // `new_card[0]` should be new, so the due dates should be different.
        assert!(old_cards[0].due != new_cards[0].due);
        assert!(old_cards[0].stability != new_cards[0].stability);
        assert!(old_cards[0].difficulty != new_cards[0].difficulty);

        assert_eq!(new_cards[0].stability, Card::new(Utc::now()).stability);
        assert_eq!(new_cards[0].difficulty, Card::new(Utc::now()).difficulty);
    }

    #[sqlx::test]
    async fn test_update_note_change_to_include_reverse(pool: SqlitePool) -> () {
        let (old_cards, new_cards, updated_note) =
            update_note_change_sides_helper(&pool, "o:1;r:").await;
        assert_eq!(old_cards.len(), 1);
        assert_eq!(new_cards.len(), 2);
        assert_eq!(old_cards[0].order, new_cards[0].order);
        assert_eq!(old_cards[0].updated_at, new_cards[0].updated_at);

        // There should now be 2 orders on that cloze instead of 1.
        assert!(updated_note.contains("o:1,2"));

        // `new_card[1]` should be new
        assert!(new_cards[0].stability != new_cards[1].stability);
        assert!(new_cards[0].difficulty != new_cards[1].difficulty);
        assert_eq!(new_cards[1].stability, Card::new(Utc::now()).stability);
        assert_eq!(new_cards[1].difficulty, Card::new(Utc::now()).difficulty);
    }
}
