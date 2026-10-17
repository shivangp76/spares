use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use spares_core::api::saved_search::delete_saved_search;
use spares_core::api::saved_search::get_saved_search;
use spares_core::api::saved_search::list_saved_searches;
use spares_core::api::saved_search::save_search;
use spares_core::schema::saved_search::SavedSearch;

use super::error_to_response;
use crate::AppState;

pub(crate) async fn list_saved_searches_handler(
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let searches = list_saved_searches(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(searches))
}

/// The saved search called `name`, or 404.
pub(crate) async fn get_saved_search_handler(
    State(data): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    match get_saved_search(&data.db, &name)
        .await
        .map_err(error_to_response)?
    {
        Some(search) => Ok(Json(search)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "message": format!("No search is saved as `{name}`") })),
        )),
    }
}

/// Saves a search, replacing the one with the same name.
pub(crate) async fn save_search_handler(
    State(data): State<Arc<AppState>>,
    Json(body): Json<SavedSearch>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let saved = save_search(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(saved))
}

/// Deletes the saved search called `name`, returning whether there was one.
pub(crate) async fn delete_saved_search_handler(
    State(data): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let deleted = delete_saved_search(&data.db, &name)
        .await
        .map_err(error_to_response)?;
    Ok(Json(deleted))
}
