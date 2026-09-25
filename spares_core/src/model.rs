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

/// When enabled, a self-healing step runs during note updates that automatically
/// creates missing cards or removes extra cards if the DB is inconsistent with
/// the parsed note data. Set to `false` if historical data corruption is no longer
/// a concern, for a minor performance gain.
pub const AUTO_FIX_MISSING_CARDS: bool = true;

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

/// What a `review_log` row records.
///
/// Discriminants are explicit and **must never change**: they are persisted in `review_log.kind`.
/// `2..=15` are reserved for future kinds that, like `Forget`, change how a scheduler replays a
/// card — for example `SetMemoryState`, `SchedulerChanged` or `Created`. The column carries a
/// matching `CHECK (kind BETWEEN 0 AND 15)`.
///
/// Only actions a scheduler must *replay* belong in `review_log`. The test is whether replaying
/// the row changes the card's memory state. Audit-only actions — suspend, bury, set due date,
/// advance, postpone — do not, and belong in the `event` table (see [`EventType`]) instead.
///
/// A row whose `kind` is not a known variant fails to decode. That is deliberate: a database
/// written by a newer version should not be silently misread as a stream of plain reviews.
///
/// Unlike Anki's `revlog.type`, this does *not* encode the card's state (that is
/// [`ReviewLog::previous_state`]) or whether the review happened under a filtered tag (that is
/// [`ReviewLog::tag_id`]).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, sqlx::Type)]
#[repr(u8)]
pub enum ReviewLogKind {
    /// A graded review. `rating`, `scheduled_time`, `recall_duration` and `rate_duration` are all
    /// meaningful.
    #[default]
    Review = 0,
    /// The card's memory state was reset to new by [`crate::api::forget_card`]. Replay restarts
    /// its fold here, and the rating and duration columns are all `NULL`.
    Forget = 1,
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
    #[serde(with = "ts_seconds")]
    pub created_at: DateTime<Utc>,
    /// Bumped when the tag's definition changes or its filtered card set is rebuilt, but not when
    /// individual cards join or leave the tag. Review snapshots rely on this to tell whether the
    /// tag was built today.
    #[serde(with = "ts_seconds")]
    pub updated_at: DateTime<Utc>,
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

/// This contains a row for every review ever done, plus a marker row for each action that changes
/// a card's memory state without being a review (see [`ReviewLogKind`]). Thus, each card has
/// multiple entries in this table.
///
/// Consumers that count or average over reviews must filter on `kind`; the rating and duration
/// columns are `NULL` for every non-`Review` row.
#[derive(Clone, Debug, Default, Deserialize, Eq, FromRow, Hash, PartialEq, Serialize)]
pub struct ReviewLog {
    pub id: i64,
    /// `None` once the card has been deleted. Review logs are deliberately *not* deleted with
    /// their card, so that historical review counts survive; the row is orphaned instead.
    pub card_id: Option<CardId>,
    /// It is comparable to Anki's `revlog.id` column.
    #[serde(with = "ts_seconds")]
    pub reviewed_at: DateTime<Utc>,
    /// What the card was recorded as, if this row is a review. `None` for every other
    /// [`ReviewLogKind`].
    ///
    /// The integer value is in relation to the scheduler specified by `scheduler_id`.
    /// It is comparable to Anki's `revlog.ease` column.
    pub rating: Option<RatingId>,
    /// Why this row exists. See [`ReviewLogKind`].
    pub kind: ReviewLogKind,
    /// The filtered tag this row was produced under, if any. `None` for an ordinary review.
    ///
    /// Set to `None` rather than deleted when the tag is deleted, for the same reason `card_id`
    /// is: the review still happened.
    pub tag_id: Option<TagId>,
    // pub scheduler_id: i64,
    pub scheduler_name: String,
    /// Duration, stored in seconds.
    /// It is comparable to Anki's `revlog.ivl` column.
    // Cannot use 'chrono::Duration` since its not supported by `sqlx`. See <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.
    pub scheduled_time: Option<i64>,
    /// How long the review took, stored in seconds
    /// It is comparable to Anki's `revlog.time` column.
    // Cannot use 'chrono::Duration` since its not supported by `sqlx`. See <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.
    pub recall_duration: Option<i64>,
    /// How long it took the rate the card. Useful to provide time estimates for reviews.
    // Cannot use 'chrono::Duration` since its not supported by `sqlx`. See <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.
    pub rate_duration: Option<i64>,
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
    /// Whether this row is a graded review, as opposed to a marker for some other action that
    /// affects replay. Use this rather than comparing `kind` inline, so that adding a variant
    /// surfaces every site that assumed two kinds.
    pub fn is_review(&self) -> bool {
        self.kind == ReviewLogKind::Review
    }

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
    /// Carries `ForgetCardPayload`, which pairs the card transitions with the id of the
    /// `review_log` marker row so undo can delete it.
    // Version 1 events stored a bare `Vec<UpdateCardPayload>` here, shared with `UpdateCards`, and
    // wrote no marker row. Those rows are still in users' databases; `invert_payload` tells the
    // two apart by JSON shape.
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sqlx::SqlitePool;
    use sqlx::migrate::Migrate;
    use sqlx::migrate::Migrator;

    fn migrator_path() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
    }

    /// The `review_log` rebuild in `20260918120000_review_log_kind` drops and recreates the one
    /// table in this schema that cannot be reconstructed from the note files, so the upgrade path
    /// is tested against a database that already holds rows rather than only against a fresh one
    /// (which is all `#[sqlx::test]` ever builds).
    #[sqlx::test(migrations = false)]
    async fn migration_review_log_kind_preserves_existing_rows(pool: SqlitePool) {
        let migrator = Migrator::new(migrator_path()).await.unwrap();
        // `Migrator::iter()` yields both up and down migrations; only the up side applies here.
        let base = migrator
            .iter()
            .find(|m| m.migration_type.is_up_migration())
            .expect("base migration exists");

        // Bring the database up to the state a user on the previous release would have.
        let mut conn = pool.acquire().await.unwrap();
        conn.ensure_migrations_table().await.unwrap();
        conn.apply(base).await.unwrap();

        sqlx::query("INSERT INTO parser (name) VALUES ('markdown')")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO note (data, custom_data, parser_id) VALUES ('n', '{}', 1)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
               desired_retention, state, custom_data) VALUES (1, 1, 1, 100, 5.5, 4.2, 0.9, 2, '{}')"#,
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        for (reviewed_at, rating) in [(1000, 3), (2000, 4)] {
            sqlx::query(
                r"INSERT INTO review_log (card_id, reviewed_at, rating, scheduler_name,
                   scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
                   VALUES (1, ?, ?, 'fsrs', 86400, 5, 2, 0, '{}')",
            )
            .bind(reviewed_at)
            .bind(rating)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        // An orphan, i.e. a review of a card that has since been deleted. These are deliberately
        // kept (`ON DELETE SET NULL`) and must survive the rebuild.
        sqlx::query(
            r"INSERT INTO review_log (card_id, reviewed_at, rating, scheduler_name,
               scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
               VALUES (NULL, 3000, 1, 'fsrs', 600, 9, 4, 2, '{}')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        // Now apply the rest, which is what a user's next `spares serve` does.
        migrator.run(&pool).await.unwrap();

        let (count, max_id, orphans, non_review): (i64, i64, i64, i64) = sqlx::query_as(
            r"SELECT COUNT(*), COALESCE(MAX(id), 0), SUM(card_id IS NULL), SUM(kind != 0)
              FROM review_log",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 3, "every pre-existing review log row must survive");
        assert_eq!(
            max_id, 3,
            "ids must be preserved: undo payloads reference them"
        );
        assert_eq!(orphans, 1, "orphaned rows must survive the rebuild");
        assert_eq!(
            non_review, 0,
            "pre-existing rows must all backfill to kind = Review"
        );

        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");
        let fk_violations: Vec<(String, i64, String, i64)> =
            sqlx::query_as("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(fk_violations.is_empty(), "{:?}", fk_violations);
    }
}
