use miette::{Error, miette};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares::schema::undo::{UndoEventRequest, UndoEventResponse};

pub(crate) async fn ensure_ok(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| miette!("{}", e))?;
        let message = response_json.get("message");
        return Err(miette!(message.unwrap().to_string()));
    }
    Ok(response)
}

pub(crate) async fn undo_event(
    base_url: &str,
    client: &Client,
    request: UndoEventRequest,
) -> Result<Option<UndoEventResponse>, String> {
    let url = format!("{}/api/undo", base_url);
    let response = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to undo event: {:?}", message));
    }
    let undo_response: Option<UndoEventResponse> =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(undo_response)
}
