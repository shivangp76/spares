use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use itertools::Itertools;
use serde_json::Value;
use sqlx::sqlite::SqlitePool;

use super::AUTOMATIC_REBUILD;
use super::delete_note_files;
use crate::Error;
use crate::LibraryError;
use crate::ParserErrorKind;
use crate::api::MAX_ROWS_IN_QUERY;
use crate::api::fetch_batched_query;
use crate::api::placeholders;
use crate::api::placeholders_2d;
use crate::api::undo::create_event_group;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::CardSnapshot;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::NoteSnapshot;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateNotePayload;
use crate::api::undo::payloads::UpdateNotesPayload;
use crate::config::read_external_config;
use crate::config::read_internal_config;
use crate::config::write_internal_config;
use crate::model::Card;
use crate::model::EventType;
use crate::model::Note;
use crate::model::NoteId;
use crate::parsers::CardData;
use crate::parsers::Parseable;
use crate::parsers::add_order_to_note_data;
use crate::parsers::extract_and_combine_keywords;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::generate_files::GenerateNoteFilesRequests;
use crate::parsers::generate_files::create_note_files_bulk;
use crate::parsers::get_cards;
use crate::schema::note::NoteResponse;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateNotesResponse;

mod cards;
mod event;
mod filtered_tags;
mod links;
mod tags;

pub use event::update_notes_event;

fn get_parser_only(
    parser_name: &str,
    parser_factories: &HashMap<&'static str, fn() -> Box<dyn Parseable>>,
) -> Result<Box<dyn Parseable>, Error> {
    parser_factories
        .get(parser_name)
        .ok_or(Error::Library(LibraryError::Parser(
            ParserErrorKind::NotFound(parser_name.to_string()),
        )))
        .map(|f| f())
}

/// Per-note state accumulated in Phase 2 and consumed in Phase 3.
/// Fields prefixed with `old_` are derived from the pre-update DB state and are
/// only needed in Phase 3 for delete-note-files and undo-snapshot construction.
struct PendingNote {
    note_id: NoteId,
    updated_note: Note,
    all_keywords: Vec<(String, bool)>, // (keyword, is_embedded)
    card_count: usize,
    old_card_orders: Vec<usize>,
    before_snapshot: Option<NoteSnapshot>,
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
    parser_name: &str,
    note_data: &str,
    parser_factories: &HashMap<&'static str, fn() -> Box<dyn Parseable>>,
) -> Result<(Box<dyn Parseable>, Vec<CardData>), Error> {
    let parser = get_parser_only(parser_name, parser_factories)?;
    let cards = get_cards(parser.as_ref(), None, note_data, false, true)?;
    Ok((parser, cards))
}

/// Followed by file generation, config update, and event logging.
/// The update proceeds in three phases to minimize DB round-trips:
///   - Batch-fetch all reference data (parsers, filtered tags, notes, keywords, tags, cards).
///   - Per-note processing: parse, write note row, update cards/tags/links, accumulate state.
///   - Batched post-pass: bulk-delete old keywords, bulk-insert new ones inside a transaction;
///     batch-fetch current tags/cards for after-snapshots; build responses, parse requests,
///     and undo payloads.
#[expect(clippy::too_many_lines)]
pub async fn update_notes(
    db: &SqlitePool,
    body: UpdateNotesRequest,
    at: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
    log: bool,
) -> Result<UpdateNotesResponse, Error> {
    // Destructuring is used so if the struct is ever updated, the compiler will warn us to make the appropriate changes here.
    let UpdateNotesRequest {
        selector,
        parser_id,
        data,
        keywords,
        tags,
        custom_data,
    } = body;

    let mut note_responses = Vec::new();
    let mut parse_note_requests = Vec::new();
    let mut update_note_payloads: Vec<UpdateNotePayload> = Vec::new();
    let mut new_tag_payloads: Vec<CreateTagPayload> = Vec::new();
    let mut all_note_keyword_entries: Vec<(NoteId, Vec<(String, bool)>)> = Vec::new();

    // ---- Phase 1: Batch pre-fetches ----

    let all_parser_rows: Vec<(i64, String)> = sqlx::query_as(r"SELECT id, name FROM parser")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let parser_name_map: HashMap<i64, &str> = all_parser_rows
        .iter()
        .map(|(id, name)| (*id, name.as_str()))
        .collect();
    // Build a name→factory map so we construct each parser at most once per call.
    let parser_factories: HashMap<&'static str, fn() -> Box<dyn Parseable>> = all_parsers
        .iter()
        .map(|f| ((*f)().get_parser_name(), *f))
        .collect();

    let existing_filtered_tag_names: Vec<String> =
        sqlx::query_scalar(r"SELECT name FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    let external_config = read_external_config().ok();

    let raw_note_ids = selector.to_note_ids(db).await?;
    // Deduplicate to avoid processing the same note twice (which would cause duplicate
    // mutations and spurious undo events). Only one update per note ID is applied.
    let note_ids: Vec<NoteId> = raw_note_ids.into_iter().unique().collect();

    let existing_notes: Vec<Note> = fetch_batched_query(db, &note_ids, async |db, chunk| {
        let query_str = format!(
            "SELECT * FROM note WHERE id IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_as::<_, Note>(&query_str);
        for id in chunk {
            query = query.bind(id);
        }
        query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })
    })
    .await?;
    let notes_map: HashMap<NoteId, Note> = existing_notes.into_iter().map(|n| (n.id, n)).collect();
    for id in &note_ids {
        if !notes_map.contains_key(id) {
            return Err(Error::Sqlx {
                source: sqlx::Error::RowNotFound,
            });
        }
    }

    let need_old_keywords = keywords.is_none() || log;
    let old_keywords_map: HashMap<NoteId, Vec<String>> = if need_old_keywords {
        let rows: Vec<(NoteId, String)> = fetch_batched_query(db, &note_ids, async |db, chunk| {
            let query_str = format!(
                "SELECT note_id, keyword FROM note_keyword \
                     WHERE note_id IN ({}) AND embedded = 0 \
                     ORDER BY note_id, keyword ASC",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as::<_, (NoteId, String)>(&query_str);
            for id in chunk {
                query = query.bind(id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
        let mut map: HashMap<NoteId, Vec<String>> = HashMap::new();
        for (note_id, keyword) in rows {
            map.entry(note_id).or_default().push(keyword);
        }
        map
    } else {
        HashMap::new()
    };

    // Batch-fetch before-snapshots (old tags + old cards) when logging
    let old_tags_map: HashMap<NoteId, Vec<String>> = if log {
        let rows: Vec<(NoteId, String)> = fetch_batched_query(db, &note_ids, async |db, chunk| {
            let query_str = format!(
                "SELECT nt.note_id, t.name FROM tag t \
                     JOIN note_tag nt ON t.id = nt.tag_id \
                     WHERE nt.note_id IN ({}) AND t.query IS NULL \
                     ORDER BY nt.note_id, t.name ASC",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as::<_, (NoteId, String)>(&query_str);
            for id in chunk {
                query = query.bind(id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
        let mut map: HashMap<NoteId, Vec<String>> = HashMap::new();
        for (note_id, name) in rows {
            map.entry(note_id).or_default().push(name);
        }
        map
    } else {
        HashMap::new()
    };

    let old_cards_map: HashMap<NoteId, Vec<Card>> = if log {
        let rows: Vec<Card> = fetch_batched_query(db, &note_ids, async |db, chunk| {
            let query_str = format!(
                "SELECT * FROM card WHERE note_id IN ({}) ORDER BY note_id, \"order\" ASC",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as::<_, Card>(&query_str);
            for id in chunk {
                query = query.bind(id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
        let mut map: HashMap<NoteId, Vec<Card>> = HashMap::new();
        for card in rows {
            map.entry(card.note_id).or_default().push(card);
        }
        map
    } else {
        HashMap::new()
    };

    // ---- Phase 2: Per-note work (parse + update + cards/tags/links) ----

    let mut pending_notes: Vec<PendingNote> = Vec::with_capacity(note_ids.len());

    for note_id in &note_ids {
        let existing_note = &notes_map[note_id];

        let before_snapshot: Option<NoteSnapshot> = if log {
            let mut keywords = old_keywords_map.get(note_id).cloned().unwrap_or_default();
            keywords.sort();
            let tags = old_tags_map.get(note_id).cloned().unwrap_or_default();
            let cards = old_cards_map
                .get(note_id)
                .map(|cards| cards.iter().map(CardSnapshot::from_card).collect())
                .unwrap_or_default();
            Some(NoteSnapshot {
                id: *note_id,
                data: existing_note.data.clone(),
                created_at: existing_note.created_at,
                parser_id: existing_note.parser_id,
                custom_data: existing_note.custom_data.clone(),
                keywords,
                tags,
                cards,
            })
        } else {
            None
        };

        let submitted_new_data = data.as_ref().unwrap_or(&existing_note.data).clone();
        let new_parser_id = parser_id.unwrap_or(existing_note.parser_id);
        let new_custom_data = custom_data
            .as_ref()
            .map(|cd| Value::Object(cd.clone()))
            .unwrap_or(existing_note.custom_data.clone());

        let new_extra_keywords: Vec<String> = if let Some(ref ks) = keywords {
            ks.clone()
        } else {
            old_keywords_map.get(note_id).cloned().unwrap_or_default()
        };

        let old_parser_name = parser_name_map
            .get(&existing_note.parser_id)
            .copied()
            .ok_or(Error::Library(LibraryError::Parser(
                ParserErrorKind::NotFound(String::new()),
            )))?;
        let (_old_parser, old_cards) = get_parser_and_cards(
            old_parser_name,
            existing_note.data.as_str(),
            &parser_factories,
        )?;
        let new_parser_name =
            parser_name_map
                .get(&new_parser_id)
                .copied()
                .ok_or(Error::Library(LibraryError::Parser(
                    ParserErrorKind::NotFound(String::new()),
                )))?;
        let new_parser = get_parser_only(new_parser_name, &parser_factories)?;
        // `add_order_to_note_data` renumbers orders sequentially and returns cards with:
        //   - `order`: new sequential positions (what the DB will store)
        //   - `previous_order`: the orders from the submitted text (references to old positions,
        //     used by `match_cards` to reconcile old DB cards with the new layout)
        let (new_data, new_cards) = add_order_to_note_data(
            new_parser.as_ref(),
            submitted_new_data.as_str(),
            external_config.as_ref().map(|c| &c.overlapper),
        )?;
        let created_at: i64 = sqlx::query_scalar(
            r"UPDATE note SET data = ?, parser_id = ?, custom_data = ?, updated_at = ? WHERE id = ? RETURNING created_at",
        )
        .bind(&new_data)
        .bind(new_parser_id)
        .bind(&new_custom_data)
        .bind(at.timestamp())
        .bind(note_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

        let all_keywords = extract_and_combine_keywords(
            new_parser.as_ref(),
            new_data.as_str(),
            &new_extra_keywords,
        )
        .map_err(Error::Library)?;

        let created_at = DateTime::from_timestamp(created_at, 0).ok_or(Error::Library(
            LibraryError::InvalidConfig("invalid created_at timestamp".to_string()),
        ))?;
        let updated_note = Note {
            id: *note_id,
            data: new_data.clone(),
            created_at,
            updated_at: at,
            parser_id: new_parser_id,
            custom_data: new_custom_data.clone(),
        };
        cards::update_cards(db, &old_cards, &new_cards, *note_id, at).await?;

        new_tag_payloads
            .extend(tags::update_tags(db, &tags, *note_id, &existing_filtered_tag_names).await?);

        links::update_note_links(db, *note_id, new_parser.as_ref(), new_data.as_str()).await?;

        let old_card_orders: Vec<usize> = old_cards.iter().filter_map(|card| card.order).collect();

        all_note_keyword_entries.push((*note_id, all_keywords.clone()));

        pending_notes.push(PendingNote {
            note_id: *note_id,
            updated_note,
            all_keywords,
            card_count: new_cards.len(),
            old_card_orders,
            before_snapshot,
        });
    }

    // ---- Phase 3: Batched post-pass ----

    let all_note_ids: Vec<NoteId> = pending_notes.iter().map(|p| p.note_id).collect();

    // NOTE: All keywords must be updated. Suppose a note had an extra keyword A and no embedded keywords. Suppose this note is updated with no new extra keywords, but an extra embedded keyword of A. Then, the extra keyword of A should be deleted and converted to an embedded keyword.
    // Batch-delete all old keywords, then bulk-insert new ones.
    // Both operations are wrapped in a transaction so that a crash between DELETE and
    // INSERT doesn't permanently lose keywords for all processed notes.
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;
    for chunk in all_note_ids.chunks(MAX_ROWS_IN_QUERY) {
        let query_str = format!(
            "DELETE FROM note_keyword WHERE note_id IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query(&query_str);
        for id in chunk {
            query = query.bind(id);
        }
        query
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    for chunk in all_note_keyword_entries.chunks(MAX_ROWS_IN_QUERY) {
        let kw_rows: Vec<(NoteId, &str, bool)> = chunk
            .iter()
            .flat_map(|(note_id, ks)| ks.iter().map(|(kw, emb)| (*note_id, kw.as_str(), *emb)))
            .collect();
        if kw_rows.is_empty() {
            continue;
        }
        let query_str = format!(
            "INSERT INTO note_keyword (note_id, keyword, embedded) VALUES {}",
            placeholders_2d(kw_rows.len(), 3)
        );
        let mut query = sqlx::query(&query_str);
        for (note_id, keyword, embedded) in &kw_rows {
            query = query.bind(note_id);
            query = query.bind(keyword);
            query = query.bind(i32::from(*embedded));
        }
        query
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;

    // Batch-fetch current tags for all notes
    let current_tags_rows: Vec<(NoteId, String)> =
        fetch_batched_query(db, &all_note_ids, async |db, chunk| {
            let query_str = format!(
                "SELECT nt.note_id, t.name FROM tag t \
                 JOIN note_tag nt ON t.id = nt.tag_id \
                 WHERE nt.note_id IN ({}) AND t.query IS NULL \
                 ORDER BY nt.note_id, t.name ASC",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as::<_, (NoteId, String)>(&query_str);
            for id in chunk {
                query = query.bind(id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
    let current_tags_map: HashMap<NoteId, Vec<String>> = {
        let mut map: HashMap<NoteId, Vec<String>> = HashMap::new();
        for (note_id, name) in current_tags_rows {
            map.entry(note_id).or_default().push(name);
        }
        // ensure every pending note has an entry
        for pn in &pending_notes {
            map.entry(pn.note_id).or_default();
        }
        map
    };

    // Batch-fetch cards for after-snapshots (only if logging)
    let after_cards_map: HashMap<NoteId, Vec<Card>> = if log {
        let rows: Vec<Card> = fetch_batched_query(db, &all_note_ids, async |db, chunk| {
            let query_str = format!(
                "SELECT * FROM card WHERE note_id IN ({}) ORDER BY note_id, \"order\" ASC",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as::<_, Card>(&query_str);
            for id in chunk {
                query = query.bind(id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
        let mut map: HashMap<NoteId, Vec<Card>> = HashMap::new();
        for card in rows {
            map.entry(card.note_id).or_default().push(card);
        }
        map
    } else {
        HashMap::new()
    };

    // Build responses, parse requests, delete old files, and after-snapshots
    for PendingNote {
        note_id,
        updated_note,
        all_keywords,
        card_count,
        old_card_orders,
        before_snapshot,
    } in pending_notes
    {
        let tags = current_tags_map.get(&note_id).cloned().unwrap_or_default();

        note_responses.push(NoteResponse::new(
            &updated_note,
            all_keywords
                .iter()
                .map(|(k, _): &(String, bool)| k.clone())
                .collect::<Vec<_>>(),
            tags.clone(),
            None,
            card_count,
        ));

        // Delete old generated files if the parser changed
        if parser_id.is_some() {
            let old_note = &notes_map[&note_id];
            if old_note.parser_id != updated_note.parser_id {
                let old_parser_name =
                    parser_name_map
                        .get(&old_note.parser_id)
                        .copied()
                        .ok_or(Error::Library(LibraryError::Parser(
                            ParserErrorKind::NotFound(String::new()),
                        )))?;
                let parser = get_parser_only(old_parser_name, &parser_factories)?;
                delete_note_files(
                    parser.as_ref(),
                    note_id,
                    &old_card_orders,
                    old_note.data.as_str(),
                )?;
            }
        }

        // Parse note
        let parse_note_request = GenerateNoteFilesRequest {
            note_id: updated_note.id,
            note_data: updated_note.data.clone(),
            keywords: all_keywords
                .iter()
                .filter(|(_, embedded)| !embedded)
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>(),
            linked_notes: None, // This is expensive so only done in `render_notes()`.
            custom_data: updated_note
                .custom_data
                .as_object()
                .cloned()
                .unwrap_or_default(),
            tags,
        };
        parse_note_requests.push((updated_note.parser_id, parse_note_request));

        // Build UpdateNotePayload for undo logging
        if let Some(before) = before_snapshot {
            let after_keywords: Vec<String> = all_keywords
                .iter()
                .filter(|(_, embedded)| !embedded)
                .map(|(k, _)| k.clone())
                .sorted()
                .collect();
            let after_tags = current_tags_map.get(&note_id).cloned().unwrap_or_default();
            let after_cards = after_cards_map
                .get(&note_id)
                .map(|cards| cards.iter().map(CardSnapshot::from_card).collect())
                .unwrap_or_default();

            let after_snapshot = NoteSnapshot {
                id: note_id,
                data: updated_note.data.clone(),
                created_at: updated_note.created_at,
                parser_id: updated_note.parser_id,
                custom_data: updated_note.custom_data.clone(),
                keywords: after_keywords,
                tags: after_tags,
                cards: after_cards,
            };

            let payload = build_update_note_payload(note_id, &before, &after_snapshot);
            if payload.has_changes() {
                update_note_payloads.push(payload);
            }
        }
    }

    // ---- File generation and config ----

    let has_changed_notes = !parse_note_requests.is_empty();

    if AUTOMATIC_REBUILD {
        // Add/Remove notes from matched filtered tags.
        // Must be done after creating other note tags and cards since those affect query matching.
        filtered_tags::rebuild_filtered_tags_for_updated_notes(db, &note_responses).await?;
    }

    for (pid, requests) in parse_note_requests.into_iter().into_group_map() {
        let parser_name =
            parser_name_map
                .get(&pid)
                .copied()
                .ok_or(Error::Library(LibraryError::Parser(
                    ParserErrorKind::NotFound(String::new()),
                )))?;
        let parser = get_parser_only(parser_name, &parser_factories)?;

        // Update note and card files, without compiling. This will also ensure that updated notes
        // will have their clozes renumbered sequentially so the note is ready to be edited again.
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

    if has_changed_notes {
        let mut config = read_internal_config(db).await?;
        config.linked_notes_generated = false;
        write_internal_config(db, &config).await?;
    }

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
    use chrono::Duration;
    use chrono::Utc;
    use indoc::indoc;
    use serde_json::Map;
    use serde_json::Value;
    use sqlx::SqlitePool;

    use crate::api::note::create_notes;
    use crate::api::note::update_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::review::submit_study_action;
    use crate::model::Card;
    use crate::model::ReviewLog;
    use crate::model::SpecialState;
    use crate::parsers::BackType;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::note::NotesSelector;
    use crate::schema::note::UpdateNotesRequest;
    use crate::schema::note::UpdateNotesResponse;
    use crate::schema::note::UpdateTags;
    use crate::schema::review::RatingSubmission;
    use crate::schema::review::StudyAction;
    use crate::schema::review::SubmitStudyActionRequest;

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

    /// `inh:NOTE_ID/ORDER` on a card added via note update copies SRS fields from the source card,
    /// and the stored note data must not contain `inh:` after the update.
    #[sqlx::test]
    async fn test_update_note_inherit_copies_srs_data(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "markdown").await;

        // Step 1: Create source note with one card and rate it so it has non-default SRS data.
        let create_res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "{{ source card }}".to_string(),
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
        let src_note_id = create_res.notes[0].id;
        let src_card: Card =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? AND "order" = 1"#)
                .bind(src_note_id)
                .fetch_one(&pool)
                .await
                .unwrap();

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
        assert_ne!(src_card_after_review.stability, src_card.stability);

        // Step 2: Create the destination note with one card.
        let dst_create_res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "{{ destination card 1 }}".to_string(),
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
        let dst_note_id = dst_create_res.notes[0].id;

        // Step 3: Update the destination note to add a second cloze with `inh:`.
        let new_data = format!(
            "{{{{[o:1] destination card 1 }}}}{{{{[inh:{src_note_id}/1] destination card 2 }}}}"
        );
        let update_res = update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![dst_note_id]),
                data: Some(new_data),
                parser_id: None,
                keywords: None,
                tags: UpdateTags::None,
                custom_data: None,
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();

        // Step 4: Verify stored note data does NOT contain `inh:`.
        let stored_data = &update_res.notes[0].data;
        assert!(
            !stored_data.contains("inh:"),
            "stored note data must not contain `inh:`, got: {stored_data}"
        );

        // Step 5: Verify the new card's SRS fields match the source card's.
        let dst_card2: Card =
            sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? AND "order" = 2"#)
                .bind(dst_note_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(dst_card2.stability, src_card_after_review.stability);
        assert_eq!(dst_card2.difficulty, src_card_after_review.difficulty);
        assert_eq!(
            dst_card2.desired_retention,
            src_card_after_review.desired_retention
        );
        assert_eq!(dst_card2.state, src_card_after_review.state);
        assert_eq!(
            dst_card2.due.timestamp(),
            src_card_after_review.due.timestamp()
        );
        assert_eq!(dst_card2.special_state, src_card_after_review.special_state);

        // Step 6: Verify the review history was also inherited.
        let src_review_logs: Vec<ReviewLog> =
            sqlx::query_as(r#"SELECT * FROM review_log WHERE card_id = ?"#)
                .bind(src_card.id)
                .fetch_all(&pool)
                .await
                .unwrap();
        let dst_review_logs: Vec<ReviewLog> =
            sqlx::query_as(r#"SELECT * FROM review_log WHERE card_id = ?"#)
                .bind(dst_card2.id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            src_review_logs.len(),
            dst_review_logs.len(),
            "review log count mismatch"
        );
        for (src_log, dst_log) in src_review_logs.iter().zip(dst_review_logs.iter()) {
            assert_eq!(dst_log.card_id, dst_card2.id);
            assert_eq!(
                dst_log.reviewed_at.timestamp(),
                src_log.reviewed_at.timestamp()
            );
            assert_eq!(dst_log.rating, src_log.rating);
            assert_eq!(dst_log.scheduler_name, src_log.scheduler_name);
            assert_eq!(dst_log.scheduled_time, src_log.scheduled_time);
            assert_eq!(dst_log.recall_duration, src_log.recall_duration);
            assert_eq!(dst_log.rate_duration, src_log.rate_duration);
            assert_eq!(dst_log.previous_state, src_log.previous_state);
            assert_eq!(dst_log.custom_data, src_log.custom_data);
        }
    }
}
