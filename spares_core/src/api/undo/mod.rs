//! # Undo Functionality
//!
//! ## Outline
//! - To undo an event, you append a new event to the event log that reverses the previous event. For example, to undo `AddNote`, you append a `DeleteNote` event.
//!
//! ## Problems and Solutions
//! - Future: Syncing data between devices
//!   - Last write wins. This is why there is a `timestamp` field. Events are merged and then replayed in chronological order. If there is a conflict, pick random one for now. This can be worked out later. (Pretty unlikely there is a conflict at the exact same timestamp.)
//!   - Future: Add `device_id` field to `Event`
//! - Branching undo logs
//!   - Events can be undone by their id, so the user can submit the exact action they want to undo.
//!   - If the user does `create parser -> add note to parser -> UNDO: create parser`, then throw error saying cannot delete parser since notes depend on it.
//! - Importing a bunch of notes at once. This should all be undone at once.
//!   - `group_id` field, so all those actions will be undone at once
//! - Redo
//!   - Every undo records the event it reverses in `reverts_event_id`. Redoing an undo reverses it
//!     the same way, so a redo is an event that reverts an undo. The `event_action` view derives
//!     whether an event is an action, an undo or a redo from that chain.

use std::collections::BTreeSet;

use chrono::Utc;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::api::review::leave_filtered_tag;
use crate::api::review::rejoin_filtered_tag;
use crate::api::undo::invert_payload::create_undo_event;
use crate::api::undo::payloads::CreateNotesPayload;
use crate::api::undo::payloads::CreateParserPayload;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::DeleteNotesPayload;
use crate::api::undo::payloads::DeleteParserPayload;
use crate::api::undo::payloads::DeleteTagPayload;
use crate::api::undo::payloads::ForgetCardPayload;
use crate::api::undo::payloads::RateCardPayload;
use crate::api::undo::payloads::UnreviewPayload;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::api::undo::payloads::UpdateNotesPayload;
use crate::api::undo::payloads::UpdateParserPayload;
use crate::api::undo::payloads::UpdateTagPayload;
use crate::model::Event;
use crate::model::EventAction;
use crate::model::EventType;
use crate::model::NoteId;
use crate::notes_vcs;
use crate::schema::undo::EventSummary;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::RedoEventResponse;
use crate::schema::undo::UndoEventRequest;
use crate::schema::undo::UndoEventResponse;

mod describe;
mod event_actions;
mod invert_payload;
pub use event_actions::create_event_group;
pub use event_actions::insert_events;
pub(crate) mod payloads;

#[cfg(test)]
mod e2e_tests;

/// A number that grows whenever notes change: the newest note event id, plus an offset that each
/// sync changing notes moves on (see [`crate::sync::NOTES_VERSION_OFFSET_KEY`]). For noticing
/// changes, not for passing to undo.
pub async fn get_latest_note_event_id(db: &SqlitePool) -> Result<i64, Error> {
    let id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0)
             + COALESCE((SELECT CAST(value AS INTEGER) FROM app_state WHERE key = ?), 0)
         FROM event WHERE kind IN (?, ?, ?)",
    )
    .bind(crate::sync::NOTES_VERSION_OFFSET_KEY)
    .bind(EventType::CreateNotes)
    .bind(EventType::UpdateNotes)
    .bind(EventType::DeleteNotes)
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(id)
}

/// SQL condition that event `e` has not been reverted, i.e. undone (or, for an undo, redone).
const NOT_REVERTED: &str = "NOT EXISTS (SELECT 1 FROM event r WHERE r.reverts_event_id = e.id)";

async fn fetch_event(db: &SqlitePool, id: i64) -> Result<Option<Event>, Error> {
    sqlx::query_as(r"SELECT * FROM event WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

async fn event_action(db: &SqlitePool, id: i64) -> Result<EventAction, Error> {
    sqlx::query_scalar(r"SELECT action FROM event_action WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

async fn is_reverted(db: &SqlitePool, id: i64) -> Result<bool, Error> {
    sqlx::query_scalar(r"SELECT EXISTS (SELECT 1 FROM event WHERE reverts_event_id = ?)")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

/// `event`, or with `whole_group`, every event in its group that has not been reverted yet.
async fn expand_group(
    db: &SqlitePool,
    event: Event,
    whole_group: bool,
) -> Result<Vec<Event>, Error> {
    let Some(group_id) = event.group_id.filter(|_| whole_group) else {
        return Ok(vec![event]);
    };
    sqlx::query_as(&format!(
        "SELECT e.* FROM event e WHERE e.group_id = ? AND {NOT_REVERTED} ORDER BY e.id ASC"
    ))
    .bind(group_id)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })
}

/// Errors if any of `events` happened before the last sync: what it did has reached the other
/// replica, which may have changed the same rows since.
async fn check_not_synced(db: &SqlitePool, events: &[Event]) -> Result<(), Error> {
    let guard: Option<String> = sqlx::query_scalar("SELECT value FROM app_state WHERE key = ?")
        .bind(crate::sync::LAST_SYNC_EVENT_ID_KEY)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let guard: i64 = guard.and_then(|g| g.parse().ok()).unwrap_or(0);
    if let Some(event) = events.iter().find(|event| event.id <= guard) {
        return Err(Error::Library(LibraryError::InvalidConfig(format!(
            "Event {} happened before the last sync, so it can no longer be reverted",
            event.id
        ))));
    }
    Ok(())
}

/// Errors unless `event` can be reverted by an undo (`redo == false`), which takes an action or a
/// redo, or by a redo, which takes an undo. Either way it must not have been reverted already.
async fn check_revertible(db: &SqlitePool, event: &Event, redo: bool) -> Result<(), Error> {
    let is_undo = event_action(db, event.id).await? == EventAction::Undo;
    if is_undo != redo {
        let msg = if redo {
            format!(
                "Event {} is not an undo, so there is nothing to redo",
                event.id
            )
        } else {
            format!("Event {} is an undo. Use redo to reverse it", event.id)
        };
        return Err(Error::Library(LibraryError::InvalidConfig(msg)));
    }
    if is_reverted(db, event.id).await? {
        let verb = if redo { "redone" } else { "undone" };
        return Err(Error::Library(LibraryError::InvalidConfig(format!(
            "Event {} has already been {verb}",
            event.id
        ))));
    }
    Ok(())
}

/// Undoes an action, or redoes an undo, by appending and applying the events that reverse
/// `events`. Returns the ids of the new events, oldest first.
///
/// When reversing several events, the new events form their own group (rather than joining the
/// group of `events`), so that the whole undo can be redone at once.
async fn revert_events(db: &SqlitePool, events: &[Event]) -> Result<Vec<i64>, Error> {
    let at = Utc::now();
    let mut group_id = None;
    let mut new_event_ids = Vec::with_capacity(events.len());
    // Reverse each event in reverse chronological order, validating dependencies just before each
    // application. Validating inline (rather than upfront) ensures that when reversing a group,
    // earlier reversals in the same batch have already cleaned up their associations before the
    // later events are validated.
    for event in events.iter().rev() {
        validate_undo_dependencies(db, event).await?;
        let mut new_event = create_undo_event(db, event, at, group_id).await?;
        if events.len() > 1 && group_id.is_none() {
            // Same convention as `create_event_group`: the group id is the first event's id
            sqlx::query(r"UPDATE event SET group_id = ? WHERE id = ?")
                .bind(new_event.id)
                .bind(new_event.id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            group_id = Some(new_event.id);
            new_event.group_id = group_id;
        }
        apply_event(db, &new_event).await?;
        new_event_ids.push(new_event.id);
    }
    Ok(new_event_ids)
}

/// Describes `events` before they are reverted, since reverting can delete what describes them.
async fn summarize_all(db: &SqlitePool, events: &[Event]) -> Result<Vec<EventSummary>, Error> {
    let mut summaries = Vec::with_capacity(events.len());
    for event in events {
        summaries.push(describe::summarize(db, event).await?);
    }
    Ok(summaries)
}

/// Undoes `body.event_id`, or if `None`, the latest action or redo that has not been undone.
pub async fn undo_event(
    db: &SqlitePool,
    body: UndoEventRequest,
) -> Result<Option<UndoEventResponse>, Error> {
    let event = if let Some(event_id) = body.event_id {
        let Some(event) = fetch_event(db, event_id).await? else {
            return Ok(None);
        };
        check_revertible(db, &event, false).await?;
        event
    } else {
        let event_opt: Option<Event> = sqlx::query_as(&format!(
            "SELECT e.* FROM event e JOIN event_action a ON a.id = e.id
             WHERE a.action != ? AND {NOT_REVERTED} ORDER BY e.id DESC LIMIT 1"
        ))
        .bind(EventAction::Undo)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        let Some(event) = event_opt else {
            return Ok(None);
        };
        event
    };

    let events = expand_group(db, event, body.undo_group).await?;
    check_not_synced(db, &events).await?;
    let undone_events = summarize_all(db, &events).await?;
    let note_ids = affected_note_ids(db, &events).await?;
    let reverted = revert_events(db, &events).await;
    // Undo changes the database only, so the notes' files are regenerated from it, also if part
    // of the group was already reverted when an event failed.
    notes_vcs::reconcile_after(db, note_ids, "undo").await;
    let undo_event_ids = reverted?;
    Ok(Some(UndoEventResponse {
        undone_events,
        undo_event_ids,
    }))
}

/// Redoes the undo `body.event_id`, or if `None`, the latest undo that has not been redone.
///
/// Like an editor, a new action clears what can be redone: with no id, an undo older than the
/// latest action is never picked, even if that action has since been undone too.
pub async fn redo_event(
    db: &SqlitePool,
    body: RedoEventRequest,
) -> Result<Option<RedoEventResponse>, Error> {
    let event = if let Some(event_id) = body.event_id {
        let Some(event) = fetch_event(db, event_id).await? else {
            return Ok(None);
        };
        check_revertible(db, &event, true).await?;
        event
    } else {
        let event_opt: Option<Event> = sqlx::query_as(&format!(
            "SELECT e.* FROM event e JOIN event_action a ON a.id = e.id
             WHERE a.action = ? AND {NOT_REVERTED}
               AND NOT EXISTS (SELECT 1 FROM event_action d WHERE d.action = ? AND d.id > e.id)
             ORDER BY e.id DESC LIMIT 1"
        ))
        .bind(EventAction::Undo)
        .bind(EventAction::Do)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        let Some(event) = event_opt else {
            return Ok(None);
        };
        event
    };

    let events = expand_group(db, event, body.redo_group).await?;
    check_not_synced(db, &events).await?;
    let redone_events = summarize_all(db, &events).await?;
    let note_ids = affected_note_ids(db, &events).await?;
    let reverted = revert_events(db, &events).await;
    notes_vcs::reconcile_after(db, note_ids, "redo").await;
    let redo_event_ids = reverted?;
    Ok(Some(RedoEventResponse {
        redone_events,
        redo_event_ids,
    }))
}

/// Notes whose files reverting `events` changes: notes they create, delete or update, and notes
/// with a tag they rename or delete, or with a parser they rename.
async fn affected_note_ids(db: &SqlitePool, events: &[Event]) -> Result<Vec<NoteId>, Error> {
    let mut ids: BTreeSet<NoteId> = BTreeSet::new();
    for event in events {
        let payload = &event.payload;
        if let Some(notes) = payload.get("notes").and_then(|n| n.as_array()) {
            ids.extend(
                notes
                    .iter()
                    .filter_map(|n| n.get("id")?.as_i64())
                    .map(NoteId::from_packed),
            );
        }
        if let Some(note_ids) = payload.get("note_ids").and_then(|n| n.as_array()) {
            ids.extend(
                note_ids
                    .iter()
                    .filter_map(serde_json::Value::as_i64)
                    .map(NoteId::from_packed),
            );
        }
        let query = match event.kind {
            EventType::UpdateTag => Some(r"SELECT note_id FROM note_tag WHERE tag_id = ?"),
            EventType::UpdateParser => Some(r"SELECT id FROM note WHERE parser_id = ?"),
            _ => None,
        };
        if let (Some(query), Some(id)) = (query, payload.get("id").and_then(|i| i.as_i64())) {
            let rows: Vec<NoteId> = sqlx::query_scalar(query)
                .bind(id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            ids.extend(rows);
        }
    }
    Ok(ids.into_iter().collect())
}

// Validates that undoing `event` won't violate referential integrity.
//
// Two cases require explicit checks; all others are either enforced by DB constraints or
// are safe by construction:
//
// - Undoing `CreateParser` deletes the parser. The DB RESTRICT FK prevents this when notes
//   still reference it, but we check early to surface a clear error message.
// - Undoing `DeleteParser` re-creates the parser. This is normally safe (RESTRICT means the
//   parser couldn't have been deleted while notes referenced it), but we guard against
//   inconsistent event-log state where the deletion was logged without actually succeeding.
//
// `DeleteTag` needs no check: after a real tag deletion `note_tag`/`card_tag` are already gone
// via CASCADE. Undoing `CreateTag` (which deletes the tag) is blocked if associations still exist,
// to prevent silently losing note/card tag data.
async fn validate_undo_dependencies(db: &SqlitePool, event: &Event) -> Result<(), Error> {
    match event.kind {
        EventType::CreateParser => {
            // Undoing CreateParser will delete the parser — block if notes still reference it.
            let payload: CreateParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            if let Some(id) = payload.id {
                let note_count: i64 =
                    sqlx::query_scalar(r"SELECT COUNT(*) FROM note WHERE parser_id = ?")
                        .bind(id)
                        .fetch_one(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                if note_count > 0 {
                    return Err(Error::Library(LibraryError::InvalidConfig(format!(
                        "Cannot undo CreateParser: {} notes still depend on parser '{}'",
                        note_count, payload.name
                    ))));
                }
            }
        }
        // A `CreateTag` that is the undo of a `DeleteTag` restored the associations that delete
        // removed, so redoing the delete removes them again, as the user originally asked.
        // `create_undo_payload` keeps them for another undo.
        EventType::CreateTag if event.reverts_event_id.is_none() => {
            // Undoing CreateTag will delete the tag (cascade-deleting note/card associations).
            // Block if any associations exist to prevent silent data loss.
            let payload: CreateTagPayload = serde_json::from_value(event.payload.clone()).unwrap();
            if let Some(id) = payload.id {
                let note_tag_count: i64 =
                    sqlx::query_scalar(r"SELECT COUNT(*) FROM note_tag WHERE tag_id = ?")
                        .bind(id)
                        .fetch_one(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                let card_tag_count: i64 =
                    sqlx::query_scalar(r"SELECT COUNT(*) FROM card_tag WHERE tag_id = ?")
                        .bind(id)
                        .fetch_one(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                if note_tag_count > 0 || card_tag_count > 0 {
                    return Err(Error::Library(LibraryError::InvalidConfig(format!(
                        "Cannot undo CreateTag: {} note tags and {} card tags still reference tag '{}'",
                        note_tag_count, card_tag_count, payload.name
                    ))));
                }
            }
        }
        EventType::DeleteParser => {
            // Guard against inconsistent state where a DeleteParser event was logged but
            // notes still reference the parser id.
            let payload: DeleteParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            let note_count: i64 =
                sqlx::query_scalar(r"SELECT COUNT(*) FROM note WHERE parser_id = ?")
                    .bind(payload.id)
                    .fetch_one(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
            if note_count > 0 {
                return Err(Error::Library(LibraryError::InvalidConfig(format!(
                    "Cannot undo DeleteParser: {} notes still depend on parser '{}'",
                    note_count, payload.name
                ))));
            }
        }
        _ => {}
    }
    Ok(())
}

async fn apply_event(db: &SqlitePool, event: &Event) -> Result<(), Error> {
    use crate::api::card::update_card_event;
    use crate::api::note::create_notes_event;
    use crate::api::note::delete_notes_event;
    use crate::api::note::update_notes_event;
    use crate::api::parser::create_parser_event;
    use crate::api::parser::delete_parser_event;
    use crate::api::parser::update_parser_event;
    use crate::api::tag::create_tag_event;
    use crate::api::tag::delete_tag_event;
    use crate::api::tag::update_tag_event;

    match event.kind {
        EventType::CreateParser => {
            let payload: CreateParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            create_parser_event(db, payload, false).await?;
        }
        EventType::DeleteParser => {
            let payload: DeleteParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            delete_parser_event(db, payload, false).await?;
        }
        EventType::UpdateParser => {
            let payload: UpdateParserPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            let id = payload.id;
            // Undo payload already has .after = value to restore (create_undo_event swapped it)
            update_parser_event(db, payload, id, false).await?;
        }
        EventType::CreateTag => {
            let payload: CreateTagPayload = serde_json::from_value(event.payload.clone()).unwrap();
            create_tag_event(db, payload, false).await?;
        }
        EventType::DeleteTag => {
            let payload: DeleteTagPayload = serde_json::from_value(event.payload.clone()).unwrap();
            delete_tag_event(db, payload, false).await?;
        }
        EventType::UpdateTag => {
            let payload: UpdateTagPayload = serde_json::from_value(event.payload.clone()).unwrap();
            let id = payload.id;
            // Undo payload already has .after = value to restore (create_undo_event swapped it)
            update_tag_event(db, payload, id, false).await?;
        }
        EventType::CreateNotes => {
            let payload: CreateNotesPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            create_notes_event(db, payload, false).await?;
        }
        EventType::DeleteNotes => {
            let payload: DeleteNotesPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            delete_notes_event(db, payload, false).await?;
        }
        EventType::UpdateNotes => {
            let payload: UpdateNotesPayload =
                serde_json::from_value(event.payload.clone()).unwrap();
            // Undo payload already has .after = value to restore (create_undo_event swapped it)
            update_notes_event(db, payload, false).await?;
        }
        // For every card kind, the payload already has .after = value to restore
        // (create_undo_event swapped it). Their `review_log` rows were handled while inverting.
        EventType::RateCard => {
            let payload: RateCardPayload = serde_json::from_value(event.payload.clone()).unwrap();
            let card_id = payload.card.card_id;
            update_card_event(db, vec![payload.card], false).await?;
            if let Some(left_tag) = payload.left_tag {
                leave_filtered_tag(db, card_id, left_tag.tag_id, left_tag.deleted_tag.is_some())
                    .await?;
            }
        }
        EventType::ForgetCard => {
            let payload: ForgetCardPayload = serde_json::from_value(event.payload.clone()).unwrap();
            update_card_event(db, vec![payload.card], false).await?;
        }
        EventType::UnrateCard | EventType::UnforgetCard => {
            let payload: UnreviewPayload = serde_json::from_value(event.payload.clone()).unwrap();
            let card_id = payload.card.card_id;
            update_card_event(db, vec![payload.card], false).await?;
            if let Some(left_tag) = payload.left_tag {
                rejoin_filtered_tag(db, card_id, &left_tag).await?;
            }
        }
        EventType::UpdateCards
        | EventType::AdvanceCards
        | EventType::PostponeCards
        | EventType::BuryCards
        | EventType::UnburyCards => {
            let payloads: Vec<UpdateCardPayload> =
                serde_json::from_value(event.payload.clone()).unwrap();
            update_card_event(db, payloads, false).await?;
        }
    }
    Ok(())
}
