use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use spares_core::api::scheduler::get_scheduler_ratings;
use spares_core::api::scheduler::resolve_rating_from_score;

use crate::AppState;
use crate::handlers::error_to_response;

#[derive(Deserialize)]
pub(crate) struct ScoreQuery {
    pub score: f64,
}

pub(crate) async fn get_scheduler_ratings_handler(
    Path(name): Path<String>,
    axum::extract::State(_data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = get_scheduler_ratings(name.as_str()).map_err(error_to_response)?;
    Ok(Json(res))
}

pub(crate) async fn get_rating_from_score_handler(
    Path(name): Path<String>,
    Query(query): Query<ScoreQuery>,
    axum::extract::State(_data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if !query.score.is_finite() || !(0.0..=1.0).contains(&query.score) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": format!(
                "score must be a finite number in [0, 1], got {}",
                query.score
            )})),
        ));
    }
    let res = resolve_rating_from_score(name.as_str(), query.score).map_err(error_to_response)?;
    Ok(Json(res))
}
