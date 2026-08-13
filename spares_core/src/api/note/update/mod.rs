use std::collections::HashMap;

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

/// Per-note parsed state computed in Phase 2a (before the transaction begins, so the `SQLite`
/// write lock is not held during parsing). Consumed by Phase 2b's DML.
struct ParsedNote {
    note_id: NoteId,
    new_data: String,
    new_parser_id: i64,
    new_custom_data: Value,
    new_parser: Box<dyn Parseable>,
    old_cards: Vec<CardData>,
    new_cards: Vec<CardData>,
    all_keywords: Vec<(String, bool)>, // (keyword, is_embedded)
    old_card_orders: Vec<usize>,
}

/// Per-note state accumulated in Phase 2b and consumed in Phase 4.
/// `old_card_orders` is derived from the pre-update note data and is only needed in
/// Phase 4 for delete-note-files.
struct PendingNote {
    note_id: NoteId,
    updated_note: Note,
    all_keywords: Vec<(String, bool)>, // (keyword, is_embedded)
    card_count: usize,
    old_card_orders: Vec<usize>,
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
/// The update proceeds in phases to minimize DB round-trips and write-lock hold time:
///   - Phase 1: batch-fetch all reference data (parsers, filtered tags, notes, keywords, tags, cards).
///   - Phase 2a: parse and validate every note (pure CPU; runs before the transaction so the
///     `SQLite` write lock is not held during parsing).
///   - Phase 2b + 3: inside a single transaction, write note rows, cards, tags, links, and
///     keywords atomically, then batch-fetch current tags/cards for the after-snapshots.
///
/// The transaction makes the DB write atomic: a crash or error mid-update can no longer leave
/// note data updated while keywords (or cards/tags/links) are stale. File generation,
/// filtered-tag rebuilds, and undo-event logging run after commit: they have side effects that
/// cannot be rolled back, so a failure there returns an error even though the DB is committed.
#[expect(clippy::too_many_lines)]
#[allow(clippy::explicit_auto_deref)] // `&mut *tx` reborrows the transaction's connection, which is required by `sqlx::Executor`
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

    // ---- Phase 1: Batch pre-fetches ----

    let all_parser_rows: Vec<(i64, String)> = sqlx::query_as(r"SELECT id, name FROM parser")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let parser_name_map: HashMap<i64, &str> = all_parser_rows
        .iter()
        .map(|(id, name)| (*id, name.as_str()))
        .collect();
    // Build a name→factory map so we construct each parser at most once per call. Each factory
    // is invoked once here only to extract its static name, so parser constructors must be cheap
    // (no I/O or expensive setup).
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

    if note_ids.is_empty() {
        return Ok(UpdateNotesResponse {
            notes: Vec::new(),
            event_id: None,
        });
    }

    let existing_notes: Vec<Note> =
        fetch_batched_query(db, &note_ids, MAX_ROWS_IN_QUERY, async |db, chunk| {
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
    // This mirrors what the old per-note `fetch_one` produced when a selector referenced a
    // non-existent note: a `RowNotFound` sqlx error.
    for id in &note_ids {
        if !notes_map.contains_key(id) {
            return Err(Error::Sqlx {
                source: sqlx::Error::RowNotFound,
            });
        }
    }

    let need_old_keywords = keywords.is_none() || log;
    let old_keywords_map: HashMap<NoteId, Vec<String>> = if need_old_keywords {
        let rows: Vec<(NoteId, String)> =
            fetch_batched_query(db, &note_ids, MAX_ROWS_IN_QUERY, async |db, chunk| {
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
        let rows: Vec<(NoteId, String)> =
            fetch_batched_query(db, &note_ids, MAX_ROWS_IN_QUERY, async |db, chunk| {
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

    // Batch-fetch before-snapshot cards as `CardSnapshot`s (only when logging). Converting
    // eagerly avoids holding both the full `Card` and a second `CardSnapshot` copy per card.
    let old_cards_map: HashMap<NoteId, Vec<CardSnapshot>> = if log {
        let rows: Vec<Card> =
            fetch_batched_query(db, &note_ids, MAX_ROWS_IN_QUERY, async |db, chunk| {
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
        let mut map: HashMap<NoteId, Vec<CardSnapshot>> = HashMap::new();
        for card in rows {
            map.entry(card.note_id)
                .or_default()
                .push(CardSnapshot::from_card(&card));
        }
        map
    } else {
        HashMap::new()
    };

    // ---- Phase 2a: parse and validate all notes (pure CPU). Runs before the transaction so
    // the SQLite write lock is not held during parsing. ----

    let mut parsed_notes: Vec<ParsedNote> = Vec::with_capacity(note_ids.len());

    for note_id in &note_ids {
        let existing_note = &notes_map[note_id];

        let submitted_new_data = data.as_ref().unwrap_or(&existing_note.data).clone();
        let new_parser_id = parser_id.unwrap_or(existing_note.parser_id);
        let new_custom_data = custom_data.as_ref().map_or_else(
            || existing_note.custom_data.clone(),
            |cd| Value::Object(cd.clone()),
        );

        let new_extra_keywords: Vec<String> = if let Some(ref ks) = keywords {
            ks.clone()
        } else {
            old_keywords_map.get(note_id).cloned().unwrap_or_default()
        };

        let old_parser_name = parser_name_map
            .get(&existing_note.parser_id)
            .copied()
            .ok_or_else(|| {
                Error::Library(LibraryError::Parser(ParserErrorKind::NotFound(
                    existing_note.parser_id.to_string(),
                )))
            })?;
        let (_old_parser, old_cards) = get_parser_and_cards(
            old_parser_name,
            existing_note.data.as_str(),
            &parser_factories,
        )?;
        let new_parser_name = parser_name_map
            .get(&new_parser_id)
            .copied()
            .ok_or_else(|| {
                Error::Library(LibraryError::Parser(ParserErrorKind::NotFound(
                    new_parser_id.to_string(),
                )))
            })?;
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
        let all_keywords = extract_and_combine_keywords(
            new_parser.as_ref(),
            new_data.as_str(),
            &new_extra_keywords,
        )
        .map_err(Error::Library)?;

        // `match_cards` (called from `update_cards`) rejects any old card lacking an explicit
        // order, so the `filter_map` below never drops a real order.
        let old_card_orders: Vec<usize> = old_cards.iter().filter_map(|card| card.order).collect();

        parsed_notes.push(ParsedNote {
            note_id: *note_id,
            new_data,
            new_parser_id,
            new_custom_data,
            new_parser,
            old_cards,
            new_cards,
            all_keywords,
            old_card_orders,
        });
    }

    // ---- Phase 2b + Phase 3 run inside a single transaction so that note rows, cards, tags,
    // links, and keywords commit atomically. A crash mid-update can no longer leave note data
    // updated while keywords (or cards/tags/links) are stale. ----

    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;

    let mut pending_notes: Vec<PendingNote> = Vec::with_capacity(parsed_notes.len());

    for parsed in parsed_notes {
        let created_at: i64 = sqlx::query_scalar(
            r"UPDATE note SET data = ?, parser_id = ?, custom_data = ?, updated_at = ? WHERE id = ? RETURNING created_at",
        )
        .bind(&parsed.new_data)
        .bind(parsed.new_parser_id)
        .bind(&parsed.new_custom_data)
        .bind(at.timestamp())
        .bind(parsed.note_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

        let created_at = DateTime::from_timestamp(created_at, 0).ok_or_else(|| {
            Error::Library(LibraryError::InvalidConfig(format!(
                "invalid created_at timestamp in database for note {}: {created_at}",
                parsed.note_id
            )))
        })?;
        let updated_note = Note {
            id: parsed.note_id,
            data: parsed.new_data.clone(),
            created_at,
            updated_at: at,
            parser_id: parsed.new_parser_id,
            custom_data: parsed.new_custom_data.clone(),
        };
        cards::update_cards(
            &mut *tx,
            &parsed.old_cards,
            &parsed.new_cards,
            parsed.note_id,
            at,
        )
        .await?;

        new_tag_payloads.extend(
            tags::update_tags(
                &mut *tx,
                &tags,
                parsed.note_id,
                &existing_filtered_tag_names,
            )
            .await?,
        );

        links::update_note_links(
            &mut *tx,
            parsed.note_id,
            parsed.new_parser.as_ref(),
            parsed.new_data.as_str(),
        )
        .await?;

        pending_notes.push(PendingNote {
            note_id: parsed.note_id,
            updated_note,
            all_keywords: parsed.all_keywords,
            card_count: parsed.new_cards.len(),
            old_card_orders: parsed.old_card_orders,
        });
    }

    // ---- Phase 3: Batched post-pass (still inside the transaction) ----

    let all_note_ids: Vec<NoteId> = pending_notes.iter().map(|p| p.note_id).collect();

    // NOTE: All keywords must be updated. Suppose a note had an extra keyword A and no embedded keywords. Suppose this note is updated with no new extra keywords, but an extra embedded keyword of A. Then, the extra keyword of A should be deleted and converted to an embedded keyword.
    // Batch-delete all old keywords, then bulk-insert new ones.
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
    // Flatten keyword rows first and chunk at the *row* level. Chunking by note would let a
    // single chunk carry thousands of keyword rows and blow past SQLite's bound-parameter
    // limit (`MAX_ROWS_IN_QUERY` rows × 3 params = 600, safely under `SQLITE_MAX_VARIABLE_NUMBER`).
    let all_kw_rows: Vec<(NoteId, &str, bool)> = pending_notes
        .iter()
        .flat_map(|p| {
            p.all_keywords
                .iter()
                .map(|(kw, emb)| (p.note_id, kw.as_str(), *emb))
        })
        .collect();
    for chunk in all_kw_rows.chunks(MAX_ROWS_IN_QUERY) {
        let query_str = format!(
            "INSERT INTO note_keyword (note_id, keyword, embedded) VALUES {}",
            placeholders_2d(chunk.len(), 3)
        );
        let mut query = sqlx::query(&query_str);
        for (note_id, keyword, embedded) in chunk {
            query = query.bind(note_id);
            query = query.bind(keyword);
            query = query.bind(i32::from(*embedded));
        }
        query
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }

    // Batch-fetch current tags for all notes (must see this transaction's writes).
    let mut current_tags_map: HashMap<NoteId, Vec<String>> = HashMap::new();
    for chunk in all_note_ids.chunks(MAX_ROWS_IN_QUERY) {
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
        for (note_id, name) in query
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
        {
            current_tags_map.entry(note_id).or_default().push(name);
        }
    }
    // ensure every pending note has an entry
    for pn in &pending_notes {
        current_tags_map.entry(pn.note_id).or_default();
    }

    // Batch-fetch cards for after-snapshots (only if logging; must see this transaction's writes).
    let after_cards_map: HashMap<NoteId, Vec<Card>> = if log {
        let mut map: HashMap<NoteId, Vec<Card>> = HashMap::new();
        for chunk in all_note_ids.chunks(MAX_ROWS_IN_QUERY) {
            let query_str = format!(
                "SELECT * FROM card WHERE note_id IN ({}) ORDER BY note_id, \"order\" ASC",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as::<_, Card>(&query_str);
            for id in chunk {
                query = query.bind(id);
            }
            for card in query
                .fetch_all(&mut *tx)
                .await
                .map_err(|e| Error::Sqlx { source: e })?
            {
                map.entry(card.note_id).or_default().push(card);
            }
        }
        map
    } else {
        HashMap::new()
    };

    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;

    // ---- Phase 4: Build responses, parse requests, delete old files, and undo snapshots ----

    for PendingNote {
        note_id,
        updated_note,
        all_keywords,
        card_count,
        old_card_orders,
    } in pending_notes
    {
        let tags = current_tags_map.get(&note_id).cloned().unwrap_or_default();

        // Compute shared keyword vectors once per note instead of re-cloning for every consumer.
        let all_keyword_strings: Vec<String> =
            all_keywords.iter().map(|(k, _)| k.clone()).collect();
        let non_embedded_keywords: Vec<String> = all_keywords
            .iter()
            .filter(|(_, embedded)| !embedded)
            .map(|(k, _)| k.clone())
            .collect();

        note_responses.push(NoteResponse::new(
            &updated_note,
            all_keyword_strings,
            tags.clone(),
            None,
            card_count,
        ));

        // Delete old generated files if the parser changed
        if parser_id.is_some() {
            let old_note = &notes_map[&note_id];
            if old_note.parser_id != updated_note.parser_id {
                let old_parser_name = parser_name_map
                    .get(&old_note.parser_id)
                    .copied()
                    .ok_or_else(|| {
                        Error::Library(LibraryError::Parser(ParserErrorKind::NotFound(
                            old_note.parser_id.to_string(),
                        )))
                    })?;
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
            keywords: non_embedded_keywords.clone(),
            linked_notes: None, // This is expensive so only done in `render_notes()`.
            // `custom_data` is a free-form JSON value in the DB; degrade gracefully rather
            // than panicking if a legacy/corrupt row ever holds a non-object.
            custom_data: updated_note
                .custom_data
                .as_object()
                .cloned()
                .unwrap_or_default(),
            tags,
        };
        parse_note_requests.push((updated_note.parser_id, parse_note_request));

        // Build UpdateNotePayload for undo logging. Before/after snapshots are reconstructed
        // from the pre-fetched maps rather than cached in `pending_notes`, avoiding a second
        // full copy of every snapshot.
        //
        // Known limitation: the "before" snapshots are read before the transaction and the
        // "after" snapshots inside it, so a concurrent write committing in between could make
        // the undo payload straddle two states. Resolving this would require the before-fetches
        // to run inside the same transaction, which conflicts with parsing outside the write
        // lock (Phase 2a); accepted for single-user/local use.
        if log {
            let before_snapshot = NoteSnapshot {
                id: note_id,
                data: notes_map[&note_id].data.clone(),
                created_at: notes_map[&note_id].created_at,
                parser_id: notes_map[&note_id].parser_id,
                custom_data: notes_map[&note_id].custom_data.clone(),
                // `old_keywords_map` is already ordered by the fetch's `ORDER BY keyword ASC`.
                keywords: old_keywords_map.get(&note_id).cloned().unwrap_or_default(),
                tags: old_tags_map.get(&note_id).cloned().unwrap_or_default(),
                cards: old_cards_map.get(&note_id).cloned().unwrap_or_default(),
            };

            let mut after_keywords = non_embedded_keywords;
            after_keywords.sort();
            let after_tags = current_tags_map.get(&note_id).cloned().unwrap_or_default();
            let after_cards = after_cards_map
                .get(&note_id)
                .map(|cards| cards.iter().map(CardSnapshot::from_card).collect())
                .unwrap_or_default();

            let after_snapshot = NoteSnapshot {
                id: note_id,
                data: updated_note.data,
                created_at: updated_note.created_at,
                parser_id: updated_note.parser_id,
                custom_data: updated_note.custom_data.clone(),
                keywords: after_keywords,
                tags: after_tags,
                cards: after_cards,
            };

            let payload = build_update_note_payload(note_id, &before_snapshot, &after_snapshot);
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
            serde_json::to_value(&payload).map_err(|e| {
                Error::Library(LibraryError::InvalidConfig(format!(
                    "failed to serialize update notes payload: {e}"
                )))
            })?,
        );
        if new_tag_payloads.is_empty() {
            let event_ids = insert_events(db, &[note_event], at, None).await?;
            Some(*event_ids.first().ok_or_else(|| {
                Error::Library(LibraryError::InvalidConfig(
                    "insert_events returned no event ids".to_string(),
                ))
            })?)
        } else {
            let mut events: Vec<(EventType, Value)> = new_tag_payloads
                .into_iter()
                .map(|p| {
                    Ok::<_, Error>((
                        EventType::CreateTag,
                        serde_json::to_value(&p).map_err(|e| {
                            Error::Library(LibraryError::InvalidConfig(format!(
                                "failed to serialize create tag payload: {e}"
                            )))
                        })?,
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?;
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
    use crate::model::NoteId;
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

    /// Creates a note via `create_notes` and returns its id.
    async fn create_test_note(
        pool: &SqlitePool,
        parser_id: i64,
        data: &str,
        keywords: Vec<String>,
    ) -> NoteId {
        create_notes(
            pool,
            CreateNotesRequest {
                parser_id,
                requests: vec![CreateNoteRequest {
                    data: data.to_string(),
                    keywords,
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
        .unwrap()
        .notes[0]
            .id
    }

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

    /// A failure while updating any note in a batch must roll back *all* notes (note rows,
    /// cards, keywords), not just the failing one.
    #[sqlx::test]
    async fn test_update_notes_rolls_back_all_notes_on_error(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "markdown").await;
        let id_a =
            create_test_note(&pool, parser.id, "{{ first }}", vec!["kw_a".to_string()]).await;
        let id_b = create_test_note(&pool, parser.id, "{{ second }}", vec![]).await;
        let stored_a: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
            .bind(id_a)
            .fetch_one(&pool)
            .await
            .unwrap();

        // Corrupt note B's stored data so it no longer carries explicit card orders. `match_cards`
        // then rejects B's update, which must roll back A's changes in the same transaction.
        sqlx::query("UPDATE note SET data = ? WHERE id = ?")
            .bind("{{ bare }}")
            .bind(id_b)
            .execute(&pool)
            .await
            .unwrap();

        let res = update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![id_a, id_b]),
                data: Some("{{ updated }}".to_string()),
                parser_id: None,
                keywords: Some(vec!["kw_new".to_string()]),
                tags: UpdateTags::None,
                custom_data: None,
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await;
        assert!(res.is_err(), "expected the batch update to fail on note B");

        // Note A must be unchanged: same data, no new keyword rows.
        let a_data: String = sqlx::query_scalar("SELECT data FROM note WHERE id = ?")
            .bind(id_a)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(a_data, stored_a, "note A must be rolled back");
        let a_kws: Vec<String> =
            sqlx::query_scalar("SELECT keyword FROM note_keyword WHERE note_id = ?")
                .bind(id_a)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(a_kws, vec!["kw_a"], "note A's keywords must be rolled back");
    }

    /// Keyword rows are chunked at the *row* level, so a single note with many keywords stays
    /// below `SQLite`'s bound-parameter limit.
    #[sqlx::test]
    async fn test_update_note_many_keywords_row_chunking(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "markdown").await;
        let res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "{{ note }}".to_string(),
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
        let id = res.notes[0].id;

        let many_keywords: Vec<String> = (0..450).map(|i| format!("kw_{i}")).collect();
        let update_res = update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![id]),
                data: None,
                parser_id: None,
                keywords: Some(many_keywords.clone()),
                tags: UpdateTags::None,
                custom_data: None,
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await;
        assert!(update_res.is_ok());

        let kw_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM note_keyword WHERE note_id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kw_count, i64::try_from(many_keywords.len()).unwrap());
    }

    /// Repeated tag names in one request must not create duplicate `tag` rows or events.
    #[sqlx::test]
    async fn test_update_note_duplicate_tag_names_are_deduplicated(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "markdown").await;
        let res = create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "{{ note }}".to_string(),
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
        let id = res.notes[0].id;

        let update_res = update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![id]),
                data: None,
                parser_id: None,
                keywords: None,
                tags: UpdateTags::ModifyTags {
                    tags_to_remove: None,
                    tags_to_add: Some(vec!["dup".to_string(), "dup".to_string()]),
                },
                custom_data: None,
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await;
        assert!(update_res.is_ok());

        let tag_count: i64 = sqlx::query_scalar(r"SELECT COUNT(*) FROM tag WHERE name = 'dup'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            tag_count, 1,
            "duplicate names must not create duplicate tag rows"
        );
        let nt_count: i64 = sqlx::query_scalar(
            r"SELECT COUNT(*) FROM note_tag nt JOIN tag t ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.name = 'dup'",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            nt_count, 1,
            "duplicate names must not create duplicate note_tag rows"
        );
    }
}
