// Some handlers have routes only on the server (`route::native_routes`), not in a browser.
#![cfg_attr(target_family = "wasm", allow(dead_code, unused_imports))]

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::Value;
use serde_json::json;
use spares_core::Error;
use spares_core::LibraryError;
use spares_core::NoteErrorKind;
use spares_core::TagErrorKind;

pub(crate) mod audio;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod auth;
pub(crate) mod card;
pub(crate) mod image;
pub(crate) mod image_occlusion;
pub(crate) mod note;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod notes_vcs;
pub(crate) mod parser;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod relay;
pub(crate) mod render_asset;
pub(crate) mod review;
pub(crate) mod saved_search;
pub(crate) mod scheduler;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod share;
pub(crate) mod sync;
pub(crate) mod tag;
pub(crate) mod undo;

#[allow(
    clippy::needless_pass_by_value,
    reason = "can easily call `.map_err()`"
)]
fn error_to_response(e: Error) -> (StatusCode, Json<Value>) {
    if matches!(e, Error::NotesVcsBusy) {
        // Nothing was written, so clients can retry.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "message": e.to_string() })),
        );
    }
    if let Error::Library(error) = &e
        && let Some(body) = note_diagnostic(error)
    {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(body));
    }
    let status = match &e {
        // The sync server changed during the sync: the client starts over.
        Error::SyncRetry => Some(StatusCode::CONFLICT),
        Error::FullSyncRequired(reason) => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(json!({ "message": reason, "full_sync_required": true })),
            );
        }
        // The device has to enroll again (`spares_core::sync::devices`).
        Error::NotEnrolled(replica) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "message": e.to_string(), "not_enrolled": true, "replica": replica })),
            );
        }
        // A search error's message marks the problem in the query, `span` says where it is, and
        // `query_field` which of the request's queries it is in, if it has several.
        Error::Library(LibraryError::Search {
            message,
            span,
            query_field,
        }) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": message, "span": span, "query_field": query_field })),
            );
        }
        // E.g. undoing what has been synced already
        Error::Sync(_)
        | Error::Library(
            LibraryError::Tag(TagErrorKind::InvalidInput(_)) | LibraryError::InvalidConfig(_),
        ) => Some(StatusCode::BAD_REQUEST),
        // A host syncing with its upstream
        Error::SyncUnreachable(_) => Some(StatusCode::BAD_GATEWAY),
        Error::Library(LibraryError::Note(NoteErrorKind::NotFound { .. })) => {
            Some(StatusCode::NOT_FOUND)
        }
        _ => None,
    };
    if let Some(status) = status {
        return (status, Json(json!({ "message": e.to_string() })));
    }
    // The message is for the user, as everywhere else, and the details for the log
    log::error!("{e:?}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "message": e.to_string() })),
    )
}

/// Offsets in parser diagnostics are UTF-8 bytes. The web editor addresses UTF-16 code units.
fn note_diagnostic(error: &LibraryError) -> Option<Value> {
    use spares_core::CardErrorKind as Card;
    let mut spans = Vec::new();
    let (code, source) = match error {
        LibraryError::Card(Card::SameGroupingNestedClozes {
            src,
            cloze_1,
            cloze_2,
        }) => {
            spans.extend([cloze_1, cloze_2]);
            ("nested_clozes", Some(src))
        }
        LibraryError::Card(Card::EmptyCloze { src, at }) => {
            spans.push(at);
            ("empty_cloze", Some(src))
        }
        LibraryError::Card(Card::InvalidSettings { src, at, .. })
        | LibraryError::Note(NoteErrorKind::InvalidSettings { src, at, .. }) => {
            spans.push(at);
            ("invalid_note_settings", Some(src))
        }
        LibraryError::Delimiter(_) => ("invalid_delimiter", None),
        LibraryError::Card(Card::NotFound { .. }) => ("no_cards", None),
        LibraryError::Card(Card::MultipleDuplicateCards { .. }) => ("duplicate_clozes", None),
        LibraryError::Card(Card::Empty | Card::MissingField(_) | Card::InvalidInput(_)) => {
            ("invalid_card", None)
        }
        _ => return None,
    };
    let ranges = source
        .map(|src| {
            spans
                .into_iter()
                .map(|span| {
                    let start = src.floor_char_boundary(span.offset());
                    let end = src.ceil_char_boundary(span.offset().saturating_add(span.len()));
                    [
                        src[..start].encode_utf16().count(),
                        src[..end].encode_utf16().count(),
                    ]
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(json!({ "code": code, "message": error.to_string(), "source": source, "spans": ranges }))
}

pub(crate) async fn health_check_handler() -> impl IntoResponse {
    const MESSAGE: &str = "API Services";

    let json_response = json!({
        "status": "ok",
        "message": MESSAGE
    });

    Json(json_response)
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn marks_both_nested_clozes_after_unicode_text() {
        let parser = spares_core::parsers::get_all_parsers()
            .into_iter()
            .map(|make| make())
            .find(|p| p.get_parser_name() == "markdown")
            .unwrap();
        let text = "🦀{{[g:1]outer {{[g:1]inner}}}}";
        let error =
            spares_core::parsers::get_cards(parser.as_ref(), None, text, false, false).unwrap_err();
        let body = note_diagnostic(&error).unwrap();
        assert_eq!(body["code"], "nested_clozes");
        let spans = body["spans"].as_array().unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0][0], 2);
        assert_eq!(
            spans[1][0],
            text[..text.rfind("{{").unwrap()].encode_utf16().count()
        );
    }
}
