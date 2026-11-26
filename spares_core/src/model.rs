//! This file should match with the migrations file. These types should follow <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html>.

use chrono::DateTime;
use chrono::Utc;
use chrono::serde::ts_seconds;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use sqlx::FromRow;

pub use crate::ids::NoteId;
use crate::parsers::BackType;
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
/// - One note can have multiple keywords. For example, one historical event might be covered in multiple books, so all those books might be keywords.
/// - Multiple notes can share a keywords. For example, all practice sentences for "Subjunctive" might have that as a keyword. However, if this is the case, then there is no guarantee which note that keyword is linked to. It is advised to instead use the keyword "Subjunctive practice" for those notes and "Subjunctive" for the note explaining the concept.
#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct NoteKeyword {
    pub note_id: NoteId,
    pub keyword: String,
    /// Whether the keyword is embedded within the note data. This is preferable to non-embedded
    /// keywords since grepping for an embedded keyword leads to its exact location within a note.
    /// For example, imagine a note which covers many related grammar rules. By adding the names of
    /// these rules as embedded keywords instead of non-embedded keywords, you can grep for a rule's
    /// name and be pointed to the exact location of the rule within the note. Otherwise, you would
    /// only know that that note contains the rule and not exactly where it is.
    ///
    /// For this reason, embedded keywords must be unique within a note, so their exact location
    /// can be uniquely determined. Multiple notes may still contain the same embedded keyword.
    pub embedded: bool,
}

// Only the specified fields below are recoverable from the note data.
#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
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
    pub stability: f64, // changes after every review
    /// FSRS-7's fast stability, which same-day reviews move most. Equal to `stability` under
    /// FSRS-6.
    pub stability_fast: f64,
    pub difficulty: f64, // changes after every review
    /// Values between 70% and 97% are considered reasonable. See <https://github.com/open-spaced-repetition/fsrs4anki/wiki/ABC-of-FSRS>.
    pub desired_retention: f64,
    // FSRS's `elapsed_days`, `scheduled_days`, `reps`, `lapses` and `last_review` aren't stored:
    // they follow from the card's `review_log` rows (`lapses` from `SrsScheduler::lapses`).
    // NOTE: This field is _not_ recoverable from the note data. In other words, this is not serialized in the cloze settings string (even though it is *de*serialized). This is because otherwise, sending a request to update a card and suspend it would require modifying the note's data. Instead, this field now only *de*serialized, not serialized.
    pub special_state: Option<SpecialState>,
    /// One of the states [`get_states`](crate::schedulers::SrsScheduler::get_states) lists for the
    /// scheduler of the card's latest review (its `scheduler_name`). If there are no reviews for
    /// this card, then it is `NEW_CARD_STATE` to represent the first state, which every scheduler
    /// has.
    pub state: StateId,
    /// JSON data for custom schedulers.
    /// This is guaranteed to be of type `Value::Object(Map<String, Value>)`.
    pub custom_data: Value,
}

impl Card {
    /// A new card of no note yet ([`NoteId::UNSET`]). Set `note_id` before inserting it.
    pub fn new(created_at: DateTime<Utc>) -> Self {
        Self {
            id: 0,
            note_id: NoteId::UNSET,
            order: 0,
            back_type: BackType::default(),
            due: created_at,
            created_at,
            updated_at: created_at,
            stability: 0.0,
            stability_fast: 0.0,
            difficulty: 0.0,
            desired_retention: DEFAULT_DESIRED_RETENTION,
            special_state: None,
            state: NEW_CARD_STATE,
            custom_data: Value::Object(Map::new()),
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
/// They fall in two ranges, so that SQL can tell them apart with one comparison (see
/// [`ReviewLogKind::FIRST_UNREPLAYED`]):
///
/// - `0..=15` are *replayed*: they change how a scheduler reconstructs a card's memory state.
///   `2..=15` are reserved for future kinds such as `SetMemoryState`, `SchedulerChanged` or
///   `Created`.
/// - `16..=31` are only *recorded*: they are part of the card's history, for statistics and the
///   card's info, but every replay skips them. `18..=31` are reserved.
///
/// The column carries a matching `CHECK (kind BETWEEN 0 AND 31)`.
///
/// Only actions that are a review, that a scheduler must replay, or that are part of the card's
/// schedule's history, like a postponement ([`ReviewLogKind::Reschedule`]), belong in
/// `review_log`. Other audit-only actions, like suspend and bury, belong in the `event` table
/// (see [`EventType`]) instead. Unlike `review_log`, that table is not synced and is cleared by a
/// full sync, so it is only for undo.
///
/// A row whose `kind` is not a known variant fails to decode. That is deliberate: a database
/// written by a newer version should not be silently misread as a stream of plain reviews.
///
/// Unlike Anki's `revlog.type`, this does *not* encode the card's state (that is
/// [`ReviewLog::previous_state`]) or whether the review happened under a filtered tag (that is
/// [`ReviewLog::tag_id`]). Anki's `Cram` type is the exception, since it changes how the row is
/// replayed: here it is [`ReviewLogKind::Cram`].
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
    /// A rating in a filtered tag that does not reschedule ([`Tag::reschedule`]), such as an
    /// `exam-cram` tag studied the night before an exam. It records that the card was studied,
    /// but not a review the scheduler learns from: the card's due date and memory state were left
    /// as they were, so replay skips the row.
    ///
    /// `rating`, `recall_duration` and `rate_duration` are meaningful, `scheduled_time` is `NULL`
    /// (the card was given no new interval), `previous_state` is the card's state at the time and
    /// `tag_id` the filtered tag.
    ///
    /// Crams count in statistics and the review heatmap, apart from reviews (see
    /// [`StatisticsResponse::cram_count`](crate::schema::review::StatisticsResponse::cram_count)
    /// and [`ReviewHistoryResponse::cram_count_by_date`](crate::schema::review::ReviewHistoryResponse::cram_count_by_date)),
    /// and show in a card's info. They do not count toward the daily
    /// limits, lapses or leeches, the optimizer's training set, or `c.rated` in searches.
    ///
    /// For example, going twice through the 60 cards of `exam-cram` gives 120 cram rows. Today's
    /// statistics then show 120 crams next to the scheduled reviews, and the heatmap counts
    /// today as studied, while `spares card reschedule` gives every card the same due date and
    /// memory state as before.
    Cram = 16,
    /// The card's due date was moved without a review: set by hand, or by `spares card postpone`,
    /// `spares card advance` or `spares card break` ([`RescheduleOrigin`]). Not to be confused
    /// with `spares card reschedule`, which recomputes due dates from the replayed history and
    /// logs nothing. The card's memory state was left as it was, so replay skips the row; the
    /// next review's elapsed time is still measured from the last real review.
    ///
    /// The rating, `scheduled_time` and duration columns are `NULL`, `previous_state` is the
    /// card's state at the time, and `custom_data` holds the due dates before and after and what
    /// moved the card, as [`RescheduleData`]. Statistics, the daily limits, lapses, the optimizer
    /// and `c.rated` leave these rows out. `c.postponed` finds the ones that moved a card later,
    /// and a card's info shows them.
    ///
    /// Until the card's next review, such a row also keeps `spares card reschedule` from moving
    /// the card (see
    /// [`reschedule_moved_cards`](crate::config::SparesExternalConfig::reschedule_moved_cards)).
    ///
    /// For example, `spares card postpone 50` on a holiday writes 50 of these rows, so that
    /// `c.postponed[today] tag=exam` finds the exam cards among them. Undoing the postponement
    /// deletes them again.
    Reschedule = 17,
}

/// What a [`ReviewLogKind::Reschedule`] row keeps in its `custom_data`: the card's due date
/// before and after it was moved, and what moved it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RescheduleData {
    #[serde(with = "ts_seconds")]
    pub previous_due: DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    pub due: DateTime<Utc>,
    pub origin: RescheduleOrigin,
}

/// What moved a card's due date without a review.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RescheduleOrigin {
    /// Given a due date by hand, with "Set Card Due Date" in review or `spares card edit --due`.
    Manual,
    /// `spares card postpone`.
    Postpone,
    /// `spares card advance`.
    Advance,
    /// `spares card break`, which moves the cards due during a break to the days after it.
    Break,
}

impl ReviewLogKind {
    /// The first kind that replay skips. Kinds below it change a card's memory state; kinds from
    /// it on are only recorded.
    ///
    /// SQL that fetches rows to replay filters on `kind < FIRST_UNREPLAYED`, so that a new
    /// recorded-only kind needs no change there.
    pub const FIRST_UNREPLAYED: u8 = 16;

    /// Whether a scheduler replays rows of this kind (see [`ReviewLogKind::FIRST_UNREPLAYED`]).
    pub fn is_replayed(self) -> bool {
        (self as u8) < Self::FIRST_UNREPLAYED
    }
}

/// Which way a link between two notes points: which of them builds on the other.
///
/// In a Typst note, `#lin[X]` is a [`Self::Prerequisite`], and `#lin(rel: "dependent")[X]` and
/// `#lin(rel: "related")[X]` are the other two. Other parsers only have prerequisites.
///
/// Whatever its relation, a link is shown with its note, matched to a keyword and found by
/// `linked_to_note` and the other link searches. The relation only decides what builds on what:
/// which cards wait for which in the queue (`new_after_linked_notes`,
/// `reviews_after_linked_notes`, `new_order = "unblocking"`), the walks of `prerequisite_of` and
/// `dependent_of`, the note graph's arrows and a goal's projection. Those all read the
/// `note_prerequisite` view, which turns every link into a (note, prerequisite) pair.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum LinkRelation {
    /// The note with the link builds on the note it links to, as in "by #lin[Fubini's theorem]".
    #[default]
    Prerequisite = 0,
    /// The note it links to builds on the note with the link, as in "used in
    /// #lin(rel: \"dependent\")[the chain rule]".
    Dependent = 1,
    /// Neither builds on the other, as in "compare #lin(rel: \"related\")[Riemann sums]".
    Related = 2,
}

impl LinkRelation {
    /// The value of `rel:` that writes this relation, as in `#lin(rel: "dependent")[X]`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prerequisite => "prerequisite",
            Self::Dependent => "dependent",
            Self::Related => "related",
        }
    }

    /// The relation a `rel:` value writes, if it is one.
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Prerequisite, Self::Dependent, Self::Related]
            .into_iter()
            .find(|relation| relation.as_str() == name)
    }
}

#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
pub struct NoteLink {
    pub parent_note_id: NoteId,
    /// Note that unmatched linked notes are still inserted to make it clear that no linked note was found.
    pub linked_note_id: Option<NoteId>,
    /// 0-based indexing
    pub order: u32,
    pub searched_keyword: String,
    pub matched_keyword: Option<String>,
    pub score: Option<Score>,
    pub relation: LinkRelation,
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
    /// For a filtered tag: whether rating a card while studying the tag reschedules it, as rating
    /// it anywhere else does. `true` by default.
    ///
    /// Set it to `false` to cram, for example before an exam: cards are shown until their
    /// scheduler's [`filtered_tag_schedule`](crate::schedulers::SrsScheduler::filtered_tag_schedule)
    /// lets them go (with FSRS, when rated Good twice or Easy once), as in any filtered tag, but
    /// their due dates and memory states are left as they were, so the card's next real review is
    /// not pushed back by having just seen it. Each rating is logged as a
    /// [`ReviewLogKind::Cram`], which counts in statistics and the heatmap as a cram, but not
    /// toward the daily limits, and which no replay of the card's history sees. Undo still takes a
    /// rating back.
    pub reschedule: bool,
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

/// This contains a row for every review ever done, plus a marker row for each action that changes
/// a card's memory state without being a review, and a row for each cram (see [`ReviewLogKind`]).
/// Thus, each card has multiple entries in this table.
///
/// Consumers that count or average over reviews must filter on `kind`; the rating and duration
/// columns are `NULL` for every `Forget` and `Reschedule` row. Consumers that replay a card's
/// history must skip the kinds that are not [replayed](ReviewLogKind::is_replayed), such as crams.
#[derive(Clone, Debug, Default, Deserialize, Eq, FromRow, Hash, PartialEq, Serialize)]
pub struct ReviewLog {
    pub id: i64,
    /// `None` once the card has been deleted. Review logs are deliberately *not* deleted with
    /// their card, so that historical review counts survive; the row is orphaned instead.
    pub card_id: Option<CardId>,
    /// It is comparable to Anki's `revlog.id` column.
    #[serde(with = "ts_seconds")]
    pub reviewed_at: DateTime<Utc>,
    /// What the card was recorded as, if this row is a review or a cram. `None` for every other
    /// [`ReviewLogKind`].
    ///
    /// The integer value is in relation to the scheduler named by `scheduler_name`.
    /// It is comparable to Anki's `revlog.ease` column.
    pub rating: Option<RatingId>,
    /// Why this row exists. See [`ReviewLogKind`].
    pub kind: ReviewLogKind,
    /// The filtered tag this row was produced under, if any. `None` for an ordinary review.
    ///
    /// Set to `None` rather than deleted when the tag is deleted, for the same reason `card_id`
    /// is: the review still happened.
    pub tag_id: Option<TagId>,
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
    /// To see how many reviews were done for each state on a given day.
    /// The integer value is in relation to the scheduler named by `scheduler_name`.
    /// It is comparable to Anki's `revlog.type` column.
    pub previous_state: StateId,
    /// JSON data for custom schedulers.
    pub custom_data: Value,
}

impl ReviewLog {
    /// Whether this row is a graded review that the scheduler learns from, as opposed to a
    /// marker for some other action that affects replay, or a cram. Use this rather than
    /// comparing `kind` inline, so that adding a variant surfaces every site that assumed two
    /// kinds.
    pub fn is_review(&self) -> bool {
        self.kind == ReviewLogKind::Review
    }

    pub fn new() -> Self {
        Self {
            custom_data: Value::Object(Map::new()),
            ..Default::default()
        }
    }

    /// The due dates of a [`ReviewLogKind::Reschedule`] row, or `None` for other kinds.
    pub fn reschedule_data(&self) -> Option<RescheduleData> {
        (self.kind == ReviewLogKind::Reschedule)
            .then(|| serde_json::from_value(self.custom_data.clone()).ok())
            .flatten()
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
    ForgetCard,
    UnburyCards,
    /// Carries `RateCardPayload`, which pairs the card transitions with the id of the
    /// `review_log` row so undo can delete it.
    RateCard,
    /// Shares payload schema with `UpdateCards`
    BuryCards,
    /// Shares payload schema with `UpdateCards`. Its moves are logged as
    /// [`ReviewLogKind::Reschedule`] rows, at the event's time, which undo deletes.
    AdvanceCards,
    /// Shares payload schema with `UpdateCards`, like `AdvanceCards`.
    PostponeCards,
    /// The undo of a `RateCard`. Carries `UnreviewPayload`, which keeps the deleted `review_log`
    /// row so a redo can restore it.
    UnrateCard,
    /// The undo of a `ForgetCard`. Carries `UnreviewPayload`, like `UnrateCard`.
    UnforgetCard,
    /// The undo of an `AdvanceCards`. Carries `UnmoveCardsPayload`, which keeps the deleted
    /// `review_log` rows so a redo can restore them.
    UnadvanceCards,
    /// The undo of a `PostponeCards`. Carries `UnmoveCardsPayload`, like `UnadvanceCards`.
    UnpostponeCards,
    /// The unburying of every buried card when the day changes. Shares payload schema with
    /// `UpdateCards`. Like every [automatic](Self::is_automatic) kind, a plain undo skips it.
    DailyUnbury,
    /// The cards a full reschedule (after changing scheduler settings) changed. Shares payload
    /// schema with `UpdateCards`. Undoing it restores the cards, not the settings. Like every
    /// [automatic](Self::is_automatic) kind, a plain undo skips it.
    RescheduleCards,
    /// Cards moved between the live notes of a sync group when their clozes moved. Carries
    /// `RelocateCardsPayload`, and is its own undo, like `UpdateNotes`.
    RelocateCards,
    /// An `UpdateCards` that gave cards a due date by hand, which is logged as
    /// [`ReviewLogKind::Reschedule`] rows like `PostponeCards`. Shares payload schema with
    /// `UpdateCards`, and may change the cards' other fields too.
    SetCardsDue,
    /// The undo of a `SetCardsDue`. Carries `UnmoveCardsPayload`, like `UnadvanceCards`.
    UnsetCardsDue,
    /// The cards `spares card break` moved past a break, logged as [`ReviewLogKind::Reschedule`]
    /// rows like `PostponeCards`. Shares payload schema with `UpdateCards`. Undoing it restores
    /// the cards, not the break's easy days in `config.toml`.
    ScheduleBreak,
    /// The undo of a `ScheduleBreak`. Carries `UnmoveCardsPayload`, like `UnadvanceCards`.
    UnscheduleBreak,
    /// Tags given to or taken from cards themselves rather than their notes. Carries
    /// `UpdateCardTagsPayload`, and is its own undo, like `UpdateNotes`. See
    /// [`crate::api::card_tags`].
    UpdateCardTags,
}

impl EventType {
    /// Kinds that record what spares did by itself rather than an action of the user's. Undo
    /// without an id skips them, and they do not clear what redo without an id can reach, but
    /// they can be undone by id.
    pub const AUTOMATIC: [Self; 2] = [Self::DailyUnbury, Self::RescheduleCards];

    pub fn is_automatic(self) -> bool {
        Self::AUTOMATIC.contains(&self)
    }
}

/// Whether an event is an action, or the undo or redo of one. Derived from the
/// `reverts_event_id` chain by the `event_action` view rather than stored.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, sqlx::Type)]
#[repr(u8)]
pub enum EventAction {
    Do = 0,
    Undo = 1,
    Redo = 2,
}

#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Serialize)]
pub struct Event {
    pub id: i64,
    pub kind: EventType,
    pub created_at: DateTime<Utc>,
    pub group_id: Option<i64>, // Maybe set this to the id of the first event in the group
    /// The event this one reverses: the action an undo undid, or the undo a redo redid.
    pub reverts_event_id: Option<i64>,
    pub payload: Value,
}

#[cfg(test)]
mod tests {

    use sqlx::SqlitePool;

    /// The last migration the baseline replaced
    const LAST_OLD_MIGRATION: i64 = 20_261_126_120_000;

    /// Rows of `_sqlx_migrations` as the migrations before the baseline left them, by version.
    async fn pretend_old_migrations_ran(pool: &SqlitePool, versions: &[i64]) {
        sqlx::query("DELETE FROM _sqlx_migrations")
            .execute(pool)
            .await
            .unwrap();
        for version in versions {
            sqlx::query(
                "INSERT INTO _sqlx_migrations
                 (version, description, success, checksum, execution_time)
                 VALUES (?, 'old', 1, x'00', 0)",
            )
            .bind(version)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    async fn schema(pool: &SqlitePool) -> Vec<(String, String)> {
        sqlx::query_as("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// A database the migrations before the baseline made has its schema already: the baseline
    /// changes nothing there, neither its schema nor its data.
    #[sqlx::test(migrations = false)]
    async fn migration_baseline_accepts_a_database_the_old_migrations_made(pool: SqlitePool) {
        let migrator = crate::collection::migrator();
        migrator.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO parser (name) VALUES ('markdown')")
            .execute(&pool)
            .await
            .unwrap();
        let token: Vec<u8> = sqlx::query_scalar("SELECT token FROM keyword_version")
            .fetch_one(&pool)
            .await
            .unwrap();
        let before = schema(&pool).await;
        let old = [20_240_326_145_400, 20_261_124_110_000, LAST_OLD_MIGRATION];
        pretend_old_migrations_ran(&pool, &old).await;

        migrator.run(&pool).await.unwrap();

        assert_eq!(schema(&pool).await, before);
        let parsers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parser")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(parsers, 1);
        let tokens: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token FROM keyword_version")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(tokens, [token]);
        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        let mut expected = old.to_vec();
        expected.extend(
            migrator
                .iter()
                .filter(|m| m.migration_type.is_up_migration())
                .map(|m| m.version),
        );
        assert_eq!(applied, expected);
    }

    /// The baseline refuses a database that stopped partway through the migrations it replaced,
    /// which it would otherwise leave without their last changes.
    #[sqlx::test(migrations = false)]
    async fn migration_baseline_refuses_a_database_partway_through_the_old_migrations(
        pool: SqlitePool,
    ) {
        let migrator = crate::collection::migrator();
        migrator.run(&pool).await.unwrap();
        pretend_old_migrations_ran(&pool, &[20_261_124_110_000, 20_261_125_060_000]).await;

        let err = migrator.run(&pool).await.unwrap_err();
        assert!(
            err.to_string().contains(
                "open_this_collection_with_a_spares_from_just_before_the_2026_11_26_baseline_first"
            ),
            "{err}"
        );
    }

    /// The backfill gives every stored card a fast stability equal to its stability: card rows,
    /// sync bases and the cards undo events keep, wherever they are in the payload. A card's
    /// changes (`{"before", "after"}`) aren't a card, so they aren't touched.
    #[sqlx::test(migrations = false)]
    async fn migration_backfill_stability_fast_writes_it_into_every_card(pool: SqlitePool) {
        use serde_json::json;

        let migrator = crate::collection::migrator();
        migrator.run_to(20_261_126_200_000, &pool).await.unwrap();
        let card = |id: i64, stability: f64| {
            json!({"id": id, "note_id": 1, "order": 1, "back_type": 0, "created_at": 0,
                   "updated_at": 0, "due": 0, "stability": stability, "difficulty": 5.0,
                   "desired_retention": 0.9, "special_state": null, "state": 2,
                   "custom_data": {}})
        };
        let changes = json!({"card_id": 1, "stability": {"before": 1.0, "after": 2.0}});
        let events = [
            json!({"notes": [{"cards": {"a": [card(1, 3.0)], "b": [card(1, 4.0), card(2, 0.5)]}}]}),
            json!({"deleted_cards": [card(3, 7.0)]}),
            json!([changes]),
        ];
        let mut conn = pool.acquire().await.unwrap();
        for statement in [
            "PRAGMA foreign_keys = OFF",
            "INSERT INTO parser (id, name) VALUES (1, 'markdown')",
            "INSERT INTO note (id, data, custom_data, parser_id) VALUES (1, '', '{}', 1)",
            r#"INSERT INTO card (id, note_id, "order", back_type, due, stability, difficulty,
               desired_retention, state, custom_data, stability_fast)
               VALUES (1, 1, 1, 0, 0, 6.0, 5.0, 0.9, 2, '{}', 0),
                      (2, 1, 2, 0, 0, 0.0, 0.0, 0.9, 0, '{}', 0)"#,
        ] {
            sqlx::query(statement).execute(&mut *conn).await.unwrap();
        }
        sqlx::query("INSERT INTO sync_base (kind, oid, snapshot) VALUES ('card', 1, ?)")
            .bind(card(1, 2.0).to_string())
            .execute(&mut *conn)
            .await
            .unwrap();
        for (i, payload) in events.iter().enumerate() {
            sqlx::query("INSERT INTO event (id, kind, payload) VALUES (?, 7, ?)")
                .bind(i64::try_from(i).unwrap() + 1)
                .bind(payload.to_string())
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        drop(conn);

        migrator.run(&pool).await.unwrap();

        let cards: Vec<(f64, f64)> =
            sqlx::query_as("SELECT stability, stability_fast FROM card ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(cards, [(6.0, 6.0), (0.0, 0.0)]);
        let base: String = sqlx::query_scalar("SELECT snapshot FROM sync_base")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&base).unwrap()["stability_fast"],
            2.0
        );
        let payloads: Vec<serde_json::Value> =
            sqlx::query_scalar::<_, String>("SELECT payload FROM event ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap()
                .iter()
                .map(|payload| serde_json::from_str(payload).unwrap())
                .collect();
        let fast = |card: &serde_json::Value| card["stability_fast"].as_f64();
        let notes = &payloads[0]["notes"][0]["cards"];
        assert_eq!(fast(&notes["a"][0]), Some(3.0));
        assert_eq!(fast(&notes["b"][0]), Some(4.0));
        assert_eq!(fast(&notes["b"][1]), Some(0.5));
        assert_eq!(fast(&payloads[1]["deleted_cards"][0]), Some(7.0));
        assert_eq!(payloads[2], events[2]);
    }

    #[test]
    fn only_kinds_below_first_unreplayed_are_replayed() {
        assert!(super::ReviewLogKind::Review.is_replayed());
        assert!(super::ReviewLogKind::Forget.is_replayed());
        assert!(!super::ReviewLogKind::Cram.is_replayed());
        assert!(!super::ReviewLogKind::Reschedule.is_replayed());
    }
}
