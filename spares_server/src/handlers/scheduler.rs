use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use spares_core::api::scheduler::get_scheduler_ratings;

use crate::AppState;
use crate::handlers::error_to_response;

pub(crate) async fn get_scheduler_ratings_handler(
    Path(name): Path<String>,
    axum::extract::State(_data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = get_scheduler_ratings(name.as_str()).map_err(error_to_response)?;
    Ok(Json(res))
}
