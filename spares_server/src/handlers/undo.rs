use crate::{AppState, handlers::error_to_response};
use axum::{Json, http::StatusCode, response::IntoResponse};
use spares::{
    api::undo::undo_event,
    schema::undo::{UndoEventRequest, UndoEventResponse},
};
use std::sync::Arc;

pub async fn undo_event_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UndoEventRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result: Option<UndoEventResponse> = undo_event(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}
