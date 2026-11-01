//! What a user may use of a host, and what they use.
//!
//! [`Limits`] has the host's defaults in `host.toml`'s `[limits]`, which an admin overrides for a
//! user in `accounts.sqlite` (`spares host limits`). The laptop's are as high as before hosts:
//! request bodies sized for one trusted user, no storage quota, no rate limit.
//!
//! [`crate::handlers::auth::authenticate`] enforces them on every request: too many requests a
//! minute get `429`, a body larger than the user's limit `413`, an upload past the storage quota
//! `507`, and a search that takes too long `503`. Usage (requests, and the size of each open
//! collection's directory, measured every few minutes) is counted in `accounts.sqlite`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use spares_core::Error;
use sqlx::SqlitePool;

use super::accounts::sqlx_error;

const MB: u64 = 1024 * 1024;

/// `[limits]` in `host.toml`, and a user's limits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// The largest push or full upload, in MB: a whole database.
    pub sync_body_mb: u64,
    /// The largest image, clip or image occlusion uploaded at once, in MB.
    pub media_body_mb: u64,
    /// How large a user's directory may grow, in MB. `0`: no quota.
    pub storage_mb: u64,
    /// Requests a minute. `0`: no limit.
    pub requests_per_minute: u64,
    /// How long a search may take, in seconds.
    pub search_seconds: u64,
    /// A user's renders at once (see `spares_core::render`); `0`: the host's `[render]` setting.
    pub renders_at_once: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            sync_body_mb: 1024,
            media_body_mb: 128,
            storage_mb: 0,
            requests_per_minute: 0,
            search_seconds: 30,
            renders_at_once: 0,
        }
    }
}

/// A user's overrides of the host's [`Limits`]; `None` keeps the host's.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, sqlx::FromRow)]
pub struct LimitOverrides {
    pub sync_body_mb: Option<i64>,
    pub media_body_mb: Option<i64>,
    pub storage_mb: Option<i64>,
    pub requests_per_minute: Option<i64>,
    pub search_seconds: Option<i64>,
    pub renders_at_once: Option<i64>,
}

impl Limits {
    #[must_use]
    pub fn with(&self, overrides: &LimitOverrides) -> Self {
        let pick =
            |user: Option<i64>, host: u64| user.and_then(|v| u64::try_from(v).ok()).unwrap_or(host);
        Self {
            sync_body_mb: pick(overrides.sync_body_mb, self.sync_body_mb),
            media_body_mb: pick(overrides.media_body_mb, self.media_body_mb),
            storage_mb: pick(overrides.storage_mb, self.storage_mb),
            requests_per_minute: pick(overrides.requests_per_minute, self.requests_per_minute),
            search_seconds: pick(overrides.search_seconds, self.search_seconds),
            renders_at_once: pick(overrides.renders_at_once, self.renders_at_once),
        }
    }

    /// The largest body a request to `path` may have.
    pub fn body_limit(&self, path: &str) -> Option<u64> {
        if path == "/api/sync/push" || path == "/api/sync/full" {
            Some(self.sync_body_mb.saturating_mul(MB))
        } else if path.starts_with("/api/sync/media/")
            || path == "/api/images"
            || path == "/api/audio"
            || path == "/api/image-occlusions"
        {
            Some(self.media_body_mb.saturating_mul(MB))
        } else {
            None
        }
    }

    /// Whether a request to `path` stores files or a database, which the storage quota applies to.
    pub fn stores(path: &str, method: &axum::http::Method) -> bool {
        method != axum::http::Method::GET
            && (path == "/api/sync/push"
                || path == "/api/sync/full"
                || path.starts_with("/api/sync/media/")
                || path == "/api/images"
                || path == "/api/images/url"
                || path == "/api/audio"
                || path == "/api/image-occlusions")
    }

    /// Whether `path` searches, which [`Limits::search_seconds`] applies to.
    pub fn searches(path: &str) -> bool {
        path.contains("/search")
            || path == "/api/notes/graph"
            || path == "/api/review/queue/position"
            || path == "/api/cards/leeches"
    }

    pub fn search_time(&self) -> Duration {
        Duration::from_secs(self.search_seconds.max(1))
    }

    pub fn storage_bytes(&self) -> Option<u64> {
        (self.storage_mb > 0).then(|| self.storage_mb.saturating_mul(MB))
    }
}

pub async fn overrides(db: &SqlitePool, user_id: &str) -> Result<LimitOverrides, Error> {
    Ok(
        sqlx::query_as("SELECT sync_body_mb, media_body_mb, storage_mb, requests_per_minute, search_seconds, renders_at_once FROM user_limits WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(db)
            .await
            .map_err(sqlx_error)?
            .unwrap_or_default(),
    )
}

pub async fn set_overrides(
    db: &SqlitePool,
    user_id: &str,
    overrides: &LimitOverrides,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO user_limits (user_id, sync_body_mb, media_body_mb, storage_mb,
             requests_per_minute, search_seconds, renders_at_once)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET sync_body_mb = excluded.sync_body_mb,
             media_body_mb = excluded.media_body_mb, storage_mb = excluded.storage_mb,
             requests_per_minute = excluded.requests_per_minute,
             search_seconds = excluded.search_seconds, renders_at_once = excluded.renders_at_once",
    )
    .bind(user_id)
    .bind(overrides.sync_body_mb)
    .bind(overrides.media_body_mb)
    .bind(overrides.storage_mb)
    .bind(overrides.requests_per_minute)
    .bind(overrides.search_seconds)
    .bind(overrides.renders_at_once)
    .execute(db)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

/// What a user has used.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, sqlx::FromRow)]
pub struct Usage {
    pub storage_bytes: i64,
    pub storage_measured_at: Option<i64>,
    pub requests: i64,
    pub last_request_at: Option<i64>,
}

pub async fn usage(db: &SqlitePool, user_id: &str) -> Result<Usage, Error> {
    Ok(sqlx::query_as(
        "SELECT storage_bytes, storage_measured_at, requests, last_request_at FROM usage
         WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)?
    .unwrap_or_default())
}

pub async fn record_storage(db: &SqlitePool, user_id: &str, bytes: u64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO usage (user_id, storage_bytes, storage_measured_at) VALUES (?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET storage_bytes = excluded.storage_bytes,
             storage_measured_at = excluded.storage_measured_at",
    )
    .bind(user_id)
    .bind(i64::try_from(bytes).unwrap_or(i64::MAX))
    .bind(Utc::now().timestamp())
    .execute(db)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

/// The size of the files under `dir`, without following links.
pub fn directory_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(metadata) if metadata.is_dir() => directory_size(&entry.path()),
            Ok(metadata) if metadata.is_file() => metadata.len(),
            _ => 0,
        })
        .sum()
}

/// Requests per user in the current minute, and the requests not yet written to `usage`.
#[derive(Debug, Default)]
pub struct Counters {
    minutes: Mutex<HashMap<String, (Instant, u64)>>,
    unrecorded: Mutex<HashMap<String, i64>>,
    /// Failed logins by name, in the current ten minutes
    failed_logins: Mutex<HashMap<String, (Instant, u64)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Failed logins a name may have in ten minutes.
const MAX_FAILED_LOGINS: u64 = 10;

impl Counters {
    /// Counts a request of `user_id`. Returns whether it is within `per_minute` (`0`: no limit).
    pub fn request(&self, user_id: &str, per_minute: u64) -> bool {
        *lock(&self.unrecorded)
            .entry(user_id.to_string())
            .or_default() += 1;
        if per_minute == 0 {
            return true;
        }
        let mut minutes = lock(&self.minutes);
        let (start, count) = minutes
            .entry(user_id.to_string())
            .or_insert((Instant::now(), 0));
        if start.elapsed() >= Duration::from_secs(60) {
            *start = Instant::now();
            *count = 0;
        }
        *count += 1;
        *count <= per_minute
    }

    /// Whether `who` may try to log in, after too many failures.
    pub fn may_log_in(&self, who: &str) -> bool {
        let mut failed = lock(&self.failed_logins);
        match failed.get(&who.to_lowercase()) {
            Some((start, _)) if start.elapsed() >= Duration::from_secs(600) => {
                failed.remove(&who.to_lowercase());
                true
            }
            Some((_, count)) => *count < MAX_FAILED_LOGINS,
            None => true,
        }
    }

    pub fn failed_login(&self, who: &str) {
        let mut failed = lock(&self.failed_logins);
        let (_, count) = failed
            .entry(who.to_lowercase())
            .or_insert((Instant::now(), 0));
        *count += 1;
    }

    /// Writes the requests counted since the last time to `usage`.
    pub async fn record(&self, db: &SqlitePool) -> Result<(), Error> {
        let counted = std::mem::take(&mut *lock(&self.unrecorded));
        let now = Utc::now().timestamp();
        for (user_id, requests) in counted {
            sqlx::query(
                "INSERT INTO usage (user_id, requests, last_request_at) VALUES (?, ?, ?)
                 ON CONFLICT (user_id) DO UPDATE SET requests = requests + excluded.requests,
                     last_request_at = excluded.last_request_at",
            )
            .bind(&user_id)
            .bind(requests)
            .bind(now)
            .execute(db)
            .await
            .map_err(sqlx_error)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Method;

    use super::*;

    #[test]
    fn user_overrides_and_where_limits_apply() {
        let host = Limits::default();
        let user = host.with(&LimitOverrides {
            storage_mb: Some(100),
            media_body_mb: Some(5),
            ..LimitOverrides::default()
        });
        assert_eq!(user.storage_bytes(), Some(100 * MB));
        assert_eq!(host.storage_bytes(), None);
        assert_eq!(user.body_limit("/api/images"), Some(5 * MB));
        assert_eq!(user.body_limit("/api/sync/full"), Some(1024 * MB));
        assert_eq!(user.body_limit("/api/notes"), None);
        assert!(Limits::stores("/api/sync/media/image/a.png", &Method::PUT));
        assert!(!Limits::stores("/api/sync/media/image/a.png", &Method::GET));
        assert!(Limits::searches("/api/notes/search/page"));
    }

    #[test]
    fn requests_and_logins_are_counted() {
        let counters = Counters::default();
        assert!(counters.request("a", 2));
        assert!(counters.request("a", 2));
        assert!(!counters.request("a", 2));
        assert!(counters.request("b", 2));
        assert!(counters.request("a", 0));
        for _ in 0..MAX_FAILED_LOGINS {
            assert!(counters.may_log_in("Me"));
            counters.failed_login("me");
        }
        assert!(!counters.may_log_in("ME"));
        assert!(counters.may_log_in("you"));
    }
}
