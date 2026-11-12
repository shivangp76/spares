//! Notes exported to share and imported, in the web app (see `spares_core::api::note::export`,
//! `spares_core::api::note::share_bundle` and `spares_core::api::note::import`). Only on the
//! server: a browser's copy doesn't have every file its notes name, so it sends these on.

use axum::Json;
use axum::body::Bytes;
use axum::extract::Multipart;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use chrono::Utc;
use serde::Deserialize;
use spares_core::api::note::export::export_notes;
use spares_core::api::note::import::ImportRequestOptions;
use spares_core::api::note::import::Upload;
use spares_core::api::note::import::import_uploaded;
use spares_core::api::note::share_bundle::import_shared;
use spares_core::api::note::share_bundle::write_bundle;
use spares_core::config::read_external_config;
use spares_core::notes_vcs::run_shared;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::note::ExportNotesRequest;
use spares_core::schema::note::ExportNotesResponse;

use super::error_to_response;
use crate::Coll;

type HandlerError = (StatusCode, Json<serde_json::Value>);

/// The most an upload to import may be: a bundle's media, zipped. An account's limit on media
/// uploads applies too (see `host::limits`).
pub(crate) const IMPORT_BODY_LIMIT: usize = 512 * 1024 * 1024;

async fn export_to_share(
    db: &sqlx::SqlitePool,
    query: String,
) -> Result<ExportNotesResponse, HandlerError> {
    let config = read_external_config().map_err(error_to_response)?;
    let request = ExportNotesRequest { query, share: true };
    export_notes(db, request, &config, &get_all_parsers())
        .await
        .map_err(error_to_response)
}

#[derive(Debug, Deserialize)]
pub(crate) struct ShareRequest {
    query: String,
}

/// `POST /api/notes/share/summary`: what the notes `query` finds would be shared as, with their
/// media's names but not their contents, to show before downloading them.
pub(crate) async fn share_summary_handler(
    Coll(data): Coll,
    Json(body): Json<ShareRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let mut export = export_to_share(&data.db, body.query).await?;
    for media in &mut export.media {
        media.contents = Vec::new();
    }
    Ok(Json(export))
}

/// `POST /api/notes/share/bundle`: the notes `query` finds, exported to share, as a zip.
pub(crate) async fn share_bundle_handler(
    Coll(data): Coll,
    Json(body): Json<ShareRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let export = export_to_share(&data.db, body.query).await?;
    let zip = write_bundle(&export).map_err(error_to_response)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"shared.zip\"",
            ),
        ],
        zip,
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImportQuery {
    /// The uploaded file's name, whose extension says which parser reads a file of notes
    name: String,
}

/// `POST /api/notes/share/import?name=`: shared notes, a file of them or a zip, added as new
/// notes.
pub(crate) async fn import_shared_handler(
    Coll(data): Coll,
    Query(query): Query<ImportQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, HandlerError> {
    let db = data.db.clone();
    // Boxed, since it is large
    let imported = run_shared(Box::pin(async move {
        import_shared(&db, &query.name, &body, Utc::now(), &get_all_parsers()).await
    }))
    .await
    .map_err(error_to_response)?;
    Ok(Json(imported))
}

fn bad_request(message: &str) -> HandlerError {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "message": message })),
    )
}

/// `POST /api/notes/import`: files of notes, or zips of them with their media, imported as
/// `spares import` imports them (see `spares_core::api::note::import::import_uploaded`).
///
/// Multipart fields: `options`, an `ImportRequestOptions` as JSON (optional), and `file`, with a
/// file name, once per file.
pub(crate) async fn import_files_handler(
    Coll(data): Coll,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, HandlerError> {
    let mut options = ImportRequestOptions::default();
    let mut uploads = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(&e.to_string()))?
    {
        match field.name() {
            Some("options") => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| bad_request(&e.to_string()))?;
                options = serde_json::from_str(&text)
                    .map_err(|e| bad_request(&format!("Invalid options: {e}")))?;
            }
            Some("file") => {
                let name = field.file_name().unwrap_or("notes").to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| bad_request(&e.to_string()))?;
                uploads.push(Upload {
                    name,
                    bytes: bytes.to_vec(),
                });
            }
            _ => {}
        }
    }
    if uploads.is_empty() {
        return Err(bad_request("Missing multipart field `file`"));
    }
    let db = data.db.clone();
    // Boxed, since it is large
    let imported = run_shared(Box::pin(async move {
        import_uploaded(&db, &uploads, &options, Utc::now()).await
    }))
    .await
    .map_err(error_to_response)?;
    Ok(Json(imported))
}
