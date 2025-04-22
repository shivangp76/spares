use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FilterOptions {
    pub page: Option<usize>,
    pub limit: Option<usize>,
}

pub mod parser {
    use crate::model::Parser;
    use serde::{Deserialize, Serialize};

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
    use crate::model::{Tag, TagId};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize)]
    pub struct CreateTagRequest {
        pub name: String,
        pub description: String,
        pub parent_id: Option<TagId>,
        pub query: Option<String>,
        pub auto_delete: bool,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateTagRequest {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub parent_id: Option<Option<TagId>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub query: Option<Option<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub auto_delete: Option<bool>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct TagResponse {
        pub id: TagId,
        pub parent_id: Option<TagId>,
        pub name: String,
        pub description: String,
        pub query: Option<String>,
        pub auto_delete: bool,
    }

    impl TagResponse {
        pub fn new(tag: &Tag) -> Self {
            Self {
                id: tag.id.to_owned(),
                parent_id: tag.parent_id,
                name: tag.name.clone(),
                description: tag.description.clone(),
                query: tag.query.clone(),
                auto_delete: tag.auto_delete,
            }
        }
    }
}

pub mod note {
    use super::card::CardResponse;
    use crate::{
        helpers::parse_list,
        model::{CustomData, Note, NoteId, NoteLink},
        search::QueryReturnItemType,
    };
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub enum GenerateFilesNoteIds {
        Query(String),
        NoteIds(Vec<NoteId>),
    }

    #[allow(clippy::struct_excessive_bools, reason = "needed to generate files")]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct RenderNotesRequest {
        /// If `None`, then all notes will have their files generated.
        pub generate_files_note_ids: Option<GenerateFilesNoteIds>,
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
    pub enum NotesSelector {
        Ids(Vec<NoteId>),
        Query(String),
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
        /// Note that `tags_to_remove` is processed before `tags_to_add`.
        /// Passing "*" in `tags_to_remove` removes all tags, so that the entire field can be overridden.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub tags_to_remove: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub tags_to_add: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub custom_data: Option<CustomData>,
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
            tags: Vec<String>,
            linked_notes: Option<Vec<LinkedNote>>,
            card_count: usize,
        ) -> Self {
            Self {
                id: note.id.to_owned(),
                data: note.data.clone(),
                parser_id: note.parser_id.to_owned(),
                keywords: parse_list(note.keywords.as_str()),
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
}

pub mod card {
    use crate::model::{Card, CardId, NoteId, SpecialState, StateId};
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

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

    #[derive(Debug, Deserialize, Serialize)]
    pub enum SpecialStateUpdate {
        Suspended,
        Buried,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UpdateCardRequest {
        pub selector: CardsSelector,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub desired_retention: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub special_state: Option<Option<SpecialStateUpdate>>,
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
}

pub mod review {
    use chrono::{DateTime, Duration, NaiveDate, Utc};
    use serde::{Deserialize, Serialize};
    use serde_with;
    use std::{collections::HashMap, path::PathBuf};

    use crate::model::{CardId, NoteId, RatingId, StateId, TagId};

    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct GetReviewCardRequest {
        // This `Option` is used instead of flattening directly to represent the fact that either both arguments are provided or none are provided. Having only 1 argument provided is invalid. For example, `only_due_today` without providing a query is invalid.
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

    #[derive(Debug, Deserialize, Serialize)]
    pub struct GetReviewCardResponse {
        pub note_id: NoteId, // To suspend all cards within the note
        pub card_order: u32,
        pub card_id: CardId,                   // For submitting a rating
        pub card_front_rendered_path: PathBuf, // To show card
        pub card_back_rendered_path: CardBackRenderedPath, // To allow the user to see the answer after rating the card
        pub note_raw_path: PathBuf, // To allow the user to edit the note if they find an error while reviewing the card
        pub parser_name: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
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
        pub duration: Duration,
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
        },
        /// When you are dealing with a large number of reviews after taking a break from Anki or after rescheduling.
        Postpone {
            count: u32,
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
    pub struct StatisticsRequest {
        pub scheduler_name: String,
        pub date: DateTime<Utc>,
    }

    #[serde_with::serde_as]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct StatisticsResponse {
        pub cards_studied_count: u32,
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub study_time: Duration,
        pub card_count_by_state: HashMap<StateId, u32>,
        pub due_count_by_state: HashMap<StateId, u32>,
        pub due_count_by_date: HashMap<NaiveDate, u32>,
        pub advance_safe_count: u32,
        pub postpone_safe_count: u32,
    }
}
