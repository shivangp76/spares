//! This file should match with the migrations file. These types should follow <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.

use crate::parsers::BackType;
use chrono::{DateTime, Utc, serde::ts_seconds};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::FromRow;

pub type NoteId = i64;
pub type CardId = i64;
pub type TagId = i64;
pub type StateId = u32;
pub type RatingId = u32;
pub type CustomData = Map<String, Value>;

pub const NEW_CARD_STATE: StateId = 0;
pub const DEFAULT_DESIRED_RETENTION: f64 = 0.9;
pub const NOTE_ID_KEY: &str = "note-id";

#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct Note {
    pub id: NoteId,
    // Note data is stored directly as it is received from the user and contains cloze delimiters as specified by the parser. Thus, the parser is needed in order for this to make sense.
    pub data: String,
    /// Comma separated
    ///
    /// Referencing other notes: Allowing citing across keywords. Don't add title field since I may want to cite just "Ransford", for example, and all the cards from that textbook should show up. I don't want uniqueness here. Also 1 theorem might be explained in multiple books, so all those books might be keywords. Suppose it is in book A and book B and we are currently taking notes in book B. It would be annoying to be forced to have to check if the theorem is already written in another book and cite that instead. It's more natural to cite an earlier theorem in book B when taking notes in book B.
    pub keywords: String,
    #[serde(with = "ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    pub updated_at: DateTime<Utc>,
    pub parser_id: i64,
    /// Stored as JSON. Note that this is not guaranteed to be ordered.
    /// This is guaranteed to be of type `Value::Object(Map<String, Value>)`.
    pub custom_data: Value,
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
    // NOTE: Can this be Buried(User), Buried(Scheduler) instead of UserBuried and SchedulerBuried?
    Suspended = 1,
    UserBuried = 2,
    SchedulerBuried = 3,
    // Buried(bool),
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
}

// Tree-like structure
#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct Tag {
    pub id: TagId,
    pub parent_id: Option<TagId>,
    pub name: String,
    pub description: String,
    pub query: Option<String>,
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
    pub duration: i64,
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
