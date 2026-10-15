use std::sync::Arc;

use axum::Json;
use axum::extract::Multipart;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use spares_core::api::audio::MAX_AUDIO_SIZE;
use spares_core::api::audio::create_audio;
use spares_core::parsers::get_all_parsers;

use crate::AppState;
use crate::handlers::error_to_response;

/// Room is left for the multipart framing.
pub(crate) const AUDIO_BODY_LIMIT: usize = MAX_AUDIO_SIZE + 1024 * 1024;

fn bad_request(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "message": message })))
}

/// Multipart fields: `parser_id` and `audio` (with a file name).
pub(crate) async fn create_audio_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut parser_id: Option<i64> = None;
    let mut audio: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(&e.to_string()))?
    {
        match field.name() {
            Some("parser_id") => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| bad_request(&e.to_string()))?;
                parser_id = Some(
                    text.parse()
                        .map_err(|_| bad_request(&format!("Invalid parser_id: {text}")))?,
                );
            }
            Some("audio") => {
                let filename = field.file_name().unwrap_or_default().to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(&e.to_string()))?;
                audio = Some((filename, bytes.to_vec()));
            }
            _ => {}
        }
    }
    let missing = |name: &str| bad_request(&format!("Missing multipart field `{name}`"));
    let parser_id = parser_id.ok_or_else(|| missing("parser_id"))?;
    let (audio_filename, audio) = audio.ok_or_else(|| missing("audio"))?;

    let result = create_audio(
        &data.db,
        parser_id,
        &audio,
        &audio_filename,
        &get_all_parsers(),
    )
    .await
    .map_err(error_to_response)?;
    Ok(Json(result))
}
