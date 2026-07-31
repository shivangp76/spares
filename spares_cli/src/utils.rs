use std::path::Path;
use std::path::PathBuf;

use miette::Error;
use miette::miette;
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::Map;
use serde_json::Value;
use spares_core::model::NoteId;
use spares_core::parsers::RenderOutputDirectoryType;
use spares_core::parsers::find_parser;
use spares_core::parsers::generate_files::CardSide;
use spares_core::parsers::generate_files::RenderOutputType;
use spares_core::parsers::get_all_parsers;
use spares_core::parsers::get_output_raw_dir;
use spares_core::schema::undo::UndoEventRequest;
use spares_core::schema::undo::UndoEventResponse;

pub(crate) fn page_limit_queries(
    page: Option<usize>,
    limit: Option<usize>,
) -> Vec<(&'static str, String)> {
    let mut queries = Vec::new();
    if let Some(page) = page {
        queries.push(("page", page.to_string()));
    }
    if let Some(limit) = limit {
        queries.push(("limit", limit.to_string()));
    }
    queries
}

pub(crate) fn parse_list(data: &str) -> Vec<String> {
    data.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
}

pub(crate) fn parse_custom_data(s: &str) -> Result<Map<String, Value>, Error> {
    let v: Value = serde_json::from_str::<Value>(s)
        .map_err(|e| miette!("invalid --custom-data JSON: {}", e))?;
    match v {
        Value::Object(m) => Ok(m),
        other => Err(miette!(
            "invalid --custom-data JSON: expected a JSON object, got {}",
            match other {
                Value::Array(_) => "array",
                Value::String(_) => "string",
                Value::Number(_) => "number",
                Value::Bool(_) => "bool",
                Value::Null => "null",
                Value::Object(_) => unreachable!(),
            }
        )),
    }
}

pub(crate) async fn ensure_ok(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| miette!("{}", e))?;
        let message = response_json.get("message");
        return Err(miette!(message.unwrap().to_string()));
    }
    Ok(response)
}

pub(crate) async fn undo_event(
    base_url: &str,
    client: &Client,
    request: UndoEventRequest,
) -> Result<Option<UndoEventResponse>, String> {
    let url = format!("{}/api/undo", base_url);
    let response = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to undo event: {:?}", message));
    }
    let undo_response: Option<UndoEventResponse> =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(undo_response)
}

pub(crate) fn compute_note_raw_path(parser_name: &str, note_id: NoteId) -> Result<PathBuf, String> {
    let parser = find_parser(parser_name, &get_all_parsers()).map_err(|e| format!("{}", e))?;
    let mut path = get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
    path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    path.set_extension(parser.file_extension());
    Ok(path)
}

pub(crate) fn compute_note_rendered_path(
    parser_name: &str,
    note_id: NoteId,
) -> Result<PathBuf, String> {
    let parser = find_parser(parser_name, &get_all_parsers()).map_err(|e| format!("{}", e))?;
    let mut path = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    Ok(path)
}

pub(crate) fn compute_card_rendered_back_path(
    parser_name: &str,
    note_id: NoteId,
    card_order: u32,
) -> Result<PathBuf, String> {
    let parser = find_parser(parser_name, &get_all_parsers()).map_err(|e| format!("{}", e))?;
    let mut path = parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
    path.push(parser.get_output_filename(
        RenderOutputType::Card(card_order as usize, CardSide::Back),
        note_id,
    ));
    if path.exists() {
        return Ok(path);
    }
    let mut note_path = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    note_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    Ok(note_path)
}

pub(crate) fn open_file(path: &Path) {
    if let Err(e) = open::that_detached(path) {
        println!("Failed to open file: {}", e);
    }
}
