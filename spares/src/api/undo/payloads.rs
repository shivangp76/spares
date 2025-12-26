use crate::{
    model::{Card, CardId, Note, NoteId, Parser, ReviewLog, SpecialState, StateId, Tag, TagId},
    parsers::BackType,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

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
