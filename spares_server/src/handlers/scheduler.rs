use crate::{AppState, handlers::error_to_response};
use axum::{Json, extract::Path, http::StatusCode, response::IntoResponse};
use spares::api::scheduler::get_scheduler_ratings;
use std::sync::Arc;

pub async fn get_scheduler_ratings_handler(
    Path(name): Path<String>,
    axum::extract::State(_data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = get_scheduler_ratings(name.as_str()).map_err(error_to_response)?;
    Ok(Json(res))
}
