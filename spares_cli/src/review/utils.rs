use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use chrono::DateTime;
use chrono::Utc;
use inquire::DateSelect;
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::Value;
use spares_core::model::CardId;
use spares_core::model::NoteId;
use spares_core::schema::card::CardResponse;
use spares_core::schema::card::CardsSelector;
use spares_core::schema::card::ForgetCardResponse;
use spares_core::schema::card::SpecialStateUpdate;
use spares_core::schema::card::UpdateCardsRequest;
use spares_core::schema::card::UpdateCardsResponse;
use spares_core::schema::note::NotesSelector;
use spares_core::schema::note::SearchNotesRequest;
use spares_core::schema::note::SearchNotesResponse;
use spares_core::schema::note::UpdateNotesRequest;
use spares_core::schema::note::UpdateNotesResponse;
use spares_core::schema::note::UpdateTags;
use spares_core::schema::review::Rating;
use spares_core::schema::review::RatingSubmission;
use spares_core::schema::review::StatisticsResponse;
use spares_core::schema::review::StudyAction;
use spares_core::schema::review::SubmitStudyActionRequest;
use spares_core::schema::review::SubmitStudyActionResponse;
use spares_core::search::QueryReturnItemType;
use tokio::io::AsyncBufReadExt;

use super::ReviewAction;

pub(crate) fn open_rendered_file(
    file_path: &Path,
    open_command_opt: Option<&str>,
    _first: bool,
) -> Result<Child, String> {
    let open_command_opt = open_command_opt.filter(|x| !x.is_empty());
    if let Some(open_command) = open_command_opt {
        let mut parts = open_command.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| "Unreachable by outer filter: Empty command".to_string())?;
        let args = parts.collect::<Vec<_>>();
        let mut command = Command::new(program);
        command.args(&args);
        command.arg(file_path);
        return command
            .stdout(Stdio::null()) // Hide output from terminal
            .stderr(Stdio::null()) // Hide output from terminal
            .spawn()
            .map_err(|e| format!("Failed to open rendered file: {}", e));
    }
    Command::new("open")
        .arg("--background") // to avoid stealing focus
        // .arg("--new") // open in a new window instead of a tab
        .arg(file_path)
        .stdout(Stdio::null()) // Hide output from terminal
        .stderr(Stdio::null()) // Hide output from terminal
        .spawn()
        .map_err(|e| format!("Failed to open rendered file: {}", e))
    // This won't work because we need the Child to kill it after
    // open::that(file_path).map_err(|e| format!("{}", e))
}

pub(crate) fn close_rendered_file(
    rendered_file_child: &mut Child,
    close_command_opt: Option<&str>,
    last: bool,
) -> Result<(), String> {
    let close_command_opt = close_command_opt.filter(|x| !x.is_empty());
    if last && let Some(close_command) = close_command_opt {
        let mut parts = close_command.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| "Unreachable by outer filter: Empty command".to_string())?;
        let args = parts.collect::<Vec<_>>();
        let mut command = Command::new(program);
        command.args(&args);
        return command
            .stdout(Stdio::null()) // Hide output from terminal
            .stderr(Stdio::null()) // Hide output from terminal
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to close rendered file: {}", e));
    }
    if close_command_opt.is_none() {
        return rendered_file_child.kill().map_err(|e| format!("{}", e));
    }
    Ok(())
}

pub(super) async fn get_scheduler_ratings(
    scheduler_name: &str,
    base_url: &str,
    client: &Client,
) -> Result<Vec<ReviewAction>, String> {
    let url = format!("{}/api/scheduler/{}/ratings", base_url, scheduler_name);
    let scheduler_ratings: Vec<Rating> = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("{}", e))?
        .json()
        .await
        .map_err(|e| format!("{}", e))?;
    Ok(scheduler_ratings
        .into_iter()
        .map(|r| ReviewAction::Rate {
            description: r.description,
            id: r.id,
        })
        .collect::<Vec<_>>())
}

pub(crate) async fn tag_note(
    note_id: NoteId,
    tag_name: &str,
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let request = UpdateNotesRequest {
        selector: NotesSelector::Ids(vec![note_id]),
        data: None,
        parser_id: None,
        keywords: None,
        tags: UpdateTags::ModifyTags {
            tags_to_remove: None,
            tags_to_add: Some(vec![tag_name.to_string()]),
        },
        custom_data: None,
    };
    let url = format!("{}/api/notes", base_url);
    let response = client
        .patch(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to add tag to note: {:?}", message));
    }
    let update_response: UpdateNotesResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(update_response.event_id)
}

pub(crate) async fn note_id_to_cards(
    note_id: NoteId,
    base_url: &str,
    client: &Client,
) -> Result<Vec<CardResponse>, String> {
    let url = format!("{}/api/cards/note_id/{}", base_url, note_id);
    let response = client.get(url).send().await.map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to get cards from note id: {:?}", message));
    }
    let cards: Vec<CardResponse> = response.json().await.map_err(|e| format!("{}", e))?;
    Ok(cards)
}

pub(crate) async fn bury_card(
    scheduler_name: &str,
    card_id: CardId,
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let submit_review_request = SubmitStudyActionRequest {
        scheduler_name: scheduler_name.to_string(),
        action: StudyAction::Bury { card_id },
    };
    let url = format!("{}/api/review/submit", base_url);
    let response = client
        .post(url)
        .json(&submit_review_request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to bury card: {:?}", message));
    }
    let submit_response: SubmitStudyActionResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(submit_response.event_id)
}

pub(crate) async fn bury_cards(
    card_ids: &[CardId],
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let body = UpdateCardsRequest {
        selector: CardsSelector::Ids(card_ids.to_vec()),
        desired_retention: None,
        special_state: Some(Some(SpecialStateUpdate::Buried)),
        due: None,
    };
    let url = format!("{}/api/cards", base_url);
    let response = client
        .patch(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to bury cards: {:?}", message));
    }
    let update_response: UpdateCardsResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(update_response.event_id)
}

pub(crate) async fn suspend_note(
    note_id: NoteId,
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let cards: Vec<CardResponse> = note_id_to_cards(note_id, base_url, client).await?;
    let card_ids = cards.into_iter().map(|card| card.id).collect::<Vec<_>>();
    suspend_cards(&card_ids, base_url, client).await
}

pub(crate) async fn bury_note(
    note_id: NoteId,
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let cards: Vec<CardResponse> = note_id_to_cards(note_id, base_url, client).await?;
    let card_ids = cards
        .into_iter()
        .filter(|card| card.special_state.is_none())
        .map(|card| card.id)
        .collect::<Vec<_>>();
    bury_cards(&card_ids, base_url, client).await
}

pub(crate) async fn suspend_cards(
    card_ids: &[CardId],
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let body = UpdateCardsRequest {
        selector: CardsSelector::Ids(card_ids.to_vec()),
        desired_retention: None,
        special_state: Some(Some(SpecialStateUpdate::Suspended)),
        due: None,
    };
    let url = format!("{}/api/cards", base_url);
    let response = client
        .patch(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to suspend card: {:?}", message));
    }
    let update_response: UpdateCardsResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(update_response.event_id)
}

pub(crate) async fn submit_rating(
    scheduler_name: &str,
    rating_submission: RatingSubmission,
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let update_review_request = SubmitStudyActionRequest {
        scheduler_name: scheduler_name.to_string(),
        action: StudyAction::Rate(rating_submission),
    };
    let url = format!("{}/api/review/submit", base_url);
    let response = client
        .post(url)
        .json(&update_review_request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to submit rating: {:?}", message));
    }
    let submit_response: SubmitStudyActionResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(submit_response.event_id)
}

pub(crate) async fn forget_card(
    card_id: i64,
    base_url: &str,
    client: &Client,
) -> Result<ForgetCardResponse, String> {
    let url = format!("{}/api/cards/{}/forget", base_url, card_id);
    let response = client
        .post(&url)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(message.unwrap().to_string());
    }
    let forget_response: ForgetCardResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(forget_response)
}

pub(crate) async fn set_due_date_with_prompt<F>(
    card_ids: F,
    base_url: &str,
    client: &Client,
) -> Result<Option<Option<i64>>, String>
where
    F: Fn(DateTime<Utc>) -> Vec<CardId>,
{
    let prompt = DateSelect::new("Select due date:");
    let date_res = prompt.prompt();
    if let Ok(naive_date) = date_res {
        let naive_dt = naive_date.and_hms_opt(0, 0, 0).unwrap();
        let dt_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, chrono::Utc);
        let event_id = set_due_date(card_ids(dt_utc), dt_utc, base_url, client).await?;
        return Ok(Some(event_id));
    }
    Ok(None)
}

pub(crate) async fn set_due_date(
    card_ids: Vec<CardId>,
    due_date: DateTime<Utc>,
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    let request = UpdateCardsRequest {
        selector: CardsSelector::Ids(card_ids),
        desired_retention: None,
        special_state: None,
        due: Some(due_date),
    };
    let url = format!("{}/api/cards", base_url);
    let response = client
        .patch(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(message.unwrap().to_string());
    }
    let update_response: UpdateCardsResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(update_response.event_id)
}

pub(super) async fn bury_until_later_today(
    card_id: CardId,
    base_url: &str,
    client: &Client,
) -> Result<Option<i64>, String> {
    // Set special_state to BuriedUntilLaterToday and due = now() (used as burial timestamp for FIFO ordering).
    // Re-pressing this on an already-buried card updates due = now(), pushing it to the back of the queue.
    let request = UpdateCardsRequest {
        selector: CardsSelector::Ids(vec![card_id]),
        desired_retention: None,
        special_state: Some(Some(SpecialStateUpdate::BuriedUntilLaterToday)),
        due: Some(Utc::now()),
    };
    let url = format!("{}/api/cards", base_url);
    let response = client
        .patch(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(message.map_or_else(
            || "Failed to bury card until later today".to_string(),
            |m| m.to_string(),
        ));
    }
    let update_response: UpdateCardsResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(update_response.event_id)
}

pub(crate) async fn browse_keyword_notes(
    keyword: &str,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let request = SearchNotesRequest {
        query: format!("linked_to_keyword=\"{}\"", keyword),
        output_type: QueryReturnItemType::Notes,
    };
    let url = format!("{}/api/notes/search", base_url);
    let response = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        let response_json: serde_json::Value =
            response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(format!("Failed to search notes by keyword: {:?}", message));
    }
    let search_response: SearchNotesResponse =
        response.json().await.map_err(|e| format!("{}", e))?;

    let notes = match search_response {
        SearchNotesResponse::Notes(notes) => notes,
        SearchNotesResponse::Cards(_) => {
            return Err("Expected Notes response from search".to_string());
        }
    };

    if notes.is_empty() {
        println!("No notes found with keyword: {}", keyword);
        return Ok(());
    }

    let mut paths: Vec<PathBuf> = Vec::new();
    for (note_response, parser_name) in &notes {
        let note_raw_path = crate::utils::compute_note_raw_path(parser_name, note_response.id)?;
        paths.push(note_raw_path);
    }

    println!("Notes matching keyword '{}':", keyword);
    for path in &paths {
        println!("  {}", path.display());
    }

    let should_copy = inquire::Confirm::new("Copy all filepaths to clipboard?")
        .with_default(false)
        .prompt()
        .unwrap_or(false);

    if should_copy {
        let all_paths: String = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        copy_to_clipboard(&all_paths)?;
        println!("Filepaths copied to clipboard!");
    }

    Ok(())
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let cmd = if cfg!(target_os = "macos") {
        "pbcopy"
    } else if cfg!(target_os = "linux") {
        "xclip"
    } else if cfg!(target_os = "windows") {
        "clip"
    } else {
        return Err("Clipboard not supported on this platform".to_string());
    };

    let mut child = Command::new(cmd)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run {}: {}", cmd, e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("Failed to write to clipboard: {}", e))?;
    }

    child.wait().map_err(|e| format!("{}", e))?;
    Ok(())
}

pub(super) fn format_duration(duration: chrono::Duration) -> String {
    let total_seconds = duration.num_seconds();
    let days = total_seconds / (24 * 3600);
    let hours = (total_seconds % (24 * 3600)) / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    let mut result = Vec::new();
    let mut started = false;

    // Start collecting components from the first non-zero value
    if days > 0 {
        result.push(format!("{}d", days));
        started = true;
    }
    if hours > 0 || started {
        result.push(format!("{}h", hours));
        started = true;
    }
    if minutes > 0 || started {
        result.push(format!("{}m", minutes));
    }
    // Always include seconds
    result.push(format!("{}s", seconds));

    result.join(" ")
}

pub(super) fn print_recall_duration(recall_duration: Duration) {
    let duration =
        chrono::Duration::from_std(recall_duration).expect("recall_duration fits chrono::Duration");
    println!("Recall Duration: {}", format_duration(duration));
}

pub(super) fn print_rate_duration(rate_duration: Duration) {
    let duration =
        chrono::Duration::from_std(rate_duration).expect("rate_duration fits chrono::Duration");
    println!("Rate Duration: {}", format_duration(duration));
}

pub(super) fn print_summary(
    session_start: Instant,
    session_recall_duration: Duration,
    session_rate_duration: Duration,
    cards_studied_count: u32,
    day_stats: Option<StatisticsResponse>,
) {
    if cards_studied_count > 0 {
        let session_duration = chrono::Duration::from_std(session_start.elapsed())
            .expect("session_duration fits chrono::Duration");
        let session_recall_duration = chrono::Duration::from_std(session_recall_duration)
            .expect("session_recall_duration fits chrono::Duration");
        let session_rate_duration = chrono::Duration::from_std(session_rate_duration)
            .expect("session_rate_duration fits chrono::Duration");
        println!();
        println!("--- Session Statistics ---");
        println!("Total Cards Reviewed:   {:?}", cards_studied_count);
        println!(
            "Total Recall Duration:  {}",
            format_duration(session_recall_duration)
        );
        println!(
            "Total Rate Duration:  {}",
            format_duration(session_rate_duration)
        );
        println!(
            "Total Session Duration: {}",
            format_duration(session_duration)
        );
        if let Some(stats) = day_stats {
            let day_cards_studied_count = cards_studied_count + stats.cards_studied_count;
            let day_recall_duration = session_recall_duration + stats.recall_duration;
            let day_rate_duration = session_rate_duration + stats.rate_duration;
            println!();
            println!("--- Day Statistics ---");
            println!("Total Cards Reviewed Today:  {}", day_cards_studied_count);
            println!(
                "Total Recall Duration Today: {}",
                format_duration(day_recall_duration)
            );
            println!(
                "Total Rate Duration Today: {}",
                format_duration(day_rate_duration)
            );
            println!(
                "Total Recall + Rate Duration Today: {}",
                format_duration(day_recall_duration + day_rate_duration)
            );
        }
    }
}

/// Read the final non-empty line of an external command's stdout and parse it
/// as a JSON object of the form `{"score": <f64 in [0,1]>}`. Any other shape
/// is rejected; this is a strict contract — bare floats are not accepted.
pub(super) fn parse_cli_score(stdout: &str) -> Result<f64, String> {
    let last_line = stdout
        .lines()
        .map(|l| l.trim())
        .rfind(|l| !l.is_empty())
        .ok_or_else(|| "CLI exec produced no score on stdout".to_string())?;
    if !last_line.trim_start().starts_with('{') {
        return Err(format!(
            "CLI exec score must be a JSON object (got `{last_line}`). For example, emit \
             `{{\"score\": 0.83}}` as the last non-empty stdout line."
        ));
    }
    let value: Value = serde_json::from_str(last_line)
        .map_err(|e| format!("Could not parse JSON from `{last_line}`: {e}"))?;
    let score = value
        .get("score")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("JSON `{last_line}` is missing a numeric `score` field"))?;
    if !(0.0..=1.0).contains(&score) {
        return Err(format!("CLI exec score {score} is out of range [0, 1]"));
    }
    Ok(score)
}

/// Spawn an external `cli`-parser exec command interactively with a
/// 30-minute timeout. stdin and stderr are inherited so the child (e.g.
/// `poker_trainer`'s `inquire` prompts) can use the TTY directly; stdout is
/// piped and read line-by-line with a 1 MiB cap. Returns (last non-empty
/// stdout line, elapsed wall-clock duration).
///
/// # Trust model
/// `exec` comes from the note's `spares: cli` block and is run through
/// `sh -c` without sanitisation. CLI cards should only be used with notes
/// from trusted sources (i.e. authored by the same user).
pub(super) async fn spawn_cli_exec(exec: &str) -> Result<(String, Duration), String> {
    const MAX_STDOUT_BYTES: usize = 1_048_576; // 1 MiB
    const EXEC_TIMEOUT: Duration = Duration::from_secs(1800); // 30 min

    let exec_owned = exec.to_string();
    let result = tokio::time::timeout(EXEC_TIMEOUT, async move {
        let recall_start = Instant::now();
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&exec_owned)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn cli exec `{exec_owned}`: {e}"))?;

        let stdout_handle = child
            .stdout
            .take()
            .ok_or_else(|| "No stdout pipe".to_string())?;

        let mut reader = tokio::io::BufReader::new(stdout_handle);
        let mut line = String::new();
        let mut total_bytes = 0usize;
        let mut last_non_empty_line = String::new();

        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("Failed to read stdout: {e}"))?;
            if n == 0 {
                break;
            }
            total_bytes += n;
            if total_bytes > MAX_STDOUT_BYTES {
                return Err(format!(
                    "cli exec `{exec_owned}` produced more than {} bytes of stdout",
                    MAX_STDOUT_BYTES
                ));
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                last_non_empty_line = trimmed.to_string();
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| format!("Failed to wait on child: {e}"))?;
        let recall_duration = recall_start.elapsed();

        if !status.success() {
            let excerpt = if last_non_empty_line.len() > 256 {
                format!("{}... (truncated)", &last_non_empty_line[..256])
            } else {
                last_non_empty_line.clone()
            };
            return Err(format!(
                "cli exec `{exec_owned}` exited with status {}\nlast stdout line: {}",
                status, excerpt
            ));
        }

        Ok((last_non_empty_line, recall_duration))
    })
    .await;

    result.map_err(|_| {
        format!(
            "cli exec `{exec}` timed out after {}s",
            EXEC_TIMEOUT.as_secs()
        )
    })?
}

/// Fetch a scheduler rating for a `[0, 1]` score via the server's
/// `rating_from_score` endpoint. Returns an error immediately if `score`
/// is not a finite number in `[0, 1]` (defense in depth).
pub(super) async fn get_rating_from_score(
    scheduler_name: &str,
    score: f64,
    base_url: &str,
    client: &Client,
) -> Result<Rating, String> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(format!("score {score} is not a finite number in [0, 1]"));
    }
    let url = format!(
        "{}/api/scheduler/{}/rating?score={}",
        base_url, scheduler_name, score
    );
    client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("rating_from_score request failed: {e}"))?
        .json::<Rating>()
        .await
        .map_err(|e| format!("rating_from_score decode failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::parse_cli_score;

    #[test]
    fn parse_score_valid() {
        let result = parse_cli_score("some output\n{\"score\": 0.83}");
        assert!((result.unwrap() - 0.83).abs() < 1e-10);
    }

    #[test]
    fn parse_score_empty_stdout() {
        let result = parse_cli_score("");
        assert!(result.is_err(), "expected error for empty stdout");
    }

    #[test]
    fn parse_score_only_whitespace() {
        let result = parse_cli_score("  \n  \n  ");
        assert!(result.is_err(), "expected error for whitespace-only stdout");
    }

    #[test]
    fn parse_score_bare_float() {
        let result = parse_cli_score("0.83");
        assert!(result.is_err(), "bare floats should be rejected");
    }

    #[test]
    fn parse_score_missing_score_field() {
        let result = parse_cli_score(r#"{"not_score": 0.5}"#);
        assert!(result.is_err(), "expected error for missing score field");
    }

    #[test]
    fn parse_score_string_score() {
        let result = parse_cli_score(r#"{"score": "0.5"}"#);
        assert!(result.is_err(), "expected error for string score");
    }

    #[test]
    fn parse_score_integer_score() {
        let result = parse_cli_score(r#"{"score": 1}"#);
        assert!((result.unwrap() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn parse_score_negative() {
        let result = parse_cli_score(r#"{"score": -0.1}"#);
        assert!(result.is_err(), "expected error for negative score");
    }

    #[test]
    fn parse_score_greater_than_one() {
        let result = parse_cli_score(r#"{"score": 1.5}"#);
        assert!(result.is_err(), "expected error for score > 1");
    }

    #[test]
    fn parse_score_uses_last_non_empty_line() {
        let result = parse_cli_score("line1\n{\"score\": 0.3}\nline2\n  \n{\"score\": 0.9}\n");
        assert!((result.unwrap() - 0.9).abs() < 1e-10);
    }

    #[test]
    fn parse_score_trailing_newline() {
        let result = parse_cli_score("{\"score\": 0.5}\n");
        assert!((result.unwrap() - 0.5).abs() < 1e-10);
    }
}
