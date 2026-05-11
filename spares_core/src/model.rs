//! This file should match with the migrations file. These types should follow <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.

use chrono::DateTime;
use chrono::Utc;
use chrono::serde::ts_seconds;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use sqlx::FromRow;

use crate::parsers::BackType;

pub type NoteId = i64;
pub type CardId = i64;
pub type TagId = i64;
pub type StateId = u32;
pub type RatingId = u32;
pub type Score = f64;
pub type CustomData = Map<String, Value>;

pub const NEW_CARD_STATE: StateId = 0;
pub const DEFAULT_DESIRED_RETENTION: f64 = 0.9;
pub const NOTE_ID_KEY: &str = "note-id";

#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct Note {
    pub id: NoteId,
    // Note data is stored directly as it is received from the user and contains cloze delimiters as specified by the parser. Thus, the parser is needed in order for this to make sense.
    pub data: String,
    #[serde(with = "ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    pub updated_at: DateTime<Utc>,
    pub parser_id: i64,
    /// Stored as JSON. Note that this is not guaranteed to be ordered.
    /// This is guaranteed to be of type `Value::Object(Map<String, Value>)`.
    pub custom_data: Value,
}

/// Used for referencing other notes.
/// - One note can have multiple keywords. For example, 1 theorem might be explained in multiple books, so all those books might be keywords.
/// - Multiple notes can share a keywords. For example, all practice problems for "Integration by parts" might have that as a keyword. However, if this is the case, then there is no guarantee which note that keyword is linked to. It is advised to instead use the keyword "Integration by parts problems" for those notes and "Integration by parts" for the note explaining the concept.
#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct NoteKeyword {
    pub note_id: NoteId,
    pub keyword: String,
    /// Whether the keyword is embedded within the note data. This is preferable to non-embedded
    /// keywords since grepping for an embedded keyword leads to its exact location within a note.
    /// For example, imagine a note which contains many related theorems. By adding the names of
    /// these theorems as embedded keywords instead of non-embedded keywords, you can grep for the
    /// theorem name and be pointed to the exact location of the theorem within the note.
    /// Otherwise, you would only know that that note contains the theorem and not exactly where it
    /// is.
    ///
    /// For this reason, embedded keywords must be unique within a note, so their exact location
    /// can be uniquely determined. Multiple notes may still contain the same embedded keyword.
    pub embedded: bool,
}

// Only the specified fields below are recoverable from the note data.
#[derive(Clone, Debug, Default, Deserialize, FromRow, Serialize)]
pub struct Card {
    pub id: CardId,
    pub note_id: NoteId,
    // An `order` field is used instead of a `data` field instead since different parsers may have different ways of rendering cloze. For example, one parser in latex may want to replace the cloze with dashes, while another makes a box. Also, this will avoid duplicating a majority of the data field between notes and cards.
    // Unsigned since card's order can't be negative. This also ensures compatibility with usize.
    /// 1-based indexing
    // NOTE: This field is recoverable from the note data.
    pub order: u32,
    /// Added for convenience when retrieving a review card. This allows the card's back file path to easily be constructed, rather than having to reparse the note's data.
    // NOTE: This field is recoverable from the note data.
    pub back_type: BackType,
    #[serde(with = "ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    pub updated_at: DateTime<Utc>,
    // See <https://github.com/open-spaced-repetition/rs-fsrs/blob/7cea5d36770b119b2584be086c31a73949185d34/src/models.rs#L93>
    #[serde(with = "ts_seconds")]
    pub due: DateTime<Utc>,
    pub stability: f64,  // changes after every review
    pub difficulty: f64, // changes after every review
    /// Values between 70% and 97% are considered reasonable. See <https://github.com/open-spaced-repetition/fsrs4anki/wiki/ABC-of-FSRS>.
    pub desired_retention: f64,
    // pub elapsed_days: i64, // Equivalent to: DateTime::Now - `SELECT time FROM review_log WHERE card_id = ? ORDER BY reviewed_date`
    // pub scheduled_days: i64, // Equivalent to: `SELECT scheduled_days FROM review_log WHERE card_id = ? ORDER BY reviewed_date`
    // pub reps: i64, // Equivalent to: `SELECT COUNT(*) FROM review_log WHERE card_id = ?`
    // pub lapses: u32, // Equivalent to: `SELECT COUNT(*) FROM review_log WHERE card_id = ? AND state = (State::Review) AND rating = (Rating::Again)`
    // NOTE: This field is _not_ recoverable from the note data. In other words, this is not serialized in the cloze settings string (even though it is *de*serialized). This is because otherwise, sending a request to update a card and suspend it would require modifying the note's data. Instead, this field now only *de*serialized, not serialized.
    pub special_state: Option<SpecialState>,
    /// The integer value is in relation to the scheduler specified by latest review's `scheduler_id`. If there are no reviews for this card, then it is `NEW_CARD_STATE` to represent the first state.
    pub state: StateId,
    // pub last_review: i64, // DateTime. Equivalent to: `SELECT reviewed_at FROM review_log WHERE card_id = ? ORDER BY reviewed_at LIMIT 1`
    // pub previous_state: i64, // Not needed.
    // pub review_log_id: Option<i64>, // Equivalent to: `SELECT id FROM review_log WHERE card_id = ? ORDER BY reviewed_at ASC LIMIT 1`
    /// JSON data for custom schedulers.
    /// This is guaranteed to be of type `Value::Object(Map<String, Value>)`.
    pub custom_data: Value,
}

impl Card {
    pub fn new(created_at: DateTime<Utc>) -> Self {
        Self {
            due: created_at,
            created_at,
            updated_at: created_at,
            desired_retention: DEFAULT_DESIRED_RETENTION,
            state: NEW_CARD_STATE,
            custom_data: Value::Object(Map::new()),
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, sqlx::Type)]
#[repr(u8)]
pub enum SpecialState {
    Suspended = 1,
    UserBuried = 2,
    SchedulerBuried = 3,
    BuriedUntilLaterToday = 4,
}

#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct NoteLink {
    // pub id: Option<i64>,
    pub parent_note_id: NoteId,
    /// Note that unmatched linked notes are still inserted to make it clear that no linked note was found.
    pub linked_note_id: Option<NoteId>,
    /// 0-based indexing
    pub order: u32,
    pub searched_keyword: String,
    pub matched_keyword: Option<String>,
    pub score: Option<Score>,
}

/// Tree-like structure using colons, like <https://hledger.org/account-names.html>
#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct Tag {
    pub id: TagId,
    pub name: String,
    pub description: String,
    pub query: Option<String>,
    /// This is useful for filtered tags. Setting this to `false` for filtered tags allows the
    /// query to be saved and the tag to be rebuilt so those cards can be reviewed again in the
    /// future.
    pub auto_delete: bool,
}

#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct NoteTag {
    pub note_id: NoteId,
    pub tag_id: TagId,
}

#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct CardTag {
    pub card_id: CardId,
    pub tag_id: TagId,
}

#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct Parser {
    pub id: i64,
    pub name: String, // NOTE: name matches that in `src/parsers/mod.rs::get_parser()`
}

// #[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
// pub struct Scheduler {
//     pub id: i64,
//     pub name: String, // NOTE: name matches that in `src/schedulers/mod.rs::get_scheduler()`
// }

/// This contains a row for every review ever done. Thus, each card has multiple entries in this table.
#[derive(Clone, Debug, Default, Deserialize, Eq, FromRow, Hash, PartialEq, Serialize)]
pub struct ReviewLog {
    pub id: i64,
    pub card_id: CardId,
    /// It is comparable to Anki's `revlog.id` column.
    #[serde(with = "ts_seconds")]
    pub reviewed_at: DateTime<Utc>,
    /// The integer value is in relation to the scheduler specified by `scheduler_id`.
    /// It is comparable to Anki's `revlog.ease` column.
    pub rating: RatingId,
    // pub scheduler_id: i64,
    pub scheduler_name: String,
    /// Duration, stored in seconds.
    /// It is comparable to Anki's `revlog.ivl` column.
    // Cannot use 'chrono::Duration` since its not supported by `sqlx`. See <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.
    pub scheduled_time: i64,
    /// How long the review took, stored in seconds
    /// It is comparable to Anki's `revlog.time` column.
    // Cannot use 'chrono::Duration` since its not supported by `sqlx`. See <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.
    pub recall_duration: i64,
    /// How long it took the rate the card. Useful to provide time estimates for reviews.
    // Cannot use 'chrono::Duration` since its not supported by `sqlx`. See <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.
    pub rate_duration: i64,
    // It is comparable to Anki's `revlog.lastIvl` column.
    // pub elapsed_time: i64, // Unix Time. Equivalent to `self.reviewed_at - previous_review.reviewed_at` or 0 if card is new.
    /// To see how many reviews were done for each state on a given day.
    /// The integer value is in relation to the scheduler specified by `scheduler_id`.
    /// It is comparable to Anki's `revlog.type` column.
    pub previous_state: StateId,
    /// JSON data for custom schedulers.
    pub custom_data: Value,
}

impl ReviewLog {
    pub fn new() -> Self {
        Self {
            custom_data: Value::Object(Map::new()),
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, sqlx::Type)]
#[repr(u8)]
pub enum EventType {
    CreateParser,
    UpdateParser,
    DeleteParser,
    CreateTag,
    UpdateTag,
    DeleteTag,
    CreateNotes, // Plural
    UpdateNotes, // Plural
    DeleteNotes, // Plural
    UpdateCards,
    /// Shares payload schema with `UpdateCards`
    // Even though it shares a payload with `UpdateCards`, we need to preserve the event type so the user can be given a description of the action they are undoing.
    ForgetCard,
    UnburyCards,
    RateCard,
    /// Shares payload schema with `UpdateCards`
    BuryCards,
    /// Shares payload schema with `UpdateCards`
    AdvanceCards,
    /// Shares payload schema with `UpdateCards`
    PostponeCards,
}

#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Serialize)]
pub struct Event {
    pub id: i64,
    pub kind: EventType,
    pub created_at: DateTime<Utc>,
    pub version: i64,
    pub group_id: Option<i64>, // Maybe set this to the id of the first event in the group
    pub payload: Value,
}
