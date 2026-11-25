use axum::Json;
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use chrono::Utc;
use spares_core::api::queue_position::queue_positions;
use spares_core::api::review::UnmaskedImageOcclusionFile;
use spares_core::api::review::create_review_snapshot_tag;
use spares_core::api::review::get_review_card;
use spares_core::api::review::get_review_card_by_id;
use spares_core::api::review::next_review_card_available_at;
use spares_core::api::review::submit_study_action;
use spares_core::api::review::unmasked_image_occlusion;
use spares_core::api::statistics::get_review_history;
use spares_core::api::statistics::get_statistics;
use spares_core::api::statistics::get_statistics_graphs;
use spares_core::api::statistics::get_steps_stats;
use spares_core::config::read_external_config;
use spares_core::model::CardId;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::review::GetReviewCardRequest;
use spares_core::schema::review::GetReviewCardResponse;
use spares_core::schema::review::QueuePositionRequest;
use spares_core::schema::review::ReviewConfigResponse;
use spares_core::schema::review::ReviewHistoryRequest;
use spares_core::schema::review::ReviewSnapshotRequest;
use spares_core::schema::review::StatisticsRequest;
use spares_core::schema::review::SubmitStudyActionRequest;
use spares_core::schema::review::UnmaskedImageOcclusionRequest;
use spares_core::schema::statistics::StatisticsGraphsRequest;
use spares_core::schema::statistics::StepsStatsRequest;

use crate::Coll;
use crate::handlers::error_to_response;

#[derive(serde::Deserialize)]
pub(crate) struct ReviewRequest {
    #[serde(flatten)]
    selection: GetReviewCardRequest,
    #[cfg(not(target_family = "wasm"))]
    #[serde(default)]
    include_front_html: bool,
}

#[derive(serde::Serialize)]
struct WebReviewCard {
    #[serde(flatten)]
    card: GetReviewCardResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    front_html: Option<String>,
}

impl ReviewRequest {
    async fn select(
        self,
        data: &spares_core::collection::Collection,
    ) -> Result<Option<WebReviewCard>, spares_core::Error> {
        let Some(card) =
            get_review_card(&data.db, self.selection, Utc::now(), &get_all_parsers()).await?
        else {
            return Ok(None);
        };
        // Device copies already have the rendered files in browser storage.
        #[cfg(target_family = "wasm")]
        let front_html = None;
        #[cfg(not(target_family = "wasm"))]
        let front_html = if self.include_front_html
            && let Some(path) = &card.card_front_web_path
        {
            small_front_html(&data.paths.files, path).await
        } else {
            None
        };
        Ok(Some(WebReviewCard { card, front_html }))
    }
}

/// Send small HTML fronts with the selection so displaying them takes one round trip.
/// Large or unreadable files go through the usual file viewer.
#[cfg(not(target_family = "wasm"))]
async fn small_front_html(files: &std::path::Path, path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    use std::path::Component;

    const MAX_BYTES: u64 = 64 * 1024;
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let path = files.join(path);
    let files = files.to_owned();
    tokio::task::spawn_blocking(move || {
        let path = path.canonicalize().ok()?;
        if !path.starts_with(files.canonicalize().ok()?) {
            return None;
        }
        let metadata = path.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return None;
        }
        let mut html = String::with_capacity(metadata.len() as usize);
        std::fs::File::open(path)
            .ok()?
            .take(MAX_BYTES + 1)
            .read_to_string(&mut html)
            .ok()?;
        (html.len() as u64 <= MAX_BYTES).then_some(html)
    })
    .await
    .ok()
    .flatten()
}

pub(crate) async fn get_review_card_handler(
    Coll(data): Coll,
    Json(body): Json<ReviewRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let review_card_response = body.select(&data).await.map_err(error_to_response)?;
    Ok(Json(review_card_response))
}

pub(crate) async fn next_review_card_available_at_handler(
    Coll(data): Coll,
    Json(body): Json<GetReviewCardRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let available_at = next_review_card_available_at(&data.db, body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(available_at))
}

pub(crate) async fn create_review_snapshot_handler(
    Coll(data): Coll,
    Json(body): Json<ReviewSnapshotRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = create_review_snapshot_tag(&data.db, body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(response))
}

pub(crate) async fn queue_positions_handler(
    Coll(data): Coll,
    Json(body): Json<QueuePositionRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = queue_positions(&data.db, body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(response))
}

#[derive(serde::Deserialize)]
pub(crate) struct SubmitRequest {
    #[serde(flatten)]
    action: SubmitStudyActionRequest,
    next_review: Option<ReviewRequest>,
}

#[derive(serde::Serialize)]
struct NextReview {
    card: Option<WebReviewCard>,
}

#[derive(serde::Serialize)]
struct SubmitResponse {
    #[serde(flatten)]
    action: spares_core::schema::review::SubmitStudyActionResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_review: Option<NextReview>,
}

pub(crate) async fn submit_study_action_handler(
    Coll(data): Coll,
    Json(body): Json<SubmitRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let action = submit_study_action(&data.db, body.action, Utc::now())
        .await
        .map_err(error_to_response)?;
    // The rating has committed. If selecting its successor fails, the client
    // can retry the read independently and still has the event ID for undo.
    let next_review = if let Some(request) = body.next_review {
        match request.select(&data).await {
            Ok(card) => Some(NextReview { card }),
            Err(error) => {
                log::warn!("Failed to fetch the next card after a study action: {error}");
                None
            }
        }
    } else {
        None
    };
    Ok(Json(SubmitResponse {
        action,
        next_review,
    }))
}

pub(crate) async fn get_review_card_by_id_handler(
    Coll(data): Coll,
    axum::extract::Path(card_id): axum::extract::Path<CardId>,
    Json(body): Json<GetReviewCardRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let response = get_review_card_by_id(
        &data.db,
        card_id,
        body.filter.as_ref(),
        Utc::now(),
        &get_all_parsers(),
    )
    .await
    .map_err(error_to_response)?;
    Ok(Json(response))
}

pub(crate) async fn get_statistics_handler(
    Coll(data): Coll,
    Json(body): Json<StatisticsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let stats_response = get_statistics(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(stats_response))
}

pub(crate) async fn get_review_history_handler(
    Coll(data): Coll,
    Query(request): Query<ReviewHistoryRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let history_response = get_review_history(&data.db, request)
        .await
        .map_err(error_to_response)?;
    Ok(Json(history_response))
}

pub(crate) async fn get_statistics_graphs_handler(
    Coll(data): Coll,
    Json(body): Json<StatisticsGraphsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let graphs = get_statistics_graphs(&data.db, body)
        .await
        .map_err(error_to_response)?;
    Ok(Json(graphs))
}

pub(crate) async fn get_steps_stats_handler(
    Coll(data): Coll,
    Json(body): Json<StepsStatsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let stats = get_steps_stats(&data.db, body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(stats))
}

pub(crate) async fn get_review_config_handler()
-> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let config = read_external_config().map_err(error_to_response)?;
    Ok(Json(ReviewConfigResponse {
        flagged_tag_name: config.flagged_tag_name,
        set_card_due_date_duration: config.set_card_due_date_duration,
        timebox: config.queue.timebox,
        auto_advance: config.queue.auto_advance,
    }))
}

/// A part of an image occlusion of a card, without masks: the original image, or its markup as an
/// SVG, or `204 No Content` if it has none (see `spares_core::api::review::unmasked_image_occlusion`).
/// Revalidated by its hash, so opening it again reads it but sends nothing.
pub(crate) async fn get_unmasked_image_occlusion_handler(
    Coll(data): Coll,
    Query(request): Query<UnmaskedImageOcclusionRequest>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let file = unmasked_image_occlusion(&data.db, &request, &get_all_parsers())
        .await
        .map_err(error_to_response)?;
    let (contents, media_type, hash) = match file {
        UnmaskedImageOcclusionFile::File {
            contents,
            media_type,
            hash,
        } => (contents, media_type, hash),
        UnmaskedImageOcclusionFile::NoMarkup => return Ok(StatusCode::NO_CONTENT.into_response()),
        UnmaskedImageOcclusionFile::NotFound => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "message": format!(
                        "Note {} card {} has no image occlusion {}.",
                        request.note_id.letters(),
                        request.card_order,
                        request.order
                    )
                })),
            ));
        }
    };
    let etag = format!("\"{hash}\"");
    let cache_headers = [
        (header::ETAG, etag.clone()),
        (header::CACHE_CONTROL, "private, no-cache".to_string()),
    ];
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|value| value.as_bytes() == etag.as_bytes())
    {
        return Ok((StatusCode::NOT_MODIFIED, cache_headers).into_response());
    }
    Ok((
        cache_headers,
        [(header::CONTENT_TYPE, media_type)],
        contents,
    )
        .into_response())
}
