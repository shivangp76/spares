// Some handlers have routes only on the server (`route::native_routes`), not in a browser.
#![cfg_attr(target_family = "wasm", allow(dead_code, unused_imports))]

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::Value;
use serde_json::json;
use spares_core::Error;
use spares_core::LibraryError;
use spares_core::NoteErrorKind;
use spares_core::TagErrorKind;

pub(crate) mod audio;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod auth;
pub(crate) mod card;
pub(crate) mod image;
pub(crate) mod image_occlusion;
pub(crate) mod note;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod notes_vcs;
pub(crate) mod parser;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod relay;
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
        // The device has to enroll again (`spares_core::sync::devices`).
        Error::NotEnrolled(replica) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "message": e.to_string(), "not_enrolled": true, "replica": replica })),
            );
        }
        // A search error's message marks the problem in the query, `span` says where it is, and
        // `query_field` which of the request's queries it is in, if it has several.
        Error::Library(LibraryError::Search {
            message,
            span,
            query_field,
        }) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": message, "span": span, "query_field": query_field })),
            );
        }
        // E.g. undoing what has been synced already
        Error::Sync(_)
        | Error::Library(
            LibraryError::Tag(TagErrorKind::InvalidInput(_)) | LibraryError::InvalidConfig(_),
        ) => Some(StatusCode::BAD_REQUEST),
        // A host syncing with its upstream
        Error::SyncUnreachable(_) => Some(StatusCode::BAD_GATEWAY),
        Error::Library(LibraryError::Note(NoteErrorKind::NotFound { .. })) => {
            Some(StatusCode::NOT_FOUND)
        }
        _ => None,
    };
    if let Some(status) = status {
        return (status, Json(json!({ "message": e.to_string() })));
    }
    // The message is for the user, as everywhere else, and the details for the log
    log::error!("{e:?}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "message": e.to_string() })),
    )
}

pub(crate) async fn health_check_handler() -> impl IntoResponse {
    const MESSAGE: &str = "API Services";

    let json_response = json!({
        "status": "ok",
        "message": MESSAGE
    });

    Json(json_response)
}
