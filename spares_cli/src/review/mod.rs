use crate::import::import_from_files;
use chrono::{Local, Utc};
use clap::Args;
use inquire::{MultiSelect, Select};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares_core::adapters::impls::spares::{SparesAdapter, SparesRequestProcessor};
use spares_core::config::read_external_config;
use spares_core::model::{NoteId, RatingId, TagId};
use spares_core::parsers::{find_parser, get_all_parsers};
use spares_core::schema::note::{NotesSelector, RenderNotesRequest};
use spares_core::schema::review::{
    CardBackRenderedPath, GetReviewCardFilterRequest, GetReviewCardRequest, GetReviewCardResponse,
    RatingSubmission, StatisticsRequest, StatisticsResponse,
};
use spares_core::schema::tag::TagResponse;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};
use strum::{EnumIter, IntoEnumIterator};
use strum_macros::{Display, EnumString};
use tokio::sync::mpsc;
use utils::{
    bury_card, bury_note, bury_until_later_today, close_rendered_file, format_duration,
    get_scheduler_ratings, open_rendered_file, print_rate_duration, print_recall_duration,
    print_summary, submit_rating, suspend_cards, suspend_note, tag_note,
};

mod utils;
use crate::review::utils::{note_id_to_cards, set_due_date, set_due_date_with_prompt};
use crate::utils::undo_event;
use spares_core::schema::card::CardResponse;
use spares_core::schema::undo::UndoEventRequest;
pub(crate) use utils::forget_card;

#[derive(Args, Debug)]
pub(crate) struct ReviewArgs {
    // Using `Option<FilterArgs>` here instead won't work since they `query` becomes a required parameter.
    #[command(flatten)]
    pub(crate) filter_args: FilterArgs,
    #[arg(short, long, default_value = "fsrs")]
    pub(crate) scheduler_name: String,
    #[arg(long, env = "SPARES_RENDERED_FILE_OPEN_COMMAND")]
    pub(crate) open_command: Option<String>,
    #[arg(long, env = "SPARES_RENDERED_FILE_CLOSE_COMMAND")]
    pub(crate) close_command: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct FilterArgs {
    /// Filter the cards due today with the supplied query
    #[arg(short, long)]
    pub(crate) query: Option<String>,
    /// Study a filtered tag with the supplied id
    #[arg(long, conflicts_with_all = ["query", "tag_name"])]
    pub(crate) tag_id: Option<TagId>,
    /// Study a filtered tag with the supplied name
    #[arg(short, long, conflicts_with_all = ["query", "tag_id"])]
    pub(crate) tag_name: Option<String>,
}

#[derive(Clone, Debug, Display, EnumIter, EnumString, PartialEq)]
enum ReviewAction {
    // Used to avoid accidentally pressing <Enter> twice and submitting a rating of 1 by accident
    #[strum(serialize = "-")]
    Loop,
    Flip,
    #[strum(to_string = "Rate: {description} ({id})")]
    Rate {
        id: RatingId,
        description: String,
    },
    #[strum(serialize = "Open Note")]
    OpenNote,
    #[strum(serialize = "Sync Note")]
    SyncNote,
    #[strum(serialize = "Bury Card")]
    BuryCard,
    #[strum(serialize = "Bury Note (card + siblings)")]
    BuryNote,
    #[strum(serialize = "Bury Until Later Today")]
    BuryUntilLaterToday,
    #[strum(serialize = "Open Linked Notes")]
    OpenLinkedNotes,
    #[strum(serialize = "Tag to modify later")]
    TagNote,
    #[strum(serialize = "Forget Card")]
    ForgetCard,
    #[strum(serialize = "Set Card Due Date")]
    SetCardDueDate,
    #[strum(to_string = "Set Card Due Date in {0}")]
    SetCardDueDateIn(String),
    #[strum(serialize = "Set Note Due Date")]
    SetNoteDueDate,
    #[strum(to_string = "Set Note Due Date in {0}")]
    SetNoteDueDateIn(String),
    #[strum(serialize = "Suspend Card")]
    SuspendCard,
    #[strum(serialize = "Suspend Note (card + siblings)")]
    SuspendNote,
    Undo,
    Exit,
}

async fn get_review_card(
    filter_args: &FilterArgs,
    open_command: Option<&str>,
    base_url: &str,
    client: &Client,
    first: bool,
) -> Result<Option<(GetReviewCardResponse, Child)>, String> {
    let url = format!("{}/api/review", base_url);
    let filter = if let Some(ref query) = filter_args.query {
        Some(GetReviewCardFilterRequest::Query(query.clone()))
    } else if let Some(ref tag_name) = filter_args.tag_name {
        let url = format!("{}/api/tags/name/{}", base_url, tag_name);
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("{}", e))?;
        let status = response.status();
        if status != StatusCode::OK {
            let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
            let message = response_json.get("message");
            return Err(message.unwrap().to_string());
        }
        let tag_response: TagResponse = response.json().await.map_err(|e| format!("{}", e))?;
        Some(GetReviewCardFilterRequest::FilteredTag {
            tag_id: tag_response.id,
        })
    } else {
        filter_args
            .tag_id
            .map(|tag_id| GetReviewCardFilterRequest::FilteredTag { tag_id })
    };
    let request = GetReviewCardRequest { filter };
    let response = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(message.unwrap().to_string());
    }
    let review_card_response: Option<GetReviewCardResponse> =
        response.json().await.map_err(|e| format!("{}", e))?;

    match review_card_response {
        Some(review_card) => {
            // Open rendered card
            let child = open_rendered_file(
                review_card.card_front_rendered_path.as_ref(),
                open_command,
                first,
            )?;
            println!("Note Id: {}", &review_card.note_id);
            println!("Card Id: {}", &review_card.card_id);
            println!(
                "Card Front File Name: {:?}",
                &review_card
                    .card_front_rendered_path
                    .file_name()
                    .unwrap()
                    .display()
            );

            // Display cards left by state
            if !review_card.cards_left_by_state.is_empty() {
                println!("Cards left by state for today:");
                let mut cards_left_by_state =
                    review_card.cards_left_by_state.iter().collect::<Vec<_>>();
                cards_left_by_state.sort_by_key(|(state_id, _)| *state_id);
                for (state_id, count) in cards_left_by_state {
                    let indicator = if *state_id == review_card.card_state {
                        "--> "
                    } else {
                        "    "
                    };
                    println!(" {}State {}: {}", indicator, state_id, count);
                }
            }
            println!(
                "Estimated Time Remaining: {}",
                format_duration(review_card.time_estimate)
            );
            println!(
                "Estimated Completion Time: {}",
                (Utc::now() + review_card.time_estimate)
                    .with_timezone(&Local)
                    .format("%H:%M:%S %P (%m-%d-%Y)")
            );

            Ok(Some((review_card, child)))
        }
        // No cards left to review
        None => Ok(None),
    }
}

async fn get_review_card_by_id(
    card_id: spares_core::model::CardId,
    base_url: &str,
    client: &Client,
) -> Result<Option<GetReviewCardResponse>, String> {
    let url = format!("{}/api/review/card/{}", base_url, card_id);
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json.get("message");
        return Err(message.unwrap().to_string());
    }
    response.json().await.map_err(|e| format!("{}", e))
}

async fn sync_note_background(
    note_id: NoteId,
    note_raw_path: PathBuf,
    parser_name: String,
    base_url: String,
    client: Client,
    tx: mpsc::UnboundedSender<Result<NoteId, String>>,
) {
    // Import note from local file to database
    let mut adapter = SparesAdapter::new(SparesRequestProcessor::Server);
    let parser = match find_parser(&parser_name, &get_all_parsers()) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(Err(format!(
                "[Note Id: {}] Failed to sync note: Failed to find parser: {}",
                note_id, e
            )));
            return;
        }
    };

    if let Err(e) = import_from_files(
        &mut adapter,
        Some(parser.as_ref()),
        None,
        &[&note_raw_path],
        false,
        true, // quiet mode
    )
    .await
    {
        let _ = tx.send(Err(format!(
            "[Note Id: {}] Failed to sync note: Failed to import note: {}",
            note_id, e
        )));
        return;
    }

    // Regenerate rendered files for this note
    let request = RenderNotesRequest {
        selector: NotesSelector::Ids(vec![note_id]),
        immutable_note_ids: None,
        overridden_output_raw_dir: None,
        include_linked_notes: true,
        include_cards: true,
        generate_rendered: true,
        force_generate_rendered: false,
    };
    let url = format!("{}/api/notes/generate_files", base_url);
    let response = match client.post(&url).json(&request).send().await {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(Err(format!(
                "[Note Id: {}] Failed to sync note: Failed to regenerate files: {}",
                note_id, e
            )));
            return;
        }
    };
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(format!(
                    "[Note Id: {}] Failed to sync note: Failed to parse error response: {}",
                    note_id, e
                )));
                return;
            }
        };
        let message = response_json.get("message");
        let _ = tx.send(Err(format!(
            "[Note Id: {}] Failed to sync note: {}",
            note_id,
            message.unwrap_or(&Value::String("Unknown error".to_string()))
        )));
        return;
    }

    let _ = tx.send(Ok(note_id));
}

async fn build_review_actions(
    scheduler_name: &str,
    set_card_due_date_duration_str: String,
    base_url: &str,
    client: &Client,
) -> Result<Vec<ReviewAction>, String> {
    let mut all_options = ReviewAction::iter()
        .filter(|x| {
            !matches!(
                *x,
                ReviewAction::Rate { .. }
                    | ReviewAction::SetCardDueDateIn(_)
                    | ReviewAction::SetNoteDueDateIn(_)
            )
        })
        .collect::<Vec<_>>();

    // Insert dynamic actions after the manual ones
    if let Some(pos) = all_options
        .iter()
        .position(|x| matches!(x, ReviewAction::SetCardDueDate))
    {
        all_options.insert(
            pos + 1,
            ReviewAction::SetCardDueDateIn(set_card_due_date_duration_str.clone()),
        );
    }
    if let Some(pos) = all_options
        .iter()
        .position(|x| matches!(x, ReviewAction::SetNoteDueDate))
    {
        all_options.insert(
            pos + 1,
            ReviewAction::SetNoteDueDateIn(set_card_due_date_duration_str),
        );
    }

    // Keep the ratings near the top (after the null action) so they are all visible
    all_options.splice(
        2..2,
        get_scheduler_ratings(scheduler_name, base_url, client).await?,
    );

    Ok(all_options)
}

fn spawn_stats_fetch(
    scheduler_name: &str,
    base_url: &str,
    client: &Client,
) -> mpsc::UnboundedReceiver<StatisticsResponse> {
    let (stats_tx, stats_rx) = mpsc::unbounded_channel::<StatisticsResponse>();
    let url = format!("{}/api/review/statistics", base_url);
    let client_clone = client.clone();
    let scheduler_name_clone = scheduler_name.to_string();
    tokio::spawn(async move {
        let request = StatisticsRequest {
            scheduler_name: scheduler_name_clone,
            date: Utc::now(),
        };
        if let Ok(response) = client_clone.post(&url).json(&request).send().await
            && response.status() == StatusCode::OK
            && let Ok(stats) = response.json::<StatisticsResponse>().await
        {
            let _ = stats_tx.send(stats);
        }
    });
    stats_rx
}

#[expect(clippy::too_many_lines)]
pub(crate) async fn review_cards(
    review_args: ReviewArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let open_command = review_args.open_command.as_deref();
    let close_command = review_args.close_command.as_deref();
    let scheduler_name = &review_args.scheduler_name;
    let tag_id = review_args.filter_args.tag_id;

    let review_card_opt = get_review_card(
        &review_args.filter_args,
        open_command,
        base_url,
        client,
        true,
    )
    .await?;

    if review_card_opt.is_none() {
        println!("Done");
        return Ok(());
    }

    let (mut review_card_response, mut card_front_rendered_child) = review_card_opt.unwrap();
    let config = read_external_config().map_err(|e| format!("{}", e))?;
    let flagged_tag_name = config.flagged_tag_name;
    let set_card_due_date_duration = config.set_card_due_date_duration;
    let set_card_due_date_duration_str = format_duration(set_card_due_date_duration);

    let all_options = build_review_actions(
        scheduler_name,
        set_card_due_date_duration_str,
        base_url,
        client,
    )
    .await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<Result<NoteId, String>>();

    // Spawn background task to fetch day statistics
    let mut stats_rx = spawn_stats_fetch(scheduler_name, base_url, client);

    let session_start = Instant::now();
    let mut session_recall_duration = Duration::default();
    let mut session_rate_duration = Duration::default();
    let mut reviewed_cards_count = 0;
    let mut card_back_rendered_child: Option<Child> = None;
    let mut card_flipped = false;
    let mut advance_review_card = false;

    let mut recall_start = Instant::now();
    let mut recall_duration = None;
    let mut rate_start = Instant::now();
    let mut rate_duration = None;
    let mut last_action_event_id: Option<i64> = None;
    let mut last_action_was_rating = false;
    let mut sync_advance_needed = false;

    loop {
        if advance_review_card || sync_advance_needed {
            sync_advance_needed = false;
            println!();
            println!();
            // Opening the card's raw file is not useful since edits must be made to the note, not the
            // card. Opening the note's raw file and the card's rendered file is more useful.
            let review_card_opt = get_review_card(
                &review_args.filter_args,
                open_command,
                base_url,
                client,
                false,
            )
            .await?;
            recall_start = Instant::now();
            recall_duration = None;
            if review_card_opt.is_none() {
                println!("Done");
                let day_stats = stats_rx.try_recv().ok();
                print_summary(
                    session_start,
                    session_recall_duration,
                    session_rate_duration,
                    reviewed_cards_count,
                    day_stats,
                );
                return Ok(());
            }
            (review_card_response, card_front_rendered_child) = review_card_opt.unwrap();
        }
        // Ask user for action
        let options = all_options
            .iter()
            .filter(|x| {
                if card_flipped {
                    !matches!(*x, ReviewAction::Flip)
                } else {
                    !matches!(*x, ReviewAction::Rate { .. } | ReviewAction::Loop)
                }
            })
            .collect::<Vec<_>>();
        let mut select = Select::new("Action:", options);
        select.vim_mode = true;
        select.page_size = 10;
        let chosen_action_res = select.prompt();
        if chosen_action_res.is_err() {
            // The user exited. (Probably pressed Escape).
            let day_stats = stats_rx.try_recv().ok();
            print_summary(
                session_start,
                session_recall_duration,
                session_rate_duration,
                reviewed_cards_count,
                day_stats,
            );
            return Ok(());
        }
        let chosen_action = chosen_action_res.as_ref().unwrap();

        advance_review_card = false;
        match chosen_action {
            ReviewAction::Loop => {}
            ReviewAction::Rate {
                description: _,
                id: rating_id,
            } => {
                card_flipped = false;

                // Close card back
                close_rendered_file(
                    &mut card_back_rendered_child.take().unwrap(),
                    close_command,
                    false,
                )?;

                // Rate duration
                let rate_duration_local = rate_duration.unwrap_or(rate_start.elapsed());
                session_rate_duration += rate_duration_local;
                print_rate_duration(rate_duration_local);

                reviewed_cards_count += 1;

                let rating_submission = RatingSubmission {
                    card_id: review_card_response.card_id,
                    rating: *rating_id,
                    recall_duration: chrono::Duration::from_std(recall_duration.unwrap()).unwrap(),
                    rate_duration: chrono::Duration::from_std(rate_duration_local).unwrap(),
                    tag_id,
                };
                last_action_event_id =
                    submit_rating(scheduler_name, rating_submission, base_url, client).await?;
                last_action_was_rating = true;

                // let old_card_rendered_path = review_card_response.card_rendered_path;

                // Advance to next review card
                advance_review_card = true;

                // Close card
                // This is done after the new card is opened to ensure the file viewer
                // always has at least 1 open tab. That way the screen doesn't flash.
                // close_rendered_file(&old_card_rendered_path);
            }
            ReviewAction::Flip => {
                card_flipped = true;

                // Close card
                close_rendered_file(&mut card_front_rendered_child, close_command, false)?;

                // Open card back to see answer
                // The duration is calculated before the card back is opened since the user already
                // recalled (or failed to recall) the card at this point. Flipping the card just
                // allows them to check if they are correct. This extra time should not count
                // towards the duration.
                // This is only done if the `recall_duation.is_none()` because a user might do
                // `StartReview -> OpenNote -> Flip -> RateX` in which case the duration is already
                // recorded during `OpenNote`.
                if recall_duration.is_none() {
                    recall_duration = Some(recall_start.elapsed());
                    session_recall_duration += recall_duration.unwrap();
                    print_recall_duration(recall_duration.unwrap());
                }
                let card_back_rendered_path = match &review_card_response.card_back_rendered_path {
                    CardBackRenderedPath::CardBack(path_buf)
                    | CardBackRenderedPath::Note(path_buf) => path_buf,
                };
                card_back_rendered_child = Some(open_rendered_file(
                    card_back_rendered_path,
                    open_command,
                    false,
                )?);
                rate_start = Instant::now();
                rate_duration = None;
            }
            ReviewAction::OpenNote => {
                // If the note is viewed before the card is flipped, then the answer is revealed.
                // This means that the user already recalled (or failed to recall) the card at this
                // point.
                if card_flipped {
                    // If the card is flipped, then the rate duration is started. We want to stop
                    // recording now since the user is editting the note. This will take time and
                    // not be representative of time spent every review.
                    rate_duration = Some(rate_start.elapsed());
                } else {
                    recall_duration = Some(recall_start.elapsed());
                    session_recall_duration += recall_duration.unwrap();
                    print_recall_duration(recall_duration.unwrap());
                }
                let open_note_res = open::that_detached(&review_card_response.note_raw_path);
                if let Err(e) = open_note_res {
                    println!("{}", e);
                }
            }
            ReviewAction::OpenLinkedNotes => {
                if review_card_response.linked_notes.is_empty() {
                    println!("No linked notes found.");
                } else {
                    let options = review_card_response
                        .linked_notes
                        .iter()
                        .map(|ln| format!("{} ({})", ln.searched_keyword, ln.note_id))
                        .collect::<Vec<_>>();
                    let mut multi_select =
                        MultiSelect::new("Select linked notes to open:", options.clone());
                    multi_select.vim_mode = true;
                    if let Ok(selected_labels) = multi_select.prompt() {
                        for label in &selected_labels {
                            if let Some(idx) = options.iter().position(|o| o == label) {
                                let path = &review_card_response.linked_notes[idx].note_raw_path;
                                if let Err(e) = open::that_detached(path) {
                                    println!("{}", e);
                                }
                            }
                        }
                    }
                }
            }
            ReviewAction::BuryCard
            | ReviewAction::BuryNote
            | ReviewAction::SuspendCard
            | ReviewAction::SuspendNote
            | ReviewAction::ForgetCard
            | ReviewAction::SetCardDueDate
            | ReviewAction::SetCardDueDateIn(_)
            | ReviewAction::SetNoteDueDate
            | ReviewAction::SetNoteDueDateIn(_)
            | ReviewAction::BuryUntilLaterToday => {
                last_action_was_rating = false;
                match chosen_action {
                    ReviewAction::BuryCard => {
                        last_action_event_id = bury_card(
                            scheduler_name,
                            review_card_response.card_id,
                            base_url,
                            client,
                        )
                        .await?;
                    }
                    ReviewAction::BuryNote => {
                        last_action_event_id =
                            bury_note(review_card_response.note_id, base_url, client).await?;
                    }
                    ReviewAction::SuspendCard => {
                        last_action_event_id =
                            suspend_cards(&[review_card_response.card_id], base_url, client)
                                .await?;
                    }
                    ReviewAction::SuspendNote => {
                        last_action_event_id =
                            suspend_note(review_card_response.note_id, base_url, client).await?;
                    }
                    ReviewAction::ForgetCard => {
                        let forget_response =
                            forget_card(review_card_response.card_id, base_url, client).await?;
                        last_action_event_id = forget_response.event_id;
                        println!("Card forgotten (scheduling reset):");
                        println!("{:#?}", &forget_response.card);
                    }
                    ReviewAction::SetCardDueDate => {
                        let result = set_due_date_with_prompt(
                            |_| vec![review_card_response.card_id],
                            base_url,
                            client,
                        )
                        .await?;
                        match result {
                            None => continue,
                            Some(event_id) => {
                                last_action_event_id = event_id;
                                println!("Due date updated.");
                            }
                        }
                    }
                    ReviewAction::SetCardDueDateIn(_) => {
                        let due_date = Utc::now() + set_card_due_date_duration;
                        last_action_event_id = set_due_date(
                            vec![review_card_response.card_id],
                            due_date,
                            base_url,
                            client,
                        )
                        .await?;
                        println!("Due date updated.");
                    }
                    ReviewAction::SetNoteDueDate => {
                        let cards: Vec<CardResponse> =
                            note_id_to_cards(review_card_response.note_id, base_url, client)
                                .await?;
                        let result = set_due_date_with_prompt(
                            |dt| {
                                cards
                                    .iter()
                                    .filter(|card| card.due <= dt)
                                    .map(|card| card.id)
                                    .collect::<Vec<_>>()
                            },
                            base_url,
                            client,
                        )
                        .await?;
                        match result {
                            None => continue,
                            Some(event_id) => {
                                last_action_event_id = event_id;
                                println!("Due date updated.");
                            }
                        }
                    }
                    ReviewAction::SetNoteDueDateIn(_) => {
                        let cards: Vec<CardResponse> =
                            note_id_to_cards(review_card_response.note_id, base_url, client)
                                .await?;
                        let due_date = Utc::now() + set_card_due_date_duration;
                        let card_ids = cards
                            .into_iter()
                            .filter(|card| card.due <= due_date)
                            .map(|card| card.id)
                            .collect::<Vec<_>>();
                        last_action_event_id =
                            set_due_date(card_ids, due_date, base_url, client).await?;
                        println!("Due date updated.");
                    }
                    ReviewAction::BuryUntilLaterToday => {
                        last_action_event_id =
                            bury_until_later_today(review_card_response.card_id, base_url, client)
                                .await?;
                        println!("Card due date set to end of today.");
                    }
                    _ => unreachable!(),
                }
                if card_flipped {
                    // Close card back
                    close_rendered_file(
                        &mut card_back_rendered_child.take().unwrap(),
                        close_command,
                        false,
                    )?;
                } else {
                    // Close card front
                    close_rendered_file(&mut card_front_rendered_child, close_command, false)?;
                }
                card_flipped = false;

                // Advance to next review card
                advance_review_card = true;
            }
            ReviewAction::TagNote => {
                last_action_event_id = tag_note(
                    review_card_response.note_id,
                    &flagged_tag_name,
                    base_url,
                    client,
                )
                .await?;
                last_action_was_rating = false;
            }
            ReviewAction::SyncNote => {
                println!("Syncing note in background...");

                // Clone values needed for background task
                let note_id = review_card_response.note_id;
                let note_raw_path = review_card_response.note_raw_path.clone();
                let parser_name = review_card_response.parser_name.clone();
                let base_url_string = base_url.to_string();
                let client_clone = client.clone();
                let tx_clone = tx.clone();

                // Spawn background task
                tokio::spawn(async move {
                    sync_note_background(
                        note_id,
                        note_raw_path,
                        parser_name,
                        base_url_string,
                        client_clone,
                        tx_clone,
                    )
                    .await;
                });
            }
            ReviewAction::Undo => {
                if card_flipped {
                    // Close card back
                    close_rendered_file(
                        &mut card_back_rendered_child.take().unwrap(),
                        close_command,
                        false,
                    )?;

                    // Reopen card front for the current card
                    card_front_rendered_child = open_rendered_file(
                        &review_card_response.card_front_rendered_path,
                        open_command,
                        false,
                    )?;

                    card_flipped = false;

                    // Reset stopwatches: undo the flip by removing the recorded recall duration
                    // and restarting the recall timer from now.
                    if let Some(d) = recall_duration.take() {
                        session_recall_duration = session_recall_duration.saturating_sub(d);
                    }
                    recall_start = Instant::now();
                    rate_duration = None;
                } else {
                    // Close card front
                    close_rendered_file(&mut card_front_rendered_child, close_command, false)?;

                    // Undo the latest review action, using the tracked event id so that
                    // background syncs in another window don't cause the wrong event to be undone.
                    let request = UndoEventRequest {
                        event_id: last_action_event_id.take(),
                        undo_group: true,
                    };
                    let undo_response_opt = undo_event(base_url, client, request).await?;
                    match undo_response_opt {
                        Some(undo_response) => {
                            println!("Undone event(s): {:?}", undo_response.undone_event_ids);
                        }
                        None => {
                            println!("No event to undo.");
                        }
                    }

                    if last_action_was_rating && reviewed_cards_count > 0 {
                        reviewed_cards_count -= 1;
                    }
                    last_action_was_rating = false;

                    // Advance to next review card which will be the previous card
                    advance_review_card = true;
                }
            }
            ReviewAction::Exit => {
                close_rendered_file(&mut card_front_rendered_child, close_command, true)?;
                if let Some(mut child) = card_back_rendered_child {
                    close_rendered_file(&mut child, close_command, true)?;
                }
                let day_stats = stats_rx.try_recv().ok();
                print_summary(
                    session_start,
                    session_recall_duration,
                    session_rate_duration,
                    reviewed_cards_count,
                    day_stats,
                );
                return Ok(());
            }
        }

        // Drain any pending messages from background sync tasks
        while let Ok(result) = rx.try_recv() {
            match result {
                Ok(synced_note_id) => {
                    println!("[Note Id: {}] Note synced successfully.", synced_note_id);
                    // If the synced note is the one being reviewed and we are not already
                    // advancing, refresh the current card's paths since card ordering may
                    // have shifted (e.g. a cloze was deleted).
                    if synced_note_id == review_card_response.note_id && !advance_review_card {
                        match get_review_card_by_id(review_card_response.card_id, base_url, client)
                            .await
                        {
                            Ok(Some(new_response)) => {
                                println!(
                                    "Current card refreshed after sync (card order may have changed)."
                                );
                                if card_flipped {
                                    if let Some(mut child) = card_back_rendered_child.take() {
                                        close_rendered_file(&mut child, close_command, false)?;
                                    }
                                    let back_path = match &new_response.card_back_rendered_path {
                                        CardBackRenderedPath::CardBack(p)
                                        | CardBackRenderedPath::Note(p) => p.clone(),
                                    };
                                    card_back_rendered_child =
                                        Some(open_rendered_file(&back_path, open_command, false)?);
                                } else {
                                    close_rendered_file(
                                        &mut card_front_rendered_child,
                                        close_command,
                                        false,
                                    )?;
                                    card_front_rendered_child = open_rendered_file(
                                        &new_response.card_front_rendered_path,
                                        open_command,
                                        false,
                                    )?;
                                }
                                review_card_response = new_response;
                            }
                            Ok(None) => {
                                println!(
                                    "Current card was deleted during sync. Advancing to next card."
                                );
                                if card_flipped {
                                    if let Some(mut child) = card_back_rendered_child.take() {
                                        close_rendered_file(&mut child, close_command, false)?;
                                    }
                                } else {
                                    close_rendered_file(
                                        &mut card_front_rendered_child,
                                        close_command,
                                        false,
                                    )?;
                                }
                                card_flipped = false;
                                sync_advance_needed = true;
                            }
                            Err(e) => {
                                println!("Failed to refresh card after sync: {}", e);
                            }
                        }
                    }
                }
                Err(msg) => println!("{}", msg),
            }
        }
    }
}
