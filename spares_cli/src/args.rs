use std::path::PathBuf;

use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use clap::ArgGroup;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use spares_core::api::tag::DEFAULT_TAG_AUTO_DELETE;
use spares_core::config::Environment;
use spares_core::model::CardId;
use spares_core::model::NoteId;
use spares_core::model::Score;
use spares_core::parsers::get_note_info_from_filepath;
use spares_core::schema::note::NotesSelector;

use crate::import::ImportArgs;
use crate::migrate::MigrateArgs;
use crate::review::ReviewArgs;
use crate::sync::SyncArgs;

pub(crate) fn get_current_utc_datetime() -> DateTime<Utc> {
    let local_time = Local::now();
    local_time.with_timezone(&Utc)
}

/// Spaced Repetition System
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub(crate) struct Cli {
    #[arg(short, long, default_value_t = Environment::Production)]
    pub(crate) environment: Environment,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    #[command(arg_required_else_help = true)]
    Add(AddArgs),
    #[command(arg_required_else_help = true)]
    Edit(EditArgs),
    #[command(arg_required_else_help = true)]
    Delete(DeleteArgs),
    #[command(arg_required_else_help = true)]
    Get(GetArgs),
    #[command(arg_required_else_help = true)]
    List(ListArgs),
    /// Generate note and card files
    Generate(GenerateArgs),
    /// Study cards
    Review(ReviewArgs),
    /// Studying statistics
    #[command(alias = "stats")]
    Statistics(StatisticsArgs),
    /// Search for notes or cards
    Search(SearchArgs),
    /// Import notes data from file
    Import(ImportArgs),
    /// Sync data between local note files, database, and adapters.
    ///
    /// By default, runs in interactive bulk mode: all changes are shown together and you choose
    /// to push or pull them as a group. Use `--individual` to review changes one at a time.
    ///
    /// The `render-diffs` subcommand is non-interactive: it writes diffs to a directory so you
    /// can use a tool like `fzf` to select which diffs to apply, then import them with
    /// `spares import`. See the workflows documentation for a more detailed example.
    Sync(SyncArgs),
    /// Migrate data from an adapter
    Migrate(MigrateArgs),
    /// Export notes matching a query
    Export(ExportArgs),
    #[command(arg_required_else_help = true)]
    Keyword(KeywordArgs),
    #[command(arg_required_else_help = true)]
    Schedule(ScheduleArgs),
    /// Undo an event
    Undo(UndoArgs),
    /// Generate shell completions
    GenerateShellCompletion {
        #[arg(value_enum)]
        shell: clap_complete_command::Shell,
    },
}

#[derive(Args, Debug)]
pub(crate) struct AddArgs {
    #[command(subcommand)]
    pub(crate) command: AddCommands,
}

#[derive(Args, Debug)]
pub(crate) struct DeleteArgs {
    #[command(subcommand)]
    pub(crate) command: DeleteCommands,
}

#[derive(Args, Debug)]
pub(crate) struct EditArgs {
    #[command(subcommand)]
    pub(crate) command: EditCommands,
}

#[derive(Args, Debug)]
pub(crate) struct GetArgs {
    #[command(subcommand)]
    pub(crate) command: GetCommands,
}

#[derive(Args, Debug)]
pub(crate) struct ListArgs {
    #[command(subcommand)]
    pub(crate) command: ListCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AddCommands {
    Parser {
        #[arg(short, long)]
        name: String,
    },
    Tag {
        #[arg(short, long)]
        name: String,
        #[arg(short, long, default_value = "")]
        description: String,
        #[arg(short, long)]
        query: Option<String>,
        #[arg(short, long, default_value_t = DEFAULT_TAG_AUTO_DELETE)]
        auto_delete: bool,
    },
    Note {
        #[arg(short, long)]
        data: String,
        #[arg(short, long)]
        parser_id: i64,
        #[arg(short, long, default_value = "")]
        keywords: String,
        #[arg(short, long, value_delimiter = ' ', num_args = 1..)]
        tags: Vec<String>,
        #[arg(short, long, default_value_t = false)]
        is_suspended: bool,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::option_option)]
pub(crate) enum EditCommands {
    Parser {
        id: i64,
        #[arg(short, long)]
        name: Option<String>,
    },
    Tag {
        /// ID of the tag to modify
        #[arg(
            long,
            required_unless_present = "tag_name",
            conflicts_with = "tag_name"
        )]
        id: Option<i64>,
        /// Name of the tag to modify
        #[arg(long, required_unless_present = "id", conflicts_with = "id")]
        tag_name: Option<String>,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
        #[arg(short, long)]
        query: Option<Option<String>>,
        #[arg(short, long)]
        auto_delete: Option<bool>,
        /// Rebuild the tag's dynamic membership instead of patching it
        #[arg(long, default_value_t = false)]
        rebuild: bool,
    },
    Note {
        #[command(flatten)]
        selector: NotesSelectorLocal,
        #[arg(short, long)]
        data: Option<String>,
        #[arg(short, long)]
        parser_id: Option<i64>,
        #[arg(short, long)]
        keywords: Option<String>,
        #[arg(long, value_delimiter = ' ', num_args = 1..)]
        tags_to_remove: Option<Vec<String>>,
        #[arg(long, value_delimiter = ' ', num_args = 1..)]
        tags_to_add: Option<Vec<String>>,
        #[arg(long, default_value_t = false)]
        remove_all_tags: bool,
    },
    Card {
        #[command(flatten)]
        selector: CardsSelectorLocal,
        #[arg(short, long)]
        desired_retention: Option<f64>,
        #[arg(short, long)]
        special_state: Option<SpecialStateLocal>,
        #[arg(long)]
        due: Option<DateTime<Utc>>,
    },
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum)]
pub(crate) enum SpecialStateLocal {
    None,
    Suspended,
    Buried,
    // This is not allowed.
    // SchedulerBuried,
}

#[derive(Debug, Parser)]
#[command(group(
    ArgGroup::new("filter")
        .args(&["ids", "files", "query"])
        .required(true)
))]
pub(crate) struct NotesSelectorLocal {
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    pub(crate) ids: Option<Vec<NoteId>>,
    #[arg(short, long, value_delimiter = ' ', num_args = 1..)]
    pub(crate) files: Option<Vec<PathBuf>>,
    #[arg(short, long)]
    pub(crate) query: Option<String>,
}

#[derive(Debug, Parser)]
#[command(group(
    ArgGroup::new("filter")
        .args(&["ids", "query"])
        .required(true)
))]
pub(crate) struct CardsSelectorLocal {
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    pub(crate) ids: Option<Vec<CardId>>,
    // #[arg(short, long, value_delimiter = ' ', num_args = 1..)]
    // files: Option<Vec<PathBuf>>,
    #[arg(short, long)]
    pub(crate) query: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DeleteCommands {
    Parser {
        id: i64,
    },
    Tag {
        id: i64,
    },
    Note {
        #[command(flatten)]
        selector: NotesSelectorLocal,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum GetCommands {
    Parser {
        id: i64,
    },
    Tag {
        #[arg(short, long)]
        id: Option<i64>,
        #[arg(short, long, conflicts_with = "id")]
        name: Option<String>,
    },
    Note {
        id: i64,
        // /// Open in editor
        // #[arg(short, long, default_value_t = false)]
        // use_editor: bool,
    },
    Card {
        #[arg(short, long)]
        id: Option<i64>,
        #[arg(short, long, conflicts_with = "id")]
        note_id: Option<i64>,
    },
}

#[derive(Debug, Copy, Clone, Default, PartialEq, ValueEnum)]
pub(crate) enum ListTagOutput {
    #[default]
    Long,
    Short,
    Tree,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ListCommands {
    Parser {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
    },
    Tag {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
        /// Display results in long format
        #[arg(long, default_value_t = true, overrides_with_all = ["short", "tree"])]
        long: bool,
        /// Display results in short format
        #[arg(long, overrides_with_all = ["long", "tree"])]
        short: bool,
        /// Display results as a tree
        #[arg(long, overrides_with_all = ["long", "short"])]
        tree: bool,
    },
    Note {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
        #[arg(long)]
        graph: bool,
    },
    NoteLink {
        /// Only notes with scores below this will be returned
        #[arg(short, long)]
        score_threshold: Score,
    },
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Args, Debug)]
pub(crate) struct GenerateArgs {
    #[arg(short, long)]
    pub(crate) query: Option<String>,
    #[arg(short, long)]
    pub(crate) overridden_output_raw_dir: Option<PathBuf>,
    #[arg(long, default_value_t = true)]
    pub(crate) include_linked_notes: bool,
    #[arg(short, long, default_value_t = true)]
    pub(crate) include_cards: bool,
    #[arg(short, long, default_value_t = false)]
    pub(crate) render: bool,
    #[arg(short, long, default_value_t = false)]
    pub(crate) force_render: bool,
}

#[derive(Args, Debug)]
pub(crate) struct StatisticsArgs {
    #[arg(short, long, default_value = "fsrs")]
    pub(crate) scheduler_name: String,
    #[arg(short, long, default_value_t = get_current_utc_datetime())]
    pub(crate) date: DateTime<Utc>,
}

#[derive(Args, Debug)]
pub(crate) struct ExportArgs {
    pub(crate) query: String,
    #[arg(short, long)]
    pub(crate) output_dir: PathBuf,
}

#[derive(Debug, Parser)]
pub(crate) struct ForgetCardArgs {
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    pub(crate) ids: Option<Vec<i64>>,
    #[arg(short, long)]
    pub(crate) query: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct AdvanceArgs {
    /// Number of cards to advance
    pub(crate) count: u32,
    #[arg(short, long, default_value = "fsrs")]
    pub(crate) scheduler_name: String,
    #[arg(short, long)]
    pub(crate) query: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct PostponeArgs {
    /// Number of cards to postpone
    pub(crate) count: u32,
    #[arg(short, long, default_value = "fsrs")]
    pub(crate) scheduler_name: String,
    #[arg(short, long)]
    pub(crate) query: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct UndoArgs {
    /// Event ID to undo. If not provided, undoes the latest event.
    #[arg(short, long)]
    pub(crate) event_id: Option<i64>,
    /// If true, undo all events in the same group as the specified event
    #[arg(short, long, default_value_t = false)]
    pub(crate) undo_group: bool,
}

#[derive(Args, Debug)]
pub(crate) struct KeywordArgs {
    #[command(subcommand)]
    pub(crate) command: KeywordCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum KeywordCommands {
    /// Get unmatched keywords
    Unmatched,
    /// Get keywords associated with more than 1 note
    Duplicate,
    /// Search for a keyword (returns best match)
    Search { keyword: String },
    /// Search for a keyword and show all matches ranked
    Ranking { keyword: String },
}

#[derive(Args, Debug)]
pub(crate) struct ScheduleArgs {
    #[command(subcommand)]
    pub(crate) command: ScheduleCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ScheduleCommands {
    /// Forget cards (reset scheduling, keep review logs)
    Forget(ForgetCardArgs),
    /// Get leeches (cards that are frequently forgotten)
    Leeches {
        #[arg(short, long, default_value = "fsrs")]
        scheduler_name: String,
    },
    /// Unbury all cards
    Unbury {
        #[arg(short, long)]
        query: Option<String>,
    },
    /// Advance cards (review material ahead of time)
    Advance(AdvanceArgs),
    /// Postpone cards (delay reviews)
    Postpone(PostponeArgs),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum)]
pub(crate) enum OutputItemType {
    Notes,
    Cards,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum)]
pub(crate) enum OutputFormat {
    RawFilepath,
    RenderedFilepath,
}

#[derive(Args, Debug)]
pub(crate) struct SearchArgs {
    #[arg(short, long, default_value = "notes")]
    pub(crate) output_type: OutputItemType,
    #[arg(long, default_value = "raw-filepath")]
    pub(crate) output_format: OutputFormat,
    pub(crate) query: String,
}

impl NotesSelectorLocal {
    pub(crate) fn get_notes_selector(self) -> Result<NotesSelector, String> {
        if let Some(ids) = self.ids {
            Ok(NotesSelector::Ids(ids))
        } else if let Some(files) = self.files {
            let notes_filepath_data_res = files
                .into_iter()
                .map(|f| get_note_info_from_filepath(&f))
                .collect::<Result<Vec<_>, _>>();
            match notes_filepath_data_res {
                Ok(note_filepath_data) => {
                    let file_note_ids = note_filepath_data
                        .into_iter()
                        .map(|d| d.note_id)
                        .collect::<Vec<_>>();
                    Ok(NotesSelector::Ids(file_note_ids))
                }
                Err(e) => Err(format!("Failed to parse files: {}", e)),
            }
        } else if let Some(query) = self.query {
            Ok(NotesSelector::Query(query))
        } else {
            Err("should be unreachable by clap conflicts with".to_string())
        }
    }
}
