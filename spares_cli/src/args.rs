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

use crate::deploy::DeployArgs;
use crate::import::ImportArgs;
use crate::migrate::MigrateArgs;
use crate::review::ReviewArgs;
use crate::statistics::Period;
use crate::statistics::Section;
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
    /// Use `--ids` or `--files` to filter to specific notes, e.g.
    /// `spares sync --from anki --to spares --ids aae aal gir`. Add `--dry-run` to only see the
    /// changes.
    ///
    /// Use `--print-files` for non-interactive output: it prints the cache-rendered paths of
    /// changed notes, which `--files` accepts, so they can be selected with fzf:
    ///
    /// `spares sync --print-files | fzf -m --preview 'bat --color=always {}' | xargs spares sync
    /// --files @-`
    Sync(SyncArgs),
    /// Migrate data from an adapter
    Migrate(MigrateArgs),
    #[command(arg_required_else_help = true)]
    Keyword(KeywordArgs),
    #[command(arg_required_else_help = true)]
    Event(EventArgs),
    /// Deploy spares to a VPS as the sync server, and set up this machine to sync with it
    ///
    /// spares is built here, from the checkout of the spares repository this runs in (or else the
    /// one this binary was built from), as it is, and uploaded: the server needs neither the
    /// memory and disk to build it nor access to GitHub (which IPv6-only servers lack). Building
    /// needs zig and cargo-zigbuild (`brew install zig && cargo install --locked cargo-zigbuild`)
    /// and bun.
    #[command(arg_required_else_help = true)]
    Deploy(DeployArgs),
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
        /// For a filtered tag, whether rating its cards while studying it reschedules them. Pass
        /// `--reschedule false` to cram without changing when cards are next due: the ratings
        /// count as crams in `spares card stats` and `spares card history`, apart from reviews.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        reschedule: bool,
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
        /// For a filtered tag, whether rating its cards while studying it reschedules them
        #[arg(long)]
        reschedule: Option<bool>,
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
    Get { id: NoteId },
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
    /// Keep notes in a git (or jj) repository you push to, instead of syncing local files
    ///
    /// Spares writes notes to a repository it commits to (`notes-spares` in the data directory),
    /// and your notes directory becomes a clone of it. Fetch to get changes made in spares; push
    /// to `main` to import your changes. The server must be running. Rerunning continues an
    /// interrupted setup.
    ///
    /// The old notes directory is kept as `notes.pre-vcs-<time>`. Notes you had edited without
    /// syncing are put in their own commit (the `pre-vcs-local-edits` branch, or a jj change), which
    /// is printed. A file can also differ because spares changed the note without rewriting it (an
    /// undo, a tag rename), and pushing such a file would revert that change, so review them before
    /// pushing them. Other files from the old directory are copied into the clone as untracked
    /// files. Delete the old directory once you are done.
    ///
    /// See "Notes as a git or jj repository" in the manual's Workflows page for using the clone.
    InitVcs {
        /// Make the notes directory a jj repository colocated with git
        #[arg(long)]
        jj: bool,
        /// Only rewrite the push hooks, e.g. after reinstalling spares or changing its address
        #[arg(long)]
        repair_hooks: bool,
    },
    /// Compare the notes repository with the database
    VcsCheck {
        /// Rewrite note files that differ from the database, and commit them
        #[arg(long)]
        fix: bool,
    },
    /// Run by the notes repository's git hooks
    #[command(hide = true)]
    VcsHook {
        phase: VcsHookPhase,
        args: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum VcsHookPhase {
    PreReceive,
    PushToCheckout,
    PostReceive,
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
        note_id: Option<NoteId>,
    },
    /// List cards
    List {
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Show a card's review log and totals (card info)
    Reviews {
        id: CardId,
        /// Print the raw review log as JSON
        #[arg(long)]
        json: bool,
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
    /// Recompute every card's memory state and due date from its review history (run after
    /// changing the scheduler, its parameters, learning steps or easy days)
    ///
    /// Changing these settings only affects cards as they are next rated. This applies the change
    /// to every card now: it replays each card's review history to recompute its memory state,
    /// then places it again with the load balancing, sibling dispersal and easy days settings.
    /// Suspended and buried cards are included and keep their state.
    ///
    /// It always reschedules every card; there is no `--query`. Cards are placed one at a time,
    /// each seeing the due dates already given to the others, so rescheduling only some of them
    /// would leave the load spread across days unbalanced.
    ///
    /// It cannot be undone, so it asks for confirmation.
    Reschedule(RescheduleArgs),
    /// Fit the scheduler's parameters to your review history and compare them with the current
    /// ones. Use `--apply` to save them and reschedule every card.
    ///
    /// Once you have a few hundred reviews, parameters fitted to your own history usually predict
    /// your memory better than the defaults. This prints the fitted parameters next to two
    /// measures of how well each set predicts the reviews you actually did, where lower is better:
    /// the log loss, and the RMSE between predicted and actual recall rates. Only reviews on a
    /// later day than the card's previous review count, and cards whose history was cut short by
    /// an import are skipped. Without `--apply`, it does not change anything.
    ///
    /// With `--apply`, the parameters are saved to `fsrs_parameters` in config.toml only if they
    /// predict your reviews better than the current ones, and every card is rescheduled. Like
    /// `spares card reschedule`, this cannot be undone, so it asks for confirmation. Optimizing
    /// again every month or so keeps the parameters in step with your history.
    Optimize(OptimizeArgs),
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
    /// Review heatmap of the past weeks, with streaks and daily average
    History {
        /// Number of weeks to show. Defaults to as many as fit in the terminal, up to a year.
        #[arg(short, long)]
        weeks: Option<usize>,
        /// Print the review count of each day as JSON
        #[arg(long)]
        json: bool,
    },
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
    /// Only the cards matching this search query, and their reviews, e.g. `tag=languages`
    #[arg(short, long)]
    pub(crate) query: Option<String>,
    /// The days the reviews, hourly breakdown, answer buttons and added cards cover, today
    /// included, or `all`
    #[arg(short, long, default_value = "30")]
    pub(crate) period: Period,
    /// The days the forecast covers, today included
    #[arg(short, long, default_value_t = 30)]
    pub(crate) forecast_days: u32,
    /// Only these sections, in the statistics page's order. Defaults to all of them.
    #[arg(long = "section", value_enum, num_args = 1..)]
    pub(crate) sections: Vec<Section>,
    /// Print the statistics as JSON: `statistics` (today's), `graphs` and `history` (the
    /// calendar's), each only when a section asked for needs it
    #[arg(long)]
    pub(crate) json: bool,
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

// There is no `--query`: rescheduling places cards one at a time against the due dates of every
// other card, so rescheduling only some of them would leave the load spread across days
// unbalanced. Settings also apply to all cards, so all cards are rescheduled.
#[derive(Args, Debug)]
pub(crate) struct RescheduleArgs {
    #[arg(short, long, default_value = "fsrs")]
    pub(crate) scheduler_name: String,
    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub(crate) yes: bool,
}

#[derive(Args, Debug)]
pub(crate) struct OptimizeArgs {
    #[arg(short, long, default_value = "fsrs")]
    pub(crate) scheduler_name: String,
    /// Save the optimized parameters to the config if they predict your reviews better than the
    /// current ones, then reschedule every card
    #[arg(short, long)]
    pub(crate) apply: bool,
    /// Skip the confirmation prompt for `--apply`
    #[arg(short, long)]
    pub(crate) yes: bool,
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
pub(crate) struct RedoArgs {
    /// Undo event ID to redo. If not provided, redoes the latest undo, unless an action was taken
    /// after it.
    #[arg(short, long)]
    pub(crate) event_id: Option<i64>,
    /// If true, redo all undo events in the same group as the specified event
    #[arg(short, long, default_value_t = false)]
    pub(crate) redo_group: bool,
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
    /// Redo an undone event
    Redo(RedoArgs),
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
    /// The query, like `tag=math c.due<=today`. `--fields` lists the fields it can use.
    #[arg(required_unless_present = "fields")]
    pub(crate) query: Option<String>,
    /// List the fields a query can use, with the kind of value each holds and an example
    #[arg(long, conflicts_with = "query")]
    pub(crate) fields: bool,
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
        assert!(Cli::try_parse_from(["spares", "card", "get", "--note-id", "gir"]).is_ok());
        // Old numeric note ids are rejected.
        assert!(Cli::try_parse_from(["spares", "card", "get", "--note-id", "1"]).is_err());
        assert!(Cli::try_parse_from(["spares", "note", "get", "42"]).is_err());
        assert!(Cli::try_parse_from(["spares", "note", "get", "GIR"]).is_ok());
        assert!(Cli::try_parse_from(["spares", "note", "delete", "--ids", "gir", "giza"]).is_ok());
    }

    #[test]
    fn card_reviews_requires_id() {
        assert!(Cli::try_parse_from(["spares", "card", "reviews"]).is_err());
        assert!(Cli::try_parse_from(["spares", "card", "reviews", "1"]).is_ok());
        assert!(Cli::try_parse_from(["spares", "card", "reviews", "1", "--json"]).is_ok());
    }

    #[test]
    fn card_statistics_parses() {
        let parse = |args: &[&str]| {
            let mut all = vec!["spares", "card", "stats"];
            all.extend(args);
            match Cli::try_parse_from(all).map(|cli| cli.command) {
                Ok(Commands::Card(CardArgs {
                    command: CardCommands::Statistics(args),
                })) => Some(args),
                _ => None,
            }
        };
        let defaults = parse(&[]).unwrap();
        assert_eq!(defaults.period, Period(Some(30)));
        assert_eq!(defaults.forecast_days, 30);
        assert_eq!(defaults.sections, []);
        let args = parse(&[
            "--query",
            "tag=a",
            "--period",
            "all",
            "--forecast-days",
            "90",
            "--section",
            "retention",
            "hourly",
        ])
        .unwrap();
        assert_eq!(args.query.as_deref(), Some("tag=a"));
        assert_eq!(args.period, Period(None));
        assert_eq!(args.sections, [Section::Retention, Section::Hourly]);
        assert!(parse(&["--section", "nope"]).is_none());
        assert!(parse(&["--period", "0"]).is_none());
    }

    #[test]
    fn card_history_parses() {
        assert!(Cli::try_parse_from(["spares", "card", "history"]).is_ok());
        assert!(
            Cli::try_parse_from(["spares", "card", "history", "--weeks", "10", "--json"]).is_ok()
        );
        assert!(Cli::try_parse_from(["spares", "card", "history", "--weeks", "x"]).is_err());
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
    fn migrate_parses() {
        assert!(Cli::try_parse_from(["spares", "migrate", "--adapter", "anki"]).is_ok());
        assert!(Cli::try_parse_from(["spares", "migrate"]).is_err());
        let cli = Cli::try_parse_from(["spares", "migrate", "ids", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Migrate(MigrateArgs {
                command: Some(crate::migrate::MigrateCommands::Ids { dry_run: true }),
                ..
            })
        ));
        assert!(Cli::try_parse_from(["spares", "migrate", "ids"]).is_ok());
    }

    #[test]
    fn parser_edit_requires_name() {
        assert!(Cli::try_parse_from(["spares", "parser", "edit", "1"]).is_err());
        assert!(Cli::try_parse_from(["spares", "parser", "edit", "1", "--name", "x"]).is_ok());
    }
}
