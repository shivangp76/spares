use crate::{AppState, handlers::error_to_response};
use axum::{Json, extract::Path, http::StatusCode, response::IntoResponse};
use chrono::Utc;
use spares::{
    api::{
        card::{get_card, get_cards, get_leeches, unbury_cards, update_cards},
        forget_card,
    },
    schema::card::{GetLeechesRequest, UpdateCardsRequest},
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

pub async fn update_cards_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateCardsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_cards_res = update_cards(&data.db, body, Utc::now(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_cards_res))
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

pub async fn forget_card_handler(
    Path(card_id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let forget_card_response = forget_card(&data.db, card_id, Utc::now(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(forget_card_response))
}

pub async fn unbury_cards_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    unbury_cards(&data.db, Utc::now(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(()))
}
