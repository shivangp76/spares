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
//! - Schema for payload changes
//!   - Handled by `version` field

use chrono::Utc;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::api::undo::invert_payload::create_undo_event;
use crate::api::undo::payloads::CreateNotesPayload;
use crate::api::undo::payloads::CreateParserPayload;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::DeleteNotesPayload;
use crate::api::undo::payloads::DeleteParserPayload;
use crate::api::undo::payloads::DeleteTagPayload;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::api::undo::payloads::UpdateNotesPayload;
use crate::api::undo::payloads::UpdateParserPayload;
use crate::api::undo::payloads::UpdateTagPayload;
use crate::model::Event;
use crate::model::EventType;
use crate::schema::undo::UndoEventRequest;
use crate::schema::undo::UndoEventResponse;

const EVENT_VERSION: i64 = 1;

mod event_actions;
mod invert_payload;
pub use event_actions::create_event_group;
pub use event_actions::insert_events;
pub(crate) mod payloads;

#[cfg(test)]
mod e2e_tests;

pub async fn get_latest_note_event_id(db: &SqlitePool) -> Result<i64, Error> {
    let id: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM event WHERE kind IN (?, ?, ?)")
            .bind(EventType::CreateNotes)
            .bind(EventType::UpdateNotes)
            .bind(EventType::DeleteNotes)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    Ok(id)
}

pub async fn undo_event(
    db: &SqlitePool,
    body: UndoEventRequest,
) -> Result<Option<UndoEventResponse>, Error> {
    // Get the event to undo
    let event_opt: Option<Event> = if let Some(event_id) = body.event_id {
        sqlx::query_as(r"SELECT * FROM event WHERE id = ?")
            .bind(event_id)
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
    } else {
        // Get latest event
        sqlx::query_as("SELECT * FROM event ORDER BY id DESC LIMIT 1")
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
    };
    if event_opt.is_none() {
        return Ok(None);
    }
    let event = event_opt.unwrap();

    // If undoing a group, get all events in the group
    let events_to_undo = if body.undo_group
        && let Some(group_id) = event.group_id
    {
        let group_events: Vec<Event> =
            sqlx::query_as(r"SELECT * FROM event WHERE group_id = ? ORDER BY id ASC")
                .bind(group_id)
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        group_events
    } else {
        vec![event]
    };
    let undone_event_ids = events_to_undo.iter().map(|e| e.id).collect::<Vec<_>>();

    // Undo each event in reverse chronological order, validating dependencies just before each
    // application. Validating inline (rather than upfront) ensures that when undoing a group,
    // earlier undos in the same batch have already cleaned up their associations before the
    // later events are validated.
    for event in events_to_undo.iter().rev() {
        validate_undo_dependencies(db, event).await?;
        let undo_event = create_undo_event(db, event, Utc::now()).await?;
        let undo_event: Event = sqlx::query_as(r"SELECT * FROM event WHERE id = ?")
            .bind(undo_event.id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        apply_event(db, &undo_event).await?;
    }

    Ok(Some(UndoEventResponse { undone_event_ids }))
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
        EventType::CreateTag => {
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
        EventType::UpdateCards
        | EventType::RateCard
        | EventType::ForgetCard
        | EventType::AdvanceCards
        | EventType::PostponeCards
        | EventType::BuryCards
        | EventType::UnburyCards => {
            // Undo payload already has .after = value to restore (create_undo_event swapped it)
            let payloads: Vec<UpdateCardPayload> =
                serde_json::from_value(event.payload.clone()).unwrap();
            update_card_event(db, payloads, false).await?;
        }
    }
    Ok(())
}
