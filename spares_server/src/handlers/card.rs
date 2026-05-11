use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use spares_core::api::card::get_card;
use spares_core::api::card::get_cards;
use spares_core::api::card::get_leeches;
use spares_core::api::card::unbury_cards;
use spares_core::api::card::update_cards;
use spares_core::api::forget_card;
use spares_core::schema::card::GetLeechesRequest;
use spares_core::schema::card::UnburyRequest;
use spares_core::schema::card::UpdateCardsRequest;

use crate::AppState;
use crate::handlers::error_to_response;

pub(crate) async fn get_card_handler(
    Path(card_id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let card_res = get_card(&data.db, card_id)
        .await
        .map_err(error_to_response)?;
    Ok(Json(card_res))
}

pub(crate) async fn get_cards_handler(
    Path(note_id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let card_res = get_cards(&data.db, note_id)
        .await
        .map_err(error_to_response)?;
    Ok(Json(card_res))
}

pub(crate) async fn update_cards_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateCardsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_cards_res = update_cards(&data.db, body, Utc::now(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_cards_res))
}

pub(crate) async fn get_leeches_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<GetLeechesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cards_res = get_leeches(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(cards_res))
}

pub(crate) async fn forget_card_handler(
    Path(card_id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let forget_card_response = forget_card(&data.db, card_id, Utc::now(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(forget_card_response))
}

pub(crate) async fn unbury_cards_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    body: Option<Json<UnburyRequest>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let query = body.as_ref().and_then(|b| b.query.as_deref());
    unbury_cards(&data.db, query, Utc::now(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(()))
}
