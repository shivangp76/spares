use super::ReviewAction;
use chrono::{DateTime, Local, TimeZone, Utc};
use inquire::DateSelect;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares::model::{CardId, NoteId};
use spares::schema::card::{CardResponse, CardsSelector, SpecialStateUpdate, UpdateCardRequest};
use spares::schema::note::{NotesSelector, UpdateNotesRequest, UpdateTags};
use spares::schema::review::{
    Rating, RatingSubmission, StatisticsResponse, StudyAction, SubmitStudyActionRequest,
};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn open_rendered_file(
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

pub fn close_rendered_file(
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

pub async fn get_scheduler_ratings(
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

pub async fn tag_note(
    note_id: NoteId,
    tag_name: &str,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
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
    Ok(())
}

pub async fn note_id_to_cards(
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

pub async fn bury_card(
    scheduler_name: &str,
    card_id: CardId,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
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
    Ok(())
}

pub async fn bury_cards(
    card_ids: &[CardId],
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let body = UpdateCardRequest {
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
    Ok(())
}

pub async fn suspend_note(note_id: NoteId, base_url: &str, client: &Client) -> Result<(), String> {
    let cards: Vec<CardResponse> = note_id_to_cards(note_id, base_url, client).await?;
    let card_ids = cards.into_iter().map(|card| card.id).collect::<Vec<_>>();
    suspend_cards(&card_ids, base_url, client).await
}

pub async fn bury_note(note_id: NoteId, base_url: &str, client: &Client) -> Result<(), String> {
    let cards: Vec<CardResponse> = note_id_to_cards(note_id, base_url, client).await?;
    let card_ids = cards
        .into_iter()
        .filter(|card| card.special_state.is_none())
        .map(|card| card.id)
        .collect::<Vec<_>>();
    bury_cards(&card_ids, base_url, client).await
}

pub async fn suspend_cards(
    card_ids: &[CardId],
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let body = UpdateCardRequest {
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
    Ok(())
}

pub async fn submit_rating(
    scheduler_name: &str,
    rating_submission: RatingSubmission,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
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
    Ok(())
}

pub async fn forget_card(
    card_id: i64,
    base_url: &str,
    client: &Client,
) -> Result<spares::schema::card::CardResponse, String> {
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
    let card_response: spares::schema::card::CardResponse =
        response.json().await.map_err(|e| format!("{}", e))?;
    Ok(card_response)
}

pub async fn set_due_date_with_prompt<F>(
    card_ids: F,
    base_url: &str,
    client: &Client,
) -> Result<bool, String>
where
    F: Fn(DateTime<Utc>) -> Vec<CardId>,
{
    let prompt = DateSelect::new("Select due date:");
    let date_res = prompt.prompt();
    if let Ok(naive_date) = date_res {
        let naive_dt = naive_date.and_hms_opt(0, 0, 0).unwrap();
        let dt_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, chrono::Utc);
        set_due_date(card_ids(dt_utc), dt_utc, base_url, client).await?;
        return Ok(true);
    }
    Ok(false)
}

pub async fn set_due_date(
    card_ids: Vec<CardId>,
    due_date: DateTime<Utc>,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let request = UpdateCardRequest {
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
    Ok(())
}

pub(super) async fn bury_until_later_today(
    card_id: CardId,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    // Get current time in UTC and convert to local timezone to get today's date
    let now_utc = Utc::now();
    // Calculate end of today (23:59:59) in local timezone, then convert to UTC
    let end_of_today_utc = Local
        .from_local_datetime(
            &now_utc
                .with_timezone(&Local)
                .date_naive()
                .and_hms_opt(23, 59, 59)
                .unwrap(),
        )
        .unwrap()
        .to_utc();

    // Send update card request
    let request = UpdateCardRequest {
        selector: CardsSelector::Ids(vec![card_id]),
        desired_retention: None,
        special_state: None,
        due: Some(end_of_today_utc),
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
            || "Failed to update card due date".to_string(),
            |m| m.to_string(),
        ));
    }
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
    let duration = chrono::Duration::from_std(recall_duration).unwrap();
    println!("Recall Duration: {}", format_duration(duration));
}

pub(super) fn print_rate_duration(rate_duration: Duration) {
    let duration = chrono::Duration::from_std(rate_duration).unwrap();
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
        let session_duration = chrono::Duration::from_std(session_start.elapsed()).unwrap();
        let session_recall_duration = chrono::Duration::from_std(session_recall_duration).unwrap();
        let session_rate_duration = chrono::Duration::from_std(session_rate_duration).unwrap();
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
