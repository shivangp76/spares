mod graph;
mod import;
mod migrate;
mod review;
mod sync;
mod tree;

use chrono::{DateTime, Local, Utc};
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use graph::chart;
use import::{ImportArgs, import_from_files};
use inquire::Confirm;
use miette::{Error, IntoDiagnostic, miette};
use migrate::{MigrateArgs, migrate_from_adapter};
use reqwest::{Client, StatusCode};
use review::{ReviewArgs, forget_card, review_cards};
use serde_json::{Map, Value};
use spares::{
    adapters::get_adapter_from_string,
    api::tag::DEFAULT_TAG_AUTO_DELETE,
    config::{Environment, get_env_config},
    model::{CardId, NoteId, NoteLink, Score},
    parsers::{
        RenderOutputDirectoryType, find_parser,
        generate_files::{CardSide, RenderOutputType},
        get_all_parsers, get_note_info_from_filepath, get_output_raw_dir,
    },
    schema::{
        card::{
            CardResponse, CardsSelector, GetLeechesRequest, SpecialStateUpdate, UpdateCardsRequest,
        },
        note::{
            CreateNoteRequest, CreateNotesRequest, DeleteNotesRequest, ExportNotesRequest,
            MatchedKeywordResponse, NoteLinksRequest, NoteResponse, NotesResponse, NotesSelector,
            RenderNotesRequest, SearchKeywordRequest, SearchNotesRequest, SearchNotesResponse,
            UnmatchedKeywordResponse, UpdateNotesRequest, UpdateTags,
        },
        parser::{CreateParserRequest, ParserResponse, UpdateParserRequest},
        review::{StatisticsRequest, StatisticsResponse, StudyAction, SubmitStudyActionRequest},
        tag::{CreateTagRequest, TagResponse, UpdateTagRequest},
    },
    search::QueryReturnItemType,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{io, path::PathBuf, str::FromStr};
use sync::{SyncArgs, sync_notes};
use tree::{build_tree, print_tree};

async fn ensure_ok(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| miette!("{}", e))?;
        let message = response_json.get("message");
        return Err(miette!(message.unwrap().to_string()));
    }
    Ok(response)
}

/// Spaced Repetition System
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long, default_value_t = Environment::Production)]
    environment: Environment,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
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
    /// There are 2 modes to sync data: interactive and rendered diffs. Interactive mode will walk
    /// you through the changes. Rendered diffs mode works by rendering the differences between the
    /// 2 data source in a separate directory. You can then use a tool like `fzf` to select which
    /// diffs you would like to push and import them with `spares_cli import`. See the workflows
    /// documentation for a more detailed example.
    Sync(SyncArgs),
    /// Migrate data from an adapter
    Migrate(MigrateArgs),
    /// Export notes matching a query
    Export(ExportArgs),
    /// Get unmatched keywords
    UnmatchedKeywords,
    /// Get keywords associated with more than 1 note
    DuplicateKeywords,
    /// Rebuild a tag's dynamic membership
    RebuildTag {
        #[arg(short, long)]
        id: i64,
    },
    /// Forget cards (reset scheduling, keep review logs)
    ForgetCard(ForgetCardArgs),
    /// Get leeches (cards that are frequently forgotten)
    Leeches {
        #[arg(short, long, default_value = "fsrs")]
        scheduler_name: String,
    },
    /// Unbury all cards
    Unbury,
    /// Advance cards (review material ahead of time)
    Advance(AdvanceArgs),
    /// Postpone cards (delay reviews)
    Postpone(PostponeArgs),
    /// Generate shell completions
    GenerateShellCompletion {
        #[arg(value_enum)]
        shell: clap_complete_command::Shell,
    },
}

#[derive(Args, Debug)]
struct AddArgs {
    #[command(subcommand)]
    command: AddCommands,
}

#[derive(Args, Debug)]
struct DeleteArgs {
    #[command(subcommand)]
    command: DeleteCommands,
}

#[derive(Args, Debug)]
struct EditArgs {
    #[command(subcommand)]
    command: EditCommands,
}

#[derive(Args, Debug)]
struct GetArgs {
    #[command(subcommand)]
    command: GetCommands,
}

#[derive(Args, Debug)]
struct ListArgs {
    #[command(subcommand)]
    command: ListCommands,
}

#[derive(Debug, Subcommand)]
enum AddCommands {
    Parser {
        #[arg(short, long)]
        name: String,
    },
    Tag {
        #[arg(short, long)]
        name: String,
        #[arg(short, long, default_value = "")]
        description: String,
        #[arg(short, long, default_value = None)]
        parent_id: Option<i64>,
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
enum EditCommands {
    Parser {
        id: i64,
        #[arg(short, long)]
        name: Option<String>,
    },
    Tag {
        id: i64,
        #[arg(short, long)]
        parent_id: Option<Option<i64>>,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
        #[arg(short, long)]
        query: Option<Option<String>>,
        #[arg(short, long)]
        auto_delete: Option<bool>,
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
enum SpecialStateLocal {
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
struct NotesSelectorLocal {
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    ids: Option<Vec<NoteId>>,
    #[arg(short, long, value_delimiter = ' ', num_args = 1..)]
    files: Option<Vec<PathBuf>>,
    #[arg(short, long)]
    query: Option<String>,
}

#[derive(Debug, Parser)]
#[command(group(
    ArgGroup::new("filter")
        .args(&["ids", "query"])
        .required(true)
))]
struct CardsSelectorLocal {
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    ids: Option<Vec<CardId>>,
    // #[arg(short, long, value_delimiter = ' ', num_args = 1..)]
    // files: Option<Vec<PathBuf>>,
    #[arg(short, long)]
    query: Option<String>,
}

#[derive(Debug, Subcommand)]
enum DeleteCommands {
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
enum GetCommands {
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
enum ListTagOutput {
    #[default]
    Full,
    Short,
    Tree,
}

#[derive(Debug, Subcommand)]
enum ListCommands {
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
        #[arg(short, long, default_value = "full")]
        output: ListTagOutput,
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
struct GenerateArgs {
    #[arg(short, long)]
    query: Option<String>,
    #[arg(short, long)]
    overridden_output_raw_dir: Option<PathBuf>,
    #[arg(long, default_value_t = true)]
    include_linked_notes: bool,
    #[arg(short, long, default_value_t = true)]
    include_cards: bool,
    #[arg(short, long, default_value_t = false)]
    render: bool,
    #[arg(short, long, default_value_t = false)]
    force_render: bool,
}

#[derive(Args, Debug)]
struct StatisticsArgs {
    #[arg(short, long, default_value = "fsrs")]
    scheduler_name: String,
    #[arg(short, long, default_value_t = get_current_utc_datetime())]
    date: DateTime<Utc>,
}

#[derive(Args, Debug)]
struct ExportArgs {
    query: String,
}

#[derive(Debug, Parser)]
struct ForgetCardArgs {
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    ids: Option<Vec<i64>>,
    #[arg(short, long)]
    query: Option<String>,
}

#[derive(Args, Debug)]
struct AdvanceArgs {
    /// Number of cards to advance
    count: u32,
    #[arg(short, long, default_value = "fsrs")]
    scheduler_name: String,
    #[arg(short, long)]
    query: Option<String>,
}

#[derive(Args, Debug)]
struct PostponeArgs {
    /// Number of cards to postpone
    count: u32,
    #[arg(short, long, default_value = "fsrs")]
    scheduler_name: String,
    #[arg(short, long)]
    query: Option<String>,
}

fn get_current_utc_datetime() -> DateTime<Utc> {
    let local_time = Local::now();
    local_time.with_timezone(&Utc)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum)]
enum OutputItemType {
    Notes,
    Cards,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum)]
enum OutputFormat {
    RawFilepath,
    RenderedFilepath,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum, Default)]
enum SearchMode {
    #[default]
    Query,
    Keyword,
    KeywordRanking,
}

#[derive(Args, Debug)]
struct SearchArgs {
    #[arg(short, long, default_value = "query")]
    mode: SearchMode,
    // This option does not work if `matches!(mode, SearchMode::Keyword)`. There is no easy way to get around this since clap does not support default subcommands.
    #[arg(short, long, default_value = "notes")]
    output_type: OutputItemType,
    // This option does not work if `matches!(mode, SearchMode::Keyword)`. There is no easy way to get around this since clap does not support default subcommands.
    #[arg(long, default_value = "raw-filepath")]
    output_format: OutputFormat,
    // Positional argument
    query: String,
}

impl NotesSelectorLocal {
    fn get_notes_selector(self) -> Result<NotesSelector, String> {
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

async fn list_parsers(
    page: Option<usize>,
    limit: Option<usize>,
    base_url: &str,
    client: &Client,
) -> Result<Vec<ParserResponse>, Error> {
    let url = format!("{}/api/parsers", base_url);
    let mut queries: Vec<(&str, String)> = Vec::new();
    if let Some(page) = page {
        queries.push(("page", page.to_string()));
    }
    if let Some(limit) = limit {
        queries.push(("limit", limit.to_string()));
    }
    let req_url = client
        .get(url)
        .query(&queries)
        .build()
        .unwrap()
        .url()
        .to_string();
    let response = client
        .get(&req_url)
        .send()
        .await
        .map_err(|e| miette!("{}", e))?;
    let response = ensure_ok(response).await?;
    let parser_responses: Vec<ParserResponse> =
        response.json().await.map_err(|e| miette!("{}", e))?;
    Ok(parser_responses)
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let args = Cli::parse();
    let res = process_args(args).await;
    if let Err(e) = res {
        println!("{:?}", e);
    }
}

fn parse_list(data: &str) -> Vec<String> {
    data.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn process_args(args: Cli) -> Result<(), Error> {
    let env_config = get_env_config(args.environment);
    let base_url = format!("http://{}", env_config.socket_address);
    let client = Client::new();

    match args.command {
        Commands::Add(add_args) => match add_args.command {
            AddCommands::Parser { name } => {
                let request = CreateParserRequest { name };
                let url = format!("{}/api/parsers", base_url);
                let response = client
                    .post(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: ParserResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
            AddCommands::Tag {
                name,
                description,
                parent_id,
                query,
                auto_delete,
            } => {
                let request = CreateTagRequest {
                    name,
                    description,
                    parent_id,
                    query,
                    auto_delete,
                };
                let url = format!("{}/api/tags", base_url);
                let response = client
                    .post(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: TagResponse = response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
            AddCommands::Note {
                data,
                parser_id,
                keywords,
                tags,
                is_suspended,
            } => {
                let create_note_request = CreateNoteRequest {
                    data,
                    keywords: parse_list(keywords.as_str()),
                    tags,
                    is_suspended,
                    custom_data: Map::new(),
                };
                let create_notes_request = CreateNotesRequest {
                    parser_id,
                    requests: vec![create_note_request],
                };
                let url = format!("{}/api/notes", base_url);
                let response = client
                    .post(url)
                    .json(&create_notes_request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: NotesResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
        },
        Commands::Edit(edit_args) => match edit_args.command {
            EditCommands::Parser { id, name } => {
                let request = UpdateParserRequest { name };
                let url = format!("{}/api/parsers/{}", base_url, id);
                let response = client
                    .patch(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: ParserResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
            EditCommands::Tag {
                id,
                parent_id,
                name,
                description,
                query,
                auto_delete,
            } => {
                let request = UpdateTagRequest {
                    parent_id,
                    name,
                    description,
                    query,
                    auto_delete,
                };
                let url = format!("{}/api/tags/{}", base_url, id);
                let response = client
                    .patch(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: TagResponse = response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
            EditCommands::Note {
                selector,
                data,
                parser_id,
                keywords,
                tags_to_remove,
                tags_to_add,
                remove_all_tags,
            } => {
                let selector = selector
                    .get_notes_selector()
                    .map_err(|e| miette!("{}", e))?;
                let tags = if remove_all_tags {
                    // If `tags_to_add` is empty, this will just set all tags to nothing which will remove all tags. Otherwise, it will remove all tags and then add `tags_to_add`
                    UpdateTags::SetTags(tags_to_add.unwrap_or_default())
                } else {
                    UpdateTags::ModifyTags {
                        tags_to_remove,
                        tags_to_add,
                    }
                };
                let request = UpdateNotesRequest {
                    selector,
                    data,
                    parser_id,
                    keywords: keywords.as_deref().map(parse_list),
                    tags,
                    custom_data: None,
                };
                let url = format!("{}/api/notes", base_url);
                let response = client
                    .patch(&url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let responses: Vec<NoteResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&responses).unwrap());
            }
            EditCommands::Card {
                selector: selector_local,
                desired_retention,
                special_state: special_state_local,
                due,
            } => {
                let selector = if let Some(ids) = selector_local.ids {
                    CardsSelector::Ids(ids)
                } else if let Some(query) = selector_local.query {
                    CardsSelector::Query(query)
                } else {
                    unreachable!("by clap conflicts_with")
                };
                let special_state = special_state_local.map(|x| match x {
                    SpecialStateLocal::Suspended => Some(SpecialStateUpdate::Suspended),
                    SpecialStateLocal::Buried => Some(SpecialStateUpdate::Buried),
                    SpecialStateLocal::None => None,
                });
                let request = UpdateCardsRequest {
                    selector,
                    desired_retention,
                    special_state,
                    due,
                };
                let url = format!("{}/api/cards", base_url);
                let response = client
                    .patch(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: Vec<CardResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
        },
        Commands::Delete(delete_args) => match delete_args.command {
            DeleteCommands::Parser { id } => {
                let url = format!("{}/api/parsers/{}", base_url, id);
                let response = client
                    .delete(url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let _ = ensure_ok(response).await?;
                println!("Done");
            }
            DeleteCommands::Tag { id } => {
                let url = format!("{}/api/tags/{}", base_url, id);
                let response = client
                    .delete(url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let _ = ensure_ok(response).await?;
                println!("Done");
            }
            DeleteCommands::Note { selector } => {
                let selector = selector
                    .get_notes_selector()
                    .map_err(|e| miette!("{}", e))?;
                let request = DeleteNotesRequest { selector };
                let url = format!("{}/api/notes", base_url);
                let response = client
                    .delete(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let _ = ensure_ok(response).await?;
                println!("Done");
            }
        },
        Commands::Get(get_args) => match get_args.command {
            GetCommands::Parser { id } => {
                let url = format!("{}/api/parsers/{}", base_url, id);
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let parser_response: ParserResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&parser_response).unwrap()
                );
            }
            GetCommands::Tag { id, name } => {
                let url = if let Some(id) = id {
                    format!("{}/api/tags/{}", base_url, id)
                } else if let Some(name) = name {
                    format!("{}/api/tags/name/{}", base_url, name)
                } else {
                    unreachable!("by clap conflicts_with");
                };
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let tag_response: TagResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&tag_response).unwrap());
            }
            GetCommands::Note { id } => {
                let url = format!("{}/api/notes/{}", base_url, id);
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let mut note_response: NoteResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                note_response.linked_notes = None;
                println!("{}", serde_json::to_string_pretty(&note_response).unwrap());
            }
            GetCommands::Card { id, note_id } => {
                let url = if let Some(id) = id {
                    format!("{}/api/cards/{}", base_url, id)
                } else if let Some(note_id) = note_id {
                    format!("{}/api/cards/note_id/{}", base_url, note_id)
                } else {
                    unreachable!()
                };
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                if id.is_some() {
                    let card_response: CardResponse =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    println!("{}", serde_json::to_string_pretty(&card_response).unwrap());
                } else if note_id.is_some() {
                    let card_responses: Vec<CardResponse> =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    println!("{}", serde_json::to_string_pretty(&card_responses).unwrap());
                } else {
                    unreachable!()
                }
            }
        },
        Commands::Export(export_args) => {
            let request = ExportNotesRequest {
                query: export_args.query,
            };
            let url = format!("{}/api/notes/export", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response_text = response.text().await.map_err(|e| miette!("{}", e))?;
            println!("{}", response_text);
        }
        Commands::List(list_args) => match list_args.command {
            ListCommands::Parser { page, limit } => {
                let parser_responses =
                    list_parsers(page, limit, base_url.as_str(), &client).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&parser_responses).unwrap()
                );
            }
            ListCommands::Tag {
                page,
                limit,
                output,
            } => {
                let url = format!("{}/api/tags", base_url);
                let mut queries: Vec<(&str, String)> = Vec::new();
                if let Some(page) = page {
                    queries.push(("page", page.to_string()));
                }
                if let Some(limit) = limit {
                    queries.push(("limit", limit.to_string()));
                }
                let req_url = client
                    .get(url)
                    .query(&queries)
                    .build()
                    .unwrap()
                    .url()
                    .to_string();
                let response = client
                    .get(&req_url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let tag_responses: Vec<TagResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                match output {
                    ListTagOutput::Full => {
                        println!("{}", serde_json::to_string_pretty(&tag_responses).unwrap());
                    }
                    ListTagOutput::Short => {
                        let tag_names = tag_responses
                            .into_iter()
                            .map(|x| x.name)
                            .collect::<Vec<_>>()
                            .join("\n");
                        println!("{}", &tag_names);
                    }
                    ListTagOutput::Tree => {
                        let tag_relations = tag_responses
                            .iter()
                            .map(|tag_response| {
                                let parent_name = if let Some(parent_id) = tag_response.parent_id {
                                    tag_responses
                                        .iter()
                                        .find(|r| r.id == parent_id)
                                        .unwrap()
                                        .name
                                        .clone()
                                } else {
                                    String::new()
                                };
                                (parent_name, tag_response.name.clone())
                            })
                            .collect::<Vec<_>>();
                        let tree = build_tree(&tag_relations);
                        for root in tree
                            .keys()
                            .filter(|&tag| tag_relations.iter().all(|(_, child)| child != tag))
                        {
                            print_tree(&tree, root, 0);
                        }
                    }
                }
            }
            ListCommands::Note { page, limit, graph } => {
                let url = format!("{}/api/notes", base_url);
                let mut queries: Vec<(&str, String)> = Vec::new();
                if let Some(page) = page {
                    queries.push(("page", page.to_string()));
                }
                if let Some(limit) = limit {
                    queries.push(("limit", limit.to_string()));
                }
                let req_url = client
                    .get(url)
                    .query(&queries)
                    .build()
                    .unwrap()
                    .url()
                    .to_string();
                let response = client
                    .get(&req_url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;

                let note_responses: Vec<NoteResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;

                // Graph
                if graph {
                    chart(note_responses);
                } else {
                    println!("{}", serde_json::to_string_pretty(&note_responses).unwrap());
                }
            }
            ListCommands::NoteLink { score_threshold } => {
                let url = format!("{}/api/notes/search/note-links", base_url);
                let request = NoteLinksRequest { score_threshold };
                let response = client
                    .post(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: Vec<NoteLink> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
        },
        Commands::Generate(GenerateArgs {
            query,
            overridden_output_raw_dir,
            include_linked_notes,
            include_cards,
            render,
            force_render,
        }) => {
            let prompt = "Note that this will overwrite any unsaved changes you have. Please sync your notes before proceeding. Are you sure you want to continue?";
            let ans = Confirm::new(prompt).with_default(false).prompt();
            if !ans.unwrap_or(false) {
                return Ok(());
            }
            let request = RenderNotesRequest {
                selector: query.map_or(NotesSelector::All, NotesSelector::Query),
                immutable_note_ids: None,
                overridden_output_raw_dir,
                include_linked_notes,
                include_cards,
                generate_rendered: render,
                force_generate_rendered: force_render,
            };
            let url = format!("{}/api/notes/generate_files", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
        Commands::Review(review_args) => {
            review_cards(review_args, &base_url, &client)
                .await
                .map_err(|e| miette!("{}", e))?;
        }
        Commands::Statistics(StatisticsArgs {
            scheduler_name,
            date,
        }) => {
            let request = StatisticsRequest {
                scheduler_name,
                date,
            };
            let url = format!("{}/api/review/statistics", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: StatisticsResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        Commands::UnmatchedKeywords => {
            let url = format!("{}/api/notes/unmatched-keywords", base_url);
            let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<UnmatchedKeywordResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        Commands::DuplicateKeywords => {
            let url = format!("{}/api/notes/duplicate-keywords", base_url);
            let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<(String, Vec<NoteId>)> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        Commands::RebuildTag { id } => {
            let url = format!("{}/api/tags/{}/rebuild", base_url, id);
            let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
        Commands::Search(SearchArgs {
            mode,
            query,
            output_type,
            output_format,
        }) => match mode {
            SearchMode::Query => {
                let return_item_type = match output_type {
                    OutputItemType::Cards => QueryReturnItemType::Cards,
                    OutputItemType::Notes => QueryReturnItemType::Notes,
                };
                let request = SearchNotesRequest {
                    query,
                    output_type: return_item_type,
                };
                let url = format!("{}/api/notes/search", base_url);
                let response = client
                    .post(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: SearchNotesResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                match response {
                    SearchNotesResponse::Notes(note_responses) => {
                        for (note_response, parser_name) in note_responses {
                            let parser = find_parser(parser_name.as_str(), &get_all_parsers())?;
                            match output_format {
                                OutputFormat::RawFilepath => {
                                    let mut note_raw_path = get_output_raw_dir(
                                        parser.get_parser_name(),
                                        RenderOutputType::Note,
                                        None,
                                    );
                                    note_raw_path.push(parser.get_output_filename(
                                        RenderOutputType::Note,
                                        note_response.id,
                                    ));
                                    note_raw_path.set_extension(parser.file_extension());
                                    println!("{}", note_raw_path.display());
                                }
                                OutputFormat::RenderedFilepath => {
                                    let mut note_rendered_path = parser
                                        .get_output_rendered_dir(RenderOutputDirectoryType::Note);
                                    note_rendered_path.push(parser.get_output_filename(
                                        RenderOutputType::Note,
                                        note_response.id,
                                    ));
                                    println!("{}", note_rendered_path.display());
                                }
                            }
                        }
                    }
                    SearchNotesResponse::Cards(card_responses) => {
                        for (card_response, parser_name) in card_responses {
                            let parser = find_parser(parser_name.as_str(), &get_all_parsers())?;
                            match output_format {
                                OutputFormat::RawFilepath => {
                                    let mut card_raw_path = get_output_raw_dir(
                                        parser.get_parser_name(),
                                        RenderOutputType::Card(
                                            card_response.order as usize,
                                            CardSide::Front,
                                        ),
                                        None,
                                    );
                                    card_raw_path.push(parser.get_output_filename(
                                        RenderOutputType::Card(
                                            card_response.order as usize,
                                            CardSide::Front,
                                        ),
                                        card_response.note_id,
                                    ));
                                    card_raw_path.set_extension(parser.file_extension());
                                    println!("{}", card_raw_path.display());
                                }
                                OutputFormat::RenderedFilepath => {
                                    let mut card_rendered_path = parser
                                        .get_output_rendered_dir(RenderOutputDirectoryType::Card);
                                    card_rendered_path.push(parser.get_output_filename(
                                        RenderOutputType::Card(
                                            card_response.order as usize,
                                            CardSide::Front,
                                        ),
                                        card_response.note_id,
                                    ));
                                    println!("{}", card_rendered_path.display());
                                }
                            }
                        }
                    }
                }
            }
            SearchMode::Keyword | SearchMode::KeywordRanking => {
                let request = SearchKeywordRequest { keyword: query };
                let url = format!("{}/api/notes/search/keyword", base_url);
                let response = client
                    .post(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: Vec<MatchedKeywordResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                if mode == SearchMode::Keyword {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&response.first()).unwrap()
                    );
                } else if mode == SearchMode::KeywordRanking {
                    println!("{}", serde_json::to_string_pretty(&response).unwrap());
                }
            }
        },
        Commands::Sync(sync_args) => {
            sync_notes(&base_url, &client, sync_args)
                .await
                .map_err(|e| miette!("{}", e))?;
        }
        Commands::Migrate(MigrateArgs {
            adapter: adapter_string,
            initial_migration,
            dry_run,
            tag_relations_file_path,
        }) => {
            let mut adapter =
                get_adapter_from_string(adapter_string.as_str()).map_err(|e| miette!("{:?}", e))?;
            let connect_options = SqliteConnectOptions::from_str(env_config.database_url.as_str())
                .map_err(|e| miette!("{:?}", e))?
                .with_regexp();
            let pool = SqlitePoolOptions::new()
                .max_lifetime(None)
                .idle_timeout(None)
                .connect_with(connect_options)
                .await
                .map_err(|e| miette!("Failed to connect to the database: {:?}", e))?;
            migrate_from_adapter(
                &base_url,
                &pool,
                &client,
                adapter.as_mut(),
                initial_migration,
                dry_run,
                tag_relations_file_path.as_deref(),
            )
            .await
            .map_err(|e| miette!("{}", e))?;
        }
        Commands::Import(ImportArgs {
            adapter: adapter_string,
            parser: parser_string_opt,
            to_parser: to_parser_string_opt,
            files,
            dry_run,
        }) => {
            let parser = parser_string_opt
                .map(|parser_string| find_parser(parser_string.as_str(), &get_all_parsers()))
                .transpose()
                .map_err(|e| miette!("{:?}", e))?;
            let mut adapter =
                get_adapter_from_string(adapter_string.as_str()).map_err(|e| miette!("{:?}", e))?;
            let to_parser_opt = to_parser_string_opt
                .map(|to_parser_string| find_parser(to_parser_string.as_str(), &get_all_parsers()))
                .transpose()
                .map_err(|e| miette!("{:?}", e))?;

            import_from_files(
                adapter.as_mut(),
                parser.as_deref(),
                to_parser_opt.as_deref(),
                files.as_slice(),
                dry_run,
                false,
            )
            .await
            .into_diagnostic()
            .map_err(|e| miette!("{:?}", e))?;
        }
        Commands::GenerateShellCompletion { shell } => {
            shell.generate(&mut Cli::command(), &mut io::stdout());
            // generate(shell, &mut Cli::command(), "spares_cli", &mut io::stdout());
        }
        Commands::ForgetCard(ForgetCardArgs { ids, query }) => {
            let mut card_ids = Vec::new();
            if let Some(ids_vec) = ids {
                card_ids = ids_vec;
            } else if let Some(q) = query {
                // Fetch card ids using existing search endpoint, adapt for cards
                let url = format!("{}/api/notes/search", base_url);
                let req = spares::schema::note::SearchNotesRequest {
                    query: q,
                    output_type: spares::search::QueryReturnItemType::Cards,
                };
                let response = client
                    .post(&url)
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let search_response: spares::schema::note::SearchNotesResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                if let spares::schema::note::SearchNotesResponse::Cards(cards) = search_response {
                    for (card, _) in cards {
                        card_ids.push(card.id);
                    }
                }
            }
            for card_id in card_ids {
                let card_response = forget_card(card_id, &base_url, &client)
                    .await
                    .map_err(|e| miette!("{}", e))?;
                println!("Forgot card: {:#?}", &card_response);
            }
        }
        Commands::Leeches { scheduler_name } => {
            let url = format!("{}/api/cards/leeches", base_url);
            let req = GetLeechesRequest { scheduler_name };
            let response = client
                .post(&url)
                .json(&req)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let card_responses: Vec<CardResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&card_responses).unwrap());
        }
        Commands::Unbury => {
            let url = format!("{}/api/cards/unbury", base_url);
            let response = client
                .post(&url)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
        Commands::Advance(AdvanceArgs {
            count,
            scheduler_name,
            query,
        }) => {
            let request = SubmitStudyActionRequest {
                scheduler_name,
                action: StudyAction::Advance { count, query },
            };
            let url = format!("{}/api/review/submit", base_url);
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Advanced {} cards.", count);
        }
        Commands::Postpone(PostponeArgs {
            count,
            scheduler_name,
            query,
        }) => {
            let request = SubmitStudyActionRequest {
                scheduler_name,
                action: StudyAction::Postpone { count, query },
            };
            let url = format!("{}/api/review/submit", base_url);
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Postponed {} cards.", count);
        }
    }
    Ok(())
}
