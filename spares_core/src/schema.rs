use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FilterOptions {
    pub page: Option<usize>,
    pub limit: Option<usize>,
}

#[allow(clippy::option_option)]
fn some_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

pub mod parser {
    use serde::Deserialize;
    use serde::Serialize;

    use crate::model::Parser;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct CreateParserRequest {
        pub name: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateParserRequest {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct ParserResponse {
        pub id: i64,
        pub name: String,
    }

    impl ParserResponse {
        pub fn new(parser: &Parser) -> Self {
            Self {
                id: parser.id,
                name: parser.name.clone(),
            }
        }
    }
}

pub mod tag {
    use chrono::DateTime;
    use chrono::Utc;
    use chrono::serde::ts_seconds;
    use serde::Deserialize;
    use serde::Serialize;

    use crate::model::Tag;
    use crate::model::TagId;
    use crate::schema::some_option;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct CreateTagRequest {
        pub name: String,
        pub description: String,
        pub query: Option<String>,
        pub auto_delete: bool,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub enum TagSelector {
        Id(TagId),
        Name(String),
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateTagRequest {
        pub tag_to_modify: TagSelector,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(default, deserialize_with = "some_option")]
        pub query: Option<Option<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub auto_delete: Option<bool>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct TagResponse {
        pub id: TagId,
        pub name: String,
        pub description: String,
        pub query: Option<String>,
        pub auto_delete: bool,
        #[serde(with = "ts_seconds")]
        pub created_at: DateTime<Utc>,
        #[serde(with = "ts_seconds")]
        pub updated_at: DateTime<Utc>,
    }

    impl TagResponse {
        pub fn new(tag: &Tag) -> Self {
            Self {
                id: tag.id.to_owned(),
                name: tag.name.clone(),
                description: tag.description.clone(),
                query: tag.query.clone(),
                auto_delete: tag.auto_delete,
                created_at: tag.created_at,
                updated_at: tag.updated_at,
            }
        }
    }
}

pub mod note {
    use std::path::PathBuf;

    use chrono::DateTime;
    use chrono::Utc;
    use serde::Deserialize;
    use serde::Serialize;
    use sqlx::SqlitePool;

    use super::card::CardResponse;
    use crate::Error;
    use crate::model::CustomData;
    use crate::model::Note;
    use crate::model::NoteId;
    use crate::model::NoteLink;
    use crate::model::Score;
    use crate::search::QueryReturnItemType;
    use crate::search::evaluator::Evaluator;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct ExportNotesRequest {
        pub query: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub enum NotesSelector {
        Ids(Vec<NoteId>),
        Query(String),
        All,
    }

    impl NotesSelector {
        pub async fn to_note_ids(self, db: &SqlitePool) -> Result<Vec<NoteId>, Error> {
            match self {
                NotesSelector::Ids(vec) => Ok(vec),
                NotesSelector::Query(query) => {
                    let evaluator = Evaluator::new(&query);
                    evaluator.get_note_ids(db).await
                }
                NotesSelector::All => {
                    let ids: Vec<NoteId> = sqlx::query_scalar(r"SELECT id FROM note")
                        .fetch_all(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                    Ok(ids)
                }
            }
        }
    }

    #[allow(clippy::struct_excessive_bools, reason = "needed to generate files")]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct RenderNotesRequest {
        pub selector: NotesSelector,
        /// If `None`, then all other notes are considered to be immutable.
        pub immutable_note_ids: Option<Vec<NoteId>>,
        pub overridden_output_raw_dir: Option<PathBuf>,
        pub include_linked_notes: bool,
        pub include_cards: bool,
        pub generate_rendered: bool,
        pub force_generate_rendered: bool,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct SearchNotesRequest {
        pub query: String,
        pub output_type: QueryReturnItemType,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub enum SearchNotesResponse {
        Notes(Vec<(NoteResponse, String)>),
        Cards(Vec<(CardResponse, String)>),
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct SearchKeywordRequest {
        pub keyword: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct MatchedKeywordResponse {
        pub matched_keyword: String,
        pub note_id: NoteId,
        pub score: Score,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct UnmatchedKeywordResponse {
        pub note_id: NoteId,
        pub searched_keyword: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct NoteLinksRequest {
        pub score_threshold: Score,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct CreateNotesRequest {
        pub parser_id: i64,
        pub requests: Vec<CreateNoteRequest>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct CreateNoteRequest {
        pub data: String,
        pub keywords: Vec<String>,
        pub tags: Vec<String>,
        /// Suspends all of its cards.
        pub is_suspended: bool,
        pub custom_data: CustomData,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateNotesRequest {
        pub selector: NotesSelector,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub parser_id: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub keywords: Option<Vec<String>>,
        pub tags: UpdateTags,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub custom_data: Option<CustomData>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct DeleteNotesRequest {
        pub selector: NotesSelector,
    }

    #[derive(Debug, Deserialize, Serialize)]
    /// To remove all tags, `UpdateTags::SetTags` must be used
    pub enum UpdateTags {
        ModifyTags {
            tags_to_remove: Option<Vec<String>>,
            tags_to_add: Option<Vec<String>>,
        },
        SetTags(Vec<String>),
        None,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct NotesResponse {
        pub notes: Vec<NoteResponse>,
    }

    impl NotesResponse {
        pub fn new(note_responses: Vec<NoteResponse>) -> Self {
            Self {
                notes: note_responses,
            }
        }
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct NoteRenderResponse {
        pub parser_name: String,
        /// Path to the rendered note. Relative to the files directory when inside it.
        pub rendered_path: PathBuf,
        /// HTML for web clients, shown instead of the rendered file. Present iff the parser has a
        /// [web render](crate::parsers::Parseable::has_web_render) and the note has HTML.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub web_path: Option<PathBuf>,
        /// The raw note source for a client to compile itself. Present iff the parser
        /// [renders in the browser](crate::parsers::Parseable::renders_in_browser).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub browser_source: Option<String>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct NoteResponse {
        pub id: NoteId,
        pub data: String,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub parser_id: i64,
        pub parser_name: String,
        pub keywords: Vec<String>,
        pub tags: Vec<String>,
        pub custom_data: CustomData,
        /// If `None`, then it is unpopulated.
        pub linked_notes: Option<Vec<LinkedNote>>,
        pub card_count: usize,
    }

    impl NoteResponse {
        pub fn new(
            note: &Note,
            parser_name: String,
            keywords: Vec<String>,
            tags: Vec<String>,
            linked_notes: Option<Vec<LinkedNote>>,
            card_count: usize,
        ) -> Self {
            Self {
                id: note.id,
                data: note.data.clone(),
                parser_id: note.parser_id.to_owned(),
                parser_name,
                keywords,
                created_at: note.created_at,
                updated_at: note.updated_at,
                tags,
                custom_data: note.custom_data.as_object().cloned().unwrap_or_default(),
                linked_notes,
                card_count,
            }
        }
    }

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    // A leaner version of `NoteLink`
    pub struct LinkedNote {
        pub searched_keyword: String,
        pub linked_note_id: Option<NoteId>,
        pub matched_keyword: Option<String>,
    }

    impl LinkedNote {
        pub fn new(note_link: NoteLink) -> Self {
            Self {
                searched_keyword: note_link.searched_keyword,
                linked_note_id: note_link.linked_note_id,
                matched_keyword: note_link.matched_keyword,
            }
        }
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateNotesResponse {
        pub notes: Vec<NoteResponse>,
        pub event_id: Option<i64>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct FindLiveNoteRequest {
        pub live_sync_name: String,
        pub block_order: i64,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct FindLiveNoteResponse {
        pub id: Option<NoteId>,
    }
}

pub mod card {
    use chrono::DateTime;
    use chrono::Utc;
    use serde::Deserialize;
    use serde::Serialize;
    use serde_json::Value;

    use crate::model::Card;
    use crate::model::CardId;
    use crate::model::NoteId;
    use crate::model::RatingId;
    use crate::model::ReviewLog;
    use crate::model::ReviewLogKind;
    use crate::model::SpecialState;
    use crate::model::StateId;
    use crate::model::TagId;
    use crate::schema::some_option;

    #[derive(Debug, Deserialize, Serialize, Clone)]
    pub struct CardResponse {
        pub id: CardId,
        pub note_id: NoteId,
        pub order: u32,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub due: DateTime<Utc>,
        pub stability: f64,
        pub difficulty: f64,
        pub desired_retention: f64,
        pub special_state: Option<SpecialState>,
        pub state: StateId,
        pub custom_data: Value,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub enum CardsSelector {
        Ids(Vec<CardId>),
        Query(String),
    }

    #[derive(Debug, Copy, Clone, Deserialize, Serialize)]
    pub enum SpecialStateUpdate {
        Suspended,
        Buried,
        BuriedUntilLaterToday,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateCardsRequest {
        pub selector: CardsSelector,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub desired_retention: Option<f64>,
        #[serde(default, deserialize_with = "some_option")]
        pub special_state: Option<Option<SpecialStateUpdate>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub due: Option<DateTime<Utc>>,
    }

    impl CardResponse {
        pub fn new(card: &Card) -> Self {
            Self {
                id: card.id,
                note_id: card.note_id,
                order: card.order,
                created_at: card.created_at,
                updated_at: card.updated_at,
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

    #[derive(Debug, Deserialize, Serialize)]
    pub struct GetLeechesRequest {
        pub scheduler_name: String,
    }

    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct UnburyRequest {
        pub query: Option<String>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateCardsResponse {
        pub cards: Vec<CardResponse>,
        pub event_id: Option<i64>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct ForgetCardResponse {
        pub card: CardResponse,
        pub event_id: Option<i64>,
    }

    /// One row of a card's review log, with the memory state the card had after it.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct CardReviewEntry {
        pub id: i64,
        pub kind: ReviewLogKind,
        pub reviewed_at: DateTime<Utc>,
        /// Relative to `scheduler_name`.
        pub rating: Option<RatingId>,
        pub scheduler_name: String,
        pub previous_state: StateId,
        /// Seconds.
        pub scheduled_time: Option<i64>,
        /// Seconds.
        pub recall_duration: Option<i64>,
        /// Seconds.
        pub rate_duration: Option<i64>,
        pub tag_id: Option<TagId>,
        /// Replayed with the scheduler's current parameters. `None` when the card has no memory
        /// state after this row, i.e. after a forget.
        pub stability: Option<f64>,
        /// See `stability`.
        pub difficulty: Option<f64>,
    }

    impl CardReviewEntry {
        pub fn new(review_log: ReviewLog, memory_state: Option<(f64, f64)>) -> Self {
            Self {
                id: review_log.id,
                kind: review_log.kind,
                reviewed_at: review_log.reviewed_at,
                rating: review_log.rating,
                scheduler_name: review_log.scheduler_name,
                previous_state: review_log.previous_state,
                scheduled_time: review_log.scheduled_time,
                recall_duration: review_log.recall_duration,
                rate_duration: review_log.rate_duration,
                tag_id: review_log.tag_id,
                stability: memory_state.map(|(stability, _)| stability),
                difficulty: memory_state.map(|(_, difficulty)| difficulty),
            }
        }
    }
}

pub mod review {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use chrono::DateTime;
    use chrono::Duration;
    use chrono::NaiveDate;
    use chrono::Utc;
    use serde::Deserialize;
    use serde::Serialize;
    use serde_with;

    use crate::config::resolve_in_files_dir;
    use crate::model::CardId;
    use crate::model::NoteId;
    use crate::model::RatingId;
    use crate::model::StateId;
    use crate::model::TagId;
    use crate::parsers::typed_answer::TypedAnswerInfo;

    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct GetReviewCardRequest {
        pub filter: Option<GetReviewCardFilterRequest>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub enum GetReviewCardFilterRequest {
        Query(String),
        FilteredTag { tag_id: TagId },
    }

    /// Resolves a review query that uses `limit` into a filtered tag that can be reviewed with
    /// [`GetReviewCardFilterRequest::FilteredTag`].
    #[derive(Debug, Deserialize, Serialize)]
    pub struct ReviewSnapshotRequest {
        pub query: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct ReviewSnapshotResponse {
        pub tag_id: TagId,
        pub tag_name: String,
        /// `false` when an existing snapshot built earlier on the same day was resumed.
        pub rebuilt: bool,
        /// Cards currently in the snapshot.
        pub card_count: u32,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub enum CardBackRenderedPath {
        CardBack(PathBuf),
        Note(PathBuf),
    }

    impl CardBackRenderedPath {
        fn resolve(self) -> Self {
            match self {
                Self::CardBack(path) => Self::CardBack(resolve_in_files_dir(&path)),
                Self::Note(path) => Self::Note(resolve_in_files_dir(&path)),
            }
        }
    }

    /// Review info for a CLI card. The spares CLI uses `exec` to spawn an
    /// external command, reads a JSON score from its trailing stdout, and
    /// submits the score to the server's `rating_from_score` endpoint.
    /// `surrounding` is the note text outside all CLI blocks, printed
    /// verbatim in the terminal before exec runs.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct CliReviewInfo {
        pub exec: String,
        /// The surrounding note text (everything outside CLI blocks), printed
        /// in the terminal before exec runs.
        pub surrounding: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct ReviewLinkedNote {
        pub searched_keyword: String,
        pub note_id: NoteId,
        pub matched_keyword: Option<String>,
        pub note_raw_path: PathBuf,
    }

    /// Response for a review card.
    ///
    /// `PathBuf` fields inside the [files directory](crate::config::get_files_dir) are relative to
    /// it, so a web client can request `{server_url}/files/{path}` directly. Local clients call
    /// [`Self::resolve_paths`] first.
    #[serde_with::serde_as]
    #[derive(Debug, Deserialize, Serialize)]
    pub struct GetReviewCardResponse {
        pub note_id: NoteId, // To suspend all cards within the note
        pub card_order: u32,
        pub card_id: CardId,     // For submitting a rating
        pub card_state: StateId, // For showing to the user
        /// Path to the rendered card front. Relative to the files directory when inside it.
        pub card_front_rendered_path: PathBuf,
        /// Path(s) to the rendered card back. Relative to the files directory when inside it.
        pub card_back_rendered_path: CardBackRenderedPath,
        /// HTML for web clients of the card front, shown instead of the rendered file. Present iff
        /// the parser has a [web render](crate::parsers::Parseable::has_web_render) and both sides
        /// have HTML. Relative to the files directory when inside it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub card_front_web_path: Option<PathBuf>,
        /// HTML for web clients of the card back. Present iff `card_front_web_path` is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub card_back_web_path: Option<CardBackRenderedPath>,
        /// Path to the raw card front source file. Relative to the files directory when inside it.
        pub card_front_raw_path: PathBuf,
        /// Path(s) to the raw card back source file. Relative to the files directory when inside it.
        pub card_back_raw_path: CardBackRenderedPath,
        /// Path to the raw note source file. Relative to the files directory when inside it.
        pub note_raw_path: PathBuf,
        pub parser_name: String,
        pub keywords: Vec<String>,
        /// Present iff this card is a CLI card.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cli: Option<CliReviewInfo>,
        /// Present iff the parser [renders in the browser](crate::parsers::Parseable::renders_in_browser).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub browser_sources: Option<BrowserRenderSources>,
        /// Present iff the card asks for its answers to be typed (the cloze key `t`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub typed_answer: Option<TypedAnswerInfo>,
        pub cards_left_by_state: HashMap<StateId, u32>, // Count of cards left in each state for the relevant query
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub time_estimate: Duration,
        pub linked_notes: Vec<ReviewLinkedNote>,
    }

    impl GetReviewCardResponse {
        /// Turns the paths into local ones, for clients on the server's machine.
        #[must_use]
        pub fn resolve_paths(mut self) -> Self {
            self.card_front_rendered_path = resolve_in_files_dir(&self.card_front_rendered_path);
            self.card_back_rendered_path = self.card_back_rendered_path.resolve();
            self.card_front_web_path = self
                .card_front_web_path
                .map(|path| resolve_in_files_dir(&path));
            self.card_back_web_path = self.card_back_web_path.map(CardBackRenderedPath::resolve);
            self.card_front_raw_path = resolve_in_files_dir(&self.card_front_raw_path);
            self.card_back_raw_path = self.card_back_raw_path.resolve();
            self.note_raw_path = resolve_in_files_dir(&self.note_raw_path);
            for linked_note in &mut self.linked_notes {
                linked_note.note_raw_path = resolve_in_files_dir(&linked_note.note_raw_path);
            }
            self
        }
    }

    /// Raw sources for a client to compile itself. Files they read are served by the render asset
    /// endpoints.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct BrowserRenderSources {
        pub card_front: String,
        /// The card back, or the whole note if the card's back is the note.
        pub card_back: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct Rating {
        pub id: RatingId,
        pub description: String,
    }

    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct OptimizeRequest {
        /// Save the optimized parameters if they predict the review history better than the
        /// current ones, then reschedule every card with them.
        #[serde(default)]
        pub apply: bool,
    }

    /// How well a set of parameters predicts the review history. Lower is better for both.
    #[derive(Clone, Copy, Debug, Deserialize, Serialize)]
    pub struct ParametersEvaluation {
        /// The log loss of the predicted recall probabilities against what was recalled.
        pub log_loss: f32,
        /// The root mean square error between the predicted and the actual recall rate, over
        /// reviews binned by their history.
        pub rmse_bins: f32,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct OptimizeResponse {
        /// The cards whose history was used.
        pub card_count: usize,
        /// The reviews the parameters were fitted to: those on a later day than the card's
        /// previous review.
        pub review_count: usize,
        pub current_parameters: Vec<f32>,
        pub current: ParametersEvaluation,
        pub optimized_parameters: Vec<f32>,
        pub optimized: ParametersEvaluation,
        /// Whether the optimized parameters were saved and every card rescheduled.
        pub applied: bool,
    }

    #[serde_with::serde_as]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct RatingSubmission {
        pub card_id: CardId,
        pub rating: RatingId,
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub recall_duration: Duration,
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub rate_duration: Duration,
        /// Filtered tag id
        pub tag_id: Option<TagId>,
    }

    /// See <https://ankiweb.net/shared/info/759844606>
    // Note that this enum is reserved for actions that require work to be done when the action is called. Values that are for future actions, like load balancing when rescheduling, are stored in `SparesExternalConfig`. This is because changing the boolean `load_balance` will not have any immediate impact. However, when another card is submitted for review and rescheduled, then `load_balance` will impact the outcome.
    #[derive(Debug, Deserialize, Serialize)]
    pub enum StudyAction {
        Rate(RatingSubmission),
        // `SuspendCard` is not included here since it is not specifically a study action. For
        // example, you may want to suspend a card because you don't care about remembering its
        // contents anymore. On the other hand, burying is specific to reviewing. You only bury a
        // card that is scheduled to be reviewed today, but you don't want to review it today.
        Bury {
            card_id: CardId,
        },
        /// When you want to review your material ahead of time. For example, before a test.
        Advance {
            count: u32,
            query: Option<String>,
        },
        /// When you are dealing with a large number of reviews after taking a break from spaced repetition or after rescheduling.
        Postpone {
            count: u32,
            query: Option<String>,
        },
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct SubmitStudyActionRequest {
        pub scheduler_name: String,
        pub action: StudyAction,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct SubmitStudyActionResponse {
        pub event_id: Option<i64>,
    }

    /// The cards a review would show next, for a web client to review offline.
    #[derive(Debug, Deserialize, Serialize)]
    pub struct ReviewQueueRequest {
        pub filter: Option<GetReviewCardFilterRequest>,
        /// The most cards to return.
        pub limit: u32,
        /// Also include cards that become due on the next `days_ahead` days, so a client that
        /// stays offline past midnight still has cards. New cards are only included up to today's
        /// remaining limit.
        #[serde(default)]
        pub days_ahead: u32,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct ReviewQueueResponse {
        /// In the order a review would show them.
        pub cards: Vec<ReviewQueueCard>,
        /// New cards that can still be studied today, before any of `cards` are.
        pub new_cards_left_today: u32,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct ReviewQueueCard {
        /// When the card becomes due, so a client knows which cards to show on which day.
        pub due: DateTime<Utc>,
        /// `cards_left_by_state` and `time_estimate` are for the whole queue as it was fetched.
        #[serde(flatten)]
        pub card: GetReviewCardResponse,
    }

    /// Study actions a client performed while offline.
    #[derive(Debug, Deserialize, Serialize)]
    pub struct SyncOfflineActionsRequest {
        pub scheduler_name: String,
        pub actions: Vec<OfflineStudyAction>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct OfflineStudyAction {
        /// Chosen by the client, unique per action. An action that was already received is not
        /// applied again, so a client can resend a batch whose response it did not get.
        pub client_id: String,
        /// When the action was performed. A time after the server's clock is taken as the
        /// server's current time.
        pub performed_at: DateTime<Utc>,
        pub action: OfflineStudyActionKind,
    }

    /// The [`StudyAction`]s that can be performed offline.
    #[derive(Debug, Deserialize, Serialize)]
    pub enum OfflineStudyActionKind {
        Rate(RatingSubmission),
        Bury { card_id: CardId },
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct SyncOfflineActionsResponse {
        /// One per action, in the order they were submitted.
        pub results: Vec<OfflineActionResult>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct OfflineActionResult {
        pub client_id: String,
        pub outcome: OfflineActionOutcome,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub enum OfflineActionOutcome {
        /// `event_id` is the event to undo it with, if it can be undone.
        Applied { event_id: Option<i64> },
        /// It was received before.
        Duplicate,
        /// It cannot be applied, e.g. because the card was deleted or is suspended. It will not
        /// be applied if resent.
        Rejected { message: String },
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct StatisticsRequest {
        pub scheduler_name: String,
        pub date: DateTime<Utc>,
    }

    #[serde_with::serde_as]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct StatisticsResponse {
        pub cards_studied_count: u32,
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub recall_duration: Duration,
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub rate_duration: Duration,
        pub card_count_by_state: HashMap<StateId, u32>,
        pub due_count_by_state: HashMap<StateId, u32>,
        pub due_count_by_date: HashMap<NaiveDate, u32>,
        pub advance_safe_count: u32,
        pub postpone_safe_count: u32,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct ReviewHistoryResponse {
        /// The number of graded reviews on each local date that has any.
        pub review_count_by_date: HashMap<NaiveDate, u32>,
    }

    /// Config values that review clients need to offer the same actions as the CLI.
    #[serde_with::serde_as]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct ReviewConfigResponse {
        pub flagged_tag_name: String,
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub set_card_due_date_duration: Duration,
    }
}

pub mod undo {
    use serde::Deserialize;
    use serde::Serialize;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UndoEventRequest {
        /// The ID of the event to undo. If `None`, then undoes the latest event.
        pub event_id: Option<i64>,
        /// If true, undo all events in the same group as this event
        pub undo_group: bool,
    }

    /// An event that undo or redo reversed, and what it did
    #[derive(Debug, Deserialize, Serialize)]
    pub struct EventSummary {
        pub id: i64,
        /// The action the user originally took, e.g. "Rate card 3 (Good)". Undoing and redoing
        /// that action both describe it this way.
        pub description: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UndoEventResponse {
        /// All events that were undone (including the original and any in the group), oldest first
        pub undone_events: Vec<EventSummary>,
        /// The IDs of the undo events that were appended, oldest first. Pass one to redo.
        pub undo_event_ids: Vec<i64>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct RedoEventRequest {
        /// The ID of the undo event to redo. If `None`, then redoes the latest undo, unless an
        /// action has been taken since.
        pub event_id: Option<i64>,
        /// If true, redo all undo events in the same group as this event
        pub redo_group: bool,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct RedoEventResponse {
        /// All undo events that were redone, oldest first
        pub redone_events: Vec<EventSummary>,
        /// The IDs of the redo events that were appended, oldest first. Pass one to undo again.
        pub redo_event_ids: Vec<i64>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct LatestEventResponse {
        pub latest_event_id: i64,
    }
}
