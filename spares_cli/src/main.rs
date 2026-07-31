mod args;
mod card;
mod event;
mod graph;
mod import;
mod keyword;
mod link;
mod migrate;
mod note;
mod parser;
mod review;
mod search;
mod sync;
mod tag;
mod tree;
mod utils;
mod view;

use std::io;
use std::str::FromStr;

use args::Cli;
use args::Commands;
use clap::CommandFactory;
use clap::Parser;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;
use reqwest::Client;
use spares_core::adapters::get_adapter_from_string;
use spares_core::config::get_env_config;
use spares_core::parsers::find_parser;
use spares_core::parsers::get_all_parsers;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use sync::sync_notes;

#[tokio::main]
async fn main() {
    env_logger::init();

    let args = Cli::parse();
    let res = process_args(args).await;
    if let Err(e) = res {
        eprintln!("{:?}", e);
        std::process::exit(1);
    }
}

async fn process_args(args: Cli) -> Result<(), Error> {
    let env_config = get_env_config(args.environment);
    let base_url = format!("http://{}", env_config.socket_address);
    let client = Client::new();

    match args.command {
        Commands::Parser(parser_args) => parser::handle(parser_args, &base_url, &client).await,
        Commands::Tag(tag_args) => tag::handle(tag_args, &base_url, &client).await,
        Commands::Note(note_args) => note::handle(note_args, &base_url, &client).await,
        Commands::Card(card_args) => card::handle(card_args, &base_url, &client).await,
        Commands::Link(link_args) => link::handle(link_args, &base_url, &client).await,
        Commands::Keyword(keyword_args) => keyword::handle(keyword_args, &base_url, &client).await,
        Commands::Event(event_args) => event::handle(event_args, &base_url, &client).await,
        Commands::Sync(sync_args) => {
            sync_notes(&base_url, &client, sync_args)
                .await
                .map_err(|e| miette!("{}", e))?;
            Ok(())
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
        Commands::Completion { shell } => {
            shell.generate(&mut Cli::command(), &mut io::stdout());
            Ok(())
        }
    }
}
