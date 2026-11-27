//! `/api/language-tools`: the dictionaries of the `markdown-language-tools` parser (see
//! `spares_core::parsers::language_tools::dictionary`), each user's own.
//!
//! - `GET /lookup?text=&language=`: the entries `text` starts with.
//! - `GET /dictionaries`, and `POST /dictionaries?language=` with a Yomitan zip as the body.
//! - `PATCH /dictionaries/{id}` (`enabled`, `language`), `DELETE /dictionaries/{id}`.
//! - `PUT /dictionaries/order` with `{"ids": […]}`.
//!
//! Only on a host: a browser's copy sends them to its host.

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::routing::patch;
use axum::routing::put;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use spares_core::parsers::language_tools::dictionary;
use spares_core::parsers::language_tools::dictionary::DictionaryError;
use spares_core::parsers::language_tools::dictionary::UpdateDictionaryRequest;
use spares_core::parsers::language_tools::dictionary::lookup::LookupRequest;

use crate::Coll;

/// A dictionary zip's largest size. Jitendex, one of the largest, is about 100 MB.
const DICTIONARY_BODY_LIMIT: usize = 512 * 1024 * 1024;

type HandlerError = (StatusCode, Json<Value>);

#[allow(clippy::needless_pass_by_value, reason = "for `map_err`")]
fn to_response(error: DictionaryError) -> HandlerError {
    let status = match error {
        DictionaryError::Invalid(_) => StatusCode::BAD_REQUEST,
        DictionaryError::NotFound(_) => StatusCode::NOT_FOUND,
        DictionaryError::Sqlx(_) | DictionaryError::Io { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({ "message": error.to_string() })))
}

pub(crate) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/language-tools/lookup", get(lookup_handler))
        .route(
            "/api/language-tools/dictionaries",
            get(list_handler)
                .post(import_handler)
                .layer(DefaultBodyLimit::max(DICTIONARY_BODY_LIMIT)),
        )
        .route(
            "/api/language-tools/dictionaries/order",
            put(reorder_handler),
        )
        .route(
            "/api/language-tools/dictionaries/{id}",
            patch(update_handler).delete(remove_handler),
        )
}

async fn lookup_handler(
    Coll(data): Coll,
    Query(request): Query<LookupRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let pool = dictionary::open(false).await.map_err(to_response)?;
    let mut response = dictionary::lookup::lookup(pool.as_ref(), &request)
        .await
        .map_err(to_response)?;
    dictionary::lookup::mark_notes(&data.db, &mut response)
        .await
        .map_err(to_response)?;
    Ok(Json(response))
}

async fn list_handler() -> Result<impl IntoResponse, HandlerError> {
    let dictionaries = match dictionary::open(false).await.map_err(to_response)? {
        Some(pool) => dictionary::list(&pool).await.map_err(to_response)?,
        None => Vec::new(),
    };
    Ok(Json(dictionaries))
}

#[derive(Deserialize)]
struct ImportQuery {
    #[serde(default)]
    language: Option<String>,
}

async fn import_handler(
    Query(query): Query<ImportQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, HandlerError> {
    let pool = dictionary::open(true)
        .await
        .map_err(to_response)?
        .expect("created");
    let info = dictionary::import::import(&pool, &body, query.language.as_deref())
        .await
        .map_err(to_response)?;
    Ok(Json(info))
}

async fn existing_pool() -> Result<sqlx::SqlitePool, HandlerError> {
    dictionary::open(false)
        .await
        .map_err(to_response)?
        .ok_or_else(|| {
            to_response(DictionaryError::Invalid(
                "No dictionaries have been imported".to_string(),
            ))
        })
}

async fn update_handler(
    Path(id): Path<i64>,
    Json(request): Json<UpdateDictionaryRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let pool = existing_pool().await?;
    let info = dictionary::update(&pool, id, &request)
        .await
        .map_err(to_response)?;
    Ok(Json(info))
}

async fn remove_handler(Path(id): Path<i64>) -> Result<impl IntoResponse, HandlerError> {
    let pool = existing_pool().await?;
    dictionary::remove(&pool, id).await.map_err(to_response)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct ReorderRequest {
    ids: Vec<i64>,
}

async fn reorder_handler(
    Json(request): Json<ReorderRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let pool = existing_pool().await?;
    let dictionaries = dictionary::reorder(&pool, &request.ids)
        .await
        .map_err(to_response)?;
    Ok(Json(dictionaries))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::AppState;
    use crate::host::tests::test_host;
    use crate::route::create_router;

    fn zip(index: &Value, terms: &Value) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("index.json", options).unwrap();
        zip.write_all(index.to_string().as_bytes()).unwrap();
        zip.start_file("term_bank_1.json", options).unwrap();
        zip.write_all(terms.to_string().as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    async fn respond(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or_default())
    }

    /// Import, look up, change and remove a dictionary through the server's router, as a user.
    #[tokio::test]
    #[allow(clippy::too_many_lines, reason = "one request after another")]
    async fn test_dictionary_routes() {
        let (host, _root) = test_host().await;
        let collection = host.collection("me").await.unwrap();
        let router = create_router(Arc::new(AppState { host }), None);
        let (status, body) = respond(
            &router,
            Request::post("/api/auth/token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "user": "me", "password": "password1", "name": "test" }).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let token = format!("Bearer {}", body["token"].as_str().unwrap());
        let request = |method: &str, uri: &str, body: Body| {
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", &token)
                .header("content-type", "application/json")
                .body(body)
                .unwrap()
        };

        // Nothing imported yet
        let (status, body) = respond(
            &router,
            request("GET", "/api/language-tools/dictionaries", Body::empty()),
        )
        .await;
        assert_eq!((status, body), (StatusCode::OK, json!([])));
        let lookup = "/api/language-tools/lookup?text=%E9%A3%9F%E3%81%B9%E3%81%9F&language=ja";
        let (status, body) = respond(&router, request("GET", lookup, Body::empty())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["entries"], json!([]));

        let dictionary = zip(
            &json!({"title": "Tiny", "revision": "1", "attribution": "CC BY-SA"}),
            &json!([["食べる", "たべる", "v1", "v1", 1, ["to eat"], 1, ""]]),
        );
        let (status, body) = respond(
            &router,
            request(
                "POST",
                "/api/language-tools/dictionaries?language=ja",
                Body::from(dictionary),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["id"].as_i64().unwrap();
        assert_eq!(body["language"], "ja");
        // In the user's own collection directory
        assert!(
            collection
                .paths
                .root
                .join("language-tools/dictionaries.sqlite")
                .is_file()
        );

        let (_, body) = respond(&router, request("GET", lookup, Body::empty())).await;
        assert_eq!(body["matched"], 3);
        assert_eq!(body["entries"][0]["expression"], "食べる");
        assert_eq!(body["entries"][0]["inflections"], json!(["past"]));

        let (status, body) = respond(
            &router,
            request(
                "PATCH",
                &format!("/api/language-tools/dictionaries/{id}"),
                Body::from(json!({"enabled": false}).to_string()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["enabled"], false);
        let (_, body) = respond(&router, request("GET", lookup, Body::empty())).await;
        assert_eq!(body["entries"], json!([]));

        let (status, _) = respond(
            &router,
            request(
                "POST",
                "/api/language-tools/dictionaries",
                Body::from("not a zip"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = respond(
            &router,
            request(
                "DELETE",
                &format!("/api/language-tools/dictionaries/{id}"),
                Body::empty(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = respond(
            &router,
            request(
                "DELETE",
                &format!("/api/language-tools/dictionaries/{id}"),
                Body::empty(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
