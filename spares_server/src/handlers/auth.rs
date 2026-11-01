//! Logging in and out, device tokens, and the middleware that tells who a request is from (see
//! [`crate::host::auth`]).

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Request;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::header;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

use crate::AppState;
use crate::Coll;
use crate::handlers::error_to_response;
use crate::host::accounts;
use crate::host::accounts::User;
use crate::host::auth;
use crate::host::auth::Device;
use crate::host::auth::Scope;
use crate::host::limits::Limits;

/// Who a request is from, which [`authenticate`] puts in its extensions.
#[derive(Clone, Debug)]
pub struct Auth {
    pub user: User,
    /// The device token it came with, if not the web app's session
    pub device: Option<Device>,
}

type HandlerError = (StatusCode, Json<serde_json::Value>);

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "message": message, "unauthorized": true })),
    )
        .into_response()
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "message": message }))).into_response()
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookies| auth::cookie_value(cookies, auth::SESSION_COOKIE))
        .map(str::to_string)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Lets in a request with a valid device token or session, and runs it in its user's collection:
/// the collection always comes from the token or session, never from the request.
///
/// - A `sync` token only reaches `/api/sync/*`.
/// - With a session, a request that changes something must send its CSRF token.
pub(crate) async fn authenticate(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let host = &state.host;
    let headers = request.headers();
    let auth = if let Some(token) = bearer(headers) {
        match auth::check_token(&host.accounts, token).await {
            Ok(Some((user, device))) => {
                if device.scope == Scope::Sync && !request.uri().path().starts_with("/api/sync/") {
                    return forbidden("this device's token can only sync");
                }
                Auth {
                    user,
                    device: Some(device),
                }
            }
            Ok(None) => return unauthorized("this device's token is not valid; log in again"),
            Err(e) => return error_to_response(e).into_response(),
        }
    } else if let Some(cookie) = session_cookie(headers) {
        match auth::check_session(&host.accounts, &cookie, host.config.session_days).await {
            Ok(Some((user, csrf_token))) => {
                let safe = matches!(
                    *request.method(),
                    Method::GET | Method::HEAD | Method::OPTIONS
                );
                let sent = headers.get(auth::CSRF_HEADER).and_then(|v| v.to_str().ok());
                if !safe && !sent.is_some_and(|sent| auth::constant_time_eq(sent, &csrf_token)) {
                    return forbidden("missing or wrong CSRF token; reload the page");
                }
                Auth { user, device: None }
            }
            Ok(None) => return unauthorized("the session has ended; log in again"),
            Err(e) => return error_to_response(e).into_response(),
        }
    } else {
        return unauthorized("log in first");
    };
    let limits = match host.limits_for(&auth.user.id).await {
        Ok(limits) => limits,
        Err(e) => return error_to_response(e).into_response(),
    };
    let length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    if let Some(refused) = over_limits(host, &auth.user.id, &limits, &path, &method, length).await {
        return refused;
    }
    let collection = match host.collection(&auth.user.id).await {
        Ok(collection) => collection,
        Err(e) => return error_to_response(e).into_response(),
    };
    collection.state.renders_at_once.store(
        usize::try_from(limits.renders_at_once).unwrap_or(usize::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
    let searches = Limits::searches(request.uri().path());
    request.extensions_mut().insert(Arc::clone(&collection));
    request.extensions_mut().insert(Arc::clone(host));
    request.extensions_mut().insert(auth);
    if searches {
        let search_time = limits.search_time();
        return match tokio::time::timeout(search_time, collection.scope(next.run(request))).await {
            Ok(response) => response,
            Err(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "message": format!(
                    "the search took longer than {} seconds; narrow it down",
                    search_time.as_secs()
                ) })),
            )
                .into_response(),
        };
    }
    collection.scope(next.run(request)).await
}

/// Refuses a request past its user's limits (see [`crate::host::limits`]): too many requests a
/// minute, a body too large, or an upload past the storage quota.
async fn over_limits(
    host: &crate::host::Host,
    user_id: &str,
    limits: &Limits,
    path: &str,
    method: &Method,
    length: u64,
) -> Option<Response> {
    let refuse = |status: StatusCode, message: String| {
        Some((status, Json(json!({ "message": message }))).into_response())
    };
    if !host.counters.request(user_id, limits.requests_per_minute) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "more than {} requests a minute; slow down",
                limits.requests_per_minute
            ),
        );
    }
    if let Some(limit) = limits.body_limit(path)
        && length > limit
    {
        return refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("larger than this account's {} MB", limit / 1024 / 1024),
        );
    }
    if Limits::stores(path, method)
        && let Some(quota) = limits.storage_bytes()
    {
        let used = crate::host::limits::usage(&host.accounts, user_id)
            .await
            .map_or(0, |usage| u64::try_from(usage.storage_bytes).unwrap_or(0));
        if used.saturating_add(length) > quota {
            return refuse(
                StatusCode::INSUFFICIENT_STORAGE,
                format!(
                    "this account's {} MB of storage are used up",
                    quota / 1024 / 1024
                ),
            );
        }
    }
    None
}

/// The account `who` names if `password` is its password, refusing a name with too many failed
/// tries lately (see [`crate::host::limits::Counters::may_log_in`]).
async fn check_login(
    host: &crate::host::Host,
    who: &str,
    password: &str,
) -> Result<Option<User>, HandlerError> {
    if !host.counters.may_log_in(who) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "message": "too many failed logins; try again in a few minutes" })),
        ));
    }
    let user = accounts::check_password(&host.accounts, who, password)
        .await
        .map_err(error_to_response)?;
    if user.is_none() {
        host.counters.failed_login(who);
    }
    if let Some(user) = &user
        && host.config.signups == crate::host::Signups::Open
        && user.email.is_some()
        && user.email_verified_at.is_none()
        && !user.admin
    {
        return Err(forbidden_json(
            "verify your email address first, with the link sent to it",
        ));
    }
    Ok(user)
}

#[derive(Debug, Deserialize)]
pub(crate) struct LoginRequest {
    /// The account's name, email or id
    user: String,
    password: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionResponse {
    user: User,
    csrf_token: String,
}

/// `POST /api/auth/login`: starts a session for the web app.
pub(crate) async fn login_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, HandlerError> {
    let host = &state.host;
    let Some(user) = check_login(host, &request.user, &request.password).await? else {
        return Ok(unauthorized("wrong name or password"));
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let session = auth::create_session(
        &host.accounts,
        &user.id,
        user_agent,
        host.config.session_days,
    )
    .await
    .map_err(error_to_response)?;
    let cookie = auth::session_cookie(
        Some(&session.cookie),
        host.config.session_days,
        host.config.secure_cookies(),
    );
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(SessionResponse {
            user,
            csrf_token: session.csrf_token,
        }),
    )
        .into_response())
}

/// `GET /api/auth/session`: the session's user and CSRF token, for the web app when it opens.
pub(crate) async fn session_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, HandlerError> {
    let host = &state.host;
    let Some(cookie) = session_cookie(&headers) else {
        return Ok(unauthorized("log in first"));
    };
    match auth::check_session(&host.accounts, &cookie, host.config.session_days)
        .await
        .map_err(error_to_response)?
    {
        Some((user, csrf_token)) => Ok(Json(SessionResponse { user, csrf_token }).into_response()),
        None => Ok(unauthorized("the session has ended; log in again")),
    }
}

/// `POST /api/auth/logout`: ends the session.
pub(crate) async fn logout_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, HandlerError> {
    if let Some(cookie) = session_cookie(&headers) {
        auth::delete_session(&state.host.accounts, &cookie)
            .await
            .map_err(error_to_response)?;
    }
    let cookie = auth::session_cookie(None, 0, state.host.config.secure_cookies());
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({}))).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct TokenRequest {
    user: String,
    password: String,
    /// What the device is called in its user's list of devices
    name: String,
    #[serde(default = "all")]
    scope: Scope,
}

fn all() -> Scope {
    Scope::All
}

#[derive(Debug, Serialize)]
pub(crate) struct TokenResponse {
    token: String,
    device: Device,
    user: User,
}

/// `POST /api/auth/token`: a device token, for `spares login`.
pub(crate) async fn token_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<TokenRequest>,
) -> Result<Response, HandlerError> {
    let host = &state.host;
    let Some(user) = check_login(host, &request.user, &request.password).await? else {
        return Ok(unauthorized("wrong name or password"));
    };
    let (device, token) =
        auth::create_token(&host.accounts, &user.id, &request.name, request.scope)
            .await
            .map_err(error_to_response)?;
    Ok(Json(TokenResponse {
        token,
        device,
        user,
    })
    .into_response())
}

#[derive(Debug, Serialize)]
pub(crate) struct MeResponse {
    user: User,
    device: Option<Device>,
    /// The collection's directories on the host, for a CLI on the same machine
    data_dir: std::path::PathBuf,
    files_dir: std::path::PathBuf,
}

/// `GET /api/me`: who the request is from.
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn me_handler(
    axum::Extension(auth): axum::Extension<Auth>,
    Coll(collection): Coll,
) -> Json<MeResponse> {
    Json(MeResponse {
        user: auth.user,
        device: auth.device,
        data_dir: collection.paths.root.clone(),
        files_dir: collection.paths.files.clone(),
    })
}

/// `GET /api/auth/devices`: the user's device tokens.
pub(crate) async fn list_devices_handler(
    State(state): State<Arc<AppState>>,
    axum::Extension(auth): axum::Extension<Auth>,
) -> Result<Json<Vec<Device>>, HandlerError> {
    Ok(Json(
        auth::devices(&state.host.accounts, &auth.user.id)
            .await
            .map_err(error_to_response)?,
    ))
}

/// `DELETE /api/auth/devices/{id}`: revokes a device's token, and frees the replica it enrolled as.
pub(crate) async fn revoke_device_handler(
    State(state): State<Arc<AppState>>,
    axum::Extension(auth): axum::Extension<Auth>,
    Coll(collection): Coll,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, HandlerError> {
    let revoked = auth::revoke_token(&state.host.accounts, &auth.user.id, id)
        .await
        .map_err(error_to_response)?;
    let Some(revoked) = revoked else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "message": format!("you have no device {id}") })),
        ));
    };
    if let Some(replica) = revoked.replica {
        crate::handlers::sync::free_replica(&collection, replica)
            .await
            .map_err(error_to_response)?;
    }
    Ok(Json(json!({ "revoked": revoked })))
}

#[derive(Debug, Deserialize)]
pub(crate) struct PasswordRequest {
    current: String,
    new: String,
}

/// `POST /api/auth/password`: changes the password, and ends the user's other sessions.
pub(crate) async fn password_handler(
    State(state): State<Arc<AppState>>,
    axum::Extension(auth): axum::Extension<Auth>,
    headers: HeaderMap,
    Json(request): Json<PasswordRequest>,
) -> Result<Json<serde_json::Value>, HandlerError> {
    let db = &state.host.accounts;
    if accounts::check_password(db, &auth.user.id, &request.current)
        .await
        .map_err(error_to_response)?
        .is_none()
    {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "the current password is wrong" })),
        ));
    }
    accounts::set_password(db, &auth.user.id, &request.new)
        .await
        .map_err(error_to_response)?;
    let cookie = session_cookie(&headers);
    auth::delete_sessions(db, &auth.user.id, cookie.as_deref())
        .await
        .map_err(error_to_response)?;
    Ok(Json(json!({})))
}

#[derive(Debug, Serialize)]
pub(crate) struct SignupsResponse {
    signups: crate::host::Signups,
    /// Whether the host sends email, which resetting a forgotten password needs
    email: bool,
}

/// `GET /api/auth/signups`: who can create an account here, for the login page.
#[allow(clippy::unused_async, reason = "an axum handler")]
pub(crate) async fn signups_handler(State(state): State<Arc<AppState>>) -> Json<SignupsResponse> {
    Json(SignupsResponse {
        signups: state.host.config.signups,
        email: state.host.config.email.command.is_some(),
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct SignupRequest {
    name: String,
    email: Option<String>,
    password: String,
    invite: Option<String>,
}

fn forbidden_json(message: &str) -> HandlerError {
    (StatusCode::FORBIDDEN, Json(json!({ "message": message })))
}

/// `POST /api/auth/signup`: creates an account, as `signups` in `host.toml` allows (see
/// [`crate::host::signup`]). With an invite, it logs the new account in; with open signups, it
/// emails a link to verify the address first.
pub(crate) async fn signup_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SignupRequest>,
) -> Result<Response, HandlerError> {
    use crate::host::Signups;
    use crate::host::signup;
    let host = &state.host;
    let email = request
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    match host.config.signups {
        Signups::Closed => return Err(forbidden_json("this host takes no new accounts")),
        Signups::Invite => {
            let valid = match &request.invite {
                Some(code) => signup::invite_is_valid(&host.accounts, code)
                    .await
                    .map_err(error_to_response)?,
                None => false,
            };
            if !valid {
                return Err(forbidden_json(
                    "this host takes new accounts by invite only",
                ));
            }
        }
        Signups::Open if email.is_none() => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "an email address is needed, to verify" })),
            ));
        }
        Signups::Open => {}
    }
    let user = host
        .create_user(accounts::NewUser {
            id: None,
            name: &request.name,
            email,
            password: &request.password,
            admin: false,
            email_verified: false,
        })
        .await
        .map_err(error_to_response)?;
    if host.config.signups == Signups::Invite {
        let code = request.invite.as_deref().unwrap_or_default();
        if !signup::use_invite(&host.accounts, code, &user.id)
            .await
            .map_err(error_to_response)?
        {
            // Used by another signup at the same time
            let _ = host.delete_user(&user.id).await;
            return Err(forbidden_json("the invite has been used"));
        }
        return start_session(host, user, &headers).await;
    }
    let token = signup::create_email_token(&host.accounts, &user.id, signup::Purpose::Verify)
        .await
        .map_err(error_to_response)?;
    let link = format!("{}/api/auth/verify?token={token}", host.config.web_origin());
    let sent = signup::send_email(
        &host.config.email,
        email.unwrap_or_default(),
        "Verify your email for spares",
        &format!(
            "Open this link to start using your account {}:\n\n{link}\n",
            user.name
        ),
    );
    if let Err(e) = sent {
        let _ = host.delete_user(&user.id).await;
        return Err(error_to_response(e));
    }
    Ok(
        Json(json!({ "verify": true, "message": "Check your email for a link to verify it." }))
            .into_response(),
    )
}

async fn start_session(
    host: &crate::host::Host,
    user: User,
    headers: &HeaderMap,
) -> Result<Response, HandlerError> {
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let session = auth::create_session(
        &host.accounts,
        &user.id,
        user_agent,
        host.config.session_days,
    )
    .await
    .map_err(error_to_response)?;
    let cookie = auth::session_cookie(
        Some(&session.cookie),
        host.config.session_days,
        host.config.secure_cookies(),
    );
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(SessionResponse {
            user,
            csrf_token: session.csrf_token,
        }),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct TokenQuery {
    token: String,
}

/// `GET /api/auth/verify?token=…`: the link a new account's email gets. Goes on to the login page.
pub(crate) async fn verify_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(TokenQuery { token }): axum::extract::Query<TokenQuery>,
) -> Result<Response, HandlerError> {
    use crate::host::signup;
    let db = &state.host.accounts;
    let user = signup::use_email_token(db, &token, signup::Purpose::Verify)
        .await
        .map_err(error_to_response)?;
    let Some(user) = user else {
        return Err(forbidden_json("this link has expired or been used"));
    };
    signup::mark_verified(db, &user)
        .await
        .map_err(error_to_response)?;
    Ok(axum::response::Redirect::to("/login?verified=1").into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResetRequest {
    email: String,
}

/// `POST /api/auth/reset-request`: emails a link to reset the password of the account with this
/// address, if there is one. The answer is the same either way.
pub(crate) async fn reset_request_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ResetRequest>,
) -> Result<Json<serde_json::Value>, HandlerError> {
    use crate::host::signup;
    let host = &state.host;
    let answer = Json(json!({ "message": "If an account has that address, it got an email." }));
    let Some(user) = accounts::find_user(&host.accounts, &request.email)
        .await
        .map_err(error_to_response)?
        .filter(|user| {
            user.email
                .as_deref()
                .is_some_and(|email| email.eq_ignore_ascii_case(request.email.trim()))
        })
    else {
        return Ok(answer);
    };
    let token = signup::create_email_token(&host.accounts, &user.id, signup::Purpose::Reset)
        .await
        .map_err(error_to_response)?;
    let link = format!("{}/reset-password?token={token}", host.config.web_origin());
    if let Err(e) = signup::send_email(
        &host.config.email,
        user.email.as_deref().unwrap_or_default(),
        "Reset your spares password",
        &format!(
            "Open this link to choose a new password for {}:\n\n{link}\n\nIt works for a day.\n",
            user.name
        ),
    ) {
        log::warn!("Failed to send a password reset: {e}");
    }
    Ok(answer)
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResetPasswordRequest {
    token: String,
    password: String,
}

/// `POST /api/auth/reset`: a new password, with the token from the email. Ends every session.
pub(crate) async fn reset_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, HandlerError> {
    use crate::host::signup;
    let db = &state.host.accounts;
    // Checked before the token is used up
    accounts::hash_password(&request.password).map_err(error_to_response)?;
    let Some(user) = signup::use_email_token(db, &request.token, signup::Purpose::Reset)
        .await
        .map_err(error_to_response)?
    else {
        return Err(forbidden_json("this link has expired or been used"));
    };
    accounts::set_password(db, &user, &request.password)
        .await
        .map_err(error_to_response)?;
    // The email reached its owner, so the address is theirs.
    signup::mark_verified(db, &user)
        .await
        .map_err(error_to_response)?;
    auth::delete_sessions(db, &user, None)
        .await
        .map_err(error_to_response)?;
    Ok(Json(json!({})))
}

/// `GET /api/account/export`: the user's whole directory, collection and all, as a `.tar.gz`
/// that another host imports (see [`crate::host::export`]).
pub(crate) async fn export_account_handler(
    State(state): State<Arc<AppState>>,
    axum::Extension(auth): axum::Extension<Auth>,
) -> Result<Response, HandlerError> {
    let host = &state.host;
    let out = host.user_dir(&auth.user.id).join("cache").join(format!(
        "export-{}.tar.gz",
        crate::host::accounts::random_string(8)
    ));
    crate::host::export::export(host, &auth.user.id, &out)
        .await
        .map_err(error_to_response)?;
    let bytes = tokio::fs::read(&out).await;
    let _ = tokio::fs::remove_file(&out).await;
    let bytes = bytes.map_err(|e| {
        error_to_response(spares_core::Error::Io {
            description: "Failed to read the export".to_string(),
            source: e,
        })
    })?;
    let name = format!(
        "attachment; filename=\"spares-{}-{}.tar.gz\"",
        auth.user.name,
        chrono::Utc::now().format("%Y%m%d")
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/gzip".to_string()),
            (header::CONTENT_DISPOSITION, name),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct DeleteAccountRequest {
    password: String,
}

/// `DELETE /api/account`: deletes the account and its directory, collection and all, given its
/// password.
pub(crate) async fn delete_account_handler(
    State(state): State<Arc<AppState>>,
    axum::Extension(auth): axum::Extension<Auth>,
    Json(request): Json<DeleteAccountRequest>,
) -> Result<Response, HandlerError> {
    let host = &state.host;
    if check_login(host, &auth.user.id, &request.password)
        .await?
        .is_none()
    {
        return Err(forbidden_json("the password is wrong"));
    }
    host.sync.stop(&auth.user.id);
    host.delete_user(&auth.user.id)
        .await
        .map_err(error_to_response)?;
    let cookie = auth::session_cookie(None, 0, host.config.secure_cookies());
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({}))).into_response())
}

/// `GET /api/metrics`: the host's numbers, in Prometheus' text format, for its admins.
pub(crate) async fn metrics_handler(
    State(state): State<Arc<AppState>>,
    axum::Extension(auth): axum::Extension<Auth>,
) -> Result<Response, HandlerError> {
    if !auth.user.admin {
        return Err(forbidden_json("only the host's admins see its metrics"));
    }
    let host = &state.host;
    let (accounts, requests, storage): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM user), COALESCE((SELECT SUM(requests) FROM usage), 0),
                COALESCE((SELECT SUM(storage_bytes) FROM usage), 0)",
    )
    .fetch_one(&host.accounts)
    .await
    .map_err(|e| error_to_response(spares_core::Error::Sqlx { source: e }))?;
    let open = host.open_collections().await.len();
    let body = format!(
        "# TYPE spares_accounts gauge\nspares_accounts {accounts}\n\
         # TYPE spares_open_collections gauge\nspares_open_collections {open}\n\
         # TYPE spares_requests_total counter\nspares_requests_total {requests}\n\
         # TYPE spares_storage_bytes gauge\nspares_storage_bytes {storage}\n"
    );
    Ok(([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response())
}
