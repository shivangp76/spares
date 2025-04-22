use super::ReviewAction;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares::model::{CardId, NoteId, RatingId, TagId};
use spares::schema::card::{CardResponse, CardsSelector, SpecialStateUpdate, UpdateCardRequest};
use spares::schema::note::{NotesSelector, UpdateNotesRequest};
use spares::schema::review::{Rating, RatingSubmission, StudyAction, SubmitStudyActionRequest};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn open_rendered_file(file_path: &Path, opener: Option<&str>) -> Result<Child, String> {
    if let Some(command) = opener {
        return Command::new(command)
            .arg(file_path)
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

pub fn close_rendered_file(rendered_file_child: &mut Child) -> Result<(), String> {
    rendered_file_child.kill().map_err(|e| format!("{}", e))
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
        tags_to_remove: None,
        tags_to_add: Some(vec![tag_name.to_string()]),
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
        dbg!(&message);
        return Err("Failed to add tag to note".to_string());
    }
    Ok(())
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
        dbg!(&message);
        return Err("Failed to bury card".to_string());
    }
    Ok(())
}

pub async fn suspend_note(note_id: NoteId, base_url: &str, client: &Client) -> Result<(), String> {
    let url = format!("{}/api/cards/note_id/{}", base_url, note_id);
    let response = client.get(url).send().await.map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        dbg!(&message);
        return Err("Failed to get cards from note id".to_string());
    }
    let cards: Vec<CardResponse> = response.json().await.map_err(|e| format!("{}", e))?;
    let card_ids = cards.into_iter().map(|card| card.id).collect::<Vec<_>>();
    suspend_cards(&card_ids, base_url, client).await
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
        dbg!(&message);
        return Err("Failed to suspend card".to_string());
    }
    Ok(())
}

pub async fn submit_rating(
    recall_duration: Duration,
    scheduler_name: &str,
    card_id: CardId,
    tag_id: Option<TagId>,
    rating_id: RatingId,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let duration = chrono::Duration::from_std(recall_duration).unwrap();
    let update_review_request = SubmitStudyActionRequest {
        scheduler_name: scheduler_name.to_string(),
        action: StudyAction::Rate(RatingSubmission {
            card_id,
            rating: rating_id,
            duration,
            tag_id,
        }),
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
        dbg!(&message);
        return Err("Failed to submit rating".to_string());
    }
    Ok(())
}

fn format_duration(duration: chrono::Duration) -> String {
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
        // started = true;
    }
    // Always include seconds
    result.push(format!("{}s", seconds));

    result.join(" ")
}

pub fn print_recall_duration(recall_duration: Duration) {
    let duration = chrono::Duration::from_std(recall_duration).unwrap();
    println!("Duration: {}", format_duration(duration));
}

pub fn print_summary(session_start: Instant, session_recall: Duration, reviewed_cards_count: u32) {
    if reviewed_cards_count > 0 {
        let session_duration = chrono::Duration::from_std(session_start.elapsed()).unwrap();
        let session_recall = chrono::Duration::from_std(session_recall).unwrap();
        println!();
        println!(
            "Total Session Duration: {}",
            format_duration(session_duration)
        );
        println!(
            "Total Recall Duration:  {}",
            format_duration(session_recall)
        );
        println!("Total Cards Reviewed:   {:?}", reviewed_cards_count);
    }
}
