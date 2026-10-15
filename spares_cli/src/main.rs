mod args;
mod card;
mod card_info;
mod deploy;
mod event;
mod graph;
mod history;
mod import;
mod keyword;
mod link;
mod migrate;
mod migrate_ids;
mod note;
mod notes_vcs;
mod parser;
mod review;
mod search;
mod statistics;
mod sync;
mod tag;
mod tree;
mod utils;
mod view;

use std::str::FromStr;

use args::Cli;
use args::Commands;
use clap::CommandFactory;
use clap::Parser;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;
use reqwest::Client;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use spares_core::adapters::get_adapter_from_string;
use spares_core::config::get_env_config;
use spares_core::config::local_api_key;
use spares_core::parsers::find_parser;
use spares_core::parsers::get_all_parsers;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use sync::sync_notes;

#[tokio::main]
async fn main() {
    // Answers shell completion requests (COMPLETE=zsh) and exits.
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
    env_logger::init();

    let args = Cli::parse();
    let res = process_args(args).await;
    if let Err(e) = res {
        eprintln!("{:?}", e);
        std::process::exit(1);
    }
}

/// The client for the local server, sending its API key (see [`local_api_key`]).
fn local_client() -> Result<Client, Error> {
    client_with_key(local_api_key().as_deref())
}

/// A client sending this API key, if any.
fn client_with_key(key: Option<&str>) -> Result<Client, Error> {
    let mut headers = HeaderMap::new();
    if let Some(key) = key {
        let mut value = HeaderValue::from_str(&format!("Bearer {key}")).into_diagnostic()?;
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }
    Client::builder()
        .default_headers(headers)
        .build()
        .into_diagnostic()
}

async fn process_args(args: Cli) -> Result<(), Error> {
    let env_config = get_env_config(args.environment);
    let base_url = format!("http://{}", env_config.socket_address);
    let client = local_client()?;

    match args.command {
        Commands::Parser(parser_args) => parser::handle(parser_args, &base_url, &client).await,
        Commands::Tag(tag_args) => tag::handle(tag_args, &base_url, &client).await,
        Commands::Note(note_args) => note::handle(note_args, &base_url, &client).await,
        Commands::Card(card_args) => card::handle(card_args, &base_url, &client).await,
        Commands::Link(link_args) => link::handle(link_args, &base_url, &client).await,
        Commands::Keyword(keyword_args) => keyword::handle(keyword_args, &base_url, &client).await,
        Commands::Event(event_args) => event::handle(event_args, &base_url, &client).await,
        Commands::Deploy(deploy_args) => deploy::handle(deploy_args, &env_config).await,
        Commands::Sync(sync_args) => {
            sync_notes(&base_url, &client, sync_args)
                .await
                .map_err(|e| miette!("{}", e))?;
            Ok(())
        }
        Commands::Migrate(migrate::MigrateArgs {
            command: Some(migrate::MigrateCommands::Ids { dry_run }),
            ..
        }) => {
            migrate_ids::migrate_ids(
                &env_config.database_url,
                &env_config.socket_address,
                dry_run,
            )
            .await
        }
        Commands::Migrate(migrate::MigrateArgs {
            command: None,
            adapter: adapter_string,
            initial_migration,
            dry_run,
        }) => {
            let adapter_string = adapter_string.ok_or_else(|| miette!("--adapter is required"))?;
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
            migrate::migrate_from_adapter(
                &base_url,
                &pool,
                &client,
                adapter.as_mut(),
                initial_migration,
                dry_run,
            )
            .await
            .map_err(|e| miette!("{}", e))?;
            Ok(())
        }
        Commands::Import(import::ImportArgs {
            adapter: adapter_string,
            parser: parser_string_opt,
            to_parser: to_parser_string_opt,
            files,
            dry_run,
            strip_liveness,
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

            import::import_from_files(
                adapter.as_mut(),
                parser.as_deref(),
                to_parser_opt.as_deref(),
                files.as_slice(),
                dry_run,
                false,
                strip_liveness,
            )
            .await
            .into_diagnostic()
            .map_err(|e| miette!("{:?}", e))?;
            Ok(())
        }
    }
}
