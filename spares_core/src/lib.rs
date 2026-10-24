pub mod adapters;
pub mod api;
pub mod config;
pub(crate) mod helpers;
pub mod ids;
pub mod model;
pub mod notes_vcs;
pub mod parsers;
pub mod schedulers;
pub mod schema;
pub mod search;
pub mod sync;

use miette::Diagnostic;
use miette::SourceSpan;
use model::RatingId;
use model::StateId;
use thiserror::Error;

pub(crate) const ALLOWED_F64_ERROR: f64 = 0.000_001;

#[derive(Debug, Diagnostic, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("Sqlx Error: {source}")]
    Sqlx { source: sqlx::Error },
    #[error("Io Error: {description}, {source}")]
    Io {
        description: String,
        source: std::io::Error,
    },
    #[error(transparent)]
    ApiRequest(#[from] reqwest::Error),
    #[error(transparent)]
    Trash(#[from] trash::Error),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Library(#[from] LibraryError),
    /// An error from keeping the notes repository (see [`notes_vcs`]) in sync with the database.
    #[error("{0}")]
    NotesVcs(String),
    /// A `git push` into spares holds the notes lock, so note changes cannot be made right now.
    #[error("a git push into spares is in progress; retry shortly")]
    NotesVcsBusy,
    /// Sync failed. Nothing was committed on this side.
    #[error("Sync failed: {0}")]
    Sync(String),
    /// The other replica changed while syncing, so the sync has to start over. Nothing was
    /// committed on the side that returns it.
    #[error("The other replica changed during the sync; retry")]
    SyncRetry,
    /// The replicas can no longer be merged change by change. One side has to replace the other's
    /// database (`spares sync cloud --full-upload` or `--full-download`).
    #[error("A full sync is required: {0}")]
    FullSyncRequired(String),
    // #[error("{0}")]
    // Other(String),
}

// Note that `LibraryError` is `Clone` while `Error` is not.
#[derive(Clone, Debug, Diagnostic, Error)]
pub enum LibraryError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Delimiter(#[from] DelimiterErrorKind),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parser(#[from] ParserErrorKind),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Adapter(#[from] AdapterErrorKind),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Scheduler(#[from] SchedulerErrorKind),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Tag(#[from] TagErrorKind),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Note(#[from] NoteErrorKind),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Card(#[from] CardErrorKind),
    #[error("{0}")]
    InvalidConfig(String),
    /// A query that can't be searched. `message` says what is wrong and marks the place in the
    /// query.
    #[error("{message}")]
    Search {
        message: String,
        /// Where in the query the problem is, as a start and end in UTF-16 code units, the way
        /// JavaScript counts them, so the web client can mark it.
        span: Option<(usize, usize)>,
        /// The field of the request the query is in, for a request with several, like
        /// `match_query` of a
        /// [`QueuePositionRequest`](crate::schema::review::QueuePositionRequest), whose filter
        /// may be a query too. `None` otherwise.
        query_field: Option<&'static str>,
    },
    #[error("{0}")]
    InvalidTimeZone(String),
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub enum NoteErrorKind {
    #[error("{description}")]
    #[diagnostic(severity(Advice))]
    SettingsWarning {
        description: String,
        #[source_code]
        src: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("{description}")]
    InvalidSettings {
        description: String,
        #[help]
        advice: Option<String>,
        #[source_code]
        src: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("{description}")]
    Other { description: String },
    /// Notes that were named by id but do not exist.
    #[error("{}", missing_notes_message(ids))]
    NotFound { ids: Vec<model::NoteId> },
}

fn missing_notes_message(ids: &[model::NoteId]) -> String {
    let letters = ids.iter().map(|id| id.letters()).collect::<Vec<_>>();
    match letters.as_slice() {
        [id] => format!("No note has id {id}"),
        _ => format!("No notes have ids {}", letters.join(", ")),
    }
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub enum CardErrorKind {
    #[error("No cards found.")]
    NotFound {
        #[source_code]
        src: String,
    },
    #[error("Multiple cards cannot have the same clozes.")]
    MultipleDuplicateCards {
        /// List of duplicate card where the inner list contains the indices of the clozes in the card.
        duplicates: Vec<Vec<usize>>,
    },
    #[error("Clozes in the same grouping can not be nested.")]
    SameGroupingNestedClozes {
        #[source_code]
        src: String,
        #[label("First cloze")]
        cloze_1: SourceSpan,
        #[label("Second cloze")]
        cloze_2: SourceSpan,
    },
    #[error("Found a card with no `{0}` fields.")]
    MissingField(String),
    #[error("Found an empty card.")]
    Empty,
    #[error("Empty clozes are not allowed.")]
    EmptyCloze {
        #[source_code]
        src: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("{description}")]
    InvalidSettings {
        description: String,
        #[source_code]
        src: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("{0}")]
    InvalidInput(String),
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub enum TagErrorKind {
    #[error("{0}")]
    InvalidInput(String),
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub enum AdapterErrorKind {
    #[error("No adapter named `{0}` was found.")]
    NotFound(String),
    #[error("`{adapter_name}` adapter returned an error: {error}")]
    Custom { adapter_name: String, error: String },
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub enum ParserErrorKind {
    #[error("No parser named `{0}` was found.")]
    NotFound(String),
    #[error("Failed to automatically determine parser: {0}")]
    FailedToGuess(String),
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub enum SchedulerErrorKind {
    #[error("No scheduler named `{0}` was found.")]
    NotFound(String),
    #[error("Card is already buried.")]
    AlreadyBuried,
    #[error("Cannot bury suspended card.")]
    Suspended,
    #[error("Invalid state. Received `{0}`.")]
    InvalidState(StateId),
    #[error("Invalid rating. Received `{0}`.")]
    InvalidRating(RatingId),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("`{scheduler_name}` scheduler returned an error: {error}")]
    Custom {
        scheduler_name: String,
        error: String,
    },
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub enum DelimiterErrorKind {
    #[error("Unequal start and end matches.")]
    UnequalMatches {
        #[source_code]
        src: String,
    },
    #[error("Could not find corresponding start match.")]
    StartMatchNotFound {
        #[source_code]
        src: String,
    },
    #[error("Could not find corresponding end match.")]
    EndMatchNotFound {
        #[source_code]
        src: String,
    },
}
