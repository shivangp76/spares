//! The HTTP API of spares: its routes ([`route`]) and their handlers, and the host that serves
//! them ([`host`]). The `spares_server` binary runs a host. A browser that keeps a copy of the
//! collection runs the same router in a worker (`spares_wasm`), without the routes that need the
//! host's files or processes.
//!
//! Every handler acts on one collection, the request's ([`Coll`]): on a host, the one of the user
//! the request's session or token belongs to, never one named by the request's path or body.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use spares_core::collection::Collection;

#[cfg(not(target_family = "wasm"))]
pub mod auto_sync;
mod handlers;
#[cfg(not(target_family = "wasm"))]
pub mod host;
pub mod route;
#[cfg(not(target_family = "wasm"))]
mod version;

/// What the router's middleware can reach.
#[derive(Debug)]
pub struct AppState {
    #[cfg(not(target_family = "wasm"))]
    pub host: Arc<host::Host>,
}

/// The request's collection, which middleware put in its extensions: on a host, its user's; in a
/// browser, the device's copy.
#[derive(Clone, Debug)]
pub struct Coll(pub Arc<Collection>);

impl<S: Send + Sync> FromRequestParts<S> for Coll {
    type Rejection = (StatusCode, &'static str);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(
            parts
                .extensions
                .get::<Arc<Collection>>()
                .cloned()
                .map(Coll)
                .ok_or((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "this route has no collection",
                )),
        )
    }
}
