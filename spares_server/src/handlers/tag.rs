use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use spares_core::api::tag::create_tag;
use spares_core::api::tag::delete_tag;
use spares_core::api::tag::get_tag;
use spares_core::api::tag::get_tag_by_name;
use spares_core::api::tag::list_tags;
use spares_core::api::tag::rebuild_tag;
use spares_core::api::tag::update_tag;
use spares_core::schema::FilterOptions;
use spares_core::schema::tag::CreateTagRequest;
use spares_core::schema::tag::UpdateTagRequest;

use crate::AppState;
use crate::handlers::error_to_response;

pub(crate) async fn create_tag_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<CreateTagRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = create_tag(&data.db, body, true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub(crate) async fn get_tag_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let tag_res = get_tag(&data.db, id).await.map_err(error_to_response)?;
    Ok(Json(tag_res))
}

pub(crate) async fn get_tag_by_name_handler(
    Path(name): Path<String>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let tag_res = get_tag_by_name(&data.db, name.as_str())
        .await
        .map_err(error_to_response)?;
    Ok(Json(tag_res))
}

pub(crate) async fn update_tag_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateTagRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_tag_res = update_tag(&data.db, body, true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_tag_res))
}

pub(crate) async fn delete_tag_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    delete_tag(&data.db, id, true)
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub(crate) async fn list_tags_handler(
    opts: Query<FilterOptions>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let list_tags_res = list_tags(&data.db, opts.0)
        .await
        .map_err(error_to_response)?;
    Ok(Json(list_tags_res))
}

pub(crate) async fn rebuild_tag_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    rebuild_tag(&data.db, id).await.map_err(error_to_response)?;
    Ok(StatusCode::OK)
}
