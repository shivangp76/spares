//! The server over HTTP: `/api/sync/*` on `sync.url` (see `spares_server`'s sync handlers).

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::devices::Device;
use super::devices::DeviceConfig;
use super::devices::EnrollRequest;
use super::devices::EnrollResponse;
use super::devices::UnenrollRequest;
use super::media::MediaFile;
use super::media::MediaKind;
use super::protocol::FullUploadResponse;
use super::protocol::MetaResponse;
use super::protocol::PullRequest;
use super::protocol::PullResponse;
use super::protocol::PushRequest;
use super::protocol::PushResponse;
use super::push::PushTarget;
use super::run::SyncRemote;
use crate::Error;

/// How long a request may take, set per request, which reqwest also does in a browser (through an
/// `AbortController`), so a hung request never leaves a device syncing forever. A push that times
/// out after the server applied it is finished by the next sync (see [`super::run`]).
const QUICK_TIMEOUT: Duration = Duration::from_secs(30);
/// A pull or a push. The server's push can wait for the notes lock, then renders the changed
/// notes before it answers.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(300);
const FULL_SYNC_TIMEOUT: Duration = Duration::from_secs(600);

/// How requests to the host say who they are from.
#[derive(Clone, Debug)]
pub enum RemoteAuth {
    /// A device token: the CLI's, or another host's copy of the collection
    Token(String),
    /// A browser's session, whose cookie the browser sends, with its CSRF token
    Session {
        csrf_token: String,
    },
    None,
}

pub struct HttpRemote {
    base_url: String,
    auth: RemoteAuth,
    client: reqwest::Client,
}

impl RemoteAuth {
    /// A device token, if there is one.
    pub fn token(token: Option<String>) -> Self {
        token.map_or(Self::None, Self::Token)
    }
}

impl HttpRemote {
    /// `base_url` is the host's address, such as `https://spares.tail1234.ts.net`.
    pub fn new(base_url: &str, auth: RemoteAuth) -> Result<Self, Error> {
        let client = reqwest::Client::builder();
        // A browser's `fetch` has no connect timeout.
        #[cfg(not(target_family = "wasm"))]
        let client = client.connect_timeout(Duration::from_secs(10));
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
            client: client.build()?,
        })
    }

    /// The server's database, for a full download. A browser keeps it in its own storage rather
    /// than in a file, so [`SyncRemote::download`] is built on this.
    pub async fn download_bytes(&self) -> Result<Vec<u8>, Error> {
        Ok(self
            .send(self.request(reqwest::Method::GET, "/api/sync/full", FULL_SYNC_TIMEOUT))
            .await?
            .bytes()
            .await?
            .to_vec())
    }

    /// Replaces the server's database with `bytes`, for a full upload.
    pub async fn upload_bytes(
        &self,
        bytes: Vec<u8>,
        replica: u8,
    ) -> Result<FullUploadResponse, Error> {
        Ok(self
            .send(
                self.request(
                    reqwest::Method::PUT,
                    &format!("/api/sync/full?replica={replica}"),
                    FULL_SYNC_TIMEOUT,
                )
                .body(bytes),
            )
            .await?
            .json()
            .await?)
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        timeout: Duration,
    ) -> reqwest::RequestBuilder {
        let request = self
            .client
            .request(method, format!("{}{path}", self.base_url))
            .header(
                super::protocol::SYNC_PROTOCOL_HEADER,
                super::protocol::SYNC_PROTOCOL_VERSION.to_string(),
            )
            .timeout(timeout);
        // So a host whose upstream leads back to it refuses its own requests
        #[cfg(not(target_family = "wasm"))]
        let request = request.header(super::protocol::SYNC_VIA_HEADER, super::protocol::host_id());
        match &self.auth {
            RemoteAuth::Token(token) => request.bearer_auth(token),
            RemoteAuth::Session { csrf_token } => request.header("X-CSRF-Token", csrf_token),
            RemoteAuth::None => request,
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
        let response = request.send().await.map_err(|e| {
            Error::SyncUnreachable(format!(
                "cannot reach the sync server {}: {e}",
                self.base_url
            ))
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let json = serde_json::from_str::<serde_json::Value>(&body).ok();
        let not_enrolled = json.as_ref().and_then(|v| {
            v.get("not_enrolled")?.as_bool()?.then_some(())?;
            u8::try_from(v.get("replica")?.as_u64()?).ok()
        });
        let message = json
            .as_ref()
            .and_then(|v| v.get("message")?.as_str().map(str::to_string))
            .unwrap_or(body);
        if status == StatusCode::FORBIDDEN
            && let Some(replica) = not_enrolled
        {
            return Err(Error::NotEnrolled(replica));
        }
        Err(match status {
            StatusCode::CONFLICT => Error::SyncRetry,
            StatusCode::PRECONDITION_FAILED => Error::FullSyncRequired(message),
            // The server's own answer, while another change holds the notes, e.g. a full upload's
            // files. Not a proxy's, which has no JSON.
            StatusCode::SERVICE_UNAVAILABLE if json.is_some() => Error::NotesVcsBusy,
            // What the request asked for can't be done, which the server's message says, e.g.
            // "Sync failed: every device number (2–4) is taken …", once
            StatusCode::BAD_REQUEST => Error::Sync(
                message
                    .strip_prefix("Sync failed: ")
                    .unwrap_or(&message)
                    .to_string(),
            ),
            StatusCode::UNAUTHORIZED => Error::Sync(format!(
                "{} refused this device's token. Log in to it again.",
                self.base_url
            )),
            _ => Error::Sync(format!("the sync server answered {status}: {message}")),
        })
    }

    /// Asks the host for a replica number for this browser (see [`super::devices`]).
    pub async fn enroll(&self, name: &str) -> Result<EnrollResponse, Error> {
        self.enroll_with(&EnrollRequest {
            name: name.to_string(),
            ..EnrollRequest::default()
        })
        .await
    }

    /// Asks the host for a replica number: for this host's copy, or one it relays the request for.
    pub async fn enroll_with(&self, request: &EnrollRequest) -> Result<EnrollResponse, Error> {
        self.post("/api/sync/enroll", request, QUICK_TIMEOUT).await
    }

    /// Frees a device's replica number: this browser's, or that of one that lost its copy.
    pub async fn unenroll(&self, replica: u8) -> Result<(), Error> {
        let _: serde_json::Value = self
            .post(
                "/api/sync/unenroll",
                &UnenrollRequest { replica },
                QUICK_TIMEOUT,
            )
            .await?;
        Ok(())
    }

    /// The enrolled devices.
    pub async fn devices(&self) -> Result<Vec<Device>, Error> {
        Ok(self
            .send(self.request(reqwest::Method::GET, "/api/sync/devices", QUICK_TIMEOUT))
            .await?
            .json()
            .await?)
    }

    /// The server's config and parser templates, for a browser's copy.
    pub async fn device_config(&self) -> Result<DeviceConfig, Error> {
        Ok(self
            .send(self.request(reqwest::Method::GET, "/api/sync/config", QUICK_TIMEOUT))
            .await?
            .json()
            .await?)
    }

    /// Where `spares sync cloud` copies files to on the server (see [`super::push`]).
    pub async fn push_target(&self) -> Result<PushTarget, Error> {
        Ok(self
            .send(self.request(reqwest::Method::GET, "/api/sync/push-target", QUICK_TIMEOUT))
            .await?
            .json()
            .await?)
    }

    /// Sends this machine's `config.toml` to the server, which keeps its own `[sync]` table (see
    /// [`super::push::apply_pushed_config`]). Returns whether the server's changed.
    pub async fn push_config(&self, contents: String) -> Result<bool, Error> {
        let response: serde_json::Value = self
            .send(
                self.request(reqwest::Method::PUT, "/api/sync/push-config", QUICK_TIMEOUT)
                    .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    .body(contents),
            )
            .await?
            .json()
            .await?;
        Ok(response
            .get("changed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true))
    }

    /// The server's files of `kind` (see [`super::media`]).
    pub async fn list_media(&self, kind: MediaKind) -> Result<Vec<MediaFile>, Error> {
        Ok(self
            .send(self.request(
                reqwest::Method::GET,
                &format!("/api/sync/media/{}", kind.as_str()),
                QUICK_TIMEOUT,
            ))
            .await?
            .json()
            .await?)
    }

    /// A file of `kind` from the server.
    pub async fn download_media(&self, kind: MediaKind, name: &str) -> Result<Vec<u8>, Error> {
        Ok(self
            .send(self.request(
                reqwest::Method::GET,
                &format!("/api/sync/media/{}/{name}", kind.as_str()),
                EXCHANGE_TIMEOUT,
            ))
            .await?
            .bytes()
            .await?
            .to_vec())
    }

    /// Stores a file this browser added on the server, before the push whose notes name it.
    pub async fn upload_media(
        &self,
        kind: MediaKind,
        name: &str,
        contents: Vec<u8>,
    ) -> Result<(), Error> {
        self.send(
            self.request(
                reqwest::Method::PUT,
                &format!("/api/sync/media/{}/{name}", kind.as_str()),
                EXCHANGE_TIMEOUT,
            )
            .body(contents),
        )
        .await?;
        Ok(())
    }

    async fn post<B: Serialize + Sync, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<R, Error> {
        Ok(self
            .send(
                self.request(reqwest::Method::POST, path, timeout)
                    .json(body),
            )
            .await?
            .json()
            .await?)
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl SyncRemote for HttpRemote {
    async fn meta(&self, replica: u8) -> Result<MetaResponse, Error> {
        Ok(self
            .send(self.request(
                reqwest::Method::GET,
                &format!("/api/sync/meta?replica={replica}"),
                QUICK_TIMEOUT,
            ))
            .await?
            .json()
            .await?)
    }

    async fn pull(&self, request: PullRequest) -> Result<PullResponse, Error> {
        self.post("/api/sync/pull", &request, EXCHANGE_TIMEOUT)
            .await
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, Error> {
        self.post("/api/sync/push", &request, EXCHANGE_TIMEOUT)
            .await
    }

    async fn download(&self, path: &Path) -> Result<(), Error> {
        let bytes = self.download_bytes().await?;
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
        self.upload_bytes(bytes, replica).await
    }
}
