use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::model::CardId;
use crate::model::NoteId;
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
#[expect(clippy::too_many_lines)]
pub async fn relocate_live_cards(
    db: &SqlitePool,
    request: RelocateLiveCardsRequest,
    at: DateTime<Utc>,
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
    let cards: Vec<(CardId, NoteId, u32, i64, String)> = sqlx::query_as(
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
    let mut cards_by_uid: HashMap<&str, Vec<(CardId, NoteId, u32, i64)>> = HashMap::new();
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

    let mut moved_card_count = 0;
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
        for &(card_id, _, _, back_type) in elsewhere {
            let (order, back_type) =
                if let Some(&(created_card_id, _, order, back_type)) = here.next() {
                    sqlx::query(r"DELETE FROM card WHERE id = ?")
                        .bind(created_card_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                    (order, back_type)
                } else {
                    let next = next_parked_order
                        .entry(destination)
                        .or_insert(PARKED_CARD_ORDER_START);
                    *next += 1;
                    (*next - 1, back_type)
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
            moved_card_count += 1;
        }
    }

    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;
    Ok(RelocateLiveCardsResponse { moved_card_count })
}
