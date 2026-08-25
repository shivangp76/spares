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
use crate::view::ViewCardArgs;
use crate::view::ViewNoteArgs;

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
    Parser(ParserArgs),
    #[command(arg_required_else_help = true)]
    Tag(TagArgs),
    #[command(arg_required_else_help = true)]
    Note(NoteArgs),
    #[command(arg_required_else_help = true)]
    Card(CardArgs),
    #[command(arg_required_else_help = true)]
    Link(LinkArgs),
    /// Import notes data from file
    Import(ImportArgs),
    /// Sync data between local note files, database, and adapters.
    ///
    /// By default, runs in interactive bulk mode: all changes are shown together and you choose
    /// to push or pull them as a group. Use `--individual` to review changes one at a time.
    ///
    /// Use `--ids` or `--files` to filter to specific notes. Use `--print-files` for
    /// non-interactive output suitable for piping to fzf for batch selection. See the
    /// workflows documentation for more details.
    Sync(SyncArgs),
    /// Migrate data from an adapter
    Migrate(MigrateArgs),
    #[command(arg_required_else_help = true)]
    Keyword(KeywordArgs),
    #[command(arg_required_else_help = true)]
    Event(EventArgs),
    /// Generate shell completions
    Completion {
        #[arg(value_enum)]
        shell: clap_complete_command::Shell,
    },
}

#[derive(Args, Debug)]
pub(crate) struct ParserArgs {
    #[command(subcommand)]
    pub(crate) command: ParserCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ParserCommands {
    /// Add a parser
    Add {
        #[arg(short, long)]
        name: String,
    },
    /// Edit a parser
    Edit {
        id: i64,
        #[arg(short, long)]
        name: String,
    },
    /// Delete a parser
    Delete { id: i64 },
    /// Get a parser
    Get { id: i64 },
    /// List parsers
    List {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
    },
}

#[derive(Args, Debug)]
pub(crate) struct TagArgs {
    #[command(subcommand)]
    pub(crate) command: TagCommands,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::option_option)]
pub(crate) enum TagCommands {
    /// Add a tag
    Add {
        #[arg(short, long)]
        name: String,
        #[arg(short, long, default_value = "")]
        description: String,
        #[arg(short, long)]
        query: Option<String>,
        #[arg(short, long, default_value_t = DEFAULT_TAG_AUTO_DELETE)]
        auto_delete: bool,
    },
    /// Edit a tag
    Edit {
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
    /// Delete a tag
    Delete { id: i64 },
    /// Get a tag by id or name
    Get {
        #[arg(short, long, required_unless_present = "name", conflicts_with = "name")]
        id: Option<i64>,
        #[arg(short, long, required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
    },
    /// List tags
    List {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
        /// Display results in long format (default)
        #[arg(long, conflicts_with_all = ["short", "tree"])]
        long: bool,
        /// Display results in short format
        #[arg(long, conflicts_with_all = ["long", "tree"])]
        short: bool,
        /// Display results as a tree
        #[arg(long, conflicts_with_all = ["long", "short"])]
        tree: bool,
    },
}

#[derive(Args, Debug)]
pub(crate) struct NoteArgs {
    #[command(subcommand)]
    pub(crate) command: NoteCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NoteCommands {
    /// Add a note
    Add {
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
        /// JSON object to set as the note's custom data (initial value on create)
        #[arg(long, value_name = "JSON")]
        custom_data: Option<String>,
    },
    /// Edit a note
    Edit {
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
        /// JSON object to set as the note's custom data (full replace on edit)
        #[arg(long, value_name = "JSON")]
        custom_data: Option<String>,
    },
    /// Delete a note
    Delete {
        #[command(flatten)]
        selector: NotesSelectorLocal,
    },
    /// Get a note
    Get { id: i64 },
    /// List notes
    List {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
        /// Display notes as a graph
        #[arg(long)]
        graph: bool,
    },
    /// View notes matching a search query
    View(ViewNoteArgs),
    /// Search for notes
    Search(SearchArgs),
    /// Export notes matching a query
    Export(ExportArgs),
    /// Generate note and card files
    Generate(GenerateArgs),
}

#[derive(Args, Debug)]
pub(crate) struct CardArgs {
    #[command(subcommand)]
    pub(crate) command: CardCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CardCommands {
    /// Edit a card
    Edit {
        #[command(flatten)]
        selector: CardsSelectorLocal,
        #[arg(short, long)]
        desired_retention: Option<f64>,
        #[arg(short, long)]
        special_state: Option<SpecialStateLocal>,
        #[arg(long)]
        due: Option<DateTime<Utc>>,
    },
    /// Get a card by id or note id
    Get {
        #[arg(
            short,
            long,
            required_unless_present = "note_id",
            conflicts_with = "note_id"
        )]
        id: Option<i64>,
        #[arg(short, long, required_unless_present = "id", conflicts_with = "id")]
        note_id: Option<i64>,
    },
    /// List cards
    List {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// View cards matching a search query (opens card backs)
    View(ViewCardArgs),
    /// Search for cards
    Search(SearchArgs),
    /// Study cards
    Review(ReviewArgs),
    /// Advance cards (review material ahead of time)
    Advance(AdvanceArgs),
    /// Postpone cards (delay reviews)
    Postpone(PostponeArgs),
    /// Forget cards (reset scheduling, keep review logs)
    Forget(ForgetCardArgs),
    /// Unbury all cards
    Unbury {
        #[arg(short, long)]
        query: Option<String>,
    },
    /// Get leeches (cards that are frequently forgotten)
    Leeches {
        #[arg(short, long, default_value = "fsrs")]
        scheduler_name: String,
    },
    /// Studying statistics
    #[command(alias = "stats")]
    Statistics(StatisticsArgs),
}

#[derive(Args, Debug)]
pub(crate) struct LinkArgs {
    #[command(subcommand)]
    pub(crate) command: LinkCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LinkCommands {
    /// List note links, only showing links with a score below the threshold
    List {
        /// Only notes with scores below this will be returned
        #[arg(short, long)]
        score_threshold: Score,
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
    #[arg(short, long)]
    pub(crate) query: Option<String>,
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
#[command(group(
    ArgGroup::new("target")
        .args(&["ids", "query"])
        .required(true)
))]
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

#[derive(Args, Debug)]
pub(crate) struct EventArgs {
    #[command(subcommand)]
    pub(crate) command: EventCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum EventCommands {
    /// Get the latest note-mutation event id (monotonically increasing; for change detection)
    Latest,
    /// Undo an event
    Undo(UndoArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum KeywordCommands {
    /// List all keywords
    List {
        /// Display only deduped keyword strings, one per line
        #[arg(long)]
        short: bool,
    },
    /// Search for a keyword (returns best match)
    Search { keyword: String },
    /// Search for a keyword and show all matches ranked
    Ranking { keyword: String },
    /// Get unmatched keywords
    Unmatched,
    /// Get keywords associated with more than 1 note
    Duplicate,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum)]
pub(crate) enum OutputFormat {
    RawFilepath,
    RenderedFilepath,
}

#[derive(Args, Debug)]
pub(crate) struct SearchArgs {
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::Parser;

    use super::*;

    #[test]
    fn cli_contract_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn card_get_requires_id_or_note_id() {
        assert!(Cli::try_parse_from(["spares", "card", "get"]).is_err());
        assert!(Cli::try_parse_from(["spares", "card", "get", "--id", "1"]).is_ok());
        assert!(Cli::try_parse_from(["spares", "card", "get", "--note-id", "1"]).is_ok());
    }

    #[test]
    fn tag_get_requires_id_or_name() {
        assert!(Cli::try_parse_from(["spares", "tag", "get"]).is_err());
        assert!(Cli::try_parse_from(["spares", "tag", "get", "--id", "1"]).is_ok());
        assert!(Cli::try_parse_from(["spares", "tag", "get", "--name", "foo"]).is_ok());
    }

    #[test]
    fn tag_edit_rebuild_accepts_id_or_tag_name() {
        assert!(
            Cli::try_parse_from(["spares", "tag", "edit", "--tag-name", "foo", "--rebuild"])
                .is_ok()
        );
        assert!(Cli::try_parse_from(["spares", "tag", "edit", "--id", "1", "--rebuild"]).is_ok());
    }

    #[test]
    fn card_forget_requires_ids_or_query() {
        assert!(Cli::try_parse_from(["spares", "card", "forget"]).is_err());
        assert!(Cli::try_parse_from(["spares", "card", "forget", "--ids", "1"]).is_ok());
        assert!(Cli::try_parse_from(["spares", "card", "forget", "--query", "foo"]).is_ok());
    }

    #[test]
    fn card_list_parses() {
        let cli = Cli::try_parse_from(["spares", "card", "list"]).unwrap();
        match cli.command {
            Commands::Card(CardArgs {
                command: CardCommands::List { page, limit },
            }) => {
                assert!(page.is_none());
                assert!(limit.is_none());
            }
            _ => panic!("expected card list"),
        }
        let cli = Cli::try_parse_from(["spares", "card", "list", "--page", "2", "--limit", "50"])
            .unwrap();
        match cli.command {
            Commands::Card(CardArgs {
                command: CardCommands::List { page, limit },
            }) => {
                assert_eq!(page, Some(2));
                assert_eq!(limit, Some(50));
            }
            _ => panic!("expected card list"),
        }
    }

    #[test]
    fn tag_list_flags_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["spares", "tag", "list", "--short", "--tree"]).is_err());
        assert!(Cli::try_parse_from(["spares", "tag", "list", "--long", "--short"]).is_err());
        assert!(Cli::try_parse_from(["spares", "tag", "list", "--short"]).is_ok());
        assert!(Cli::try_parse_from(["spares", "tag", "list"]).is_ok());
    }

    #[test]
    fn parser_edit_requires_name() {
        assert!(Cli::try_parse_from(["spares", "parser", "edit", "1"]).is_err());
        assert!(Cli::try_parse_from(["spares", "parser", "edit", "1", "--name", "x"]).is_ok());
    }
}
