use crate::{AppState, handlers::error_to_response};
use axum::{
    Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use spares::{
    api::note::{
        create_notes, delete_note, export::export_notes, get_note, get_note_links,
        get_unmatched_keywords, list_notes, render_notes, search_keyword, search_notes,
        update_notes,
    },
    parsers::get_all_parsers,
    schema::{
        FilterOptions,
        note::{
            CreateNotesRequest, ExportNotesRequest, NoteLinksRequest, RenderNotesRequest,
            SearchKeywordRequest, SearchNotesRequest, UpdateNotesRequest,
        },
    },
};
use std::sync::Arc;

pub async fn create_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<CreateNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = create_notes(&data.db, body, Utc::now(), &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

pub async fn get_note_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let note_res = get_note(&data.db, id).await.map_err(error_to_response)?;
    Ok(Json(note_res))
}

pub async fn update_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let update_notes_res = update_notes(&data.db, body, Utc::now(), &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(Json(update_notes_res))
}

pub async fn delete_note_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    delete_note(&data.db, id, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub async fn list_notes_handler(
    opts: Query<FilterOptions>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let list_notes_res = list_notes(&data.db, opts.0)
        .await
        .map_err(error_to_response)?;
    Ok(Json(list_notes_res))
}

pub async fn search_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<SearchNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let search_notes_res = search_notes(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(search_notes_res))
}

pub async fn search_keyword_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<SearchKeywordRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let search_keyword_res = search_keyword(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(search_keyword_res))
}

pub async fn generate_note_files_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<RenderNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    render_notes(&data.db, body, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::OK)
}

pub async fn get_unmatched_keywords_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let unmatched_keywords = get_unmatched_keywords(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(unmatched_keywords))
}

pub async fn get_note_links_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<NoteLinksRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let note_links = get_note_links(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(note_links))
}

pub async fn export_notes_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<ExportNotesRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = export_notes(&data.db, body, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(result) // Returns String directly
}

pub async fn get_duplicate_keywords_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let duplicate_keywords = spares::api::note::get_duplicate_keywords(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(duplicate_keywords))
}
