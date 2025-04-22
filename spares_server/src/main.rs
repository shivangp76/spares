mod handlers;
mod route;

use crate::route::create_router;
use axum::http::{Method, header::CONTENT_TYPE};
use clap::Parser;
use spares::config::{Environment, get_data_dir, get_env_config};
use sqlx::{
    Sqlite,
    migrate::{MigrateDatabase, Migrator},
    sqlite::{SqlitePool, SqlitePoolOptions},
};
use std::{path::PathBuf, sync::Arc};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

#[derive(Debug)]
struct AppState {
    pub db: SqlitePool,
}

async fn start_server(args: Args) -> Result<(), String> {
    // Create directory for database file, if it doesn't exit
    let _ = get_data_dir();

    // Create database
    let env_config = get_env_config(args.environment);
    let mut database_already_exists = false;
    if Sqlite::database_exists(env_config.database_url.as_str())
        .await
        .unwrap_or(false)
    {
        println!("Database already exists. Skipping creation.");
        database_already_exists = true;
    } else {
        println!("Creating database: {}", env_config.database_url);
        Sqlite::create_database(env_config.database_url.as_str())
            .await
            .map_err(|e| e.to_string())?;
    }

    let pool = SqlitePoolOptions::new()
        // .max_connections(10)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect(&env_config.database_url)
        .await
        .map_err(|e| format!("Failed to connect to the database: {:?}", e))?;
    println!("Connection to the database is successful.");

    // Migrations
    // run_migrations(&pool).await?;
    if !database_already_exists {
        let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let mut migrations_path = PathBuf::from(&crate_dir);
        migrations_path.push("..");
        migrations_path.push("spares");
        migrations_path.push("migrations");
        Migrator::new(migrations_path)
            .await
            .unwrap()
            .run(&pool)
            .await
            .map_err(|e| format!("Failed to migrate the database: {:?}", e))?;
        println!("Migration successful.");
    }

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(Any)
        .allow_headers([CONTENT_TYPE]);
    let app = create_router(Arc::new(AppState { db: pool.clone() })).layer(cors);
    let listener = TcpListener::bind(&env_config.socket_address).await.unwrap();
    println!("Starting server at {:?}", env_config.socket_address);
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
    Ok(())
}

/// Spares Web Server
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = Environment::Production)]
    environment: Environment,
}

#[tokio::main(worker_threads = 5)]
async fn main() {
    env_logger::init();

    let args = Args::parse();
    let res = start_server(args).await;
    if let Err(e) = res {
        println!("{}", e);
    }
}
