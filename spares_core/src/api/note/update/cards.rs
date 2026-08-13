use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use sqlx::sqlite::SqliteConnection;

use crate::CardErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::api::MAX_ROWS_IN_QUERY;
use crate::api::note::apply_srs_inheritance_on;
use crate::api::note::copy_review_logs_on;
use crate::api::note::create_cards_on;
use crate::api::placeholders;
use crate::model::AUTO_FIX_MISSING_CARDS;
use crate::model::Card;
use crate::model::CardId;
use crate::model::NoteId;
use crate::model::SpecialState;
use crate::parsers::CardData;
use crate::parsers::MatchCardsResult;
use crate::parsers::ReadableCardIdentifier;
use crate::parsers::match_cards;

#[expect(clippy::too_many_lines)]
pub(super) async fn update_cards(
    conn: &mut SqliteConnection,
    old_cards: &[CardData],
    new_cards: &[CardData],
    note_id: NoteId,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    // Line up cards
    // The card's id in the database cannot change since they are referred to in `review_log`.
    let old_cards_orders = old_cards.iter().map(|x| x.order).collect::<Vec<_>>();
    // `previous_order` holds the order references from the submitted note text (before
    // sequential renumbering), which is what `match_cards` needs to reconcile with old DB cards.
    let new_cards_orders = new_cards
        .iter()
        .map(|x| x.previous_order)
        .collect::<Vec<_>>();
    let match_cards_result = match_cards(&old_cards_orders, &new_cards_orders)?;
    let MatchCardsResult {
        move_card_indices,
        mut delete_card_indices,
        mut create_card_indices,
        same_indices,
    } = match_cards_result;

    let old_cards_by_order: HashMap<usize, &CardData> = old_cards
        .iter()
        .filter_map(|c| c.order.map(|o| (o, c)))
        .collect();

    // When a card switches between forward and reverse (e.g. `ro:` added or removed), it is
    // fundamentally a different card — front and back are swapped — so it must be deleted and
    // recreated rather than updated in place (which would incorrectly preserve the old schedule).
    let reverse_changed_indices: Vec<usize> = same_indices
        .iter()
        .copied()
        .filter(|&i| {
            let old_card = &old_cards_by_order[&i];
            let new_card = &new_cards[i - 1];
            old_card.is_reverse() != new_card.is_reverse()
        })
        .collect();
    let same_indices: Vec<usize> = same_indices
        .into_iter()
        .filter(|i| !reverse_changed_indices.contains(i))
        .collect();
    delete_card_indices.extend(reverse_changed_indices.iter().copied());
    create_card_indices.extend(reverse_changed_indices.iter().copied());

    let create_indices_set: HashSet<usize> = create_card_indices.iter().copied().collect();
    for (i, card_data) in new_cards.iter().enumerate() {
        if card_data.inherit.is_some() {
            let order = i + 1;
            if !create_indices_set.contains(&order) {
                return Err(Error::Library(LibraryError::Card(
                    CardErrorKind::InvalidInput(format!(
                        "`inh:` was specified on card at order {order} but this card already \
                         exists (was matched, not newly created). `inh:` is only valid on \
                         newly created cards."
                    )),
                )));
            }
        }
    }

    let changed_same_indices: Vec<usize> = same_indices
        .iter()
        .copied()
        .filter(|&i| {
            let old_card = &old_cards_by_order[&i];
            let new_card = &new_cards[i - 1];
            old_card.back_type != new_card.back_type
                || old_card.is_suspended != new_card.is_suspended
                || old_card.cloze_uid != new_card.cloze_uid
        })
        .collect();

    // Update moved cards (or cards with the same index where their `back_type`, `special_state`, or `cloze_uid` changed)
    let indices = move_card_indices
        .iter()
        .map(|(x, _)| *x)
        .chain(changed_same_indices.iter().copied())
        .collect::<Vec<_>>();
    let mut moved_cards: Vec<Card> = Vec::new();
    for chunk in indices.chunks(MAX_ROWS_IN_QUERY) {
        let query_str = format!(
            "SELECT * FROM card WHERE note_id = ? AND \"order\" IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_as(&query_str);
        query = query.bind(note_id);
        for index in chunk {
            query = query.bind(*index as u32);
        }
        moved_cards.extend(
            query
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?,
        );
    }

    let move_card_indices_map = move_card_indices
        .into_iter()
        .chain(changed_same_indices.into_iter().map(|i| (i, i)))
        .collect::<HashMap<usize, usize>>();
    for moved_card in &mut moved_cards {
        let to_card_index = move_card_indices_map
            .get(&(moved_card.order as usize))
            .copied()
            .ok_or_else(|| {
                Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(format!(
                    "card order {} not found in move map for note {note_id}",
                    moved_card.order
                ))))
            })?;
        moved_card.order = to_card_index as u32;
        let new_card = new_cards.get(to_card_index - 1).ok_or_else(|| {
            Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(format!(
                "new card index {to_card_index} out of bounds for note {note_id}"
            ))))
        })?;
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
        // Merge cloze_uid into custom_data. When the parser produces a cloze_uid
        // (from an `id:` key in the note text) we set it; when it doesn't (note
        // text has no `id:` after e.g. a strip-liveness pass) we remove any stale uid.
        let mut custom_data_map = match moved_card.custom_data.clone() {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        if let Some(ref uid) = new_card.cloze_uid {
            custom_data_map.insert(
                "cloze_uid".to_string(),
                serde_json::Value::String(uid.to_string()),
            );
        } else {
            custom_data_map.remove("cloze_uid");
        }
        let updated_custom_data = serde_json::Value::Object(custom_data_map);
        let _update_card_result =
            sqlx::query(r#"UPDATE card SET "order" = ?, back_type = ?, special_state = ?, custom_data = ?, updated_at = ? WHERE id = ?"#)
                .bind(moved_card.order)
                .bind(moved_card.back_type)
                .bind(moved_card.special_state)
                .bind(&updated_custom_data)
                .bind(moved_card.updated_at.timestamp())
                .bind(moved_card.id)
                .execute(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
    }

    // Delete cards
    for chunk in delete_card_indices.chunks(MAX_ROWS_IN_QUERY) {
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
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }

    // Create new cards
    let new_card_data_ref = new_cards; // keep reference before shadowing
    let new_cards_created = create_card_indices
        .into_iter()
        .map(|i| -> Result<Card, Error> {
            let new_card = new_card_data_ref.get(i - 1).ok_or_else(|| {
                Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(format!(
                    "create index {i} out of bounds for note {note_id}"
                ))))
            })?;
            let mut card = Card::new(at);
            card.note_id = note_id;
            card.order = i as u32;
            if new_card.is_suspended.unwrap_or(false) {
                card.special_state = Some(SpecialState::Suspended);
            }
            card.back_type = new_card.back_type;
            if let Some(ref uid) = new_card.cloze_uid {
                let uid_str = uid.to_string();
                if let serde_json::Value::Object(ref mut map) = card.custom_data {
                    map.insert("cloze_uid".to_string(), serde_json::Value::String(uid_str));
                } else {
                    let mut map = serde_json::Map::new();
                    map.insert("cloze_uid".to_string(), serde_json::Value::String(uid_str));
                    card.custom_data = serde_json::Value::Object(map);
                }
            }
            Ok(card)
        })
        .collect::<Result<Vec<_>, _>>()?;
    create_cards_on(&mut *conn, &new_cards_created).await?;

    // Apply SRS inheritance for newly created cards that carried `inh:NOTE_ID/ORDER`
    let inherit_entries: Vec<(NoteId, u32, NoteId, usize)> = new_cards_created
        .iter()
        .filter_map(|card| {
            let card_data = new_card_data_ref.get(card.order as usize - 1)?;
            let ReadableCardIdentifier {
                note_id: src_note_id,
                order: src_order,
            } = card_data.inherit?;
            Some((note_id, card.order, src_note_id, src_order))
        })
        .collect();
    apply_srs_inheritance_on(&mut *conn, &inherit_entries).await?;
    if AUTO_FIX_MISSING_CARDS {
        verify_card_consistency(&mut *conn, note_id, new_card_data_ref, at).await?;
    }
    Ok(())
}

/// Ensures the set of cards in the database exactly matches what the parser
/// produced for the note. This is a safety net for historical data corruption
/// where the DB may have fewer (or more) cards than expected.
#[expect(clippy::too_many_lines)]
async fn verify_card_consistency(
    conn: &mut SqliteConnection,
    note_id: NoteId,
    new_cards: &[CardData],
    at: DateTime<Utc>,
) -> Result<(), Error> {
    let existing_cards: Vec<Card> =
        sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER BY "order""#)
            .bind(note_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let existing_orders: Vec<u32> = existing_cards.iter().map(|c| c.order).collect();
    let expected_orders: Vec<u32> = (1..=new_cards.len() as u32).collect();
    let missing_orders: Vec<u32> = expected_orders
        .iter()
        .copied()
        .filter(|o| !existing_orders.contains(o))
        .collect();

    if !missing_orders.is_empty() {
        let template = existing_cards.first();
        let cards_to_create: Vec<Card> = missing_orders
            .iter()
            .map(|&order| -> Result<Card, Error> {
                let card_data = new_cards
                    .get(order as usize - 1)
                    .ok_or_else(|| {
                        Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(format!(
                            "missing order {order} is outside the parser's card range for note {note_id}"
                        ))))
                    })?;
                let mut card = if let Some(t) = template {
                    let mut c = t.clone();
                    c.special_state = None;
                    c.updated_at = at;
                    c
                } else {
                    Card::new(at)
                };
                card.note_id = note_id;
                card.order = order;
                card.back_type = card_data.back_type;
                if card_data.is_suspended.unwrap_or(false) {
                    card.special_state = Some(SpecialState::Suspended);
                }
                if let Some(ref uid) = card_data.cloze_uid {
                    let uid_str = uid.to_string();
                    if let serde_json::Value::Object(ref mut map) = card.custom_data {
                        map.insert("cloze_uid".to_string(), serde_json::Value::String(uid_str));
                    } else {
                        let mut map = serde_json::Map::new();
                        map.insert("cloze_uid".to_string(), serde_json::Value::String(uid_str));
                        card.custom_data = serde_json::Value::Object(map);
                    }
                }
                Ok(card)
            })
            .collect::<Result<Vec<_>, _>>()?;
        create_cards_on(&mut *conn, &cards_to_create).await?;
        // Copy review logs from template to newly created cards
        if let Some(template) = template {
            let orders: Vec<u32> = cards_to_create.iter().map(|c| c.order).collect();
            let mut rows: Vec<(CardId, u32)> = Vec::new();
            for chunk in orders.chunks(MAX_ROWS_IN_QUERY) {
                let query_str = format!(
                    r#"SELECT id, "order" FROM card WHERE note_id = ? AND "order" IN ({})"#,
                    placeholders(chunk.len())
                );
                let mut query = sqlx::query_as::<_, (CardId, u32)>(&query_str);
                query = query.bind(note_id);
                for order in chunk {
                    query = query.bind(*order);
                }
                rows.extend(
                    query
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?,
                );
            }
            let card_map: HashMap<u32, CardId> =
                rows.into_iter().map(|(id, order)| (order, id)).collect();
            let dst_card_ids: Vec<CardId> = cards_to_create
                .iter()
                .map(|c| {
                    card_map.get(&c.order).copied().ok_or_else(|| {
                        Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(format!(
                            "newly created card order {} not found in id map for note {note_id}",
                            c.order
                        ))))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            copy_review_logs_on(&mut *conn, template.id, &dst_card_ids).await?;
        }
    }

    let extra_orders: Vec<u32> = existing_orders
        .iter()
        .copied()
        .filter(|o| !expected_orders.contains(o))
        .collect();
    for order in &extra_orders {
        sqlx::query(r#"DELETE FROM card WHERE note_id = ? AND "order" = ?"#)
            .bind(note_id)
            .bind(order)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }

    Ok(())
}
