mod handlers;
mod route;

use crate::route::create_router;
use axum::http::{Method, header::CONTENT_TYPE};
use clap::Parser;
use log::{info, warn};
use spares_core::config::{Environment, get_data_dir, get_env_config};
use sqlx::{
    Sqlite,
    migrate::{MigrateDatabase, Migrator},
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
};
use std::{path::PathBuf, str::FromStr, sync::Arc};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

#[derive(Debug)]
struct AppState {
    pub(crate) db: SqlitePool,
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
        info!("Database already exists. Skipping creation.");
        database_already_exists = true;
    } else {
        info!("Creating database: {}", env_config.database_url);
        Sqlite::create_database(env_config.database_url.as_str())
            .await
            .map_err(|e| e.to_string())?;
    }

    let connect_options = SqliteConnectOptions::from_str(env_config.database_url.as_str())
        .map_err(|e| format!("{:?}", e))?
        .with_regexp();
    let pool = SqlitePoolOptions::new()
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_with(connect_options)
        .await
        .map_err(|e| format!("Failed to connect to the database: {:?}", e))?;
    info!("Connected to database successfully.");

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
        info!("Migration successful.");
    }

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(Any)
        .allow_headers([CONTENT_TYPE]);
    let app = create_router(Arc::new(AppState { db: pool.clone() })).layer(cors);
    let listener = match TcpListener::bind(&env_config.socket_address).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            warn!(
                "Server is already running at {:?}. Exiting.",
                env_config.socket_address
            );
            return Ok(());
        }
        Err(e) => {
            return Err(format!(
                "Failed to bind to {}: {}",
                env_config.socket_address, e
            ));
        }
    };
    info!("Starting server at {:?}", env_config.socket_address);
    axum::serve(listener, app.into_make_service())
        .await
        .map_err(|e| format!("Server error: {}", e))?;
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
