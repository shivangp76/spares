use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use sqlx::sqlite::SqlitePool;

use super::super::apply_srs_inheritance;
use super::super::create_cards;
use crate::CardErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::api::execute_batched_query;
use crate::api::fetch_batched_query;
use crate::api::placeholders;
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
    db: &SqlitePool,
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
        })
        .collect();

    // Update moved cards (or cards with the same index where their `back_type` or `special_state` changed)
    let indices = move_card_indices
        .iter()
        .map(|(x, _)| *x)
        .chain(changed_same_indices.iter().copied())
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
        .chain(changed_same_indices.into_iter().map(|i| (i, i)))
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
    let new_card_data_ref = new_cards; // keep reference before shadowing
    let new_cards_created = create_card_indices
        .into_iter()
        .map(|i| {
            let new_card = new_card_data_ref.get(i - 1).unwrap();
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
    create_cards(db, &new_cards_created).await?;

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
    apply_srs_inheritance(db, &inherit_entries).await?;
    Ok(())
}
