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
use miette::{Error, IntoDiagnostic, miette};
use migrate::{MigrateArgs, migrate_from_adapter};
use reqwest::{Client, StatusCode};
use review::{ReviewArgs, review_cards};
use serde_json::{Map, Value};
use spares::{
    adapters::get_adapter_from_string,
    api::tag::DEFAULT_TAG_AUTO_DELETE,
    config::{Environment, get_env_config},
    model::{CardId, NoteId},
    parsers::{
        RenderOutputDirectoryType, find_parser,
        generate_files::{CardSide, RenderOutputType},
        get_all_parsers, get_note_info_from_filepath, get_output_raw_dir,
    },
    schema::{
        card::{CardResponse, CardsSelector, SpecialStateUpdate, UpdateCardRequest},
        note::{
            CreateNoteRequest, CreateNotesRequest, GenerateFilesNoteIds, NoteResponse,
            NotesResponse, NotesSelector, RenderNotesRequest, SearchKeywordRequest,
            SearchNotesRequest, SearchNotesResponse, UpdateNotesRequest,
        },
        parser::{CreateParserRequest, ParserResponse, UpdateParserRequest},
        review::{StatisticsRequest, StatisticsResponse},
        tag::{CreateTagRequest, TagResponse, UpdateTagRequest},
    },
    search::QueryReturnItemType,
};
use sqlx::sqlite::SqlitePoolOptions;
use std::{io, path::PathBuf};
use sync::{SyncArgs, sync_notes};
use tree::{build_tree, print_tree};

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
        // This is really an action, not a setting that can be updated.
        #[arg(short, long, default_value_t = false)]
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
    },
    Card {
        #[command(flatten)]
        selector: CardsSelectorLocal,
        #[arg(short, long)]
        desired_retention: Option<f64>,
        #[arg(short, long)]
        special_state: Option<Option<SpecialStateLocal>>,
    },
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, ValueEnum)]
enum SpecialStateLocal {
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
    Parser { id: i64 },
    Tag { id: i64 },
    Note { id: i64 },
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
        #[arg(short, long, conflicts_with_all = ["id", "leeches"])]
        note_id: Option<i64>,
        #[arg(short, long, conflicts_with_all = ["id", "note_id"])]
        leeches: bool,
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
}

#[derive(Args, Debug)]
struct SearchArgs {
    #[arg(short, long, default_value = "query")]
    search_mode: SearchMode,
    // This option does not work if `matches!(search_mode, SearchMode::Keyword)`. There is no easy way to get around this since clap does not support default subcommands.
    #[arg(short, long, default_value = "notes")]
    output_type: OutputItemType,
    // This option does not work if `matches!(search_mode, SearchMode::Keyword)`. There is no easy way to get around this since clap does not support default subcommands.
    #[arg(long, default_value = "raw-filepath")]
    output_format: OutputFormat,
    // Positional argument
    query: String,
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
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| miette!("{}", e))?;
        let message = response_json.get("message");
        return Err(miette!(message.unwrap().to_string()));
    }
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let response: ParserResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &response);
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let response: TagResponse = response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &response);
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let response: NotesResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &response);
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let response: ParserResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &response);
            }
            EditCommands::Tag {
                id,
                parent_id,
                name,
                description,
                query,
                auto_delete,
                rebuild,
            } => {
                let rebuild_only = parent_id.is_none()
                    && name.is_none()
                    && description.is_none()
                    && query.is_none()
                    && auto_delete.is_none();
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let response: TagResponse = response.json().await.map_err(|e| miette!("{}", e))?;
                if !rebuild_only {
                    println!("{:#?}", &response);
                }
                if rebuild {
                    let url = format!("{}/api/tags/{}/rebuild", base_url, id);
                    let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
                    let status = response.status();
                    if status != StatusCode::OK {
                        let response_json: Value =
                            response.json().await.map_err(|e| miette!("{}", e))?;
                        let message = response_json.get("message");
                        return Err(miette!(message.unwrap().to_string()));
                    }
                    println!("Done");
                }
            }
            EditCommands::Note {
                selector,
                data,
                parser_id,
                keywords,
                tags_to_remove,
                tags_to_add,
            } => {
                let selector = if let Some(ids) = selector.ids {
                    NotesSelector::Ids(ids)
                } else if let Some(files) = selector.files {
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
                            NotesSelector::Ids(file_note_ids)
                        }
                        Err(e) => {
                            println!("Failed to parse files: {}", e);
                            return Ok(());
                        }
                    }
                } else if let Some(query) = selector.query {
                    NotesSelector::Query(query)
                } else {
                    unreachable!("by clap conflicts with")
                };
                let request = UpdateNotesRequest {
                    selector,
                    data,
                    parser_id,
                    keywords: keywords.as_deref().map(parse_list),
                    tags_to_remove,
                    tags_to_add,
                    custom_data: None,
                };
                let url = format!("{}/api/notes", base_url);
                let response = client
                    .patch(&url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let responses: Vec<NoteResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &responses);
            }
            EditCommands::Card {
                selector: selector_local,
                desired_retention,
                special_state: special_state_local,
            } => {
                let selector = if let Some(ids) = selector_local.ids {
                    CardsSelector::Ids(ids)
                } else if let Some(query) = selector_local.query {
                    CardsSelector::Query(query)
                } else {
                    unreachable!("by clap conflicts_with")
                };
                let special_state = special_state_local.map(|x| {
                    x.map(|y| match y {
                        SpecialStateLocal::Suspended => SpecialStateUpdate::Suspended,
                        SpecialStateLocal::Buried => SpecialStateUpdate::Buried,
                    })
                });
                let request = UpdateCardRequest {
                    selector,
                    desired_retention,
                    special_state,
                };
                let url = format!("{}/api/cards", base_url);
                let response = client
                    .patch(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let response: Vec<CardResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &response);
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                println!("Done");
            }
            DeleteCommands::Tag { id } => {
                let url = format!("{}/api/tags/{}", base_url, id);
                let response = client
                    .delete(url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                println!("Done");
            }
            DeleteCommands::Note { id } => {
                let url = format!("{}/api/notes/{}", base_url, id);
                let response = client
                    .delete(url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let parser_response: ParserResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &parser_response);
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let tag_response: TagResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &tag_response);
            }
            GetCommands::Note { id } => {
                let url = format!("{}/api/notes/{}", base_url, id);
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let note_response: NoteResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &note_response);
            }
            GetCommands::Card {
                id,
                note_id,
                leeches,
            } => {
                let url = if let Some(id) = id {
                    format!("{}/api/cards/{}", base_url, id)
                } else if let Some(note_id) = note_id {
                    format!("{}/api/cards/note_id/{}", base_url, note_id)
                } else if leeches {
                    format!("{}/api/cards/leeches", base_url)
                } else {
                    unreachable!()
                };
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                if id.is_some() {
                    let card_response: CardResponse =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    println!("{:#?}", &card_response);
                } else if note_id.is_some() || leeches {
                    let card_responses: Vec<CardResponse> =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    println!("{:#?}", &card_responses);
                } else {
                    unreachable!()
                }
            }
        },
        Commands::List(list_args) => match list_args.command {
            ListCommands::Parser { page, limit } => {
                let parser_responses =
                    list_parsers(page, limit, base_url.as_str(), &client).await?;
                println!("{:#?}", &parser_responses);
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let tag_responses: Vec<TagResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                match output {
                    ListTagOutput::Full => {
                        println!("{:#?}", &tag_responses);
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }

                let note_responses: Vec<NoteResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;

                // Graph
                if graph {
                    chart(note_responses);
                } else {
                    println!("{:#?}", &note_responses);
                }
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
            let request = RenderNotesRequest {
                generate_files_note_ids: query.map(GenerateFilesNoteIds::Query),
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
            let status = response.status();
            if status != StatusCode::OK {
                let response_json: Value = response.json().await.map_err(|e| miette!("{}", e))?;
                let message = response_json.get("message");
                return Err(miette!(message.unwrap().to_string()));
            }
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
            let status = response.status();
            if status != StatusCode::OK {
                let response_json: Value = response.json().await.map_err(|e| miette!("{}", e))?;
                let message = response_json.get("message");
                return Err(miette!(message.unwrap().to_string()));
            }
            let response: StatisticsResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{:#?}", &response);
        }
        Commands::Search(SearchArgs {
            search_mode,
            query,
            output_type,
            output_format,
        }) => match search_mode {
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
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
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
            SearchMode::Keyword => {
                let request = SearchKeywordRequest { keyword: query };
                let url = format!("{}/api/notes/search/keyword", base_url);
                let response = client
                    .post(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let status = response.status();
                if status != StatusCode::OK {
                    let response_json: Value =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    let message = response_json.get("message");
                    return Err(miette!(message.unwrap().to_string()));
                }
                let response: Option<(NoteId, String)> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{:#?}", &response);
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
            run,
            tag_relations_file_path,
        }) => {
            let mut adapter =
                get_adapter_from_string(adapter_string.as_str()).map_err(|e| miette!("{:?}", e))?;
            let pool = SqlitePoolOptions::new()
                // .max_connections(10)
                .max_lifetime(None)
                .idle_timeout(None)
                .connect(&env_config.database_url)
                .await
                .map_err(|e| miette!("Failed to connect to the database: {:?}", e))?;
            migrate_from_adapter(
                &base_url,
                &pool,
                &client,
                adapter.as_mut(),
                initial_migration,
                run,
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
            run,
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
                files,
                run,
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
    }
    Ok(())
}
