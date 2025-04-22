pub mod adapters;
pub mod api;
pub mod config;
pub(crate) mod helpers;
pub mod model;
pub mod parsers;
pub mod schedulers;
pub mod schema;
pub mod search;

use miette::{Diagnostic, SourceSpan};
use model::{RatingId, StateId};
use thiserror::Error;

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
    #[error("{0}")]
    Search(String),
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
