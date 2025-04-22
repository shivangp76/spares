use crate::{AppState, handlers::error_to_response};
use axum::{
    Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use spares::{
    api::tag::{
        create_tag, delete_tag, get_tag, get_tag_by_name, list_tags, rebuild_tag, update_tag,
    },
    schema::{
        FilterOptions,
        tag::{CreateTagRequest, UpdateTagRequest},
    },
};
use std::sync::Arc;

pub async fn create_tag_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<CreateTagRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = create_tag(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub async fn get_tag_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let tag_res = get_tag(&data.db, id).await.map_err(error_to_response)?;
    Ok(Json(tag_res))
}

pub async fn get_tag_by_name_handler(
    Path(name): Path<String>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let tag_res = get_tag_by_name(&data.db, name.as_str())
        .await
        .map_err(error_to_response)?;
    Ok(Json(tag_res))
}

pub async fn update_tag_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateTagRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_tag_res = update_tag(&data.db, body, id)
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_tag_res))
}

pub async fn delete_tag_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    delete_tag(&data.db, id).await.map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub async fn list_tags_handler(
    opts: Option<Query<FilterOptions>>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let Query(opts) = opts.unwrap_or_default();
    let list_tags_res = list_tags(&data.db, opts).await.map_err(error_to_response)?;
    Ok(Json(list_tags_res))
}

pub async fn rebuild_tag_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    rebuild_tag(&data.db, id).await.map_err(error_to_response)?;
    Ok(StatusCode::OK)
}
