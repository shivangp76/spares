use std::path::Path;
use std::process::Child;
use std::time::Duration;
use std::time::Instant;

use chrono::Utc;
use clap::Args;
use inquire::MultiSelect;
use inquire::Select;
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::Value;
use spares_core::adapters::impls::spares::SparesAdapter;
use spares_core::adapters::impls::spares::SparesRequestProcessor;
use spares_core::config::read_external_config;
use spares_core::model::NoteId;
use spares_core::model::RatingId;
use spares_core::model::TagId;
use spares_core::parsers::find_parser;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::note::NotesSelector;
use spares_core::schema::note::RenderNotesRequest;
use spares_core::schema::review::CardBackRenderedPath;
use spares_core::schema::review::CliReviewInfo;
use spares_core::schema::review::GetReviewCardFilterRequest;
use spares_core::schema::review::GetReviewCardRequest;
use spares_core::schema::review::GetReviewCardResponse;
use spares_core::schema::review::Rating;
use spares_core::schema::review::RatingSubmission;
use spares_core::schema::review::StatisticsRequest;
use spares_core::schema::review::StatisticsResponse;
use spares_core::schema::tag::TagResponse;
use strum::EnumIter;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumString;
use tokio::sync::mpsc;
use utils::bury_card;
use utils::bury_note;
use utils::bury_until_later_today;
use utils::close_rendered_file;
use utils::format_duration;
use utils::get_rating_from_score;
use utils::get_scheduler_ratings;
use utils::open_rendered_file;
use utils::parse_cli_score;
use utils::print_rate_duration;
use utils::print_recall_duration;
use utils::print_summary;
use utils::spawn_cli_exec;
use utils::submit_rating;
use utils::suspend_cards;
use utils::suspend_note;
use utils::tag_note;

use crate::import::import_from_files;

pub(crate) mod utils;
use spares_core::parsers::cloze_tag_str;
use spares_core::schema::card::CardResponse;
use spares_core::schema::undo::UndoEventRequest;
pub(crate) use utils::forget_card;

use crate::review::utils::note_id_to_cards;
use crate::review::utils::set_due_date;
use crate::review::utils::set_due_date_with_prompt;
use crate::utils::undo_event;

#[derive(Args, Debug)]
pub(crate) struct ReviewArgs {
    // Using `Option<FilterArgs>` here instead won't work since they `query` becomes a required parameter.
    #[command(flatten)]
    pub(crate) filter_args: FilterArgs,
    #[arg(short, long, default_value = "fsrs")]
    pub(crate) scheduler_name: String,
    #[arg(long, env = "SPARES_RENDERED_FILE_OPEN_COMMAND")]
    pub(crate) open_command: Option<String>,
    #[arg(long, env = "SPARES_RENDERED_FILE_OPEN_COMMAND_CARD")]
    pub(crate) open_command_card: Option<String>,
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
    #[strum(serialize = "Browse Keywords")]
    BrowseKeywords,
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
) -> Result<Option<(GetReviewCardResponse, Option<Child>)>, String> {
    let url = format!("{}/api/review", base_url);
    let filter = filter_args_to_filter(filter_args, base_url, client).await?;
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
        let message = response_json
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(message.to_string());
    }
    let review_card_response: Option<GetReviewCardResponse> =
        response.json().await.map_err(|e| format!("{}", e))?;

    match review_card_response {
        Some(review_card) => {
            let is_cli = review_card.cli.is_some();
            // Open rendered card. CLI cards have no rendered document;
            // their review is driven by an external command spawned later in
            // the loop, so there is nothing to open here.
            let child = if is_cli {
                None
            } else {
                Some(open_rendered_file(
                    review_card.card_front_rendered_path.as_ref(),
                    open_command,
                    Some(&cloze_tag_str(review_card.note_id, review_card.card_order)),
                    first,
                )?)
            };
            println!("Note Id: {}", review_card.note_id);
            println!("Card Id: {}", review_card.card_id);
            if !is_cli {
                let file_name = review_card
                    .card_front_rendered_path
                    .file_name()
                    .map_or_else(|| "<unknown>".to_string(), |f| f.display().to_string());
                println!("Card Front File Name: {file_name:?}");
            }
            utils::print_cards_left_by_state_and_time_estimate(&review_card);

            if is_cli && let Some(cli) = &review_card.cli {
                println!("\n{}", cli.surrounding);
            }

            Ok(Some((review_card, child)))
        }
        // No cards left to review
        None => Ok(None),
    }
}

async fn filter_args_to_filter(
    filter_args: &FilterArgs,
    base_url: &str,
    client: &Client,
) -> Result<Option<GetReviewCardFilterRequest>, String> {
    if let Some(ref query) = filter_args.query {
        Ok(Some(GetReviewCardFilterRequest::Query(query.clone())))
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
            let message = response_json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            return Err(message.to_string());
        }
        let tag_response: TagResponse = response.json().await.map_err(|e| format!("{}", e))?;
        Ok(Some(GetReviewCardFilterRequest::FilteredTag {
            tag_id: tag_response.id,
        }))
    } else {
        Ok(filter_args
            .tag_id
            .map(|tag_id| GetReviewCardFilterRequest::FilteredTag { tag_id }))
    }
}

async fn get_review_card_by_id(
    card_id: spares_core::model::CardId,
    filter: Option<GetReviewCardFilterRequest>,
    base_url: &str,
    client: &Client,
) -> Result<Option<GetReviewCardResponse>, String> {
    let url = format!("{}/api/review/card/{}", base_url, card_id);
    let request = GetReviewCardRequest { filter };
    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let response_json: Value = response.json().await.map_err(|e| format!("{}", e))?;
        let message = response_json
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(message.to_string());
    }
    response.json().await.map_err(|e| format!("{}", e))
}

pub(crate) async fn sync_note(
    note_id: NoteId,
    note_raw_path: &Path,
    parser_name: &str,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let mut adapter = SparesAdapter::new(SparesRequestProcessor::Server);
    let parser = find_parser(parser_name, &get_all_parsers())
        .map_err(|e| format!("Failed to find parser: {e}"))?;

    import_from_files(
        &mut adapter,
        Some(parser.as_ref()),
        None,
        &[note_raw_path],
        false,
        true,
    )
    .await
    .map_err(|e| format!("Failed to import note: {e}"))?;

    let request = RenderNotesRequest {
        selector: NotesSelector::Ids(vec![note_id]),
        immutable_note_ids: None,
        overridden_output_raw_dir: None,
        include_linked_notes: true,
        include_cards: true,
        generate_rendered: true,
        force_generate_rendered: false,
    };
    let url = format!("{base_url}/api/notes/generate_files");
    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Failed to regenerate files: {e}"))?;
    if response.status() != StatusCode::OK {
        let response_json: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse error response: {e}"))?;
        let message = response_json
            .get("message")
            .unwrap_or(&Value::String("Unknown error".to_string()))
            .to_string();
        return Err(message);
    }

    Ok(())
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

async fn setup_cli_card_state(
    review_card_response: &GetReviewCardResponse,
    scheduler_name: &str,
    base_url: &str,
    client: &Client,
) -> Result<(Rating, Duration), String> {
    let CliReviewInfo {
        exec,
        surrounding: _,
    } = review_card_response
        .cli
        .as_ref()
        .ok_or_else(|| "CLI card response missing `cli` field".to_string())?;

    let (stdout, recall_duration) = spawn_cli_exec(exec).await?;
    let score = parse_cli_score(&stdout)?;
    let rating = get_rating_from_score(scheduler_name, score, base_url, client).await?;
    println!("cli score: {:.3} → {}", score, rating.description);
    Ok((rating, recall_duration))
}

fn build_action_menu(
    all_options: &[ReviewAction],
    card_flipped: bool,
    cli_rating: Option<Rating>,
) -> Vec<ReviewAction> {
    let is_cli = cli_rating.is_some();
    let mut options: Vec<ReviewAction> = all_options
        .iter()
        .filter(|x| {
            if is_cli {
                !matches!(
                    **x,
                    ReviewAction::Flip | ReviewAction::Rate { .. } | ReviewAction::Loop
                )
            } else if card_flipped {
                !matches!(**x, ReviewAction::Flip)
            } else {
                !matches!(**x, ReviewAction::Rate { .. } | ReviewAction::Loop)
            }
        })
        .cloned()
        .collect();
    if let Some(rating) = cli_rating {
        options.insert(
            0,
            ReviewAction::Rate {
                id: rating.id,
                description: rating.description,
            },
        );
    }
    options
}

/// Result of handling a CLI exec error.
enum CliExecErrorAction {
    Exit,
    Advance,
}

/// Show an error prompt for a failed CLI exec and return the chosen action.
fn handle_cli_exec_error(
    e: &str,
    stats_rx: &mut mpsc::UnboundedReceiver<StatisticsResponse>,
    session_start: Instant,
    session_recall_duration: Duration,
    session_rate_duration: Duration,
    reviewed_cards_count: u32,
) -> CliExecErrorAction {
    println!("Error reviewing cli card: {e}");
    let mut select = Select::new("Action:", vec![ReviewAction::Loop, ReviewAction::Exit]);
    select.vim_mode = true;
    match select.prompt() {
        Ok(ReviewAction::Exit) | Err(_) => {
            let day_stats = stats_rx.try_recv().ok();
            print_summary(
                session_start,
                session_recall_duration,
                session_rate_duration,
                reviewed_cards_count,
                day_stats,
            );
            CliExecErrorAction::Exit
        }
        Ok(_) => CliExecErrorAction::Advance,
    }
}

/// Apply the result of a successful CLI card setup to the review loop state.
#[expect(clippy::too_many_arguments)]
fn apply_cli_setup_state(
    rating: Rating,
    exec_duration: Duration,
    pending_cli_rating: &mut Option<Rating>,
    recall_duration: &mut Option<Duration>,
    session_recall_duration: &mut Duration,
    card_flipped: &mut bool,
    rate_start: &mut Instant,
    rate_duration: &mut Option<Duration>,
) {
    *pending_cli_rating = Some(rating);
    *recall_duration = Some(exec_duration);
    *session_recall_duration += exec_duration;
    print_recall_duration(exec_duration);
    *card_flipped = true;
    *rate_start = Instant::now();
    *rate_duration = None;
}

#[expect(clippy::too_many_lines)]
pub(crate) async fn review_cards(
    review_args: ReviewArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let open_command = review_args.open_command.as_deref();
    let open_command_card = review_args.open_command_card.as_deref();
    let open_command_card_used = open_command_card.or(open_command);
    let close_command = review_args.close_command.as_deref();
    let scheduler_name = &review_args.scheduler_name;
    let tag_id = review_args.filter_args.tag_id;

    let review_card_opt = get_review_card(
        &review_args.filter_args,
        open_command_card_used,
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
    let mut pending_cli_rating: Option<Rating> = None;

    if review_card_response.cli.is_some() {
        match setup_cli_card_state(&review_card_response, scheduler_name, base_url, client).await {
            Ok((rating, exec_duration)) => {
                apply_cli_setup_state(
                    rating,
                    exec_duration,
                    &mut pending_cli_rating,
                    &mut recall_duration,
                    &mut session_recall_duration,
                    &mut card_flipped,
                    &mut rate_start,
                    &mut rate_duration,
                );
            }
            Err(e) => {
                if matches!(
                    handle_cli_exec_error(
                        e.as_str(),
                        &mut stats_rx,
                        session_start,
                        session_recall_duration,
                        session_rate_duration,
                        reviewed_cards_count,
                    ),
                    CliExecErrorAction::Exit,
                ) {
                    return Ok(());
                }
                advance_review_card = true;
            }
        }
    }

    loop {
        if advance_review_card {
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

            pending_cli_rating = None;
            card_flipped = false;
            if review_card_response.cli.is_some() {
                match setup_cli_card_state(&review_card_response, scheduler_name, base_url, client)
                    .await
                {
                    Ok((rating, exec_duration)) => {
                        apply_cli_setup_state(
                            rating,
                            exec_duration,
                            &mut pending_cli_rating,
                            &mut recall_duration,
                            &mut session_recall_duration,
                            &mut card_flipped,
                            &mut rate_start,
                            &mut rate_duration,
                        );
                    }
                    Err(e) => {
                        if matches!(
                            handle_cli_exec_error(
                                e.as_str(),
                                &mut stats_rx,
                                session_start,
                                session_recall_duration,
                                session_rate_duration,
                                reviewed_cards_count,
                            ),
                            CliExecErrorAction::Exit,
                        ) {
                            return Ok(());
                        }
                        advance_review_card = true;
                        continue;
                    }
                }
            }
        }
        // Ask user for action
        let options = build_action_menu(&all_options, card_flipped, pending_cli_rating.clone());
        let mut select = Select::new("Action:", options);
        select.vim_mode = true;
        select.page_size = 10;
        let Ok(chosen_action) = select.prompt() else {
            let day_stats = stats_rx.try_recv().ok();
            print_summary(
                session_start,
                session_recall_duration,
                session_rate_duration,
                reviewed_cards_count,
                day_stats,
            );
            return Ok(());
        };

        advance_review_card = false;
        match &chosen_action {
            ReviewAction::Loop => {}
            ReviewAction::Rate {
                description: _,
                id: rating_id,
            } => {
                card_flipped = false;

                // Close card back
                if let Some(mut child) = card_back_rendered_child.take() {
                    close_rendered_file(&mut child, close_command, false)?;
                }

                // Rate duration
                let rate_duration_local = rate_duration.unwrap_or(rate_start.elapsed());
                session_rate_duration += rate_duration_local;
                print_rate_duration(rate_duration_local);

                reviewed_cards_count += 1;

                let rating_submission = RatingSubmission {
                    card_id: review_card_response.card_id,
                    rating: *rating_id,
                    recall_duration: chrono::Duration::from_std(
                        recall_duration.expect("recall_duration set before Rate"),
                    )
                    .expect("recall_duration fits chrono::Duration"),
                    rate_duration: chrono::Duration::from_std(rate_duration_local)
                        .expect("rate_duration_local fits chrono::Duration"),
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
                if let Some(child) = card_front_rendered_child.as_mut() {
                    close_rendered_file(child, close_command, false)?;
                }

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
                    let rd = recall_duration.expect("recall_duration set in Flip guard");
                    session_recall_duration += rd;
                    print_recall_duration(rd);
                }
                let card_back_rendered_path = match &review_card_response.card_back_rendered_path {
                    CardBackRenderedPath::CardBack(path_buf)
                    | CardBackRenderedPath::Note(path_buf) => path_buf,
                };
                card_back_rendered_child = Some(open_rendered_file(
                    card_back_rendered_path,
                    open_command_card_used,
                    Some(&cloze_tag_str(
                        review_card_response.note_id,
                        review_card_response.card_order,
                    )),
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
                    let rd = recall_duration.expect("recall_duration set in OpenNote guard");
                    session_recall_duration += rd;
                    print_recall_duration(rd);
                }
                let open_note_res = open::that_detached(&review_card_response.note_raw_path);
                if let Err(e) = open_note_res {
                    println!("{}", e);
                }
            }
            ReviewAction::BrowseKeywords => {
                if review_card_response.keywords.is_empty() {
                    println!("No keywords found.");
                } else {
                    let keyword_select =
                        Select::new("Select a keyword:", review_card_response.keywords.clone());
                    if let Ok(selected_keyword) = keyword_select.prompt() {
                        utils::browse_keyword_notes(&selected_keyword, base_url, client).await?;
                    }
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
                match &chosen_action {
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
                        println!("{:#?}", forget_response.card);
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
                    if let Some(mut child) = card_back_rendered_child.take() {
                        close_rendered_file(&mut child, close_command, false)?;
                    }
                } else {
                    // Close card front
                    if let Some(child) = card_front_rendered_child.as_mut() {
                        close_rendered_file(child, close_command, false)?;
                    }
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
                println!("Syncing note...");

                // Phase 1: Import (DB update) — await inline so that get_review_card_by_id
                // sees the latest state before we re-prompt.
                let mut adapter = SparesAdapter::new(SparesRequestProcessor::Server);
                let parser = find_parser(&review_card_response.parser_name, &get_all_parsers())
                    .map_err(|e| format!("Failed to find parser: {e}"))?;

                import_from_files(
                    &mut adapter,
                    Some(parser.as_ref()),
                    None,
                    &[review_card_response.note_raw_path.as_path()],
                    false,
                    true,
                )
                .await
                .map_err(|e| format!("Failed to import note: {e}"))?;

                let note_id = review_card_response.note_id;

                // Check whether the current card still exists after the import.
                let active_filter = match filter_args_to_filter(
                    &review_args.filter_args,
                    base_url,
                    client,
                )
                .await
                {
                    Ok(f) => f,
                    Err(e) => {
                        println!(
                            "Warning: Could not resolve filter context: {e}. Refreshing card without filter context."
                        );
                        None
                    }
                };
                match get_review_card_by_id(
                    review_card_response.card_id,
                    active_filter,
                    base_url,
                    client,
                )
                .await
                {
                    Ok(Some(new_response)) => {
                        println!(
                            "[Note Id: {}] Current card refreshed after sync (card order may have changed).",
                            note_id
                        );
                        let is_cli = new_response.cli.is_some();
                        if card_flipped {
                            if let Some(mut child) = card_back_rendered_child.take() {
                                close_rendered_file(&mut child, close_command, false)?;
                            }
                            if !is_cli {
                                let back_path = match &new_response.card_back_rendered_path {
                                    CardBackRenderedPath::CardBack(p)
                                    | CardBackRenderedPath::Note(p) => p.clone(),
                                };
                                card_back_rendered_child = Some(open_rendered_file(
                                    &back_path,
                                    open_command_card_used,
                                    Some(&cloze_tag_str(
                                        new_response.note_id,
                                        new_response.card_order,
                                    )),
                                    false,
                                )?);
                            }
                        } else {
                            if let Some(child) = card_front_rendered_child.as_mut() {
                                close_rendered_file(child, close_command, false)?;
                            }
                            if !is_cli {
                                card_front_rendered_child = Some(open_rendered_file(
                                    &new_response.card_front_rendered_path,
                                    open_command_card_used,
                                    Some(&cloze_tag_str(
                                        new_response.note_id,
                                        new_response.card_order,
                                    )),
                                    false,
                                )?);
                            }
                        }
                        review_card_response = new_response;
                        utils::print_cards_left_by_state_and_time_estimate(&review_card_response);
                    }
                    Ok(None) => {
                        println!(
                            "[Note Id: {}] Current card was deleted during sync. Advancing to next card.",
                            note_id
                        );
                        if card_flipped {
                            if let Some(mut child) = card_back_rendered_child.take() {
                                close_rendered_file(&mut child, close_command, false)?;
                            }
                        } else if let Some(child) = card_front_rendered_child.as_mut() {
                            close_rendered_file(child, close_command, false)?;
                        }
                        card_flipped = false;
                        advance_review_card = true;
                    }
                    Err(e) => {
                        println!(
                            "[Note Id: {}] Failed to refresh card after sync: {}",
                            note_id, e
                        );
                    }
                }

                // Phase 2: File rendering — spawn in background (slower, non-blocking).
                let base_url_string = base_url.to_string();
                let client_clone = client.clone();
                tokio::spawn(async move {
                    let request = RenderNotesRequest {
                        selector: NotesSelector::Ids(vec![note_id]),
                        immutable_note_ids: None,
                        overridden_output_raw_dir: None,
                        include_linked_notes: true,
                        include_cards: true,
                        generate_rendered: true,
                        force_generate_rendered: false,
                    };
                    let url = format!("{base_url_string}/api/notes/generate_files");
                    let response = client_clone.post(&url).json(&request).send().await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status();
                            if status != StatusCode::OK {
                                match resp.json::<Value>().await {
                                    Ok(json) => {
                                        let msg = json
                                            .get("message")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("Unknown error");
                                        eprintln!(
                                            "[Note Id: {note_id}] Failed to regenerate files: {msg}"
                                        );
                                    }
                                    Err(parse_err) => {
                                        eprintln!(
                                            "[Note Id: {note_id}] Failed to regenerate files (status={}, parse_error={})",
                                            status, parse_err
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => eprintln!("[Note Id: {note_id}] Failed to regenerate files: {e}"),
                    }
                });
            }
            ReviewAction::Undo => {
                if card_flipped && pending_cli_rating.is_none() {
                    // Close card back
                    if let Some(mut child) = card_back_rendered_child.take() {
                        close_rendered_file(&mut child, close_command, false)?;
                    }

                    // Reopen card front for the current card
                    card_front_rendered_child = Some(open_rendered_file(
                        &review_card_response.card_front_rendered_path,
                        open_command_card_used,
                        Some(&cloze_tag_str(
                            review_card_response.note_id,
                            review_card_response.card_order,
                        )),
                        false,
                    )?);

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
                    if let Some(child) = card_front_rendered_child.as_mut() {
                        close_rendered_file(child, close_command, false)?;
                    }

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
                if let Some(child) = card_front_rendered_child.as_mut() {
                    close_rendered_file(child, close_command, true)?;
                }
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
    }
}
