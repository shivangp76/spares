use std::path::PathBuf;
use std::sync::Arc;

use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::header::AUTHORIZATION;
use axum::http::header::CONTENT_TYPE;
use clap::CommandFactory;
use clap::Parser;
use log::info;
use log::warn;
use spares_core::parsers::RenderOutputDirectoryType;
use spares_core::parsers::get_all_parsers;
use spares_server::AppState;
use spares_server::host::Host;
use spares_server::host::accounts;
use spares_server::host::lock::HostLock;
use spares_server::route::create_router;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

async fn start_server(args: Args) -> Result<(), String> {
    let root = args.root.unwrap_or_else(spares_server::host::default_root);
    let _lock = HostLock::take(&root).map_err(|e| e.to_string())?;
    let host = Host::open(&root).await.map_err(|e| e.to_string())?;
    info!("Serving the host in {}", root.display());
    spares_core::render::set_config(host.config.render.clone());
    let users = accounts::users(&host.accounts)
        .await
        .map_err(|e| e.to_string())?;
    info!("{} accounts", users.len());
    // Collections open when they are first used, except those that sync with an upstream
    // automatically, which stay open. A host with one account opens it now, so a problem with it
    // shows at once.
    let only = users.len() == 1;
    for user in users
        .iter()
        .filter(|user| only || spares_server::auto_sync::syncs_automatically(&host.paths(&user.id)))
    {
        let collection = host
            .collection(&user.id)
            .await
            .map_err(|e| format!("Failed to open the collection of {}: {e}", user.id))?;
        collection
            .scope(async {
                info!(
                    "User {} ({}): replica {}",
                    user.name,
                    user.id,
                    collection.replica().map_or(0, |r| r.number())
                );
                if spares_core::notes_vcs::enabled() {
                    info!(
                        "Notes are kept in the git repository {}",
                        spares_core::notes_vcs::server_repo_dir().display()
                    );
                }
                for parser in get_all_parsers().iter().map(|p| p()) {
                    info!(
                        "Parser `{}` renders to {}",
                        parser.get_parser_name(),
                        parser
                            .get_output_rendered_dir(RenderOutputDirectoryType::Note)
                            .display()
                    );
                }
            })
            .await;
        host.sync.start(Arc::clone(&collection)).await;
    }

    // Only the web app's own origin may call the API from a browser.
    let origin = HeaderValue::from_str(&host.config.web_origin()).map_err(|e| e.to_string())?;
    let cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
        ])
        .allow_origin(origin)
        .allow_credentials(true)
        .allow_headers([
            CONTENT_TYPE,
            AUTHORIZATION,
            axum::http::HeaderName::from_static(spares_server::host::auth::CSRF_HEADER),
        ]);
    let bind = host.config.bind.clone();
    let frontend_dir: Option<PathBuf> = host.config.frontend_dir.clone();
    let app = create_router(
        Arc::new(AppState {
            host: Arc::clone(&host),
        }),
        frontend_dir,
    )
    .layer(cors);
    let listener = match TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            warn!("Server is already running at {bind:?}. Exiting.");
            return Ok(());
        }
        Err(e) => {
            return Err(format!("Failed to bind to {bind}: {e}"));
        }
    };
    info!("Starting server at {bind:?}");
    spawn_idle_closer(Arc::clone(&host));
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal(Arc::clone(&host)))
        .await
        .map_err(|e| format!("Server error: {}", e))?;
    host.sync.stop_all();
    host.close_all().await;
    Ok(())
}

/// Closes collections nothing has used for a while.
fn spawn_idle_closer(host: Arc<Host>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            host.record_usage().await;
            host.close_idle().await;
            host.clean_render_scratch().await;
            if let Err(e) = spares_server::host::auth::prune_sessions(&host.accounts).await {
                warn!("Failed to remove expired sessions: {e}");
            }
        }
    });
}

/// Waits for Ctrl+C or SIGTERM, then stops every collection's running renders so in-flight requests
/// finish quickly.
async fn shutdown_signal(host: Arc<Host>) {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("Failed to listen for Ctrl+C: {}", e);
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                warn!("Failed to listen for SIGTERM: {}", e);
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    info!("Shutting down server");
    for collection in host.open_collections().await {
        spares_core::parsers::kill_renders_of(&collection.state);
    }
}

/// Spares Web Server: serves the host in a root directory that `spares host init` created
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// The host's root directory (default: `~/.local/share/spares`)
    #[arg(long, env = "SPARES_ROOT")]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// The admin's commands, as `spares host` (for a machine with only the server)
    Host(spares_server::host::admin::HostArgs),
}

#[tokio::main(worker_threads = 5)]
async fn main() {
    // A Typst worker serves renders and exits (see `spares_core::render::init_typst_workers`).
    spares_core::render::init_typst_workers();
    // Answers shell completion requests (COMPLETE=zsh) and exits.
    clap_complete::CompleteEnv::with_factory(Args::command).complete();
    // With the user whose collection a line is about, if any. Note texts never go in the log.
    env_logger::Builder::from_default_env()
        .format(|buf, record| {
            use std::io::Write;
            let user = spares_core::collection::current_id()
                .map(|id| format!(" user={id}"))
                .unwrap_or_default();
            let level_style = buf.default_level_style(record.level());
            writeln!(
                buf,
                "[{} {level_style}{}{level_style:#} {}{user}] {}",
                buf.timestamp(),
                record.level(),
                record.target(),
                record.args()
            )
        })
        .init();

    let mut args = Args::parse();
    let res = match args.command.take() {
        Some(Command::Host(mut host_args)) => {
            host_args.root = host_args.root.or(args.root);
            spares_server::host::admin::run(host_args)
                .await
                .map_err(|e| e.to_string())
        }
        None => start_server(args).await,
    };
    if let Err(e) = res {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
