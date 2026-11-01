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
    let collection = match host.collection(&auth.user.id).await {
        Ok(collection) => collection,
        Err(e) => return error_to_response(e).into_response(),
    };
    request.extensions_mut().insert(Arc::clone(&collection));
    request.extensions_mut().insert(Arc::clone(host));
    request.extensions_mut().insert(auth);
    collection.scope(next.run(request)).await
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
    let Some(user) = accounts::check_password(&host.accounts, &request.user, &request.password)
        .await
        .map_err(error_to_response)?
    else {
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
    let Some(user) = accounts::check_password(&host.accounts, &request.user, &request.password)
        .await
        .map_err(error_to_response)?
    else {
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
