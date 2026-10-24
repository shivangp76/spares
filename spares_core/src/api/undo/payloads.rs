use chrono::DateTime;
use chrono::Utc;
use chrono::serde::ts_seconds;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::model::Card;
use crate::model::CardId;
use crate::model::NoteId;
use crate::model::ReviewLog;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::model::Tag;
use crate::model::TagId;
use crate::parsers::BackType;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateTagPayload {
    pub id: Option<TagId>,
    pub name: String,
    pub description: String,
    pub query: Option<String>,
    pub auto_delete: bool,
    /// Absent from events recorded before tags had it, which all rescheduled.
    #[serde(default = "reschedule_default")]
    pub reschedule: bool,
    #[serde(with = "ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub note_ids: Vec<NoteId>,
    #[serde(default)]
    pub card_ids: Vec<CardId>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateTagPayload {
    pub id: TagId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Transition<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Transition<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<Transition<Option<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_delete: Option<Transition<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reschedule: Option<Transition<bool>>,
}

fn reschedule_default() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteTagPayload {
    pub id: Option<TagId>,
    pub name: String,
    pub description: String,
    pub query: Option<String>,
    pub auto_delete: bool,
    /// Absent from events recorded before tags had it, which all rescheduled.
    #[serde(default = "reschedule_default")]
    pub reschedule: bool,
    #[serde(with = "ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub note_ids: Vec<NoteId>,
    #[serde(default)]
    pub card_ids: Vec<CardId>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transition<T> {
    #[serde(rename = "b")]
    pub before: T,
    #[serde(rename = "a")]
    pub after: T,
}
type FieldChange<T> = Option<Transition<T>>;

impl<T> Transition<T> {
    pub fn swap(self) -> Self {
        Self {
            before: self.after,
            after: self.before,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateParserPayload {
    // The parser id is needed so if the user is undoing a delete parser event, then that exact parser id can be restored. This way notes still reference the correct parser id.
    //
    // If the parser id is not stored, then the following scenario will not work:
    // - Create Note
    // - Create Tag
    // - Tag Note
    // - Undo create tag: Delete tag (This will save the ntoe id)
    // - Undo create note: Delete note
    // - Undo delete note: Create note (This wil have a different note id)
    // - Undo delete tag: Create tag (This tag won't restore to the correct note id because the note id changed.)
    pub id: Option<i64>,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateParserPayload {
    pub id: i64,
    pub name: FieldChange<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteParserPayload {
    pub id: Option<i64>,
    pub name: String,
    /// Note ids that used the parser
    pub note_ids: Vec<NoteId>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CardSnapshot {
    pub id: CardId,
    pub order: u32,
    pub back_type: BackType,
    pub due: DateTime<Utc>,
    pub stability: f64,
    pub difficulty: f64,
    pub desired_retention: f64,
    pub special_state: Option<SpecialState>,
    pub state: StateId,
    pub custom_data: Value,
}

impl CardSnapshot {
    pub fn from_card(card: &Card) -> Self {
        Self {
            id: card.id,
            order: card.order,
            back_type: card.back_type,
            due: card.due,
            stability: card.stability,
            difficulty: card.difficulty,
            desired_retention: card.desired_retention,
            special_state: card.special_state,
            state: card.state,
            custom_data: card.custom_data.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NoteSnapshot {
    pub id: NoteId,
    pub data: String,
    pub created_at: DateTime<Utc>,
    pub parser_id: i64,
    pub custom_data: Value,
    /// Non-embedded keyword strings
    pub keywords: Vec<String>,
    /// Non-filtered tag names
    pub tags: Vec<String>,
    pub cards: Vec<CardSnapshot>,
}

/// Payload for `CreateNotes` event.
/// Contains full snapshots — used both for normal create logging and for undoing a `DeleteNotes`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateNotesPayload {
    pub notes: Vec<NoteSnapshot>,
    /// Carried from `DeleteNotesPayload::card_links` when undoing a delete
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub card_links: Vec<CardLinks>,
    /// Carried from `DeleteNotesPayload::deleted_tags` when undoing a delete, and restored before
    /// the notes link to them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restored_tags: Vec<Tag>,
}

/// Payload for `DeleteNotes` event.
/// Contains full snapshots — so notes can be recreated when undoing a delete.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteNotesPayload {
    pub notes: Vec<NoteSnapshot>,
    /// What deleting the notes' cards detached from them, for cards that had any. Empty in events
    /// recorded before it was kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub card_links: Vec<CardLinks>,
    /// [`auto_delete`](Tag::auto_delete) tags deleted because these notes were the last to have
    /// them, so undo can restore them as they were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted_tags: Vec<Tag>,
}

/// What deleting a card detached from it, so undoing the delete can reattach it
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct CardLinks {
    pub card_id: CardId,
    /// Its reviews, which the delete kept with no card (`review_log.card_id` is `ON DELETE SET
    /// NULL`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_log_ids: Vec<i64>,
    /// The filtered tags it was in. A note snapshot's tags are only its own, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filtered_tag_ids: Vec<TagId>,
}

/// Per-note transition payload for `UpdateNotes`
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateNotePayload {
    pub id: NoteId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Transition<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parser_id: Option<Transition<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Transition<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Transition<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_data: Option<Transition<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cards: Option<Transition<Vec<CardSnapshot>>>,
}

impl UpdateNotePayload {
    pub fn has_changes(&self) -> bool {
        self.data.is_some()
            || self.parser_id.is_some()
            || self.keywords.is_some()
            || self.tags.is_some()
            || self.custom_data.is_some()
            || self.cards.is_some()
    }
}

/// Payload for `UpdateNotes` event
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateNotesPayload {
    pub notes: Vec<UpdateNotePayload>,
    /// [`auto_delete`](Tag::auto_delete) tags deleted because the notes no longer had them, so
    /// undo can restore them as they were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted_tags: Vec<Tag>,
    /// Tags to restore before the notes link to them: an undo's, swapped from the `deleted_tags`
    /// of the update it reverts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restored_tags: Vec<Tag>,
}

/// Payload for a `ForgetCard` event.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ForgetCardPayload {
    pub review_log_id: i64,
    pub card: UpdateCardPayload,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RateCardPayload {
    /// The review the rating wrote, or for a rating in a filtered tag that does not reschedule
    /// ([`Tag::reschedule`](crate::model::Tag::reschedule)), its
    /// [cram](crate::model::ReviewLogKind::Cram). `None` for such a rating from before crams were
    /// logged, which wrote nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_log_id: Option<i64>,
    pub card: UpdateCardPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_tag: Option<FilteredTagExit>,
    /// The card's siblings the rating buried, as
    /// [`QueueConfig::bury_new_siblings`](crate::config::QueueConfig::bury_new_siblings) and
    /// [`QueueConfig::bury_review_siblings`](crate::config::QueueConfig::bury_review_siblings) say
    /// to, so undoing the rating unburies them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buried_siblings: Vec<UpdateCardPayload>,
}

/// A card leaving the filtered tag that its review finished, kept so undo can put it back.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FilteredTagExit {
    pub tag_id: TagId,
    /// The tag itself, when the card was the last one in an auto-delete tag and the tag was
    /// deleted with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_tag: Option<Tag>,
}

/// Payload for `UnrateCard` and `UnforgetCard`, the undos of `RateCard` and `ForgetCard`.
///
/// Undoing those deletes their `review_log` row, so the row is kept here for a redo to reinsert.
/// Its `id` is not reused on reinsert, since SQLite may have handed it to a newer row.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UnreviewPayload {
    /// `None` when the rating wrote no row (see `RateCardPayload::review_log_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_log: Option<ReviewLog>,
    pub card: UpdateCardPayload,
    /// Carried from `RateCardPayload::left_tag`; always `None` for `UnforgetCard`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_tag: Option<FilteredTagExit>,
    /// Carried from `RateCardPayload::buried_siblings`, swapped; always empty for
    /// `UnforgetCard`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buried_siblings: Vec<UpdateCardPayload>,
}

/// Payload for `UnadvanceCards` and `UnpostponeCards`, the undos of `AdvanceCards` and
/// `PostponeCards`.
///
/// Undoing those deletes the [`ReviewLogKind::Reschedule`](crate::model::ReviewLogKind::Reschedule)
/// rows they wrote, so the rows are kept here for a redo to reinsert.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UnmoveCardsPayload {
    /// The moves, swapped.
    pub cards: Vec<UpdateCardPayload>,
    /// Empty for a move from before moves were logged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_logs: Vec<ReviewLog>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateCardPayload {
    // This can't be a Vec<CardId> since each card will have a different old copy of the data.
    pub card_id: CardId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: FieldChange<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_type: FieldChange<BackType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: FieldChange<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stability: FieldChange<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difficulty: FieldChange<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desired_retention: FieldChange<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub special_state: FieldChange<Option<SpecialState>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: FieldChange<StateId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_data: FieldChange<Value>,
}

impl UpdateCardPayload {
    /// Swaps before and after on every field, giving the payload that reverses this one.
    pub fn swap(self) -> Self {
        Self {
            card_id: self.card_id,
            order: self.order.map(Transition::swap),
            back_type: self.back_type.map(Transition::swap),
            due: self.due.map(Transition::swap),
            stability: self.stability.map(Transition::swap),
            difficulty: self.difficulty.map(Transition::swap),
            desired_retention: self.desired_retention.map(Transition::swap),
            special_state: self.special_state.map(Transition::swap),
            state: self.state.map(Transition::swap),
            custom_data: self.custom_data.map(Transition::swap),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn notes_payloads_from_before_card_links_and_deleted_tags_deserialize() {
        let note = json!({
            "id": 1, "data": "x", "created_at": "2026-10-01T00:00:00Z", "parser_id": 8,
            "custom_data": {}, "keywords": [], "tags": ["a"], "cards": [],
        });
        let delete: DeleteNotesPayload = serde_json::from_value(json!({"notes": [note]})).unwrap();
        assert!(delete.card_links.is_empty() && delete.deleted_tags.is_empty());
        let create: CreateNotesPayload = serde_json::from_value(json!({"notes": [note]})).unwrap();
        assert!(create.card_links.is_empty() && create.restored_tags.is_empty());
        let update: UpdateNotesPayload =
            serde_json::from_value(json!({"notes": [{"id": 1}]})).unwrap();
        assert!(update.deleted_tags.is_empty() && update.restored_tags.is_empty());
        // And they are written as before when there is nothing to add
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            json!({"notes": [{"id": 1}]})
        );
    }
}
