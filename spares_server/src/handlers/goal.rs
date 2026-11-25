use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use spares_core::api::goal::goal_status;
use spares_core::config::read_external_config;

use super::error_to_response;
use crate::Coll;

/// How each goal of `config.toml` is going (see `spares_core::api::goal::goal_status`).
pub(crate) async fn get_goal_status_handler(
    Coll(data): Coll,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let config = read_external_config().map_err(error_to_response)?;
    let statuses = goal_status(&data.db, &config, Utc::now())
        .await
        .map_err(error_to_response)?;
    Ok(Json(statuses))
}
