use crate::{AppState, handlers::error_to_response};
use axum::{Json, http::StatusCode, response::IntoResponse};
use chrono::Utc;
use spares_core::api::review::{get_review_card, get_review_card_by_id, submit_study_action};
use spares_core::api::statistics::get_statistics;
use spares_core::model::CardId;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::review::{
    GetReviewCardRequest, StatisticsRequest, SubmitStudyActionRequest,
};
use std::sync::Arc;

pub(crate) async fn get_review_card_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<GetReviewCardRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let review_card_response = get_review_card(&data.db, body, Utc::now(), &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(Json(review_card_response))
}

pub(crate) async fn submit_study_action_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<SubmitStudyActionRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let submit_study_action_response = submit_study_action(&data.db, body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(submit_study_action_response))
}

pub(crate) async fn get_review_card_by_id_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    axum::extract::Path(card_id): axum::extract::Path<CardId>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = get_review_card_by_id(&data.db, card_id, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(Json(response))
}

pub(crate) async fn get_statistics_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<StatisticsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let stats_response = get_statistics(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(stats_response))
}
