use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::CardSnapshot;
use crate::api::undo::payloads::MovedCard;
use crate::api::undo::payloads::NoteCardSnapshot;
use crate::api::undo::payloads::RelocateCardsPayload;
use crate::api::undo::payloads::Transition;
use crate::model::Card;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::NoteId;
use crate::parsers::BackType;
use crate::schema::note::RelocateLiveCardsRequest;
use crate::schema::note::RelocateLiveCardsResponse;

/// Cards parked in a note by [`relocate_live_cards`] have an order of at least this, past any
/// card's real order, until the note's update gives them their place among its cards.
pub(crate) const PARKED_CARD_ORDER_START: u32 = 1 << 30;

/// Moves each card of a live sync group to the note that now holds its cloze, so the card keeps
/// its review history when the cloze changes notes.
///
/// Live notes are matched to the database by position, so inserting, deleting, or splitting a
/// note in its source file moves the clozes after it into other notes of the group. Matching the
/// cards of a note by `id:` alone would then delete each moved cloze's card from its old note and
/// create a new one in its new note.
///
/// `request.notes` are the group's notes with the clozes of their new data, after the import has
/// created its new notes but before it updates the others. A card whose cloze is in another note is
/// moved there:
/// - into a note the import just created, in place of the card it created for the cloze;
/// - into any other note, parked at an order past any real one (`PARKED_CARD_ORDER_START`), so
///   the update of the note it left does not delete it, and the update of the note it joined
///   takes it in instead of creating a card for the cloze (see `update_cards`).
///
/// With `log`, the moves are a [`EventType::RelocateCards`] event, which undo reverses.
#[expect(clippy::too_many_lines)]
pub async fn relocate_live_cards(
    db: &SqlitePool,
    request: RelocateLiveCardsRequest,
    at: DateTime<Utc>,
    log: bool,
) -> Result<RelocateLiveCardsResponse, Error> {
    let RelocateLiveCardsRequest {
        live_sync_name,
        notes,
    } = request;
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;

    let group_note_ids: HashSet<NoteId> = sqlx::query_scalar(
        r"SELECT id FROM note WHERE json_extract(custom_data, '$.live_sync_name') = ?",
    )
    .bind(&live_sync_name)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| Error::Sqlx { source: e })?
    .into_iter()
    .collect();
    let mut destinations: Vec<(&str, NoteId, bool)> = Vec::new();
    for note in &notes {
        if !group_note_ids.contains(&note.note_id) {
            return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "Note {} is not a live note of live-sync-name `{live_sync_name}`.",
                    note.note_id
                ),
            })));
        }
        destinations.extend(
            note.cloze_uids
                .iter()
                .map(|uid| (uid.as_str(), note.note_id, note.created)),
        );
    }

    // (id, note id, order, back type) of the group's cards, by cloze uid
    let cards: Vec<(CardId, NoteId, u32, BackType, String)> = sqlx::query_as(
        r#"SELECT c.id, c.note_id, c."order", c.back_type, json_extract(c.custom_data, '$.cloze_uid')
           FROM card c JOIN note n ON n.id = c.note_id
           WHERE json_extract(n.custom_data, '$.live_sync_name') = ?
             AND json_extract(c.custom_data, '$.cloze_uid') IS NOT NULL
           ORDER BY c.note_id, c."order""#,
    )
    .bind(&live_sync_name)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let mut next_parked_order: HashMap<NoteId, u32> = HashMap::new();
    let mut cards_by_uid: HashMap<&str, Vec<(CardId, NoteId, u32, BackType)>> = HashMap::new();
    for (card_id, note_id, order, back_type, uid) in &cards {
        if *order >= PARKED_CARD_ORDER_START {
            let next = next_parked_order
                .entry(*note_id)
                .or_insert(PARKED_CARD_ORDER_START);
            *next = (*next).max(order + 1);
        }
        cards_by_uid
            .entry(uid.as_str())
            .or_default()
            .push((*card_id, *note_id, *order, *back_type));
    }

    let mut payload = RelocateCardsPayload {
        moved: Vec::new(),
        deleted_cards: Vec::new(),
        restored_cards: Vec::new(),
    };
    for (uid, destination, created) in destinations {
        let Some(holders) = cards_by_uid.get(uid) else {
            continue;
        };
        let (here, elsewhere): (Vec<_>, Vec<_>) = holders
            .iter()
            .partition(|(_, note_id, _, _)| *note_id == destination);
        // A note that is not new and already has a card for the cloze keeps it; which card
        // should continue the cloze is unclear, so neither is touched.
        if elsewhere.is_empty() || (!here.is_empty() && !created) {
            continue;
        }
        // An `r:` cloze has a forward and a backward card, so each card taken over is paired with
        // one created for the cloze in order.
        let mut here = here.into_iter();
        for &(card_id, from_note_id, from_order, from_back_type) in elsewhere {
            let (order, back_type) =
                if let Some(&(created_card_id, _, order, back_type)) = here.next() {
                    let created_card: Card =
                        sqlx::query_as(r"DELETE FROM card WHERE id = ? RETURNING *")
                            .bind(created_card_id)
                            .fetch_one(&mut *tx)
                            .await
                            .map_err(|e| Error::Sqlx { source: e })?;
                    payload.deleted_cards.push(NoteCardSnapshot {
                        note_id: destination,
                        card: CardSnapshot::from_card(&created_card),
                    });
                    (order, back_type)
                } else {
                    let next = next_parked_order
                        .entry(destination)
                        .or_insert(PARKED_CARD_ORDER_START);
                    *next += 1;
                    (*next - 1, from_back_type)
                };
            sqlx::query(
                r#"UPDATE card SET note_id = ?, "order" = ?, back_type = ?, updated_at = ? WHERE id = ?"#,
            )
            .bind(destination)
            .bind(order)
            .bind(back_type)
            .bind(at.timestamp())
            .bind(card_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
            payload.moved.push(MovedCard {
                card_id,
                note_id: Transition {
                    before: from_note_id,
                    after: destination,
                },
                order: Transition {
                    before: from_order,
                    after: order,
                },
                back_type: Transition {
                    before: from_back_type,
                    after: back_type,
                },
            });
        }
    }

    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;
    let moved_card_count = payload.moved.len();
    let event_id = if log && moved_card_count > 0 {
        let event_ids = insert_events(
            db,
            &[(
                EventType::RelocateCards,
                serde_json::to_value(&payload).unwrap(),
            )],
            at,
            None,
        )
        .await?;
        event_ids.first().copied()
    } else {
        None
    };
    Ok(RelocateLiveCardsResponse {
        moved_card_count,
        event_id,
    })
}

/// Applies a [`EventType::RelocateCards`] event's payload, when undoing or redoing it
pub(crate) async fn relocate_cards_event(
    db: &SqlitePool,
    payload: &RelocateCardsPayload,
) -> Result<(), Error> {
    for card in &payload.deleted_cards {
        sqlx::query(r"DELETE FROM card WHERE id = ?")
            .bind(card.card.id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    for card in &payload.moved {
        sqlx::query(r#"UPDATE card SET note_id = ?, "order" = ?, back_type = ? WHERE id = ?"#)
            .bind(card.note_id.after)
            .bind(card.order.after)
            .bind(card.back_type.after)
            .bind(card.card_id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    for NoteCardSnapshot { note_id, card } in &payload.restored_cards {
        sqlx::query(
            r#"INSERT INTO card (id, note_id, "order", back_type, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(card.id)
        .bind(note_id)
        .bind(card.order)
        .bind(card.back_type)
        .bind(Utc::now().timestamp())
        .bind(card.due.timestamp())
        .bind(card.stability)
        .bind(card.difficulty)
        .bind(card.desired_retention)
        .bind(card.special_state)
        .bind(card.state)
        .bind(&card.custom_data)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(())
}
