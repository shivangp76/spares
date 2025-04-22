use crate::{AppState, handlers::error_to_response};
use axum::{Json, http::StatusCode, response::IntoResponse};
use chrono::Utc;
use spares::api::review::{get_review_card, submit_study_action};
use spares::api::statistics::get_statistics;
use spares::parsers::get_all_parsers;
use spares::schema::review::{GetReviewCardRequest, StatisticsRequest, SubmitStudyActionRequest};
use std::sync::Arc;

pub async fn get_review_card_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<GetReviewCardRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let review_card_response = get_review_card(&data.db, body, Utc::now(), &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(Json(review_card_response))
}

pub async fn submit_study_action_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<SubmitStudyActionRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    submit_study_action(&data.db, body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub async fn get_statistics_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<StatisticsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let stats_response = get_statistics(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(stats_response))
}
