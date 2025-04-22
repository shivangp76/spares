use crate::{
    parsers::generate_files::GenerateNoteFilesRequest,
    schema::{
        note::{CreateNoteRequest, CreateNotesRequest, NoteResponse, NotesResponse},
        parser::{CreateParserRequest, ParserResponse},
    },
};
use chrono::Utc;
use itertools::Itertools;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::{collections::HashMap, time::Instant};

#[derive(Debug, Clone)]
pub struct MigrationData {
    pub front: String,
    pub back: String,
    pub parser_name: String,
    pub is_suspended: bool,
}

pub type MigrationFunc = fn(MigrationData) -> (String, String);

async fn create_parsers(
    client: &Client,
    base_url: &str,
    create_note_requests: &HashMap<String, Vec<CreateNoteRequest>>,
    run: bool,
) -> Result<Vec<ParserResponse>, String> {
    println!("Creating parsers...");
    let url = format!("{}/api/parsers", base_url);
    let mut parser_responses = Vec::new();
    for (i, parser_name) in create_note_requests.keys().enumerate() {
        let request = CreateParserRequest {
            name: parser_name.to_string(),
        };
        let parser_response = if run {
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| format!("{}", e))?;
            let status = response.status();
            if status != StatusCode::OK {
                let body: Value = response.json().await.map_err(|e| format!("{}", e))?;
                return Err(body.to_string());
            }
            let parser_response: ParserResponse =
                response.json().await.map_err(|e| format!("{}", e))?;
            parser_response
        } else {
            ParserResponse {
                id: i64::try_from(i).unwrap_or_default(),
                name: parser_name.to_string(),
            }
        };
        parser_responses.push(parser_response);
    }
    Ok(parser_responses)
}

pub async fn create_notes(
    client: &Client,
    base_url: &str,
    parse_note_requests: Vec<(String, GenerateNoteFilesRequest)>,
    run: bool,
) -> Result<Vec<NotesResponse>, String> {
    println!("Getting notes...");
    let start = Instant::now();
    let create_note_requests = parse_note_requests
        .into_iter()
        .map(
            |(
                parser_name,
                GenerateNoteFilesRequest {
                    note_data,
                    keywords,
                    tags,
                    custom_data,
                    note_id: _,
                    linked_notes: _,
                },
            )| {
                (
                    parser_name,
                    CreateNoteRequest {
                        data: note_data,
                        keywords,
                        tags,
                        is_suspended: false,
                        custom_data,
                    },
                )
            },
        )
        .into_group_map();

    // Create parsers
    let parser_responses = create_parsers(client, base_url, &create_note_requests, run).await?;

    println!("Creating notes...");
    let mut all_notes_responses = Vec::new();
    for (parser_name, requests) in create_note_requests {
        let parser_id = parser_responses
            .iter()
            .find(|pr| pr.name == parser_name)
            .ok_or("Could not find parser.")
            .map(|p| p.id)?;
        let create_notes_request = CreateNotesRequest {
            parser_id,
            requests,
        };
        let url = format!("{}/api/notes", base_url);
        let notes_response: NotesResponse = if run {
            let response = client
                .post(url)
                .json(&create_notes_request)
                .send()
                .await
                .map_err(|e| format!("{}", e))?;
            let status = response.status();
            if status != StatusCode::OK {
                let body: Value = response.json().await.map_err(|e| format!("{}", e))?;
                return Err(body.to_string());
            }
            response.json().await.map_err(|e| format!("{}", e))?
        } else {
            let notes = create_notes_request
                .requests
                .into_iter()
                .enumerate()
                .map(|(i, request)| NoteResponse {
                    id: i64::try_from(i).unwrap(),
                    data: request.data,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    parser_id: create_notes_request.parser_id,
                    keywords: request.keywords,
                    tags: request.tags,
                    custom_data: request.custom_data.clone(),
                    linked_notes: None,
                    // NOTE: This is fake data
                    card_count: 1,
                })
                .collect::<Vec<_>>();
            NotesResponse { notes }
        };
        all_notes_responses.push(notes_response);
    }
    let duration = start.elapsed();
    println!("Notes creation duration: {:?}", duration);
    Ok(all_notes_responses)
}
