//! The server over HTTP: `/api/sync/*` on `sync_url` (see `spares_server`'s sync handlers).

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::protocol::FullUploadResponse;
use super::protocol::MetaResponse;
use super::protocol::PullRequest;
use super::protocol::PullResponse;
use super::protocol::PushRequest;
use super::protocol::PushResponse;
use super::run::SyncRemote;
use crate::Error;

/// The environment variable with the server's API key (its `SPARES_API_KEY`).
pub const SYNC_API_KEY_ENV: &str = "SPARES_SYNC_API_KEY";

pub struct HttpRemote {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl HttpRemote {
    /// `base_url` is the server's address, such as `https://spares.tail1234.ts.net`. The API key
    /// comes from `SPARES_SYNC_API_KEY`.
    pub fn new(base_url: &str) -> Result<Self, Error> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: std::env::var(SYNC_API_KEY_ENV).ok(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(600))
                .build()?,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self
            .client
            .request(method, format!("{}{path}", self.base_url));
        match &self.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
        let response = request.send().await.map_err(|e| {
            Error::Sync(format!(
                "cannot reach the sync server {}: {e}",
                self.base_url
            ))
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or(body);
        Err(match status {
            StatusCode::CONFLICT => Error::SyncRetry,
            StatusCode::PRECONDITION_FAILED => Error::FullSyncRequired(message),
            StatusCode::UNAUTHORIZED => Error::Sync(format!(
                "the sync server refused the API key. Set {SYNC_API_KEY_ENV} to its \
                 SPARES_API_KEY."
            )),
            _ => Error::Sync(format!("the sync server answered {status}: {message}")),
        })
    }

    async fn post<B: Serialize + Sync, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R, Error> {
        Ok(self
            .send(self.request(reqwest::Method::POST, path).json(body))
            .await?
            .json()
            .await?)
    }
}

#[async_trait]
impl SyncRemote for HttpRemote {
    async fn meta(&self, replica: u8) -> Result<MetaResponse, Error> {
        Ok(self
            .send(self.request(
                reqwest::Method::GET,
                &format!("/api/sync/meta?replica={replica}"),
            ))
            .await?
            .json()
            .await?)
    }

    async fn pull(&self, request: PullRequest) -> Result<PullResponse, Error> {
        self.post("/api/sync/pull", &request).await
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, Error> {
        self.post("/api/sync/push", &request).await
    }

    async fn download(&self, path: &Path) -> Result<(), Error> {
        let bytes = self
            .send(self.request(reqwest::Method::GET, "/api/sync/full"))
            .await?
            .bytes()
            .await?;
        std::fs::write(path, &bytes).map_err(|e| Error::Io {
            description: format!("Failed to write {}", path.display()),
            source: e,
        })
    }

    async fn upload(&self, path: &Path, replica: u8) -> Result<FullUploadResponse, Error> {
        let bytes = std::fs::read(path).map_err(|e| Error::Io {
            description: format!("Failed to read {}", path.display()),
            source: e,
        })?;
        Ok(self
            .send(
                self.request(
                    reqwest::Method::PUT,
                    &format!("/api/sync/full?replica={replica}"),
                )
                .body(bytes),
            )
            .await?
            .json()
            .await?)
    }
}
