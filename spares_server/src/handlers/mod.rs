use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use spares::Error;

pub mod card;
pub mod note;
pub mod parser;
pub mod review;
pub mod scheduler;
pub mod tag;

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

pub async fn health_check_handler() -> impl IntoResponse {
    const MESSAGE: &str = "API Services";

    let json_response = json!({
        "status": "ok",
        "message": MESSAGE
    });

    Json(json_response)
}
