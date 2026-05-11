use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sqlx::FromRow;

#[derive(Debug, Deserialize, FromRow, Serialize)]
pub struct DbNoteRow {
    pub id: i64,
    pub flds: String,
    pub tags: String,
}

#[derive(Debug, Deserialize, FromRow, Serialize)]
pub struct DbCardRow {
    pub id: i64,
    pub queue: i64,
    pub r#type: i64,
    pub due: i64,
    pub data: Value,
}

#[derive(Debug, Deserialize, FromRow, Serialize)]
pub struct DbRevLogRow {
    pub id: i64,     // reviewed_at, but this is in milliseconds
    pub ease: i64,   // rating
    pub r#type: i64, // new state of the card
    pub ivl: i64,    // scheduled_time
    pub time: i64,   // duration, but this is in milliseconds
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum ModelName {
    #[serde(rename = "Basic")]
    Basic,
    #[serde(rename = "Basic (and reversed card)")]
    BasicAndReversed,
    #[serde(rename = "Cloze")]
    Cloze,
}

// API
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApiRequest {
    pub action: ApiAction,
    pub params: ApiRequestParams,
    pub version: u32, // 6
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ApiAction {
    #[serde(rename = "addNote")]
    AddNote,
    #[serde(rename = "updateNote")]
    UpdateNote,
    #[serde(rename = "deleteNotes")]
    DeleteNote,
    #[serde(rename = "guiBrowse")]
    GuiBrowse,
    #[serde(rename = "findCards")]
    FindCards,
    #[serde(rename = "suspend")]
    Suspend,
    #[serde(rename = "modelFieldNames")]
    GetModelFieldNames,
    #[serde(rename = "modelFieldAdd")]
    AddFieldToModel,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ApiRequestParams {
    AddNote(AddNoteApiRequestData),
    UpdateNote(UpdateNoteApiRequestData),
    DeleteNote(DeleteNoteApiRequestData),
    GuiBrowse(GuiBrowseApiRequestData),
    FindCards(FindCardsApiRequestData),
    Suspend(SuspendApiRequestData),
    GetModelFieldNames(GetModelFieldNamesApiRequestData),
    AddFieldToModel(AddFieldToModelApiRequestData),
}

// General Note Fields
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NoteFields {
    #[serde(rename = "Front", skip_serializing_if = "Option::is_none")]
    pub front: Option<String>,
    #[serde(rename = "Back", skip_serializing_if = "Option::is_none")]
    pub back: Option<String>,
    #[serde(rename = "Keywords", skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    #[serde(rename = "SparesId", skip_serializing_if = "Option::is_none")]
    pub spares_id: Option<String>,
    #[serde(rename = "SparesParserName", skip_serializing_if = "Option::is_none")]
    pub spares_parser_name: Option<String>,
}

// Add Note
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddNoteApiRequestData {
    pub note: AddNoteApiRequestNoteData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddNoteApiRequestNoteData {
    #[serde(rename = "deckName")]
    pub deck_name: String,
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
    pub fields: NoteFields,
    pub tags: Vec<String>,
    pub options: AddNoteApiRequestOptions,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct AddNoteApiRequestOptions {
    #[serde(rename = "allowDuplicate")]
    pub allow_duplicate: bool, // True
}

// Update Note Fields
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateNoteApiRequestData {
    pub note: UpdateNoteApiRequestNoteData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateNoteApiRequestNoteData {
    #[serde(rename = "deckName")]
    pub deck_name: String,
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
    pub id: i64,
    pub fields: NoteFields,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

// Delete Note
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteNoteApiRequestData {
    pub notes: Vec<i64>,
}

// Gui Browse
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GuiBrowseApiRequestData {
    pub query: String,
}

// Find Cards
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FindCardsApiRequestData {
    pub query: String,
}

// Suspend
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SuspendApiRequestData {
    pub cards: Vec<i64>,
}

// Get model field names
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetModelFieldNamesApiRequestData {
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
}

// Add field to model
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddFieldToModelApiRequestData {
    #[serde(rename = "modelName")]
    pub model_name: ModelName,
    #[serde(rename = "fieldName")]
    pub field_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}
