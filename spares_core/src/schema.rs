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
    }

    impl TagResponse {
        pub fn new(tag: &Tag) -> Self {
            Self {
                id: tag.id.to_owned(),
                name: tag.name.clone(),
                description: tag.description.clone(),
                query: tag.query.clone(),
                auto_delete: tag.auto_delete,
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
    pub struct NoteResponse {
        pub id: NoteId,
        pub data: String,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub parser_id: i64,
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
            keywords: Vec<String>,
            tags: Vec<String>,
            linked_notes: Option<Vec<LinkedNote>>,
            card_count: usize,
        ) -> Self {
            Self {
                id: note.id.to_owned(),
                data: note.data.clone(),
                parser_id: note.parser_id.to_owned(),
                keywords,
                created_at: note.created_at,
                updated_at: note.updated_at,
                tags,
                custom_data: note.custom_data.as_object().unwrap().clone(),
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
    use crate::model::SpecialState;
    use crate::model::StateId;
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

    use crate::model::CardId;
    use crate::model::NoteId;
    use crate::model::RatingId;
    use crate::model::StateId;
    use crate::model::TagId;

    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct GetReviewCardRequest {
        pub filter: Option<GetReviewCardFilterRequest>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub enum GetReviewCardFilterRequest {
        Query(String),
        FilteredTag { tag_id: TagId },
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub enum CardBackRenderedPath {
        CardBack(PathBuf),
        Note(PathBuf),
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
    /// All `PathBuf` fields are absolute paths when `SPARES_FILES_DIR` is unset.
    /// When `SPARES_FILES_DIR` is set (e.g. in `spares_server`), paths are returned
    /// relative to that directory so a web client can construct
    /// `{server_url}/files/{path}` directly.
    #[serde_with::serde_as]
    #[derive(Debug, Deserialize, Serialize)]
    pub struct GetReviewCardResponse {
        pub note_id: NoteId, // To suspend all cards within the note
        pub card_order: u32,
        pub card_id: CardId,     // For submitting a rating
        pub card_state: StateId, // For showing to the user
        /// Path to the rendered card front. Relative to `SPARES_FILES_DIR` when set.
        pub card_front_rendered_path: PathBuf,
        /// Path(s) to the rendered card back. Relative to `SPARES_FILES_DIR` when set.
        pub card_back_rendered_path: CardBackRenderedPath,
        /// Path to the raw card front source file. Relative to `SPARES_FILES_DIR` when set.
        pub card_front_raw_path: PathBuf,
        /// Path(s) to the raw card back source file. Relative to `SPARES_FILES_DIR` when set.
        pub card_back_raw_path: CardBackRenderedPath,
        /// Path to the raw note source file. Relative to `SPARES_FILES_DIR` when set.
        pub note_raw_path: PathBuf,
        pub parser_name: String,
        pub keywords: Vec<String>,
        /// Present iff this card is a CLI card.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cli: Option<CliReviewInfo>,
        pub cards_left_by_state: HashMap<StateId, u32>, // Count of cards left in each state for the relevant query
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub time_estimate: Duration,
        pub linked_notes: Vec<ReviewLinkedNote>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct Rating {
        pub id: RatingId,
        pub description: String,
    }

    #[serde_with::serde_as]
    #[derive(Debug, Deserialize, Serialize)]
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
        /// When you either:
        /// 1. Update easy days
        /// 2. Change schedulers
        /// 3. Update the scheduler's parameters
        // Replaces `ApplyEasyDays`
        Reschedule,
        // Undo,
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

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UndoEventResponse {
        /// The IDs of all events that were undone (including the original and any in the group)
        pub undone_event_ids: Vec<i64>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct LatestEventResponse {
        pub latest_event_id: i64,
    }
}
