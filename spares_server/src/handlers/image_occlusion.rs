use std::path::Path as FsPath;
use std::sync::Arc;

use axum::Json;
use axum::extract::Multipart;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use spares_core::api::image_occlusion::IMAGE_OCCLUSION_TEMPLATE;
use spares_core::api::image_occlusion::create_image_occlusion;
use spares_core::api::image_occlusion::list_note_image_occlusions;
use spares_core::api::image_occlusion::read_image_occlusion_file;
use spares_core::api::image_occlusion::update_note_image_occlusion;
use spares_core::parsers::get_all_parsers;

use crate::AppState;
use crate::handlers::error_to_response;

/// Uploads can be large photos or scans.
pub(crate) const IMAGE_OCCLUSION_BODY_LIMIT: usize = 64 * 1024 * 1024;

fn bad_request(message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "message": message })))
}

pub(crate) async fn get_image_occlusion_template_handler() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        IMAGE_OCCLUSION_TEMPLATE,
    )
}

/// Multipart fields: `parser_id`, `image` (with a file name) and `clozes` (the SVG).
pub(crate) async fn create_image_occlusion_handler(
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut parser_id: Option<i64> = None;
    let mut image: Option<(String, Vec<u8>)> = None;
    let mut clozes: Option<String> = None;
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
            Some("clozes") => {
                clozes = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| bad_request(&e.to_string()))?,
                );
            }
            _ => {}
        }
    }
    let missing = |name: &str| bad_request(&format!("Missing multipart field `{name}`"));
    let parser_id = parser_id.ok_or_else(|| missing("parser_id"))?;
    let (image_filename, image) = image.ok_or_else(|| missing("image"))?;
    let clozes = clozes.ok_or_else(|| missing("clozes"))?;

    let result = create_image_occlusion(
        &data.db,
        parser_id,
        &image,
        &image_filename,
        &clozes,
        &get_all_parsers(),
    )
    .await
    .map_err(error_to_response)?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub(crate) struct ImageOcclusionFileQuery {
    path: String,
}

pub(crate) async fn get_image_occlusion_file_handler(
    Query(ImageOcclusionFileQuery { path }): Query<ImageOcclusionFileQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let content_type = match FsPath::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    };
    let contents = read_image_occlusion_file(&path).map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": e.to_string() })),
        )
    })?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            // Stored files are never overwritten, so they can be cached
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        contents,
    ))
}

pub(crate) async fn list_note_image_occlusions_handler(
    Path(id): Path<i64>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = list_note_image_occlusions(&data.db, id, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub(crate) struct UpdateNoteImageOcclusionRequest {
    clozes: String,
}

/// `index` is 0 based, in the order the image occlusions appear in the note.
pub(crate) async fn update_note_image_occlusion_handler(
    Path((id, index)): Path<(i64, usize)>,
    axum::extract::State(data): axum::extract::State<Arc<AppState>>,
    Json(body): Json<UpdateNoteImageOcclusionRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let result = update_note_image_occlusion(
        &data.db,
        id,
        index,
        &body.clozes,
        Utc::now(),
        &get_all_parsers(),
    )
    .await
    .map_err(error_to_response)?;
    Ok(Json(result))
}
