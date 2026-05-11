use std::sync::Arc;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use spares_core::api::undo::undo_event;
use spares_core::schema::undo::UndoEventRequest;
use spares_core::schema::undo::UndoEventResponse;

use crate::AppState;
use crate::handlers::error_to_response;

pub(crate) async fn undo_event_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UndoEventRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result: Option<UndoEventResponse> = undo_event(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}
