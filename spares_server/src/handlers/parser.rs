use crate::{AppState, handlers::error_to_response};
use axum::{
    Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use spares::{
    api::parser::{create_parser, delete_parser, get_parser, list_parsers, update_parser},
    schema::{
        FilterOptions,
        parser::{CreateParserRequest, UpdateParserRequest},
    },
};
use std::sync::Arc;

pub async fn create_parser_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<CreateParserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = create_parser(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub async fn get_parser_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let parser_res = get_parser(&data.db, id).await.map_err(error_to_response)?;
    Ok(Json(parser_res))
}

pub async fn update_parser_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateParserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_parser_res = update_parser(&data.db, body, id)
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_parser_res))
}

pub async fn delete_parser_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    delete_parser(&data.db, id)
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub async fn list_parsers_handler(
    opts: Option<Query<FilterOptions>>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let Query(opts) = opts.unwrap_or_default();
    let list_parsers_res = list_parsers(&data.db, opts)
        .await
        .map_err(error_to_response)?;
    Ok(Json(list_parsers_res))
}
