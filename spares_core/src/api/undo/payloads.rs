use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde_json::Value;

use crate::model::Card;
use crate::model::CardId;
use crate::model::Note;
use crate::model::NoteId;
use crate::model::Parser;
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteTagPayload {
    pub id: Option<TagId>,
    pub name: String,
    pub description: String,
    pub query: Option<String>,
    pub auto_delete: bool,
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
}

/// Payload for `DeleteNotes` event.
/// Contains full snapshots — so notes can be recreated when undoing a delete.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteNotesPayload {
    pub notes: Vec<NoteSnapshot>,
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RateCardPayload {
    pub review_log_id: i64,
    pub card: UpdateCardPayload,
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
