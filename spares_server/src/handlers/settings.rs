//! `/api/settings`: the collection's settings (see `spares_core::settings`), which the web app's
//! Settings page and `spares config` show and change. A browser's copy answers these itself, so a
//! device changes them offline too, and the change syncs.

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use spares_core::settings::ReplaceError;
use spares_core::settings::ReplaceSettingsRequest;
use spares_core::settings::SettingsChangeResponse;
use spares_core::settings::UpdateSettingsRequest;

use super::error_to_response;
use crate::Coll;

type HandlerError = (StatusCode, Json<serde_json::Value>);

/// Renders again, in the background, the notes whose files a change to the settings rendering
/// reads left out of date. A browser's copy renders nothing: the host does once the change syncs.
fn render_if_needed(
    collection: &std::sync::Arc<spares_core::collection::Collection>,
    response: &SettingsChangeResponse,
) {
    let renders = response
        .changed_keys
        .iter()
        .any(|key| spares_core::settings::affects_rendering(key));
    #[cfg(not(target_family = "wasm"))]
    if renders {
        let db = collection.db.clone();
        spares_core::collection::spawn(async move {
            spares_core::sync::files::render_out_of_date(&db).await;
        });
    }
    #[cfg(target_family = "wasm")]
    let _ = (collection, renders);
}

pub(crate) async fn get_settings_handler(
    Coll(data): Coll,
) -> Result<impl IntoResponse, HandlerError> {
    let view = spares_core::settings::view(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(Json(view))
}

/// Replaces the settings. With a `base` that is no longer the settings, answers 409 with the
/// current text, changing nothing.
pub(crate) async fn replace_settings_handler(
    Coll(data): Coll,
    Json(request): Json<ReplaceSettingsRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    match spares_core::settings::replace_settings(&data.db, request).await {
        Ok(response) => {
            render_if_needed(&data, &response);
            Ok(Json(response))
        }
        Err(ReplaceError::ChangedSince(current)) => Err((
            StatusCode::CONFLICT,
            Json(json!({
                "message": "The settings changed since you opened them (on another device, \
                            say). Your changes were not saved.",
                "current": current,
            })),
        )),
        Err(ReplaceError::Other(e)) => Err(error_to_response(e)),
    }
}

pub(crate) async fn update_settings_handler(
    Coll(data): Coll,
    Json(request): Json<UpdateSettingsRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let response = spares_core::settings::update_settings(&data.db, request)
        .await
        .map_err(error_to_response)?;
    render_if_needed(&data, &response);
    Ok(Json(response))
}
