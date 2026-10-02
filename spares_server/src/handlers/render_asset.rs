use std::sync::Arc;

use axum::Json;
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use spares_core::api::browser_render::get_render_asset;
use spares_core::api::browser_render::list_render_packages;

use crate::AppState;
use crate::handlers::error_to_response;

#[derive(Deserialize)]
pub(crate) struct RenderAssetQuery {
    /// The absolute path the source refers to the file by.
    path: String,
}

pub(crate) async fn get_render_asset_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Query(RenderAssetQuery { path }): Query<RenderAssetQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": format!("No render asset for {path}") })),
        )
    };
    let (stored_path, hash) = get_render_asset(&data.db, &path)
        .await
        .map_err(error_to_response)?
        .ok_or_else(not_found)?;
    // Contents are addressed by hash, so the hash is a strong validator.
    let etag = format!("\"{hash}\"");
    let cache_headers = [
        (header::ETAG, etag.clone()),
        (header::CACHE_CONTROL, "private, no-cache".to_string()),
    ];
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|value| value.as_bytes() == etag.as_bytes())
    {
        return Ok((StatusCode::NOT_MODIFIED, cache_headers, Vec::new()));
    }
    let contents = tokio::fs::read(&stored_path)
        .await
        .map_err(|_| not_found())?;
    Ok((StatusCode::OK, cache_headers, contents))
}

pub(crate) async fn list_render_packages_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let packages = list_render_packages(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(packages))
}
