mod handlers;
mod route;

use crate::route::create_router;
use axum::http::{Method, header::CONTENT_TYPE};
use clap::Parser;
use log::{info, warn};
use spares_core::config::{Environment, get_data_dir, get_env_config};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::{str::FromStr, sync::Arc};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

#[derive(Debug)]
struct AppState {
    pub(crate) db: SqlitePool,
}

async fn start_server(args: Args) -> Result<(), String> {
    // Ensure default data directory exists (no-op when DATABASE_URL is set explicitly)
    let _ = get_data_dir();

    let env_config = get_env_config(args.environment);

    let connect_options = SqliteConnectOptions::from_str(env_config.database_url.as_str())
        .map_err(|e| format!("{:?}", e))?
        .create_if_missing(true)
        .with_regexp();
    let pool = SqlitePoolOptions::new()
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_with(connect_options)
        .await
        .map_err(|e| format!("Failed to connect to the database: {:?}", e))?;
    info!("Connected to database: {}", env_config.database_url);

    sqlx::migrate!("../spares_core/migrations")
        .run(&pool)
        .await
        .map_err(|e| format!("Failed to migrate the database: {:?}", e))?;
    info!("Migration successful.");

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
