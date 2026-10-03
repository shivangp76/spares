//! Human readable descriptions of events, so undo and redo can say what they reversed.

use sqlx::SqlitePool;

use crate::Error;
use crate::api::scheduler::get_scheduler_ratings;
use crate::api::undo::event_action;
use crate::api::undo::fetch_event;
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
use crate::model::ReviewLog;
use crate::schema::undo::EventSummary;

/// Summarizes the action that reverting `event` takes back (for an action or a redo) or brings
/// back (for an undo). Either way it is described as the action the user originally took, so
/// redoing "Rate card 3" reads the same as undoing it, rather than as "Unrate card 3".
///
/// Must run before `event` is reverted, since reverting a rating deletes its `review_log` row.
pub(crate) async fn summarize(db: &SqlitePool, event: &Event) -> Result<EventSummary, Error> {
    // An undo's payload is the inverse of the action, so describe the event it reverted instead.
    // Undoing a rating deleted its `review_log` row, but the undo kept it.
    let (action, review_log) = if event_action(db, event.id).await? == EventAction::Undo {
        let review_log = (event.kind == EventType::UnrateCard).then(|| {
            serde_json::from_value::<UnreviewPayload>(event.payload.clone())
                .unwrap()
                .review_log
        });
        (fetch_reverted(db, event).await?, review_log)
    } else {
        (event.clone(), None)
    };
    // A redo of an undone bury is an `UpdateCards`, so the verb comes from the original action
    let mut original = action.clone();
    while original.reverts_event_id.is_some() {
        original = fetch_reverted(db, &original).await?;
    }
    Ok(EventSummary {
        id: event.id,
        description: describe(db, &action, original.kind, review_log).await?,
    })
}

async fn fetch_reverted(db: &SqlitePool, event: &Event) -> Result<Event, Error> {
    let id = event
        .reverts_event_id
        .expect("only called on an event that reverts another");
    Ok(fetch_event(db, id)
        .await?
        .expect("reverts_event_id references an existing event"))
}

/// Describes `event`, which is an action or a redo, so its payload is in the action's direction.
/// `review_log` is the row of a rating whose row has since been deleted.
async fn describe(
    db: &SqlitePool,
    event: &Event,
    kind: EventType,
    review_log: Option<ReviewLog>,
) -> Result<String, Error> {
    let payload = event.payload.clone();
    let description = match kind {
        EventType::CreateParser => {
            let p: CreateParserPayload = serde_json::from_value(payload).unwrap();
            format!("Create parser '{}'", p.name)
        }
        EventType::UpdateParser => {
            let p: UpdateParserPayload = serde_json::from_value(payload).unwrap();
            match p.name {
                Some(name) => format!("Rename parser '{}' to '{}'", name.before, name.after),
                None => format!("Update parser {}", p.id),
            }
        }
        EventType::DeleteParser => {
            let p: DeleteParserPayload = serde_json::from_value(payload).unwrap();
            format!("Delete parser '{}'", p.name)
        }
        EventType::CreateTag => {
            let p: CreateTagPayload = serde_json::from_value(payload).unwrap();
            format!("Create tag '{}'", p.name)
        }
        EventType::UpdateTag => {
            let p: UpdateTagPayload = serde_json::from_value(payload).unwrap();
            if let Some(name) = p.name {
                format!("Rename tag '{}' to '{}'", name.before, name.after)
            } else {
                let name: Option<String> = sqlx::query_scalar(r"SELECT name FROM tag WHERE id = ?")
                    .bind(p.id)
                    .fetch_optional(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
                match name {
                    Some(name) => format!("Update tag '{name}'"),
                    None => format!("Update tag {}", p.id),
                }
            }
        }
        EventType::DeleteTag => {
            let p: DeleteTagPayload = serde_json::from_value(payload).unwrap();
            format!("Delete tag '{}'", p.name)
        }
        EventType::CreateNotes => {
            let p: CreateNotesPayload = serde_json::from_value(payload).unwrap();
            let ids: Vec<i64> = p.notes.iter().map(|n| n.id).collect();
            format!("Create {}", count_ids("note", &ids))
        }
        EventType::UpdateNotes => {
            let p: UpdateNotesPayload = serde_json::from_value(payload).unwrap();
            let ids: Vec<i64> = p.notes.iter().map(|n| n.id).collect();
            format!("Edit {}", count_ids("note", &ids))
        }
        EventType::DeleteNotes => {
            let p: DeleteNotesPayload = serde_json::from_value(payload).unwrap();
            let ids: Vec<i64> = p.notes.iter().map(|n| n.id).collect();
            format!("Delete {}", count_ids("note", &ids))
        }
        EventType::RateCard => {
            let p: RateCardPayload = serde_json::from_value(payload).unwrap();
            let card = format!("Rate card {}", p.card.card_id);
            let review_log = match review_log {
                Some(review_log) => Some(review_log),
                None => sqlx::query_as(r"SELECT * FROM review_log WHERE id = ?")
                    .bind(p.review_log_id)
                    .fetch_optional(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?,
            };
            match review_log.as_ref().and_then(rating_description) {
                Some(rating) => format!("{card} ({rating})"),
                None => card,
            }
        }
        EventType::ForgetCard => {
            let p: ForgetCardPayload = serde_json::from_value(payload).unwrap();
            format!("Forget card {}", p.card.card_id)
        }
        // Only ever undo events, which `summarize` never describes
        EventType::UnrateCard => "Unrate card".to_string(),
        EventType::UnforgetCard => "Unforget card".to_string(),
        EventType::UpdateCards
        | EventType::AdvanceCards
        | EventType::PostponeCards
        | EventType::BuryCards
        | EventType::UnburyCards => {
            let p: Vec<UpdateCardPayload> = serde_json::from_value(payload).unwrap();
            let ids: Vec<i64> = p.iter().map(|c| c.card_id).collect();
            let verb = match kind {
                EventType::AdvanceCards => "Advance",
                EventType::PostponeCards => "Postpone",
                EventType::BuryCards => "Bury",
                EventType::UnburyCards => "Unbury",
                _ => "Update",
            };
            format!("{verb} {}", count_ids("card", &ids))
        }
    };
    Ok(description)
}

/// The scheduler's name for the rating in `review_log`, if it has one.
fn rating_description(review_log: &ReviewLog) -> Option<String> {
    let rating = review_log.rating?;
    get_scheduler_ratings(&review_log.scheduler_name)
        .ok()?
        .into_iter()
        .find(|r| r.id == rating)
        .map(|r| r.description)
}

/// "note 3", or "3 notes (1, 2, 3)", listing at most a few ids.
fn count_ids(noun: &str, ids: &[i64]) -> String {
    const MAX_LISTED: usize = 5;
    if let [id] = ids {
        return format!("{noun} {id}");
    }
    let mut listed: Vec<String> = ids.iter().take(MAX_LISTED).map(i64::to_string).collect();
    if ids.len() > MAX_LISTED {
        listed.push("…".to_string());
    }
    format!("{} {noun}s ({})", ids.len(), listed.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_ids_formats_one_many_and_truncates() {
        assert_eq!(count_ids("note", &[3]), "note 3");
        assert_eq!(count_ids("card", &[1, 2]), "2 cards (1, 2)");
        assert_eq!(
            count_ids("note", &[1, 2, 3, 4, 5, 6, 7]),
            "7 notes (1, 2, 3, 4, 5, …)"
        );
    }
}
