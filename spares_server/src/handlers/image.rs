use std::sync::Arc;

use axum::Json;
use axum::extract::Multipart;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use spares_core::api::image::create_image;
use spares_core::parsers::get_all_parsers;

use crate::AppState;
use crate::handlers::error_to_response;

/// Pasted screenshots and dropped photos can be large.
pub(crate) const IMAGE_BODY_LIMIT: usize = 64 * 1024 * 1024;

fn bad_request(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "message": message })))
}

/// Multipart fields: `parser_id` and `image` (with a file name).
pub(crate) async fn create_image_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut parser_id: Option<i64> = None;
    let mut image: Option<(String, Vec<u8>)> = None;
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
            Some("image") => {
                let filename = field.file_name().unwrap_or_default().to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(&e.to_string()))?;
                image = Some((filename, bytes.to_vec()));
            }
            _ => {}
        }
    }
    let missing = |name: &str| bad_request(&format!("Missing multipart field `{name}`"));
    let parser_id = parser_id.ok_or_else(|| missing("parser_id"))?;
    let (image_filename, image) = image.ok_or_else(|| missing("image"))?;

    let result = create_image(
        &data.db,
        parser_id,
        &image,
        &image_filename,
        &get_all_parsers(),
    )
    .await
    .map_err(error_to_response)?;
    Ok(Json(result))
}
