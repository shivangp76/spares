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
        /// See [`Tag::reschedule`]. `true` when left out.
        #[serde(default = "reschedule_default")]
        pub reschedule: bool,
    }

    fn reschedule_default() -> bool {
        true
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub reschedule: Option<bool>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct TagResponse {
        pub id: TagId,
        pub name: String,
        pub description: String,
        pub query: Option<String>,
        pub auto_delete: bool,
        pub reschedule: bool,
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
                reschedule: tag.reschedule,
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

    /// One page of the results of a search, in the order the search returns them.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct SearchPageRequest {
        pub query: String,
        pub output_type: QueryReturnItemType,
        /// The page, from 1.
        pub page: usize,
        /// How many results a page has, from 1 to 1000.
        pub limit: usize,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct SearchPageResponse {
        /// How many results there are on every page together.
        pub total: usize,
        pub items: SearchPageItems,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub enum SearchPageItems {
        Notes(Vec<NoteMatch>),
        Cards(Vec<(CardResponse, String)>),
    }

    /// A note a search found, and where its text has what the search looks for.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct NoteMatch {
        pub note: NoteResponse,
        pub parser_name: String,
        /// Where the note's text has the words, text or regexes the query searches it for, as
        /// starts and ends in UTF-16 code units, in order and not overlapping. Terms the query
        /// excludes, like `-dog`, are not marked.
        pub matches: Vec<(usize, usize)>,
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

    /// The notes a query finds, with every note they link to, directly or through other notes,
    /// even those the query doesn't find: the notes they build on, which come first. Notes that
    /// link to the ones found, which build on them, are left out unless the query finds them too
    /// or [`Self::descendant_depth`] asks for them.
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct NoteGraphRequest {
        /// The notes to show, with the notes they link to. Empty for every note.
        pub query: String,
        /// The highest score a link can have and still be followed. Defaults to the queue's
        /// `linked_note_max_score`, so the graph shows the links that hold cards back.
        #[serde(default)]
        pub max_score: Option<Score>,
        /// How many links to follow up from the notes found: `Some(1)` shows only the notes they
        /// link to directly. All of them if `None`.
        #[serde(default)]
        pub ancestor_depth: Option<u32>,
        /// How many links down from the notes found to follow, to the notes that build on them:
        /// `Some(1)` shows the notes that link to them directly. All of them if `None`. Defaults
        /// to `Some(0)`, for none.
        #[serde(default = "no_descendants")]
        pub descendant_depth: Option<u32>,
        /// Queries that put the notes they find in a group, to color them by. A note is in the
        /// first group whose query finds it. An empty query finds none.
        #[serde(default)]
        pub groups: Vec<String>,
        /// The most notes to show, up to [`MAX_GRAPH_NODES`]. Notes the query finds come first,
        /// then the notes they link to, nearest first, then the notes that link to them.
        #[serde(default = "default_graph_nodes")]
        pub max_nodes: usize,
    }

    /// The most notes [`NoteGraphRequest::max_nodes`] can ask for.
    pub const MAX_GRAPH_NODES: usize = 5000;

    fn default_graph_nodes() -> usize {
        2000
    }

    #[allow(clippy::unnecessary_wraps, reason = "a serde default")]
    fn no_descendants() -> Option<u32> {
        Some(0)
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct NoteGraphResponse {
        pub nodes: Vec<GraphNode>,
        /// Every link between two of the notes in `nodes`.
        pub edges: Vec<GraphEdge>,
        /// The links of the notes in `nodes` that match no note's keyword, which point to notes
        /// that are yet to be written.
        pub unmatched: Vec<UnmatchedLink>,
        /// Whether notes were left out to keep to `max_nodes`.
        pub truncated: bool,
        /// Whether links have been found between notes yet. Until they are, there are none to show.
        pub links_generated: bool,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct GraphNode {
        pub id: NoteId,
        pub keywords: Vec<String>,
        pub tags: Vec<String>,
        pub relation: GraphRelation,
        /// How many links from the nearest note the query found: 0 for those notes.
        pub depth: u32,
        /// The index of the first of [`NoteGraphRequest::groups`] whose query finds the note.
        pub group: Option<usize>,
        pub cards: GraphNodeCards,
    }

    /// Why a note is in a graph.
    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(rename_all = "lowercase")]
    pub enum GraphRelation {
        /// The query found it.
        Found,
        /// A note the query found builds on it.
        Prerequisite,
        /// It builds on a note the query found.
        Dependent,
    }

    /// A link that matches no note's keyword.
    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    pub struct UnmatchedLink {
        /// The note with the link.
        pub from: NoteId,
        /// The text of the link, which no keyword matches.
        pub keyword: String,
    }

    /// How many of a note's cards are in each state. Suspended cards are counted only as
    /// suspended.
    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct GraphNodeCards {
        pub new: u32,
        /// Learning or relearning.
        pub learning: u32,
        pub review: u32,
        pub suspended: u32,
    }

    /// A note linking to a note it builds on.
    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    pub struct GraphEdge {
        /// The note with the link.
        pub from: NoteId,
        /// The note it links to.
        pub to: NoteId,
        /// The keyword of `to` the link matched.
        pub keyword: String,
        pub score: Option<Score>,
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
        /// The event to undo creating the notes with, if it was logged
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub event_id: Option<i64>,
    }

    impl NotesResponse {
        pub fn new(note_responses: Vec<NoteResponse>) -> Self {
            Self {
                notes: note_responses,
                event_id: None,
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

    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct DeleteNotesResponse {
        /// The event to undo the delete with, if it was logged and deleted any notes
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

    /// The notes of one live sync group as an import is about to leave them. See
    /// [`crate::api::note::relocate_live_cards`].
    #[derive(Debug, Deserialize, Serialize)]
    pub struct RelocateLiveCardsRequest {
        pub live_sync_name: String,
        pub notes: Vec<LiveNoteClozes>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct LiveNoteClozes {
        pub note_id: NoteId,
        /// The `id:` keys of the clozes of the note's new data.
        pub cloze_uids: Vec<String>,
        /// Whether the import just created the note, so its cards were made from its new data.
        pub created: bool,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct RelocateLiveCardsResponse {
        pub moved_card_count: usize,
        /// The event to undo the moves with, if any card moved and it was logged
        #[serde(default)]
        pub event_id: Option<i64>,
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
    use crate::model::RescheduleData;
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
        /// Whether this review was a lapse, as `SrsScheduler::lapses` counts them.
        #[serde(default)]
        pub lapse: bool,
        /// For a [`ReviewLogKind::Reschedule`] row, the due dates before and after the card was
        /// moved: a postponement if `due` is later.
        #[serde(default)]
        pub reschedule: Option<RescheduleData>,
    }

    impl CardReviewEntry {
        pub fn new(review_log: ReviewLog, memory_state: Option<(f64, f64)>, lapse: bool) -> Self {
            let reschedule = review_log.reschedule_data();
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
                lapse,
                reschedule,
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
    use crate::parsers::audio::CardAudio;
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
        /// When the note was last changed, for clients to tell whether the card shown is out of
        /// date, e.g. after `spares import` of its file. The same as the note's `updated_at`.
        #[serde(default)]
        pub note_updated_at: DateTime<Utc>,
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
        /// Present iff the card has audio clips. Their paths are absolute local paths, which web
        /// clients fetch from the render assets.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub audio: Option<CardAudio>,
        /// The number of cards left in each state for the relevant query, as many as today's
        /// limits leave room for. With `reviews_daily_limit = 200`, 600 reviews due and 50
        /// already done today, reviews show 150, not 550. Learning and relearning cards, and the
        /// cards of a filtered tag, are not limited, so every one due is counted. A state with
        /// nothing left is left out. `time_estimate` is for the cards counted, and the learning cards due later today.
        pub cards_left_by_state: HashMap<StateId, u32>,
        /// How long what is left should take, from how long the user's latest reviews of each
        /// preset took, counting the learning steps a new card goes through the same day.
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

    #[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
    pub struct Rating {
        pub id: RatingId,
        pub description: String,
    }

    /// What spares treats a [`CardState`] as, whatever the scheduler calls it. See
    /// [`crate::schedulers::SrsScheduler::get_states`].
    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash, Serialize)]
    pub enum StateRole {
        /// Never studied. Counts toward the daily new card limit, and is buried with
        /// [`crate::config::QueueConfig::bury_new_siblings`].
        New,
        /// In the first learning steps: comes back the same day, and is never limited.
        Learning,
        /// Graduated from its learning steps. Counts toward the daily review limit, is buried
        /// with [`crate::config::QueueConfig::bury_review_siblings`], and placed by
        /// [`crate::schedulers::placement`]. Failing it is a lapse.
        Review,
        /// Lapsed and back in relearning steps: comes back the same day like
        /// [`StateRole::Learning`], and holds back linked reviews with
        /// [`crate::config::QueueConfig::reviews_after_linked_notes`].
        Relearning,
    }

    /// A state a scheduler can put a card in.
    #[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
    pub struct CardState {
        /// What [`crate::model::Card::state`] holds for a card in this state.
        pub id: StateId,
        /// What search's `c.state=<name>`, statistics and card info call it.
        pub name: String,
        pub role: StateRole,
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
        /// The earliest a client may show the card, for a learning or relearning card held back
        /// by [`QueueConfig::learn_ahead_limit`](crate::config::QueueConfig::learn_ahead_limit):
        /// its `due` minus the limit. `None` for other cards, and for every card of a filtered
        /// tag, which holds nothing back.
        ///
        /// For example, with the default limit of 20 minutes, a card rated at 12:00 onto a 1
        /// hour step is due at 13:00 and has `shown_from` 12:40. Offline review shows it once
        /// nothing else is left and it is 12:40, as online review does, rather than at once.
        pub shown_from: Option<DateTime<Utc>>,
        /// `cards_left_by_state` and `time_estimate` are for the whole queue as it was fetched.
        #[serde(flatten)]
        pub card: GetReviewCardResponse,
    }

    /// Where the cards matching `match_query` are in the queue of the review with `filter` (see
    /// [`queue_positions`](crate::api::queue_position::queue_positions)).
    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct QueuePositionRequest {
        /// The review, as for [`GetReviewCardRequest`]. `None` for the default review.
        pub filter: Option<GetReviewCardFilterRequest>,
        /// The cards to locate, as a search query whose card filters (like `c.state=0`) pick
        /// cards rather than whole notes. `None` lists the whole queue.
        #[serde(default)]
        pub match_query: Option<String>,
        /// The most cards to list in [`QueuePositionResponse::positions`] and in
        /// [`QueuePositionResponse::not_shown`]. `None` lists them all.
        #[serde(default)]
        pub limit: Option<u32>,
    }

    /// A card of a [`QueuePositionResponse`].
    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct QueueCard {
        pub card_id: CardId,
        pub note_id: NoteId,
        pub card_order: u32,
        pub card_state: StateId,
        pub due: DateTime<Utc>,
        /// The start of the note's text, on one line, as a list of notes shows it: at most 100
        /// characters, with `…` where it is cut.
        pub note_preview: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct QueuePositionResponse {
        /// How many cards the review would show now.
        pub total: u32,
        /// How many of them match the query: `total` when there is none.
        pub matching: u32,
        /// Where the matching cards are, in order, up to the request's `limit`.
        pub positions: Vec<QueuePosition>,
        /// Matching learning and relearning cards due later today, held back by
        /// [`QueueConfig::learn_ahead_limit`](crate::config::QueueConfig::learn_ahead_limit).
        /// They have no position: review shows them once nothing else is left and their time has
        /// come. Soonest first.
        pub held_back: Vec<HeldBackCard>,
        /// How many matching cards of the review it would not show today.
        pub not_shown_total: u32,
        /// Those cards, up to the request's `limit`, and why. Empty when there is no query.
        pub not_shown: Vec<NotShownCard>,
        /// How many matching cards are not in the review's filter at all.
        pub not_in_review: u32,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct QueuePosition {
        /// From 1, for the card review shows next.
        pub position: u32,
        #[serde(flatten)]
        pub card: QueueCard,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct HeldBackCard {
        /// The earliest review can show the card: its `due` minus the learn ahead limit.
        pub shown_from: DateTime<Utc>,
        #[serde(flatten)]
        pub card: QueueCard,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct NotShownCard {
        pub reason: NotShownReason,
        #[serde(flatten)]
        pub card: QueueCard,
    }

    /// Why a review would not show a card of its filter today.
    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub enum NotShownReason {
        Suspended,
        /// Buried by hand or by the scheduler, until it is unburied
        Buried,
        /// Due after today
        NotDue,
        /// Held back by
        /// [`QueueConfig::new_after_linked_notes`](crate::config::QueueConfig::new_after_linked_notes)
        /// or
        /// [`QueueConfig::reviews_after_linked_notes`](crate::config::QueueConfig::reviews_after_linked_notes)
        /// until a note it links to is studied
        WaitingOnLinkedNote,
        /// Today's new card limit is used up, the top-level one or that of `preset`
        NewLimit {
            preset: Option<String>,
        },
        /// Today's review limit is used up, the top-level one or that of `preset`
        ReviewLimit {
            preset: Option<String>,
        },
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
        /// The IANA time zone to tell days apart in, e.g. `Europe/Berlin`. `None` is the server's
        /// own. Web clients send the browser's zone and the CLI the computer's, so a day is the
        /// user's day even when the server is elsewhere. The daily limits left are still counted
        /// in the server's zone, as review counts them.
        #[serde(default)]
        pub tz: Option<String>,
    }

    #[serde_with::serde_as]
    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct StatisticsResponse {
        /// Distinct cards reviewed on the date. Crams are counted apart, in `cram_count`.
        pub cards_studied_count: u32,
        /// Time spent on the date's reviews, crams excluded.
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub recall_duration: Duration,
        /// See `recall_duration`.
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        pub rate_duration: Duration,
        /// Ratings on the date in filtered tags that do not reschedule
        /// ([`ReviewLogKind::Cram`](crate::model::ReviewLogKind::Cram)), counting each rating, so
        /// a card crammed twice counts twice. For example, going twice through the 60 cards of
        /// an `exam-cram` tag gives 120, while `cards_studied_count` stays 0.
        // Defaulted, so responses cached by clients before crams were logged still decode.
        #[serde(default)]
        pub cram_count: u32,
        /// Time spent on those crams, recalling and rating together.
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        #[serde(default)]
        pub cram_duration: Duration,
        pub card_count_by_state: HashMap<StateId, u32>,
        /// The cards due on the date by state, overdue ones included, with new and review cards
        /// capped by what is left of the date's limits, like
        /// [`GetReviewCardResponse::cards_left_by_state`]. For example, with the default limit of
        /// 20 new cards, 5 studied on the date and 300 new cards in the collection, new shows
        /// 15. Cards buried by the user or the scheduler count on the next day instead.
        pub due_count_by_state: HashMap<StateId, u32>,
        pub due_count_by_date: HashMap<NaiveDate, u32>,
        pub advance_safe_count: u32,
        pub postpone_safe_count: u32,
    }

    /// The query string of `GET /api/review/history`. Both are optional, so a request without
    /// any counts every review in the server's time zone, as before they existed.
    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct ReviewHistoryRequest {
        /// Only reviews of the cards matching this search query. Orphaned reviews, whose card was
        /// deleted, match no query, so they count only without one.
        #[serde(default)]
        pub query: Option<String>,
        /// See [`StatisticsRequest::tz`].
        #[serde(default)]
        pub tz: Option<String>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct ReviewHistoryResponse {
        /// The number of graded reviews on each local date that has any.
        pub review_count_by_date: HashMap<NaiveDate, u32>,
        /// The number of crams
        /// ([`ReviewLogKind::Cram`](crate::model::ReviewLogKind::Cram)) on each local date that
        /// has any. A heatmap adds them to the reviews, so a day spent cramming counts as a day
        /// studied, and can show them apart.
        #[serde(default)]
        pub cram_count_by_date: HashMap<NaiveDate, u32>,
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

pub mod statistics {
    //! `POST /api/statistics/graphs`: the statistics page's graphs, like Anki's statistics screen.
    //!
    //! Definitions used throughout, computed in `api::statistics::graphs`:
    //!
    //! - **Reviews** are the `review_log` rows of kind
    //!   [`Review`](crate::model::ReviewLogKind::Review). Forgets are never counted. Orphaned
    //!   reviews, whose card was deleted, count in totals without a query, but in no per-card
    //!   figure.
    //! - **Crams** ([`Cram`](crate::model::ReviewLogKind::Cram)) count in [`TodayStats`],
    //!   [`ReviewDay`], [`HourStats`] and the review heatmap, always apart from reviews, with a %
    //!   correct of their own. They are left out of true retention, answer buttons, young/mature
    //!   and last review times, since the scheduler did not learn from them. Going twice through a
    //!   60 card tag adds 120 ratings, mostly Good the second time, which would swamp the reviews'
    //!   % correct.
    //! - **Passed** means rated with a rating the scheduler counts as remembered
    //!   ([`SrsScheduler::is_pass`](crate::schedulers::SrsScheduler::is_pass)): Hard, Good or Easy
    //!   with FSRS.
    //! - **Learning, relearning, young and mature** classify a review by the card's state before
    //!   it. Reviews of new and learning cards are learning, of relearning cards relearning, and of
    //!   review cards young or mature: mature when the interval going into the review (see
    //!   `review_log.previous_interval`) was at least
    //!   [`StatisticsConfig::mature_threshold_days`](crate::config::StatisticsConfig::mature_threshold_days).
    //!   A card is mature when the time from its last review to its due date is.
    //! - **Days and hours** are local to [`StatisticsGraphsRequest::tz`].
    use std::collections::HashMap;

    use chrono::DateTime;
    use chrono::NaiveDate;
    use chrono::Utc;
    use serde::Deserialize;
    use serde::Serialize;

    use crate::model::RatingId;
    use crate::model::StateId;
    use crate::schema::review::Rating;

    fn default_forecast_days() -> u32 {
        30
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct StatisticsGraphsRequest {
        pub scheduler_name: String,
        /// Only the cards matching this search query, and their reviews. `None` is every card.
        #[serde(default)]
        pub query: Option<String>,
        /// The days the reviews, hourly breakdown, answer buttons and added cards cover: the date
        /// and the `period_days - 1` days before it. `None` is all time.
        #[serde(default)]
        pub period_days: Option<u32>,
        /// The days the forecast covers, the date included. Defaults to 30, and is at most
        /// [`MAX_FORECAST_DAYS`].
        #[serde(default = "default_forecast_days")]
        pub forecast_days: u32,
        /// "Now": the day statistics are for. Reviews after it are left out.
        pub date: DateTime<Utc>,
        /// See [`StatisticsRequest::tz`](crate::schema::review::StatisticsRequest::tz).
        #[serde(default)]
        pub tz: Option<String>,
    }

    /// The most days [`StatisticsGraphsRequest::forecast_days`] can ask for, ten years.
    pub const MAX_FORECAST_DAYS: u32 = 3650;

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    pub struct StatisticsGraphsResponse {
        /// The [`StatisticsConfig::mature_threshold_days`](crate::config::StatisticsConfig::mature_threshold_days)
        /// young and mature were told apart by, for labels such as "Mature (≥ 21 days)".
        pub mature_threshold_days: u32,
        /// The local date of [`StatisticsGraphsRequest::date`].
        pub date: NaiveDate,
        /// The first day of the period. With `period_days: None`, the day of the earliest review
        /// or card, or `date` if there are none.
        pub period_start: NaiveDate,
        pub today: TodayStats,
        /// One entry per day from `date`, `forecast_days` in all.
        pub forecast: Vec<ForecastDay>,
        /// The days of the period with reviews or crams, in order. Days without any are left out.
        pub reviews: Vec<ReviewDay>,
        pub reviews_summary: ReviewsSummary,
        /// Today, yesterday, the last 7 days, the last 30 days, the last year and all time, in
        /// that order.
        pub true_retention: Vec<RetentionRow>,
        pub answer_buttons: AnswerButtons,
        /// Indexed by local hour, 0 to 23, over the period.
        pub hourly: [HourStats; 24],
        pub card_counts: CardCounts,
        /// The days of the period with cards added, in order.
        pub added: Vec<AddedDay>,
        pub distributions: Distributions,
    }

    /// The reviews and crams on [`StatisticsGraphsRequest::date`].
    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct TodayStats {
        /// Every rating counts, so a card failed and then passed counts twice. Crams excluded.
        pub reviews: u32,
        /// The reviews passed. `passed / reviews` is the % correct.
        pub passed: u32,
        /// Spent recalling and rating the reviews. `seconds / reviews` is the time per card.
        pub seconds: i64,
        /// `reviews` split by the card's state before the review.
        pub learning: u32,
        pub relearning: u32,
        pub young: u32,
        pub mature: u32,
        pub crams: u32,
        pub crams_passed: u32,
        pub cram_seconds: i64,
    }

    #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct ForecastDay {
        pub date: NaiveDate,
        /// Learning and relearning cards due on the day.
        pub learning: u32,
        pub review: u32,
    }

    #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct ReviewDay {
        pub date: NaiveDate,
        pub learning: u32,
        pub relearning: u32,
        pub young: u32,
        pub mature: u32,
        pub cram: u32,
        /// Spent on the day's reviews and crams together.
        pub seconds: i64,
    }

    #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct ReviewsSummary {
        /// Reviews over the period, crams excluded.
        pub reviews: u32,
        pub crams: u32,
        /// Spent on reviews and crams.
        pub seconds: i64,
        /// The days of the period with a review or a cram.
        pub days_studied: u32,
        /// The days in the period, today included.
        pub days: u32,
        /// Reviews and crams per day of the period, days without any included.
        pub daily_average: f64,
    }

    #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct PassFail {
        pub passed: u32,
        pub failed: u32,
    }

    /// Anki's true retention: of the reviews of cards in the review state over a span of days,
    /// how many were passed. Learning and relearning steps are left out, since they are
    /// reviewed again within minutes.
    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    pub struct RetentionRow {
        /// E.g. "Last 7 days".
        pub label: String,
        /// The local days covered, today included, or `None` for all time. Yesterday is 1.
        pub days: Option<u32>,
        pub young: PassFail,
        pub mature: PassFail,
    }

    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct AnswerButtons {
        /// The scheduler's ratings, worst first, to label the counts with.
        pub ratings: Vec<Rating>,
        /// Reviews of new, learning and relearning cards.
        pub learning: AnswerGroup,
        pub young: AnswerGroup,
        pub mature: AnswerGroup,
    }

    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct AnswerGroup {
        /// One per rating of [`AnswerButtons::ratings`], in the same order, zeros included.
        pub counts: Vec<(RatingId, u32)>,
        pub passed: u32,
        pub total: u32,
    }

    #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct HourStats {
        pub reviews: u32,
        pub passed: u32,
        pub crams: u32,
        pub crams_passed: u32,
    }

    /// Every card matching the query once. Suspended and buried cards are counted apart from the
    /// states, so `by_state`, `suspended` and `buried` add up to `total`.
    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct CardCounts {
        /// Cards neither suspended nor buried, by state.
        pub by_state: HashMap<StateId, u32>,
        /// The review state of `by_state`, split. Cards without a review to measure their
        /// interval from count as young.
        pub young: u32,
        pub mature: u32,
        pub suspended: u32,
        /// Buried by the user or the scheduler, or until later today.
        pub buried: u32,
        pub total: u32,
    }

    #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct AddedDay {
        pub date: NaiveDate,
        pub cards: u32,
    }

    /// Histograms over the cards matching the query that are not new.
    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct Distributions {
        /// In days, from each card's last review to its due date.
        pub interval: Histogram,
        /// In days.
        pub stability: Histogram,
        pub difficulty: Histogram,
        /// From 0 to 1, now. `None` if the scheduler has no forgetting curve
        /// ([`SrsScheduler::retrievability`](crate::schedulers::SrsScheduler::retrievability)).
        pub retrievability: Option<Histogram>,
        /// The sum of every card's retrievability: how many of them you would recall now.
        pub total_knowledge: Option<f64>,
    }

    /// Cards counted into buckets: bucket `i` holds the values from `edges[i]` up to
    /// `edges[i + 1]`, the last bucket including its upper edge. Long tails are folded into the
    /// last bucket, so it can be wider than the rest.
    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct Histogram {
        /// One more than `counts`, ascending. Empty when there are no values.
        pub edges: Vec<f64>,
        pub counts: Vec<u32>,
        /// The mean of every value, `None` when there are none.
        pub mean: Option<f64>,
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

    /// Events to put in one group, so one undo of the group reverts them all, e.g. those of an
    /// import made of several requests
    #[derive(Debug, Deserialize, Serialize)]
    pub struct GroupEventsRequest {
        pub event_ids: Vec<i64>,
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

pub mod saved_search {
    use serde::Deserialize;
    use serde::Serialize;

    /// A search saved by name. See [`crate::api::saved_search`].
    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, sqlx::FromRow)]
    pub struct SavedSearch {
        pub name: String,
        pub query: String,
    }
}
