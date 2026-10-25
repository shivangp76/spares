//! The HTTP API of spares: its routes ([`route`]) and their handlers. The `spares_server`
//! binary serves them. A browser that keeps a copy of the collection runs the same router in a
//! worker (`spares_wasm`), without the routes that need the server's files, processes or role.

use sqlx::SqlitePool;

mod handlers;
pub mod route;
#[cfg(not(target_family = "wasm"))]
mod version;

/// What every handler can reach.
#[derive(Debug)]
pub struct AppState {
    pub db: SqlitePool,
    /// The key requests must send (`SPARES_API_KEY`). `None` lets every request in.
    pub api_key: Option<String>,
}
