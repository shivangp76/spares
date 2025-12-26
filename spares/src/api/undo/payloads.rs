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
