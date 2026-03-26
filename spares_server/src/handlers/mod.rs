use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use spares_core::Error;

pub(crate) mod card;
pub(crate) mod note;
pub(crate) mod parser;
pub(crate) mod review;
pub(crate) mod scheduler;
pub(crate) mod tag;
pub(crate) mod undo;

#[allow(
    clippy::needless_pass_by_value,
    reason = "can easily call `.map_err()`"
)]
fn error_to_response(e: Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "message": format!("{:?}", e)
        })),
    )
}

pub(crate) async fn health_check_handler() -> impl IntoResponse {
    const MESSAGE: &str = "API Services";

    let json_response = json!({
        "status": "ok",
        "message": MESSAGE
    });

    Json(json_response)
}
