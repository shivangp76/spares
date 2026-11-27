use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use serde::Deserialize;
use spares_core::api::scheduler::apply_optimize_proposal;
use spares_core::api::scheduler::auto_optimize;
use spares_core::api::scheduler::auto_optimize_due;
use spares_core::api::scheduler::discard_optimize_proposal;
use spares_core::api::scheduler::get_scheduler_ratings;
use spares_core::api::scheduler::get_scheduler_states;
use spares_core::api::scheduler::optimize_scheduler;
use spares_core::api::scheduler::optimize_status;
use spares_core::api::scheduler::reschedule_preview;
use spares_core::api::scheduler::reschedule_scheduler;
use spares_core::api::scheduler::resolve_rating_from_score;
use spares_core::schema::review::OptimizeRequest;

use crate::Coll;
use crate::handlers::error_to_response;

#[derive(Deserialize)]
pub(crate) struct ScoreQuery {
    pub score: f64,
}

/// The names of the schedulers, the default first. The web app checks the one it logs in with.
pub(crate) async fn list_schedulers_handler() -> impl IntoResponse {
    Json(spares_core::schedulers::scheduler_names())
}

pub(crate) async fn get_scheduler_ratings_handler(
    Path(name): Path<String>,
    Coll(_data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = get_scheduler_ratings(name.as_str()).map_err(error_to_response)?;
    Ok(Json(res))
}

pub(crate) async fn get_scheduler_states_handler(
    Path(name): Path<String>,
    Coll(_data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = get_scheduler_states(name.as_str()).map_err(error_to_response)?;
    Ok(Json(res))
}

pub(crate) async fn get_rating_from_score_handler(
    Path(name): Path<String>,
    Query(query): Query<ScoreQuery>,
    Coll(_data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if !query.score.is_finite() || !(0.0..=1.0).contains(&query.score) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": format!(
                "score must be a finite number in [0, 1], got {}",
                query.score
            )})),
        ));
    }
    let res = resolve_rating_from_score(name.as_str(), query.score).map_err(error_to_response)?;
    Ok(Json(res))
}

pub(crate) async fn optimize_scheduler_handler(
    Path(name): Path<String>,
    Coll(data): Coll,
    Json(body): Json<OptimizeRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = optimize_scheduler(&data.db, name.as_str(), body, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(res))
}

/// Whether optimizing again is worth it (see `optimize_status`). On a host that optimizes by
/// itself (`auto_optimize`), it also starts fitting the parameters in the background once that is
/// due, and the proposal shows in a later answer.
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn optimize_status_handler(
    Path(name): Path<String>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let now = Utc::now();
    let mut status = optimize_status(&data.db, name.as_str(), now)
        .await
        .map_err(error_to_response)?;
    let running = auto_optimizing(&data.id);
    if !running
        && auto_optimize_due(&data.db, name.as_str(), now)
            .await
            .map_err(error_to_response)?
    {
        start_auto_optimize(Arc::clone(&data), name);
        status.optimizing = true;
        status.suggested = false;
    } else if running {
        status.optimizing = true;
        status.suggested = false;
    }
    Ok(Json(status))
}

/// The collections fitting parameters by themselves now, by id, so each fits them once at a time.
#[cfg(not(target_family = "wasm"))]
static AUTO_OPTIMIZING: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

#[cfg(not(target_family = "wasm"))]
fn auto_optimizing(collection: &str) -> bool {
    AUTO_OPTIMIZING.lock().unwrap().contains(collection)
}

/// Fits the parameters of `collection` in the background, for `auto_optimize`.
#[cfg(not(target_family = "wasm"))]
fn start_auto_optimize(collection: Arc<spares_core::collection::Collection>, scheduler: String) {
    if !AUTO_OPTIMIZING
        .lock()
        .unwrap()
        .insert(collection.id.clone())
    {
        return;
    }
    // In the request's collection, which `spawn` carries over
    spares_core::collection::spawn(async move {
        match auto_optimize(&collection.db, &scheduler, Utc::now()).await {
            Ok(Some(p)) => log::info!(
                "Fitted parameters that predict the reviews better ({} to {}), waiting to be applied",
                p.current.log_loss,
                p.optimized.log_loss
            ),
            Ok(None) => log::info!("Fitted parameters that predict the reviews no better"),
            Err(e) => log::warn!("Failed to optimize the scheduler's parameters: {e}"),
        }
        AUTO_OPTIMIZING.lock().unwrap().remove(&collection.id);
    });
}

/// Saves the parameters `auto_optimize` proposed and reschedules every card with them.
pub(crate) async fn apply_optimize_proposal_handler(
    Path(name): Path<String>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = apply_optimize_proposal(&data.db, name.as_str(), Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(res))
}

/// Discards the parameters `auto_optimize` proposed.
pub(crate) async fn discard_optimize_proposal_handler(
    Path(_name): Path<String>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    discard_optimize_proposal(&data.db)
        .await
        .map_err(error_to_response)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn reschedule_scheduler_handler(
    Path(name): Path<String>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = reschedule_scheduler(&data.db, name.as_str(), Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(res))
}

pub(crate) async fn reschedule_preview_handler(
    Path(name): Path<String>,
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let res = reschedule_preview(&data.db, name.as_str())
        .await
        .map_err(error_to_response)?;
    Ok(Json(res))
}
