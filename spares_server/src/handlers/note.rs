use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use spares_core::api::note::create_notes;
use spares_core::api::note::delete_notes;
use spares_core::api::note::export::export_notes;
use spares_core::api::note::get_duplicate_keywords;
use spares_core::api::note::get_note;
use spares_core::api::note::get_note_links;
use spares_core::api::note::get_unmatched_keywords;
use spares_core::api::note::list_notes;
use spares_core::api::note::render_notes;
use spares_core::api::note::search_keyword;
use spares_core::api::note::search_notes;
use spares_core::api::note::update_notes;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::FilterOptions;
use spares_core::schema::note::CreateNotesRequest;
use spares_core::schema::note::DeleteNotesRequest;
use spares_core::schema::note::ExportNotesRequest;
use spares_core::schema::note::NoteLinksRequest;
use spares_core::schema::note::RenderNotesRequest;
use spares_core::schema::note::SearchKeywordRequest;
use spares_core::schema::note::SearchNotesRequest;
use spares_core::schema::note::UpdateNotesRequest;

use crate::AppState;
use crate::handlers::error_to_response;

pub(crate) async fn create_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<CreateNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = create_notes(&data.db, body, Utc::now(), &get_all_parsers(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub(crate) async fn get_note_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let note_res = get_note(&data.db, id).await.map_err(error_to_response)?;
    Ok(Json(note_res))
}

pub(crate) async fn update_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_notes_res = update_notes(&data.db, body, Utc::now(), &get_all_parsers(), true)
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_notes_res))
}

pub(crate) async fn delete_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<DeleteNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    delete_notes(&data.db, body, &get_all_parsers(), true)
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub(crate) async fn list_notes_handler(
    opts: Query<FilterOptions>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let list_notes_res = list_notes(&data.db, opts.0)
        .await
        .map_err(error_to_response)?;
    Ok(Json(list_notes_res))
}

pub(crate) async fn search_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<SearchNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let search_notes_res = search_notes(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(search_notes_res))
}

pub(crate) async fn search_keyword_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<SearchKeywordRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let search_keyword_res = search_keyword(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(search_keyword_res))
}

pub(crate) async fn generate_note_files_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<RenderNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    render_notes(&data.db, body, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub(crate) async fn get_unmatched_keywords_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let unmatched_keywords = get_unmatched_keywords(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(unmatched_keywords))
}

pub(crate) async fn get_note_links_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<NoteLinksRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let note_links = get_note_links(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(note_links))
}

pub(crate) async fn export_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<ExportNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = export_notes(&data.db, body, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub(crate) async fn get_duplicate_keywords_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let duplicate_keywords = get_duplicate_keywords(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(duplicate_keywords))
}
