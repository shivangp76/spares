//! A host whose copy of a collection has an upstream relays its devices' sync to the upstream, and
//! every host checks that a device speaks its sync protocol.
//!
//! **Why relay.** The protocol's server never merges and keeps no merge bases: a pull stamps its
//! changes with the server's counter and drops the bases (see `spares_core::sync::server`). A copy
//! that is a client of its upstream needs those bases and its own dirty rows, so it can't also be
//! the server of its devices. So a device of such a host (a browser keeping a copy on the laptop)
//! syncs with the root through it: the host sends its requests to the upstream with its own token
//! and returns the answers. Its notes reach the host at the host's next sync with the upstream,
//! like any other device's. Enrolling and unenrolling are relayed too, so the root gives out the
//! numbers. Files (rendered files, config, media) are the host's own and aren't relayed; a device's
//! uploaded image is also sent on to the upstream.
//!
//! **Loops.** Each request a host sends on names it in `SYNC_VIA_HEADER`, and a host refuses one
//! that names it ([`refuse_loops`]), so an upstream that leads back to the host fails at once.

use axum::body::Body;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use serde_json::json;
use spares_core::config::read_external_config;
use spares_core::sync::protocol::SYNC_PROTOCOL_HEADER;
use spares_core::sync::protocol::SYNC_PROTOCOL_VERSION;
use spares_core::sync::protocol::SYNC_VIA_HEADER;
use spares_core::sync::protocol::host_id;

use crate::handlers::sync::SYNC_BODY_LIMIT;

/// Refuses a sync request without this host's [`SYNC_PROTOCOL_VERSION`], e.g. from a device of an
/// older spares.
pub(crate) async fn require_protocol(request: Request, next: Next) -> Response {
    let sent = request
        .headers()
        .get(SYNC_PROTOCOL_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok());
    if sent == Some(SYNC_PROTOCOL_VERSION) {
        return next.run(request).await;
    }
    let message = match sent {
        Some(version) => format!(
            "this host speaks sync protocol {SYNC_PROTOCOL_VERSION}, and the device {version}. \
             Update spares on the older side."
        ),
        None => format!(
            "this host speaks sync protocol {SYNC_PROTOCOL_VERSION}, which the device doesn't. \
             Update spares on it."
        ),
    };
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({ "message": message, "protocol_version": SYNC_PROTOCOL_VERSION })),
    )
        .into_response()
}

/// Refuses a sync request that has already come through this host (see [`SYNC_VIA_HEADER`]).
/// Without it, a root whose `[sync] url` points at itself, as a copied config.toml can make it,
/// relays each request to itself until it runs out of open files, and then answers nothing.
pub(crate) async fn refuse_loops(request: Request, next: Next) -> Response {
    let looped = request
        .headers()
        .get(SYNC_VIA_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|via| {
            let id = host_id();
            via.split(',').any(|sent| sent.trim() == id)
        });
    if !looped {
        return next.run(request).await;
    }
    let url = upstream().map_or_else(|| "its upstream".to_string(), |(url, _)| url);
    let message = format!(
        "this host's upstream ({url}, `[sync] url` in the collection's config.toml) leads back to \
         this host. Remove `url` from the root's [sync] table."
    );
    (
        StatusCode::LOOP_DETECTED,
        axum::Json(json!({ "message": message, "loop": true })),
    )
        .into_response()
}

/// [`SYNC_VIA_HEADER`] for a request sent on from this host: the hosts it came through, then this.
pub(crate) fn via(headers: &axum::http::HeaderMap) -> String {
    match headers.get(SYNC_VIA_HEADER).and_then(|v| v.to_str().ok()) {
        Some(via) if !via.is_empty() => format!("{via},{}", host_id()),
        _ => host_id(),
    }
}

/// The current collection's upstream and its token there, if it has one.
pub(crate) fn upstream() -> Option<(String, Option<String>)> {
    let sync = read_external_config().ok()?.sync;
    Some((sync.url?.trim_end_matches('/').to_string(), sync.token))
}

/// Sends the request to the collection's upstream, if it has one (see the [module docs](self)).
pub(crate) async fn relay_to_upstream(request: Request, next: Next) -> Response {
    let Some((url, token)) = upstream() else {
        return next.run(request).await;
    };
    match relay(&url, token.as_deref(), request).await {
        Ok(response) => response,
        Err(message) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({ "message": message, "unreachable": true })),
        )
            .into_response(),
    }
}

/// Sends `request` to `url` with `token`, returning the upstream's answer.
pub(crate) async fn relay(
    url: &str,
    token: Option<&str>,
    request: Request,
) -> Result<Response, String> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_string(), ToString::to_string);
    let body = axum::body::to_bytes(body, SYNC_BODY_LIMIT)
        .await
        .map_err(|e| format!("Failed to read the request: {e}"))?;
    let method =
        reqwest::Method::from_bytes(parts.method.as_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut upstream = reqwest::Client::new()
        .request(method, format!("{url}{path}"))
        .header(SYNC_PROTOCOL_HEADER, SYNC_PROTOCOL_VERSION.to_string())
        .header(SYNC_VIA_HEADER, via(&parts.headers))
        .timeout(std::time::Duration::from_secs(600))
        .body(body.to_vec());
    if let Some(content_type) = parts.headers.get(header::CONTENT_TYPE) {
        upstream = upstream.header(header::CONTENT_TYPE.as_str(), content_type.as_bytes());
    }
    if let Some(token) = token {
        upstream = upstream.bearer_auth(token);
    }
    let answer = upstream
        .send()
        .await
        .map_err(|e| format!("This host's upstream {url} can't be reached: {e}"))?;
    let status = StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = answer
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| HeaderValue::from_bytes(v.as_bytes()).ok());
    let bytes = answer
        .bytes()
        .await
        .map_err(|e| format!("Failed to read {url}'s answer: {e}"))?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(response)
}
