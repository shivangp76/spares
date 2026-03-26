mod args;
mod graph;
mod import;
mod migrate;
mod review;
mod sync;
mod tree;
mod utils;

use args::{Cli, Commands, AddCommands, EditCommands, SpecialStateLocal, DeleteCommands, GetCommands, ListCommands, GenerateArgs, StatisticsArgs, KeywordArgs, KeywordCommands, SearchArgs, OutputItemType, OutputFormat, ScheduleArgs, ScheduleCommands, ForgetCardArgs, AdvanceArgs, PostponeArgs, UndoArgs};
use clap::{CommandFactory, Parser};
use crate::tree::{build_tree, tree_to_string};
use graph::chart;
use import::import_from_files;
use inquire::Confirm;
use miette::{Error, IntoDiagnostic, miette};
use migrate::migrate_from_adapter;
use reqwest::Client;
use review::{forget_card, review_cards};
use serde_json::Map;
use spares_core::{
    adapters::get_adapter_from_string,
    config::get_env_config,
    model::NoteLink,
    parsers::{
        find_parser,
        generate_files::CardSide,
        get_all_parsers, get_output_raw_dir,
    },
    schema::{
        card::{
            CardResponse, CardsSelector, GetLeechesRequest, UnburyRequest,
            UpdateCardsRequest, UpdateCardsResponse,
        },
        note::{
            CreateNoteRequest, CreateNotesRequest, DeleteNotesRequest, ExportNotesRequest,
            MatchedKeywordResponse, NoteLinksRequest, NoteResponse, NotesResponse, NotesSelector,
            RenderNotesRequest, SearchKeywordRequest, SearchNotesRequest, SearchNotesResponse,
            UnmatchedKeywordResponse, UpdateNotesRequest, UpdateNotesResponse, UpdateTags,
        },
        parser::{CreateParserRequest, ParserResponse, UpdateParserRequest},
        review::{StatisticsRequest, StatisticsResponse, StudyAction, SubmitStudyActionRequest},
        tag::{CreateTagRequest, TagResponse, TagSelector, UpdateTagRequest},
        undo::UndoEventRequest,
    },
    search::QueryReturnItemType,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{io, str::FromStr};
use sync::sync_notes;
use utils::{ensure_ok, undo_event};

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

#[expect(clippy::too_many_lines)]
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
                query,
                auto_delete,
            } => {
                let request = CreateTagRequest {
                    name,
                    description,
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
                id: tag_id_opt,
                tag_name: tag_name_opt,
                name,
                description,
                query,
                auto_delete,
                rebuild,
            } => {
                if rebuild {
                    let tag_id = tag_id_opt.expect("--id is required when using --rebuild");
                    let url = format!("{}/api/tags/{}/rebuild", base_url, tag_id);
                    let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
                    let _ = ensure_ok(response).await?;
                    println!("Done");
                } else {
                    let tag_to_modify = if let Some(tag_id) = tag_id_opt {
                        TagSelector::Id(tag_id)
                    } else if let Some(tag_name) = tag_name_opt {
                        TagSelector::Name(tag_name)
                    } else {
                        unreachable!("required by clap");
                    };
                    let request = UpdateTagRequest {
                        tag_to_modify,
                        name,
                        description,
                        query,
                        auto_delete,
                    };
                    let url = format!("{}/api/tags", base_url);
                    let response = client
                        .patch(url)
                        .json(&request)
                        .send()
                        .await
                        .map_err(|e| miette!("{}", e))?;
                    let response = ensure_ok(response).await?;
                    let response: TagResponse =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    println!("{}", serde_json::to_string_pretty(&response).unwrap());
                }
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
                let update_response: UpdateNotesResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&update_response.notes).unwrap()
                );
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
                    SpecialStateLocal::Suspended => Some(spares_core::schema::card::SpecialStateUpdate::Suspended),
                    SpecialStateLocal::Buried => Some(spares_core::schema::card::SpecialStateUpdate::Buried),
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
                let update_response: UpdateCardsResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&update_response.cards).unwrap()
                );
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
            let result: std::collections::HashMap<String, String> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            std::fs::create_dir_all(&export_args.output_dir).into_diagnostic()?;
            for (extension, content) in &result {
                let filename = format!("export.{}", extension);
                let path = export_args.output_dir.join(&filename);
                std::fs::write(&path, content).into_diagnostic()?;
                println!("Wrote {}", filename);
            }
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
                long,
                short,
                tree,
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
                if short {
                    let tag_names = tag_responses
                        .into_iter()
                        .map(|x| x.name)
                        .collect::<Vec<_>>()
                        .join("\n");
                    println!("{}", &tag_names);
                } else if tree {
                    let tag_names = tag_responses
                        .into_iter()
                        .map(|r| r.name)
                        .collect::<Vec<_>>();
                    let tree = build_tree(tag_names);
                    let output = tree_to_string(&tree, 0);
                    println!("{}", &output);
                } else if long {
                    println!("{}", serde_json::to_string_pretty(&tag_responses).unwrap());
                } else {
                    unreachable!("by clap");
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
        Commands::Keyword(KeywordArgs { command }) => match command {
            KeywordCommands::Unmatched => {
                let url = format!("{}/api/notes/unmatched-keywords", base_url);
                let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: Vec<UnmatchedKeywordResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
            KeywordCommands::Duplicate => {
                let url = format!("{}/api/notes/duplicate-keywords", base_url);
                let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: Vec<(String, Vec<spares_core::model::NoteId>)> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
            KeywordCommands::Search { keyword } => {
                let request = SearchKeywordRequest { keyword };
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
                println!("{}", serde_json::to_string_pretty(&response.first()).unwrap());
            }
            KeywordCommands::Ranking { keyword } => {
                let request = SearchKeywordRequest { keyword };
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
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
        },
        Commands::Search(SearchArgs {
            query,
            output_type,
            output_format,
        }) => {
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
                                    spares_core::parsers::generate_files::RenderOutputType::Note,
                                    None,
                                );
                                note_raw_path.push(parser.get_output_filename(
                                    spares_core::parsers::generate_files::RenderOutputType::Note,
                                    note_response.id,
                                ));
                                note_raw_path.set_extension(parser.file_extension());
                                println!("{}", note_raw_path.display());
                            }
                            OutputFormat::RenderedFilepath => {
                                let mut note_rendered_path = parser
                                    .get_output_rendered_dir(spares_core::parsers::RenderOutputDirectoryType::Note);
                                note_rendered_path.push(parser.get_output_filename(
                                    spares_core::parsers::generate_files::RenderOutputType::Note,
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
                                    spares_core::parsers::generate_files::RenderOutputType::Card(
                                        card_response.order as usize,
                                        CardSide::Front,
                                    ),
                                    None,
                                );
                                card_raw_path.push(parser.get_output_filename(
                                    spares_core::parsers::generate_files::RenderOutputType::Card(
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
                                    .get_output_rendered_dir(spares_core::parsers::RenderOutputDirectoryType::Card);
                                card_rendered_path.push(parser.get_output_filename(
                                    spares_core::parsers::generate_files::RenderOutputType::Card(
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
        Commands::Sync(sync_args) => {
            sync_notes(&base_url, &client, sync_args)
                .await
                .map_err(|e| miette!("{}", e))?;
        }
        Commands::Migrate(migrate::MigrateArgs {
            adapter: adapter_string,
            initial_migration,
            dry_run,
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
            )
            .await
            .map_err(|e| miette!("{}", e))?;
        }
        Commands::Import(import::ImportArgs {
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
            // generate(shell, &mut Cli::command(), "spares", &mut io::stdout());
        }
        Commands::Schedule(ScheduleArgs { command }) => match command {
            ScheduleCommands::Forget(ForgetCardArgs { ids, query }) => {
                let mut card_ids = Vec::new();
                if let Some(ids_vec) = ids {
                    card_ids = ids_vec;
                } else if let Some(q) = query {
                    let url = format!("{}/api/notes/search", base_url);
                    let req = spares_core::schema::note::SearchNotesRequest {
                        query: q,
                        output_type: spares_core::search::QueryReturnItemType::Cards,
                    };
                    let response = client
                        .post(&url)
                        .json(&req)
                        .send()
                        .await
                        .map_err(|e| miette!("{}", e))?;
                    let response = ensure_ok(response).await?;
                    let search_response: spares_core::schema::note::SearchNotesResponse =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    if let spares_core::schema::note::SearchNotesResponse::Cards(cards) =
                        search_response
                    {
                        for (card, _) in cards {
                            card_ids.push(card.id);
                        }
                    }
                }
                for card_id in card_ids {
                    let forget_response = forget_card(card_id, &base_url, &client)
                        .await
                        .map_err(|e| miette!("{}", e))?;
                    println!("Forgot card: {:#?}", &forget_response.card);
                }
            }
            ScheduleCommands::Leeches { scheduler_name } => {
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
            ScheduleCommands::Unbury { query } => {
                let url = format!("{}/api/cards/unbury", base_url);
                let req = UnburyRequest { query };
                let response = client
                    .post(&url)
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let _ = ensure_ok(response).await?;
                println!("Done");
            }
            ScheduleCommands::Advance(AdvanceArgs {
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
            ScheduleCommands::Postpone(PostponeArgs {
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
        Commands::Undo(UndoArgs {
            event_id,
            undo_group,
        }) => {
            let request = UndoEventRequest {
                event_id,
                undo_group,
            };
            let undo_response_opt = undo_event(&base_url, &client, request)
                .await
                .map_err(|e| miette!("{}", e))?;
            match undo_response_opt {
                Some(undo_response) => {
                    println!("Undone event(s): {:?}", undo_response.undone_event_ids);
                }
                None => {
                    println!("No event to undo");
                }
            }
        }
    }
    Ok(())
}
