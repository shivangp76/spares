use axum::Json;
use axum::debug_handler;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use spares_core::api::parser::create_parser;
use spares_core::api::parser::delete_parser;
use spares_core::api::parser::get_parser;
use spares_core::api::parser::get_parser_syntax;
use spares_core::api::parser::list_parsers;
use spares_core::api::parser::update_parser;
use spares_core::notes_vcs::run_exclusive;
use spares_core::schema::FilterOptions;
use spares_core::schema::parser::CreateParserRequest;
use spares_core::schema::parser::UpdateParserRequest;

use crate::Coll;
use crate::handlers::error_to_response;

pub(crate) async fn create_parser_handler(
    Coll(data): Coll,
    Json(body): Json<CreateParserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = create_parser(&data.db, body, true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub(crate) async fn get_parser_handler(
    Path(id): Path<i64>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let parser_res = get_parser(&data.db, id).await.map_err(error_to_response)?;
    Ok(Json(parser_res))
}

/// The names of the parsers spares has, which a parser here must have to be used.
pub(crate) async fn list_known_parsers_handler() -> impl IntoResponse {
    Json(spares_core::parsers::parser_names())
}

/// The examples of how notes are written for the parser, for the web app's note editor.
pub(crate) async fn get_parser_syntax_handler(
    Path(id): Path<i64>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let examples = get_parser_syntax(&data.db, id)
        .await
        .map_err(error_to_response)?;
    Ok(Json(examples))
}

pub(crate) async fn update_parser_handler(
    Path(id): Path<i64>,
    Coll(data): Coll,
    Json(body): Json<UpdateParserRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = data.db.clone();
    let update_parser_res = run_exclusive(async move { update_parser(&db, body, id, true).await })
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_parser_res))
}

pub(crate) async fn delete_parser_handler(
    Path(id): Path<i64>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = data.db.clone();
    let event_id = run_exclusive(async move { delete_parser(&db, id, true).await })
        .await
        .map_err(error_to_response)?;
    // The event to undo the delete with
    Ok(Json(serde_json::json!({ "event_id": event_id })))
}

#[debug_handler]
pub(crate) async fn list_parsers_handler(
    opts: Query<FilterOptions>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let list_parsers_res = list_parsers(&data.db, opts.0)
        .await
        .map_err(error_to_response)?;
    Ok(Json(list_parsers_res))
}
