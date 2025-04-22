use crate::{AppState, handlers::error_to_response};
use axum::{Json, extract::Path, http::StatusCode, response::IntoResponse};
use chrono::Utc;
use spares::{
    api::card::{get_card, get_cards, get_leeches, update_card},
    schema::card::{GetLeechesRequest, UpdateCardRequest},
};
use std::sync::Arc;

pub async fn get_card_handler(
    Path(card_id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let card_res = get_card(&data.db, card_id)
        .await
        .map_err(error_to_response)?;
    Ok(Json(card_res))
}

pub async fn get_cards_handler(
    Path(note_id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let card_res = get_cards(&data.db, note_id)
        .await
        .map_err(error_to_response)?;
    Ok(Json(card_res))
}

pub async fn update_card_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateCardRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_card_res = update_card(&data.db, body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_card_res))
}

pub async fn get_leeches_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<GetLeechesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cards_res = get_leeches(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(cards_res))
}
