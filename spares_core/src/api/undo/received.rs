//! Whether sync changed what an event did since: then it can't be undone (or redone), since that
//! would write back over the other side's change.
//!
//! An event can otherwise be undone also after a sync sent it to the other replicas, as the undo is
//! a change like any other, which the next sync sends them too. Deleting a review an undo takes
//! back included, through its grave. The triggers of migration `20261101130000_undo_after_sync`
//! record in `sync_received` each note, card, tag and parser that sync writes, with the newest
//! event then. An event can't be reverted once one of the rows it changed has a record from it on.

use std::collections::BTreeSet;

use serde::de::DeserializeOwned;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::api::undo::describe;
use crate::api::undo::payloads::CreateNotesPayload;
use crate::api::undo::payloads::CreateParserPayload;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::DeleteNotesPayload;
use crate::api::undo::payloads::DeleteParserPayload;
use crate::api::undo::payloads::DeleteTagPayload;
use crate::api::undo::payloads::DeletedTag;
use crate::api::undo::payloads::FilteredTagExit;
use crate::api::undo::payloads::ForgetCardPayload;
use crate::api::undo::payloads::RateCardPayload;
use crate::api::undo::payloads::RelocateCardsPayload;
use crate::api::undo::payloads::UnmoveCardsPayload;
use crate::api::undo::payloads::UnreviewPayload;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::api::undo::payloads::UpdateNotesPayload;
use crate::api::undo::payloads::UpdateParserPayload;
use crate::api::undo::payloads::UpdateTagPayload;
use crate::model::Event;
use crate::model::EventType;

/// What [`check_not_received`] checks `events` for, which its error names.
#[derive(Clone, Copy)]
pub(super) enum Reverting {
    Undo,
    Redo,
}

/// Errors if sync wrote any row one of `events` changed, after the event. The error names the
/// change, as undo describes it, not the event.
pub(super) async fn check_not_received(
    db: &SqlitePool,
    events: &[Event],
    reverting: Reverting,
) -> Result<(), Error> {
    for event in events {
        let mut received = false;
        for (kind, id) in changed_rows(event) {
            let after: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM sync_received WHERE kind = ? AND oid = ? AND event_id >= ?",
            )
            .bind(kind)
            .bind(id)
            .bind(event.id)
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
            if after.is_some() {
                received = true;
                break;
            }
        }
        if !received {
            continue;
        }
        // An undo is described as the action it undid
        let change = describe::summarize(db, event).await?.description;
        let message = match reverting {
            Reverting::Undo => format!(
                "\"{change}\" can no longer be undone, since a sync has changed the same notes or \
                 cards since"
            ),
            Reverting::Redo => format!(
                "The undo of \"{change}\" can no longer be redone, since a sync has changed the \
                 same notes or cards since"
            ),
        };
        return Err(Error::Library(LibraryError::InvalidConfig(message)));
    }
    Ok(())
}

/// The rows `event` changed, as `sync_received` names them: a note, card, tag or parser and its id.
fn changed_rows(event: &Event) -> BTreeSet<(&'static str, i64)> {
    let mut rows = Rows::default();
    match event.kind {
        EventType::CreateParser => rows.parser(payload::<CreateParserPayload>(event).id),
        EventType::DeleteParser => rows.parser(payload::<DeleteParserPayload>(event).id),
        EventType::UpdateParser => rows.parser(Some(payload::<UpdateParserPayload>(event).id)),
        EventType::CreateTag => {
            let payload: CreateTagPayload = payload(event);
            rows.tag(payload.id);
            rows.notes(payload.note_ids.iter().map(|id| id.packed()));
            rows.cards(payload.card_ids.iter().copied());
        }
        EventType::DeleteTag => {
            let payload: DeleteTagPayload = payload(event);
            rows.tag(payload.id);
            rows.notes(payload.note_ids.iter().map(|id| id.packed()));
            rows.cards(payload.card_ids.iter().copied());
        }
        EventType::UpdateTag => rows.tag(Some(payload::<UpdateTagPayload>(event).id)),
        EventType::CreateNotes => {
            let payload: CreateNotesPayload = payload(event);
            for note in &payload.notes {
                rows.notes([note.id.packed()]);
                rows.cards(note.cards.iter().map(|card| card.id));
            }
            rows.deleted_tags(&payload.restored_tags);
        }
        EventType::DeleteNotes => {
            let payload: DeleteNotesPayload = payload(event);
            for note in &payload.notes {
                rows.notes([note.id.packed()]);
                rows.cards(note.cards.iter().map(|card| card.id));
            }
            rows.deleted_tags(&payload.deleted_tags);
        }
        EventType::UpdateNotes => {
            let payload: UpdateNotesPayload = payload(event);
            for note in &payload.notes {
                rows.notes([note.id.packed()]);
                if let Some(cards) = &note.cards {
                    rows.cards(cards.before.iter().chain(&cards.after).map(|card| card.id));
                }
            }
            rows.deleted_tags(&payload.deleted_tags);
            rows.deleted_tags(&payload.restored_tags);
        }
        EventType::RelocateCards => {
            let payload: RelocateCardsPayload = payload(event);
            rows.notes(payload.note_ids().map(|id| id.packed()));
            rows.cards(payload.moved.iter().map(|card| card.card_id));
            rows.cards(
                payload
                    .deleted_cards
                    .iter()
                    .chain(&payload.restored_cards)
                    .map(|card| card.card.id),
            );
        }
        EventType::RateCard => {
            let payload: RateCardPayload = payload(event);
            rows.card_changes([&payload.card].into_iter().chain(&payload.buried_siblings));
            rows.left_tag(payload.left_tag.as_ref());
        }
        EventType::UnrateCard | EventType::UnforgetCard => {
            let payload: UnreviewPayload = payload(event);
            rows.card_changes([&payload.card].into_iter().chain(&payload.buried_siblings));
            rows.left_tag(payload.left_tag.as_ref());
        }
        EventType::ForgetCard => {
            rows.card_changes([&payload::<ForgetCardPayload>(event).card]);
        }
        EventType::UpdateCards
        | EventType::AdvanceCards
        | EventType::PostponeCards
        | EventType::BuryCards
        | EventType::UnburyCards
        | EventType::DailyUnbury
        | EventType::RescheduleCards => {
            rows.card_changes(&payload::<Vec<UpdateCardPayload>>(event));
        }
        EventType::UnadvanceCards | EventType::UnpostponeCards => {
            rows.card_changes(&payload::<UnmoveCardsPayload>(event).cards);
        }
    }
    rows.0
}

/// As `apply_event` reads it, which a stored event always has
fn payload<T: DeserializeOwned>(event: &Event) -> T {
    serde_json::from_value(event.payload.clone()).unwrap()
}

#[derive(Default)]
struct Rows(BTreeSet<(&'static str, i64)>);

impl Rows {
    fn parser(&mut self, id: Option<i64>) {
        self.0.extend(id.map(|id| ("parser", id)));
    }

    fn tag(&mut self, id: Option<i64>) {
        self.0.extend(id.map(|id| ("tag", id)));
    }

    fn notes(&mut self, ids: impl IntoIterator<Item = i64>) {
        self.0.extend(ids.into_iter().map(|id| ("note", id)));
    }

    fn cards(&mut self, ids: impl IntoIterator<Item = i64>) {
        self.0.extend(ids.into_iter().map(|id| ("card", id)));
    }

    fn card_changes<'a>(&mut self, changes: impl IntoIterator<Item = &'a UpdateCardPayload>) {
        self.cards(changes.into_iter().map(|change| change.card_id));
    }

    fn deleted_tags(&mut self, tags: &[DeletedTag]) {
        for tag in tags {
            self.tag(Some(tag.tag.id));
        }
    }

    fn left_tag(&mut self, left: Option<&FilteredTagExit>) {
        if let Some(left) = left {
            self.tag(Some(left.tag_id));
        }
    }
}
