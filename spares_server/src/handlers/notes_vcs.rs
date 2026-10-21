use std::sync::Arc;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use spares_core::notes_vcs;
use spares_core::notes_vcs::CheckRequest;
use spares_core::notes_vcs::FinishRequest;
use spares_core::notes_vcs::ReceiveRequest;
use spares_core::notes_vcs::RemoveSavedCopiesRequest;

use crate::AppState;
use crate::handlers::error_to_response;

pub(crate) async fn notes_vcs_status_handler() -> impl IntoResponse {
    Json(notes_vcs::status())
}

/// Called by the `push-to-checkout` hook. Holds the exclusive notes lock until the push settles.
pub(crate) async fn notes_vcs_receive_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<ReceiveRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = notes_vcs::receive(data.db.clone(), body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

/// Called by the `post-receive` hook once git has updated `main`.
pub(crate) async fn notes_vcs_finish_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<FinishRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = notes_vcs::finish(data.db.clone(), body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

/// Creates and activates the server copy of the notes repository (`spares note init-vcs`).
pub(crate) async fn notes_vcs_init_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = data.db.clone();
    let result = notes_vcs::run_exclusive(async move { notes_vcs::init_server_repo(&db).await })
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

/// Compares the notes repository with the database (`spares note vcs-check`).
pub(crate) async fn notes_vcs_check_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<CheckRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = data.db.clone();
    let result = notes_vcs::run_exclusive(async move { notes_vcs::check(&db, body).await })
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

/// Deletes copies of files spares overwrote, once the user checked nothing in them is lost.
pub(crate) async fn notes_vcs_remove_saved_copies_handler(
    Json(body): Json<RemoveSavedCopiesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    notes_vcs::run_exclusive(async move { notes_vcs::remove_saved_copies(&body) })
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::NO_CONTENT)
}
