//! Who a request is from: the web app's cookie sessions, and devices' bearer tokens.
//!
//! - **Sessions** are for the web app. Logging in with a name and password
//!   (`POST /api/auth/login`) sets the `spares_session` cookie: `HttpOnly`, `SameSite=Lax`, and
//!   `Secure` when the host's `public_url` is `https`. A session lasts `session_days` from its
//!   last use, so a browser is asked for the password about as often as it is new. Requests that
//!   change something must also send the session's CSRF token in `X-CSRF-Token`: another site can
//!   make a browser send the cookie, but can't read the token.
//! - **Device tokens** are for the CLI and for other hosts' copies of a collection
//!   (`Authorization: Bearer spt_…`), from `spares login` or `spares host token`. Each can be
//!   revoked on its own, and a `sync` token only reaches `/api/sync/*`.
//!
//! Only blake3 hashes of sessions and tokens are stored: they are random and long, so a slow hash
//! like the passwords' adds nothing.

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use spares_core::Error;
use sqlx::SqlitePool;

use super::accounts::User;
use super::accounts::invalid;
use super::accounts::random_string;
use super::accounts::sqlx_error;

pub const SESSION_COOKIE: &str = "spares_session";
pub const CSRF_HEADER: &str = "x-csrf-token";
const TOKEN_PREFIX: &str = "spt_";
const SESSION_PREFIX: &str = "sps_";
/// How often a token's or session's last use is written, at most
const TOUCH_EVERY_SECONDS: i64 = 60;

fn hash(secret: &str) -> String {
    blake3::hash(secret.as_bytes()).to_hex().to_string()
}

/// What a device token may do.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Scope {
    /// Everything its user can
    All,
    /// Only `/api/sync/*`: another host syncing its copy of the collection
    Sync,
}

/// A device token, without its secret.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Device {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub scope: Scope,
    pub replica: Option<u8>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: i64,
    user_id: String,
    name: String,
    scope: String,
    replica: Option<i64>,
    created_at: i64,
    last_used_at: Option<i64>,
}

impl From<DeviceRow> for Device {
    fn from(row: DeviceRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            name: row.name,
            scope: row.scope.parse().unwrap_or(Scope::Sync),
            replica: row.replica.and_then(|r| u8::try_from(r).ok()),
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        }
    }
}

const DEVICE_COLUMNS: &str = "id, user_id, name, scope, replica, created_at, last_used_at";

/// Creates a device token for `user_id`, and returns it with its secret, which isn't stored.
pub async fn create_token(
    db: &SqlitePool,
    user_id: &str,
    name: &str,
    scope: Scope,
) -> Result<(Device, String), Error> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(invalid("a device needs a name of up to 100 characters"));
    }
    let secret = format!("{TOKEN_PREFIX}{}", random_string(40));
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO device_token (user_id, name, token_hash, scope, created_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash(&secret))
    .bind(scope.to_string())
    .bind(Utc::now().timestamp())
    .fetch_one(db)
    .await
    .map_err(sqlx_error)?;
    let device = device(db, id)
        .await?
        .ok_or_else(|| invalid("the new token is gone"))?;
    Ok((device, secret))
}

pub async fn device(db: &SqlitePool, id: i64) -> Result<Option<Device>, Error> {
    let row: Option<DeviceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DEVICE_COLUMNS} FROM device_token WHERE id = ? AND revoked_at IS NULL"
    )))
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)?;
    Ok(row.map(Device::from))
}

/// A user's devices that haven't been revoked.
pub async fn devices(db: &SqlitePool, user_id: &str) -> Result<Vec<Device>, Error> {
    let rows: Vec<DeviceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DEVICE_COLUMNS} FROM device_token WHERE user_id = ? AND revoked_at IS NULL
         ORDER BY id"
    )))
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(sqlx_error)?;
    Ok(rows.into_iter().map(Device::from).collect())
}

/// Revokes one of `user_id`'s tokens. Returns it, if it was theirs and not revoked yet, so its
/// replica can be freed.
pub async fn revoke_token(
    db: &SqlitePool,
    user_id: &str,
    id: i64,
) -> Result<Option<Device>, Error> {
    let Some(device) = device(db, id).await?.filter(|d| d.user_id == user_id) else {
        return Ok(None);
    };
    sqlx::query("UPDATE device_token SET revoked_at = ? WHERE id = ?")
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(Some(device))
}

/// Records the replica a device enrolled as, or that it has none.
pub async fn set_token_replica(db: &SqlitePool, id: i64, replica: Option<u8>) -> Result<(), Error> {
    sqlx::query("UPDATE device_token SET replica = ? WHERE id = ?")
        .bind(replica.map(i64::from))
        .bind(id)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// The user and device a bearer token belongs to, if it is valid.
pub async fn check_token(db: &SqlitePool, secret: &str) -> Result<Option<(User, Device)>, Error> {
    if !secret.starts_with(TOKEN_PREFIX) {
        return Ok(None);
    }
    let row: Option<DeviceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {DEVICE_COLUMNS} FROM device_token WHERE token_hash = ? AND revoked_at IS NULL"
    )))
    .bind(hash(secret))
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)?;
    let Some(device) = row.map(Device::from) else {
        return Ok(None);
    };
    let Some(user) = active_user(db, &device.user_id).await? else {
        return Ok(None);
    };
    let now = Utc::now().timestamp();
    if device
        .last_used_at
        .is_none_or(|at| now - at >= TOUCH_EVERY_SECONDS)
    {
        sqlx::query("UPDATE device_token SET last_used_at = ? WHERE id = ?")
            .bind(now)
            .bind(device.id)
            .execute(db)
            .await
            .map_err(sqlx_error)?;
    }
    Ok(Some((user, device)))
}

async fn active_user(db: &SqlitePool, id: &str) -> Result<Option<User>, Error> {
    Ok(super::accounts::user(db, id)
        .await?
        .filter(|user| user.disabled_at.is_none()))
}

/// A session, with its secrets: the cookie's value and the CSRF token.
#[derive(Clone, Debug)]
pub struct NewSession {
    pub cookie: String,
    pub csrf_token: String,
    pub expires_at: i64,
}

pub async fn create_session(
    db: &SqlitePool,
    user_id: &str,
    user_agent: Option<&str>,
    days: u64,
) -> Result<NewSession, Error> {
    let cookie = format!("{SESSION_PREFIX}{}", random_string(40));
    let csrf_token = random_string(32);
    let now = Utc::now().timestamp();
    let expires_at = now + lifetime(days);
    sqlx::query(
        "INSERT INTO session
         (token_hash, user_id, csrf_token, user_agent, created_at, last_used_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(hash(&cookie))
    .bind(user_id)
    .bind(&csrf_token)
    .bind(user_agent.map(|agent| agent.chars().take(200).collect::<String>()))
    .bind(now)
    .bind(now)
    .bind(expires_at)
    .execute(db)
    .await
    .map_err(sqlx_error)?;
    Ok(NewSession {
        cookie,
        csrf_token,
        expires_at,
    })
}

fn lifetime(days: u64) -> i64 {
    i64::try_from(days.max(1).saturating_mul(86_400)).unwrap_or(i64::MAX / 2)
}

/// A valid session's user and CSRF token, moving its expiry on.
pub async fn check_session(
    db: &SqlitePool,
    cookie: &str,
    days: u64,
) -> Result<Option<(User, String)>, Error> {
    if !cookie.starts_with(SESSION_PREFIX) {
        return Ok(None);
    }
    let now = Utc::now().timestamp();
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT user_id, csrf_token, last_used_at FROM session
         WHERE token_hash = ? AND expires_at > ?",
    )
    .bind(hash(cookie))
    .bind(now)
    .fetch_optional(db)
    .await
    .map_err(sqlx_error)?;
    let Some((user_id, csrf_token, last_used_at)) = row else {
        return Ok(None);
    };
    let Some(user) = active_user(db, &user_id).await? else {
        return Ok(None);
    };
    if now - last_used_at >= TOUCH_EVERY_SECONDS {
        sqlx::query("UPDATE session SET last_used_at = ?, expires_at = ? WHERE token_hash = ?")
            .bind(now)
            .bind(now + lifetime(days))
            .bind(hash(cookie))
            .execute(db)
            .await
            .map_err(sqlx_error)?;
    }
    Ok(Some((user, csrf_token)))
}

pub async fn delete_session(db: &SqlitePool, cookie: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM session WHERE token_hash = ?")
        .bind(hash(cookie))
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// Ends every session of a user, e.g. after their password changes. Keeps `except`'s.
pub async fn delete_sessions(
    db: &SqlitePool,
    user_id: &str,
    except: Option<&str>,
) -> Result<(), Error> {
    sqlx::query("DELETE FROM session WHERE user_id = ? AND token_hash IS NOT ?")
        .bind(user_id)
        .bind(except.map(hash))
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// Removes expired sessions.
pub async fn prune_sessions(db: &SqlitePool) -> Result<(), Error> {
    sqlx::query("DELETE FROM session WHERE expires_at <= ?")
        .bind(Utc::now().timestamp())
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// The value of cookie `name` in a `Cookie` header.
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

/// A `Set-Cookie` value for the session cookie; `None` clears it.
pub fn session_cookie(value: Option<&str>, max_age_days: u64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    match value {
        Some(value) => format!(
            "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            lifetime(max_age_days)
        ),
        None => format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}"),
    }
}

/// Whether `a` and `b` are equal, taking as long whatever they are.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0_u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::tests::test_host;

    #[tokio::test]
    async fn tokens_and_sessions() {
        let (host, _root) = test_host().await;
        let db = &host.accounts;
        let (device, secret) = create_token(db, "me", "laptop CLI", Scope::All)
            .await
            .unwrap();
        assert!(secret.starts_with(TOKEN_PREFIX));
        let (user, checked) = check_token(db, &secret).await.unwrap().unwrap();
        assert_eq!(user.id, "me");
        assert_eq!(checked.id, device.id);
        assert!(check_token(db, "spt_wrong").await.unwrap().is_none());
        assert!(check_token(db, &secret[1..]).await.unwrap().is_none());
        set_token_replica(db, device.id, Some(0)).await.unwrap();
        assert_eq!(devices(db, "me").await.unwrap()[0].replica, Some(0));
        assert!(
            revoke_token(db, "someone else", device.id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            revoke_token(db, "me", device.id)
                .await
                .unwrap()
                .map(|d| d.replica),
            Some(Some(0))
        );
        assert!(check_token(db, &secret).await.unwrap().is_none());

        let session = create_session(db, "me", Some("Firefox"), 30).await.unwrap();
        let (user, csrf) = check_session(db, &session.cookie, 30)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.id, "me");
        assert_eq!(csrf, session.csrf_token);
        assert!(check_session(db, "sps_nope", 30).await.unwrap().is_none());
        delete_sessions(db, "me", Some(&session.cookie))
            .await
            .unwrap();
        assert!(
            check_session(db, &session.cookie, 30)
                .await
                .unwrap()
                .is_some()
        );
        delete_session(db, &session.cookie).await.unwrap();
        assert!(
            check_session(db, &session.cookie, 30)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cookies() {
        let header = "a=1; spares_session=sps_abc; b=2";
        assert_eq!(cookie_value(header, SESSION_COOKIE), Some("sps_abc"));
        assert_eq!(cookie_value(header, "c"), None);
        assert!(session_cookie(Some("x"), 1, true).contains("Secure"));
        assert!(session_cookie(None, 1, false).contains("Max-Age=0"));
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
    }
}
