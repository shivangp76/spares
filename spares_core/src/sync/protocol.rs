//! Requests and responses between the client and the server (`/api/sync/*`).

use serde::Deserialize;
use serde::Serialize;

use super::records::Changes;
use crate::config::SyncRole;

/// `GET /api/sync/meta?replica=N`: what the client needs to decide how to sync.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MetaResponse {
    /// The server's counter. Rows changed since the client's last sync have a larger usn.
    pub usn: i64,
    /// Changes whenever the server's database is replaced (a full upload). A client that last
    /// synced in another epoch has to do a full sync.
    pub epoch: String,
    /// The server's clock, as Unix time.
    pub server_time: i64,
    /// The newest database migration the server has run.
    pub schema_version: i64,
    pub replica: u8,
    pub role: String,
    /// Whether the server has no notes, as on the first sync.
    pub empty: bool,
    /// The usn the asking replica last synced at, as the server recorded it.
    pub client_last_usn: Option<i64>,
}

impl MetaResponse {
    pub fn is_server(&self) -> bool {
        self.role == SyncRole::Server.to_string()
    }
}

/// `POST /api/sync/pull`
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PullRequest {
    /// The server's usn at the client's last sync.
    pub since: i64,
    pub replica: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PullResponse {
    /// The server's usn now. The push must name it.
    pub usn: i64,
    /// Rows changed since `since`, with every card of each changed note.
    pub changes: Changes,
}

/// A `render_asset` row: the client sends all of them on every push, since assets are only
/// added on the laptop and their files are copied separately.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, sqlx::FromRow)]
pub struct RenderAssetRecord {
    pub path: String,
    pub hash: String,
    pub mtime: i64,
    pub size: i64,
}

/// `POST /api/sync/push`
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PushRequest {
    /// The usn the pull returned. If the server changed since, it answers 409 and the client
    /// starts over.
    pub pull_usn: i64,
    pub replica: u8,
    /// The client's merged changes, applied as given.
    pub changes: Changes,
    #[serde(default)]
    pub render_assets: Vec<RenderAssetRecord>,
    #[serde(default)]
    pub render_packages: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Counts {
    pub parsers: i64,
    pub tags: i64,
    pub notes: i64,
    pub cards: i64,
    pub review_logs: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PushResponse {
    /// The server's usn after the push. The client stamps what it pushed with it.
    pub usn: i64,
    /// Row counts after the push, which the client's must match.
    pub counts: Counts,
}

/// `PUT /api/sync/full` (upload) response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FullUploadResponse {
    pub usn: i64,
    pub epoch: String,
}
