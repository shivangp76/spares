//! Requests and responses between the client and the server (`/api/sync/*`).
//!
//! There is no chunking: there are a few replicas (the laptop, the server and up to three browsers,
//! see [More replicas](super#more-replicas)), and a normal sync sends only what changed. A normal
//! sync:
//!
//! 1. **`meta`**. The client refuses to sync if the clocks differ by more than 5 minutes, the
//!    schema versions differ, both sides have the same replica number, or the roles aren't one
//!    `client` and one `server`. The server refuses a replica from 2 up that isn't enrolled
//!    ([`super::devices`]). It requires a full sync (412, with the reason) if it has never
//!    synced, `needs_full_sync` is set, the server's epoch changed (another full upload replaced
//!    it), its last usn is ahead of the server's (server restored from a backup), or behind what
//!    `sync_client` recorded for it (client restored from a backup).
//! 2. **`pull {since}`**. The server stamps everything changed on it since its last stamp (phone
//!    edits included) with `usn + 1`, and returns rows with `usn > since`, plus every card of each
//!    changed note.
//! 3. The client merges in one local transaction ([`super::client`]), then **`push`**es everything
//!    dirty. The server answers 409 if anything changed on it since the pull (a phone edit while
//!    the client merged); the client rolls back and starts over, up to 3 times. Otherwise the
//!    server applies the changes in one transaction (foreign keys deferred), stamps them, records
//!    the client in `sync_client`, and returns the new usn and row counts.
//! 4. The client stamps its dirty rows, clears its graves and bases, compares counts and commits.
//!    If the counts differ, it rolls back and sets `needs_full_sync`.
//!
//! **Crash safety:** if the client fails after the server committed a push, the next pull returns
//! the client's own changes, which match its still-dirty rows and merge to themselves.

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
    /// A device's: the [digest](RenderAssets::digest) of its render assets, so the pull brings the
    /// server's if they differ. Other clients have their own, and send none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_assets_digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PullResponse {
    /// The server's usn now. The push must name it.
    pub usn: i64,
    /// Rows changed since `since`, with every card of each changed note.
    pub changes: Changes,
    /// The server's render assets, for a device whose differ, which it replaces its own with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_assets: Option<RenderAssets>,
}

/// A `render_asset` row. Assets are found on the laptop, which sends all of them on every push,
/// and their files are copied separately. A device gets them from the server (see
/// [`PullResponse::render_assets`]), and sends none, so its copies never replace newer ones.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, sqlx::FromRow)]
pub struct RenderAssetRecord {
    pub path: String,
    pub hash: String,
    pub mtime: i64,
    pub size: i64,
}

/// Every `render_asset` row, by path, and every `render_package` spec, sorted.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RenderAssets {
    pub assets: Vec<RenderAssetRecord>,
    pub packages: Vec<String>,
}

impl RenderAssets {
    /// A hash of all of it, the same on both sides for the same rows.
    pub fn digest(&self) -> String {
        let json = serde_json::to_vec(self).expect("render assets serialize");
        blake3::hash(&json).to_hex().to_string()
    }
}

/// An `id_counter` row of the pushing replica: the largest id it ever handed out for `tbl`. The
/// server keeps the larger of its own and this, so a full download passes it on to the next device
/// enrolled under the same number, which then never hands out an id this one did (see
/// [`crate::ids`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, sqlx::FromRow)]
pub struct IdCounterRecord {
    pub tbl: String,
    pub last: i64,
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
    /// The client's own `id_counter` rows
    #[serde(default)]
    pub id_counters: Vec<IdCounterRecord>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Counts {
    pub parsers: i64,
    pub tags: i64,
    pub notes: i64,
    pub cards: i64,
    pub review_logs: i64,
    pub saved_searches: i64,
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
