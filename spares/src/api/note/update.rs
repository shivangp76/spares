use super::{AUTOMATIC_REBUILD, create_cards, delete_empty_tags, delete_note_files};
use crate::{
    Error, LibraryError, ParserErrorKind, TagErrorKind,
    api::{
        card::{create_card_tags, delete_card_tags},
        tag::{DEFAULT_TAG_AUTO_DELETE, create_tag},
    },
    config::{read_internal_config, write_internal_config},
    helpers::parse_list,
    model::{Card, CardId, Note, NoteId, SpecialState, TagId},
    parsers::{
        CardData, MatchCardsResult, Parseable, add_order_to_note_data, find_parser,
        generate_files::{
            GenerateNoteFilesRequest, GenerateNoteFilesRequests, create_note_files_bulk,
        },
        get_cards, match_cards,
    },
    schema::{
        note::{NoteResponse, NotesSelector, UpdateNotesRequest},
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
    } = match_cards_result;

    // Update moved cards
    let moved_cards_query_str = format!(
        "SELECT * FROM card WHERE \"order\" IN ({})",
        vec!["?"; move_card_indices.len()].join(", ")
    );
    let mut query = sqlx::query_as(moved_cards_query_str.as_str());
    for (from_card_index, _to_card_index) in &move_card_indices {
        query = query.bind(*from_card_index as u32);
    }
    let mut moved_cards: Vec<Card> = query
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let move_card_indices_map = move_card_indices
        .into_iter()
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
    let delete_query_str = format!(
        "DELETE FROM card WHERE note_id = ? AND \"order\" IN ({})",
        vec!["?"; delete_card_indices.len()].join(", ")
    );
    let mut query = sqlx::query(delete_query_str.as_str());
    query = query.bind(note_id);
    for card_index in &delete_card_indices {
        query = query.bind(*card_index as u32);
    }
    let _delete_cards_result = query
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

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
    tags_to_remove: Option<&Vec<String>>,
    tags_to_add: Option<&Vec<String>>,
    note_id: NoteId,
) -> Result<(), Error> {
    // Validate tags do not contain filtered tags
    let existing_filtered_tags: Vec<(String,)> =
        sqlx::query_as(r"SELECT name FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let existing_filtered_tags_names = existing_filtered_tags
        .iter()
        .map(|(x,)| x.as_str())
        .collect::<Vec<_>>();

    if let Some(tags_to_remove) = tags_to_remove {
        if let Some(filtered_tag) = tags_to_remove
            .iter()
            .find(|t| existing_filtered_tags_names.contains(&t.as_str()))
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
        if tags_to_remove.contains(&"*".to_string()) {
            // Get tags for the note that have `auto_delete` enabled
            let tags_tuple: Vec<(TagId,)> = sqlx::query_as(r"SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.auto_delete = 1")
                .bind(note_id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let tag_ids: Vec<TagId> = tags_tuple.into_iter().map(|t| t.0).collect();
            tags_to_check.extend(tag_ids);

            // Remove all tags
            let _delete_note_tag_result = sqlx::query(r"DELETE FROM note_tag WHERE note_id = ?")
                .bind(note_id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        } else if !tags_to_remove.is_empty() {
            // Get tags for the note that have `auto_delete` enabled
            let get_tags_query_str = format!(
                "SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.name in ({}) AND t.auto_delete = 1",
                vec!["?"; tags_to_remove.len()].join(", ")
            );
            let mut query = sqlx::query_as(get_tags_query_str.as_str());
            query = query.bind(note_id);
            for tag_name in tags_to_remove {
                query = query.bind(tag_name);
            }
            let tags_tuple: Vec<(TagId,)> = query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let tag_ids: Vec<TagId> = tags_tuple.into_iter().map(|t| t.0).collect();
            tags_to_check.extend(tag_ids);

            let delete_note_tag_query_str = format!(
                "DELETE FROM note_tag WHERE tag_id IN (SELECT id FROM tag WHERE name IN ({}))",
                vec!["?"; tags_to_remove.len()].join(", ")
            );
            let mut query = sqlx::query(delete_note_tag_query_str.as_str());
            for tag_name in tags_to_remove {
                query = query.bind(tag_name);
            }
            let _delete_tags_res = query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        }
        // Delete tags with no more notes
        delete_empty_tags(db, &tags_to_check).await?;
    }
    if let Some(tags_to_add) = tags_to_add {
        if let Some(filtered_tag) = tags_to_add
            .iter()
            .find(|t| existing_filtered_tags_names.contains(&t.as_str()))
        {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput(format!(
                    "Cannot manually add filtered tag `{}`. Filtered tags are dynamically assigned.",
                    filtered_tag
                )),
            )));
        }

        // Add tags
        let mut new_tag_ids: Vec<i64> = Vec::new();

        // Determine new tags
        let get_tags_str = format!(
            "SELECT id, name FROM tag WHERE name IN ({})",
            vec!["?"; tags_to_add.len()].join(", ")
        );
        let mut query = sqlx::query_as(get_tags_str.as_str());
        for tag_name in tags_to_add {
            query = query.bind(tag_name);
        }
        let tags_info: Vec<(i64, String)> = query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        new_tag_ids.extend(tags_info.iter().map(|x| x.0).collect::<Vec<_>>());
        let existing_tag_names = tags_info.iter().map(|x| x.1.clone()).collect::<Vec<_>>();

        let new_tags = tags_to_add
            .iter()
            .filter(|tag_name| !existing_tag_names.contains(tag_name))
            .collect::<Vec<_>>();

        // Create new tags
        let tag_responses = try_join_all(
            new_tags
                .iter()
                .map(|tag| {
                    create_tag(
                        db,
                        CreateTagRequest {
                            name: (*tag).to_string(),
                            description: String::new(),
                            parent_id: None,
                            query: None,
                            auto_delete: DEFAULT_TAG_AUTO_DELETE,
                        },
                    )
                })
                .collect::<Vec<_>>(),
        )
        .await?;
        new_tag_ids.extend(tag_responses.into_iter().map(|r| r.id).collect::<Vec<_>>());

        // Add these tags
        if !new_tag_ids.is_empty() {
            let insert_note_tag_query_str = format!(
                "INSERT INTO note_tag (note_id, tag_id) VALUES {}",
                vec!["(?, ?)"; new_tag_ids.len()].join(", ")
            );
            let mut query = sqlx::query(insert_note_tag_query_str.as_str());
            for tag_id in &new_tag_ids {
                query = query.bind(note_id);
                query = query.bind(tag_id);
            }
            let _insert_result = query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
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
) -> Result<Vec<NoteResponse>, Error> {
    let mut note_responses = Vec::new();
    // Destructuring is used so if the struct is ever updated, the compiler will warn us to make the appropriate changes here.
    let UpdateNotesRequest {
        selector,
        parser_id,
        data,
        keywords,
        tags_to_remove,
        tags_to_add,
        custom_data,
    } = body;

    let mut parse_note_requests = Vec::new();
    let note_ids = match selector {
        NotesSelector::Ids(vec) => vec,
        NotesSelector::Query(query) => {
            let evaluator = Evaluator::new(&query);
            evaluator.get_note_ids(db).await?
        }
    };
    for note_id in &note_ids {
        let existing_note: Note = sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
            .bind(note_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        // Get new values (if empty, use old value)
        let new_keywords_str = keywords
            .clone()
            .map_or_else(|| existing_note.keywords.clone(), |x| x.join(","));
        let submitted_new_data = data.as_ref().unwrap_or(&existing_note.data).clone();
        let new_parser_id = parser_id.unwrap_or(existing_note.parser_id);
        let new_custom_data = custom_data
            .clone()
            .map(Value::Object)
            .unwrap_or(existing_note.custom_data);

        // Get parsers and cards
        let parser_rows: Vec<(i64, String)> =
            sqlx::query_as(r"SELECT id, name FROM parser WHERE id IN (?, ?)")
                .bind(existing_note.parser_id)
                .bind(new_parser_id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        // NOTE: PERF - `get_cards()` is called 3 times here: once in `get_parser_and_cards()` which is called twice and once in `add_order_to_note_data()`.
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

        // Update note, adding orders sequentially
        let (new_data, _) =
            add_order_to_note_data(new_parser.as_ref(), submitted_new_data.as_str())?;
        let (created_at,): (i64,) =
        sqlx::query_as(r"UPDATE note SET data = ?, keywords = ?, parser_id = ?, custom_data = ?, updated_at = ? WHERE id = ? RETURNING created_at")
            .bind(&new_data)
            .bind(&new_keywords_str)
            .bind(new_parser_id)
            .bind(&new_custom_data)
            .bind(at.timestamp())
            .bind(note_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let created_at = DateTime::from_timestamp(created_at, 0).unwrap();
        let updated_at = at;
        let updated_note = Note {
            id: *note_id,
            data: new_data.clone(),
            keywords: new_keywords_str.clone(),
            created_at,
            updated_at,
            parser_id: new_parser_id,
            custom_data: new_custom_data.clone(),
        };

        update_cards(db, &old_cards, &new_cards, *note_id, at).await?;

        update_tags(db, tags_to_remove.as_ref(), tags_to_add.as_ref(), *note_id).await?;

        // Get all tags without a query
        let tags_tuple: Vec<(String,)> = sqlx::query_as(r"SELECT name FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.query IS NULL ORDER BY name ASC")
            .bind(note_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let tags = tags_tuple.into_iter().map(|t| t.0).collect::<Vec<_>>();
        note_responses.push(NoteResponse::new(
            &updated_note,
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
            note_data: updated_note.data.to_string(),
            keywords: parse_list(updated_note.keywords.as_str()),
            linked_notes: None, // This is expensive so only done in `render_notes()`,
            custom_data: updated_note.custom_data.as_object().unwrap().clone(),
            tags,
        };
        parse_note_requests.push((updated_note.parser_id, parse_note_request));
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
        let query_str = format!(
            "SELECT id FROM cards WHERE note_id IN ({})",
            vec!["?"; note_responses.len()].join(", ")
        );
        let mut query = sqlx::query_as(query_str.as_str());
        for note in &note_responses {
            query = query.bind(note.id);
        }
        let created_card_id_tuples: Vec<(CardId,)> = query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let created_card_ids = created_card_id_tuples
            .into_iter()
            .map(|(x,)| x)
            .collect::<Vec<_>>();
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
            let query_str = format!(
                "SELECT card_id, tag_id FROM card_tag WHERE card_id IN ({}) AND tag_id = ?",
                vec!["?"; created_card_ids.len()].join(", ")
            );
            let mut query = sqlx::query_as(query_str.as_str());
            for card_id in &created_card_ids {
                query = query.bind(card_id);
            }
            let existing_card_tags: Vec<(CardId, TagId)> =
                query
                    .bind(tag_id)
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
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
        let (parser_name,): (String,) = sqlx::query_as(r"SELECT name FROM parser WHERE id = ?")
            .bind(parser_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
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

    Ok(note_responses)
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
            note::{CreateNoteRequest, CreateNotesRequest, NotesSelector, UpdateNotesRequest},
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
        let create_notes_res = create_notes(&pool, request, at, &get_all_parsers()).await;
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
            tags_to_add: None,
            tags_to_remove: None,
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(notes_res.is_ok());
        let notes = notes_res.unwrap();
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
        assert_eq!(card.back_type, BackType::OnlyAnswered);

        // Verify the second card is suspended and has its `back_type` updated
        let card = cards.get(2).unwrap();
        assert_eq!(card.special_state, Some(SpecialState::Suspended));
        assert_eq!(card.back_type, BackType::OnlyAnswered);

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
        let create_notes_res = create_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
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
                duration: Duration::seconds(5),
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
            tags_to_add: None,
            tags_to_remove: None,
            custom_data: None,
        };
        let notes_res = update_notes(&pool, request, Utc::now(), &get_all_parsers()).await;
        assert!(notes_res.is_ok());
        let notes = notes_res.unwrap();
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
