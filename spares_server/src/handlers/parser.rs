use std::sync::Arc;

use axum::Json;
use axum::debug_handler;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use spares_core::api::parser::create_parser;
use spares_core::api::parser::delete_parser;
use spares_core::api::parser::get_parser;
use spares_core::api::parser::list_parsers;
use spares_core::api::parser::update_parser;
use spares_core::schema::FilterOptions;
use spares_core::schema::parser::CreateParserRequest;
use spares_core::schema::parser::UpdateParserRequest;

use crate::AppState;
use crate::handlers::error_to_response;

pub(crate) async fn create_parser_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<CreateParserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = create_parser(&data.db, body, true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub(crate) async fn get_parser_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let parser_res = get_parser(&data.db, id).await.map_err(error_to_response)?;
    Ok(Json(parser_res))
}

pub(crate) async fn update_parser_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateParserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_parser_res = update_parser(&data.db, body, id, true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_parser_res))
}

pub(crate) async fn delete_parser_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    delete_parser(&data.db, id, true)
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

#[debug_handler]
pub(crate) async fn list_parsers_handler(
    opts: Query<FilterOptions>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let list_parsers_res = list_parsers(&data.db, opts.0)
        .await
        .map_err(error_to_response)?;
    Ok(Json(list_parsers_res))
}
