use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use serde_json::Value;
use serde_json::json;
use spares_core::Error;
use spares_core::LibraryError;

use crate::AppState;

pub(crate) mod audio;
pub(crate) mod card;
pub(crate) mod image;
pub(crate) mod image_occlusion;
pub(crate) mod note;
pub(crate) mod notes_vcs;
pub(crate) mod parser;
pub(crate) mod render_asset;
pub(crate) mod review;
pub(crate) mod saved_search;
pub(crate) mod scheduler;
pub(crate) mod sync;
pub(crate) mod tag;
pub(crate) mod undo;

#[allow(
    clippy::needless_pass_by_value,
    reason = "can easily call `.map_err()`"
)]
fn error_to_response(e: Error) -> (StatusCode, Json<Value>) {
    if matches!(e, Error::NotesVcsBusy) {
        // Nothing was written, so clients can retry.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "message": e.to_string() })),
        );
    }
    let status = match &e {
        // The sync server changed during the sync: the client starts over.
        Error::SyncRetry => Some(StatusCode::CONFLICT),
        Error::FullSyncRequired(reason) => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(json!({ "message": reason, "full_sync_required": true })),
            );
        }
        // A search error's message marks the problem in the query, and `span` says where it is.
        Error::Library(LibraryError::Search { message, span }) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": message, "span": span })),
            );
        }
        Error::Sync(_) => Some(StatusCode::BAD_REQUEST),
        _ => None,
    };
    if let Some(status) = status {
        return (status, Json(json!({ "message": e.to_string() })));
    }
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "message": format!("{:?}", e)
        })),
    )
}

pub(crate) async fn require_api_key(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(expected) = &state.api_key {
        let token = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v: &str| v.strip_prefix("Bearer "));
        if token != Some(expected.as_str()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(request).await)
}

pub(crate) async fn health_check_handler() -> impl IntoResponse {
    const MESSAGE: &str = "API Services";

    let json_response = json!({
        "status": "ok",
        "message": MESSAGE
    });

    Json(json_response)
}
