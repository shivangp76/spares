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
use spares_core::schema::undo::EventSummary;
use spares_core::schema::undo::RedoEventRequest;
use spares_core::schema::undo::RedoEventResponse;
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
        let message = match response_json.get("message") {
            Some(Value::String(message)) => message.clone(),
            Some(message) => message.to_string(),
            None => status.to_string(),
        };
        return Err(miette!("{message}"));
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

/// Prints what an undo or redo reversed, e.g. `Undone: Rate card 3 (Good) [event 12]`.
pub(crate) fn print_reverted(verb: &str, events: &[EventSummary]) {
    if let [event] = events {
        println!("{verb}: {} [event {}]", event.description, event.id);
        return;
    }
    println!("{verb} {} events:", events.len());
    for event in events {
        println!("  - {} [event {}]", event.description, event.id);
    }
}

pub(crate) async fn redo_event(
    base_url: &str,
    client: &Client,
    request: RedoEventRequest,
) -> Result<Option<RedoEventResponse>, String> {
    let url = format!("{}/api/redo", base_url);
    let response = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to redo event: {:?}", message));
    }
    let redo_response: Option<RedoEventResponse> =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(redo_response)
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

/// This machine's time zone, e.g. `America/New_York`. `spares deploy` sets the server to it so
/// that both count days the same, and statistics ask the server for days in it. `None` if it
/// cannot be told.
pub(crate) fn local_timezone() -> Option<String> {
    let zone = std::fs::read_link("/etc/localtime").ok()?;
    let zone = zone.to_str()?;
    zone.split_once("/zoneinfo/")
        .map(|(_, zone)| zone.to_string())
}

/// Asks a yes or no question before a command changes data, no by default. `Ok(false)` when the
/// user answers no or presses Escape or Ctrl-C, after printing "Cancelled.". When it can't ask, e.g.
/// without a terminal (scripts, agents, `ssh host cmd`), it fails with `skip_hint` (such as "pass
/// --yes to go ahead") rather than doing nothing with exit status 0, which looks like success.
pub(crate) fn confirm_or_fail(prompt: &str, skip_hint: &str) -> Result<bool, Error> {
    let answer = inquire::Confirm::new(prompt).with_default(false).prompt();
    let confirmed = confirm_answer(answer, skip_hint)?;
    if !confirmed {
        println!("Cancelled.");
    }
    Ok(confirmed)
}

/// What [`confirm_or_fail`] makes of the prompt's answer.
fn confirm_answer(
    answer: Result<bool, inquire::InquireError>,
    skip_hint: &str,
) -> Result<bool, Error> {
    use inquire::InquireError;
    match answer {
        Ok(answer) => Ok(answer),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(false),
        Err(InquireError::NotTTY) => Err(miette!(
            "can't ask for confirmation without a terminal; {skip_hint}"
        )),
        Err(e) => Err(miette!("can't ask for confirmation ({e}); {skip_hint}")),
    }
}

/// Awaits `work` while a spinner on stderr says what is being waited on and for how long, so a
/// request the server takes a while to answer, e.g. rendering every note, does not look like a
/// hang. The spinner is cleared when `work` is done and is not shown when stderr is not a
/// terminal.
pub(crate) async fn with_spinner<T>(message: &'static str, work: impl Future<Output = T>) -> T {
    let spinner = indicatif::ProgressBar::new_spinner().with_message(message);
    spinner.set_style(
        indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg} ({elapsed})")
            .expect("the template is valid"),
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    let output = work.await;
    spinner.finish_and_clear();
    output
}

#[cfg(test)]
mod tests {
    use inquire::InquireError;

    use super::*;

    #[test]
    fn a_confirmation_that_cannot_ask_fails() {
        let hint = "pass --yes to go ahead";
        assert!(confirm_answer(Ok(true), hint).unwrap());
        assert!(!confirm_answer(Ok(false), hint).unwrap());
        assert!(!confirm_answer(Err(InquireError::OperationCanceled), hint).unwrap());
        assert!(!confirm_answer(Err(InquireError::OperationInterrupted), hint).unwrap());
        let error = confirm_answer(Err(InquireError::NotTTY), hint).unwrap_err();
        assert_eq!(
            error.to_string(),
            "can't ask for confirmation without a terminal; pass --yes to go ahead"
        );
        let error = confirm_answer(Err(InquireError::IO(std::io::Error::other("closed"))), hint)
            .unwrap_err();
        assert!(error.to_string().contains("pass --yes"), "{error}");
    }
}
