use super::{AUTOMATIC_REBUILD, create_note_keywords, delete_note_files};
use crate::{
    Error, LibraryError, ParserErrorKind,
    api::{
        note::basic::fetch_note_snapshot,
        parser::get_parser_name,
        undo::{
            create_event_group, insert_events,
            payloads::{CreateTagPayload, NoteSnapshot, Transition, UpdateNotePayload, UpdateNotesPayload},
        },
    },
    config::{read_internal_config, write_internal_config},
    model::{EventType, Note, NoteId},
    parsers::{
        CardData, Parseable, add_order_to_note_data, extract_and_combine_keywords, find_parser,
        generate_files::{GenerateNoteFilesRequest, GenerateNoteFilesRequests, create_note_files_bulk},
        get_cards,
    },
    schema::note::{NoteResponse, NotesSelector, UpdateNotesRequest, UpdateNotesResponse, UpdateTags},
};
use chrono::{DateTime, Utc};
use itertools::Itertools;
use serde_json::Value;
use sqlx::sqlite::SqlitePool;

mod cards;
mod event;
mod filtered_tags;
mod links;
mod tags;

pub use event::update_notes_event;

fn get_parser_only(
    parser_rows: &[(i64, String)],
    parser_id: i64,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Box<dyn Parseable>, Error> {
    let (_, parser_name) =
        parser_rows
            .iter()
            .find(|row| row.0 == parser_id)
            .ok_or(Error::Library(LibraryError::Parser(
                ParserErrorKind::NotFound(String::new()),
            )))?;
    find_parser(parser_name.as_str(), all_parsers)
}

/// Builds an `UpdateNotePayload` by comparing two snapshots of the same note.
/// All fields that did not change will have their transition set to `None`.
fn build_update_note_payload(
    note_id: NoteId,
    before: &NoteSnapshot,
    after: &NoteSnapshot,
) -> UpdateNotePayload {
    UpdateNotePayload {
        id: note_id,
        data: (before.data != after.data).then(|| Transition {
            before: before.data.clone(),
            after: after.data.clone(),
        }),
        parser_id: (before.parser_id != after.parser_id).then_some(Transition {
            before: before.parser_id,
            after: after.parser_id,
        }),
        keywords: (before.keywords != after.keywords).then(|| Transition {
            before: before.keywords.clone(),
            after: after.keywords.clone(),
        }),
        tags: (before.tags != after.tags).then(|| Transition {
            before: before.tags.clone(),
            after: after.tags.clone(),
        }),
        custom_data: (before.custom_data != after.custom_data).then(|| Transition {
            before: before.custom_data.clone(),
            after: after.custom_data.clone(),
        }),
        cards: (before.cards != after.cards).then(|| Transition {
            before: before.cards.clone(),
            after: after.cards.clone(),
        }),
    }
}

fn get_parser_and_cards(
    parser_rows: &[(i64, String)],
    parser_id: i64,
    note_data: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(Box<dyn Parseable>, Vec<CardData>), Error> {
    let parser = get_parser_only(parser_rows, parser_id, all_parsers)?;
    let cards = get_cards(parser.as_ref(), None, note_data, false, true)?;
    Ok((parser, cards))
}

#[expect(clippy::too_many_lines)]
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
        let new_parser = get_parser_only(&parser_rows, new_parser_id, all_parsers)?;
        // `add_order_to_note_data` renumbers orders sequentially and returns cards with:
        //   - `order`: new sequential positions (what the DB will store)
        //   - `previous_order`: the orders from the submitted text (references to old positions,
        //     used by `match_cards` to reconcile old DB cards with the new layout)
        let (new_data, new_cards) =
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
        cards::update_cards(db, &old_cards, &new_cards, *note_id, at).await?;

        new_tag_payloads.extend(tags::update_tags(db, &tags, *note_id).await?);

        // Update note links
        links::update_note_links(db, *note_id, new_parser.as_ref(), new_data.as_str()).await?;

        let tags: Vec<String> = sqlx::query_scalar(r"SELECT name FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.query IS NULL ORDER BY name ASC")
            .bind(note_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        note_responses.push(NoteResponse::new(
            &updated_note,
            all_keywords
                .iter()
                .map(|(k, _): &(String, bool)| k.clone())
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
            update_note_payloads.push(build_update_note_payload(*note_id, &before, &after_snapshot));
        }
    }

    if AUTOMATIC_REBUILD {
        // Add/Remove notes from matched filtered tags.
        // Must be done after creating other note tags and cards since those affect query matching.
        filtered_tags::rebuild_filtered_tags_for_updated_notes(db, &note_responses).await?;
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

    #[sqlx::test]
    async fn test_update_note_same_indices_skips_unchanged_cards(pool: SqlitePool) -> () {
        // Tests that cards in same_indices whose back_type and is_suspended haven't
        // changed do NOT get their updated_at bumped, while cards that did change do.
        let at = Utc::now();
        let original_note_data: &str = indoc! {r"
        {{[o:1] First cloze }}
        {{[o:2] Second cloze }}
        {{[o:3] Third cloze }}"};
        let parser = create_parser_helper(&pool, "markdown").await;
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: original_note_data.to_string(),
                keywords: Vec::new(),
                tags: Vec::new(),
                is_suspended: false,
                custom_data: Map::new(),
            }],
        };
        let created_notes = create_notes(&pool, request, at, &get_all_parsers(), false)
            .await
            .unwrap();
        let created_note = created_notes.notes.first().unwrap();

        let old_cards: Vec<Card> =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER BY "order""#)
                .bind(created_note.id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(old_cards.len(), 3);

        // Update: card 1 unchanged, card 2 gets back_type changed (f:all;b:a → CardFilePath),
        // card 3 gets suspended (s:)
        let update_at = at + Duration::seconds(1);
        let new_note_data: &str = indoc! {r"
        {{[o:1] First cloze }}
        {{[o:2;f:all;b:a] Second cloze }}
        {{[o:3;s:] Third cloze }}"};
        let request = UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![created_note.id]),
            data: Some(new_note_data.to_string()),
            parser_id: None,
            keywords: None,
            tags: UpdateTags::None,
            custom_data: None,
        };
        update_notes(&pool, request, update_at, &get_all_parsers(), false)
            .await
            .unwrap();

        let new_cards: Vec<Card> =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER BY "order""#)
                .bind(created_note.id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(new_cards.len(), 3);

        // Card 1: unchanged — updated_at must not be bumped
        assert_eq!(new_cards[0].back_type, BackType::NoteFilePath);
        assert_eq!(new_cards[0].special_state, None);
        assert_eq!(new_cards[0].updated_at, old_cards[0].updated_at);

        // Card 2: back_type changed — updated_at must be bumped
        assert_eq!(new_cards[1].back_type, BackType::CardFilePath);
        assert_ne!(new_cards[1].updated_at, old_cards[1].updated_at);

        // Card 3: suspended — updated_at must be bumped
        assert_eq!(new_cards[2].special_state, Some(SpecialState::Suspended));
        assert_ne!(new_cards[2].updated_at, old_cards[2].updated_at);
    }
}
