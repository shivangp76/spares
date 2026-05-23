use std::collections::HashMap;
use std::process::Child;

use chrono::Local;
use clap::Args;
use inquire::MultiSelect;
use inquire::Select;
use inquire::Text;
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::Value;
use spares_core::schema::card::CardResponse;
use spares_core::schema::note::NoteResponse;
use spares_core::schema::note::SearchNotesRequest;
use spares_core::schema::note::SearchNotesResponse;
use spares_core::schema::parser::ParserResponse;
use spares_core::search::QueryReturnItemType;
use strum_macros::Display;

use crate::review::sync_note;
use crate::review::utils::close_rendered_file;
use crate::review::utils::open_rendered_file;
use crate::utils;

#[derive(Args, Debug)]
pub(crate) struct ViewNoteArgs {
    /// Search query to filter notes
    pub(crate) query: String,
    #[arg(long, env = "SPARES_RENDERED_FILE_OPEN_COMMAND")]
    pub(crate) open_command: Option<String>,
    #[arg(long, env = "SPARES_RENDERED_FILE_CLOSE_COMMAND")]
    pub(crate) close_command: Option<String>,
}

#[derive(Clone, Debug, Display, PartialEq)]
enum ViewNoteAction {
    #[strum(serialize = "Previous")]
    Previous,
    #[strum(serialize = "Next")]
    Next,
    #[strum(serialize = "Open Note")]
    OpenNote,
    #[strum(serialize = "Open Linked Notes")]
    OpenLinkedNotes,
    #[strum(serialize = "Sync Note")]
    SyncNote,
    #[strum(serialize = "Go to Note Number")]
    GoTo,
    #[strum(serialize = "Exit")]
    Exit,
}

fn display_note_info(note: &NoteResponse, parser_name: &str, index: usize, total: usize) {
    println!();
    println!("--- Note {} of {} ---", index + 1, total);
    println!("Note Id:    {}", note.id);
    println!("Parser:     {}", parser_name);
    println!("Keywords:   {}", note.keywords.join(", "));
    println!("Card Count: {}", note.card_count);
    if let Some(ref linked_notes) = note.linked_notes {
        println!("Linked:     {} note(s)", linked_notes.len());
    }
    println!();
}

#[derive(Args, Debug)]
pub(crate) struct ViewCardArgs {
    /// Search query to filter cards
    pub(crate) query: String,
    #[arg(long, env = "SPARES_RENDERED_FILE_OPEN_COMMAND")]
    pub(crate) open_command: Option<String>,
    #[arg(long, env = "SPARES_RENDERED_FILE_CLOSE_COMMAND")]
    pub(crate) close_command: Option<String>,
}

#[derive(Clone, Debug, Display, PartialEq)]
enum ViewCardAction {
    #[strum(serialize = "Prev")]
    Previous,
    #[strum(serialize = "Next")]
    Next,
    #[strum(serialize = "Open Note")]
    OpenNote,
    #[strum(serialize = "Sync Note")]
    SyncNote,
    #[strum(serialize = "Exit")]
    Exit,
}

fn display_card_info(card: &CardResponse, parser_name: &str, index: usize, total: usize) {
    println!();
    println!("--- Card {} of {} ---", index + 1, total);
    println!("Card Id:    {}", card.id);
    println!("Note Id:    {}", card.note_id);
    println!("Parser:     {}", parser_name);
    println!("Order:      {}", card.order);
    println!("State:      {}", card.state);
    println!(
        "Due:        {}",
        card.due.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S")
    );
    if let Some(ref special_state) = card.special_state {
        println!("Special:    {:?}", special_state);
    }
    println!();
}

async fn search_notes_api(
    query: String,
    output_type: QueryReturnItemType,
    base_url: &str,
    client: &Client,
) -> Result<SearchNotesResponse, String> {
    let request = SearchNotesRequest { query, output_type };
    let url = format!("{}/api/notes/search", base_url);
    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(message.unwrap().to_string());
    }
    response.json().await.map_err(|e| format!("{}", e))
}

fn prompt_select_action<'a, T: std::fmt::Display>(
    action_options: &'a [T],
    rendered_file_child: &mut Option<Child>,
    close_command: Option<&str>,
) -> Result<&'a T, ()> {
    let display_options: Vec<String> = action_options.iter().map(|a| a.to_string()).collect();
    let mut select = Select::new("Action:", display_options.clone());
    select.vim_mode = true;
    select.page_size = 10;
    let Ok(chosen_str) = select.prompt() else {
        if let Some(mut child) = rendered_file_child.take() {
            let _ = close_rendered_file(&mut child, close_command, true);
        }
        return Err(());
    };
    let chosen_idx = display_options
        .iter()
        .position(|o| o == &chosen_str)
        .unwrap();
    Ok(&action_options[chosen_idx])
}

#[expect(clippy::too_many_lines)]
pub(crate) async fn view_notes(
    view_args: ViewNoteArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let open_command = view_args.open_command.as_deref();
    let close_command = view_args.close_command.as_deref();

    let search_response = search_notes_api(
        view_args.query,
        QueryReturnItemType::Notes,
        base_url,
        client,
    )
    .await?;

    let notes = match search_response {
        SearchNotesResponse::Notes(notes) => notes,
        SearchNotesResponse::Cards(_) => {
            return Err("Expected notes search, got cards".to_string());
        }
    };

    if notes.is_empty() {
        println!("No notes found.");
        return Ok(());
    }

    // Fetch parsers for linked note resolution
    let parsers_url = format!("{}/api/parsers", base_url);
    let parsers_resp = client
        .get(&parsers_url)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let parser_list: Vec<ParserResponse> =
        parsers_resp.json().await.map_err(|e| format!("{}", e))?;
    let parser_map: HashMap<i64, String> =
        parser_list.into_iter().map(|p| (p.id, p.name)).collect();

    let total = notes.len();
    let mut index = 0;
    let mut rendered_file_child: Option<Child> = None;

    loop {
        if let Some(mut child) = rendered_file_child.take() {
            close_rendered_file(&mut child, close_command, false)?;
        }

        let (note, parser_name) = &notes[index];
        display_note_info(note, parser_name, index, total);

        if let Ok(path) = utils::compute_note_rendered_path(parser_name, note.id) {
            rendered_file_child = Some(open_rendered_file(path.as_ref(), open_command, false)?);
        }

        let mut action_options: Vec<ViewNoteAction> = vec![
            ViewNoteAction::Previous,
            ViewNoteAction::Next,
            ViewNoteAction::OpenNote,
        ];
        if note.linked_notes.as_ref().map_or(0, |v| v.len()) > 0 {
            action_options.push(ViewNoteAction::OpenLinkedNotes);
        }
        action_options.push(ViewNoteAction::SyncNote);
        action_options.push(ViewNoteAction::GoTo);
        action_options.push(ViewNoteAction::Exit);

        let Ok(chosen_action) =
            prompt_select_action(&action_options, &mut rendered_file_child, close_command)
        else {
            return Ok(());
        };

        match chosen_action {
            ViewNoteAction::Previous => {
                index = (index + total - 1) % total;
            }
            ViewNoteAction::Next => {
                index = (index + 1) % total;
            }
            ViewNoteAction::OpenNote => {
                if let Ok(path) = utils::compute_note_raw_path(parser_name, note.id) {
                    utils::open_file(&path);
                }
            }
            ViewNoteAction::OpenLinkedNotes => {
                if let Some(ref linked_notes) = note.linked_notes {
                    let ln_options: Vec<String> = linked_notes
                        .iter()
                        .map(|ln| {
                            format!(
                                "{} ({})",
                                ln.searched_keyword,
                                ln.linked_note_id.unwrap_or(0)
                            )
                        })
                        .collect();
                    let mut multi =
                        MultiSelect::new("Select linked notes to open:", ln_options.clone());
                    multi.vim_mode = true;
                    if let Ok(selected) = multi.prompt() {
                        for label in &selected {
                            if let Some(idx) = ln_options.iter().position(|o| o == label)
                                && let Some(linked_id) = linked_notes[idx].linked_note_id
                            {
                                let note_url = format!("{}/api/notes/{}", base_url, linked_id);
                                if let Ok(resp) = client.get(&note_url).send().await
                                    && resp.status() == StatusCode::OK
                                    && let Ok(linked_note) = resp.json::<NoteResponse>().await
                                    && let Some(pn) = parser_map.get(&linked_note.parser_id)
                                    && let Ok(path) = utils::compute_note_raw_path(pn, linked_id)
                                {
                                    utils::open_file(&path);
                                }
                            }
                        }
                    }
                }
            }
            ViewNoteAction::SyncNote => {
                println!("Syncing note...");
                if let Ok(path) = utils::compute_note_raw_path(parser_name, note.id) {
                    match sync_note(note.id, &path, parser_name, base_url, client).await {
                        Ok(()) => println!("Note synced successfully."),
                        Err(e) => println!("Failed to sync note: {e}"),
                    }
                }
            }
            ViewNoteAction::GoTo => {
                let prompt_text = format!("Enter note number (1-{total}):");
                let prompt = Text::new(&prompt_text);
                match prompt.prompt() {
                    Ok(input) => match input.trim().parse::<usize>() {
                        Ok(num) if num >= 1 && num <= total => {
                            index = num - 1;
                        }
                        _ => {
                            println!(
                                "Invalid note number. Please enter a number between 1 and {total}."
                            );
                        }
                    },
                    Err(_) => {
                        println!("Cancelled.");
                    }
                }
            }
            ViewNoteAction::Exit => {
                if let Some(mut child) = rendered_file_child.take() {
                    close_rendered_file(&mut child, close_command, true)?;
                }
                return Ok(());
            }
        }
    }
}
pub(crate) async fn view_cards(
    view_args: ViewCardArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let open_command = view_args.open_command.as_deref();
    let close_command = view_args.close_command.as_deref();

    let search_response = search_notes_api(
        view_args.query,
        QueryReturnItemType::Cards,
        base_url,
        client,
    )
    .await?;

    let cards = match search_response {
        SearchNotesResponse::Cards(cards) => cards,
        SearchNotesResponse::Notes(_) => {
            return Err("Expected cards search, got notes".to_string());
        }
    };

    if cards.is_empty() {
        println!("No cards found.");
        return Ok(());
    }

    let total = cards.len();
    let mut index = 0;
    let mut rendered_file_child: Option<Child> = None;

    loop {
        if let Some(mut child) = rendered_file_child.take() {
            close_rendered_file(&mut child, close_command, false)?;
        }

        let (card, parser_name) = &cards[index];
        display_card_info(card, parser_name, index, total);

        if let Ok(path) =
            utils::compute_card_rendered_back_path(parser_name, card.note_id, card.order)
        {
            rendered_file_child = Some(open_rendered_file(path.as_ref(), open_command, false)?);
        }

        let action_options: Vec<ViewCardAction> = vec![
            ViewCardAction::Previous,
            ViewCardAction::Next,
            ViewCardAction::OpenNote,
            ViewCardAction::SyncNote,
            ViewCardAction::Exit,
        ];

        let Ok(chosen_action) =
            prompt_select_action(&action_options, &mut rendered_file_child, close_command)
        else {
            return Ok(());
        };

        match chosen_action {
            ViewCardAction::Previous => {
                index = (index + total - 1) % total;
            }
            ViewCardAction::Next => {
                index = (index + 1) % total;
            }
            ViewCardAction::OpenNote => {
                if let Ok(path) = utils::compute_note_raw_path(parser_name, card.note_id) {
                    utils::open_file(&path);
                }
            }
            ViewCardAction::SyncNote => {
                println!("Syncing note...");
                if let Ok(path) = utils::compute_note_raw_path(parser_name, card.note_id) {
                    match sync_note(card.note_id, &path, parser_name, base_url, client).await {
                        Ok(()) => println!("Note synced successfully."),
                        Err(e) => println!("Failed to sync note: {e}"),
                    }
                }
            }
            ViewCardAction::Exit => {
                if let Some(mut child) = rendered_file_child.take() {
                    close_rendered_file(&mut child, close_command, true)?;
                }
                return Ok(());
            }
        }
    }
}
