use clap::Args;
use inquire::Select;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares::config::read_external_config;
use spares::model::{RatingId, TagId};
use spares::schema::review::{
    CardBackRenderedPath, GetReviewCardFilterRequest, GetReviewCardRequest, GetReviewCardResponse,
};
use spares::schema::tag::TagResponse;
use std::process::Child;
use std::time::{Duration, Instant};
use strum::{EnumIter, IntoEnumIterator};
use strum_macros::{Display, EnumString};
use utils::{
    bury_card, close_rendered_file, get_scheduler_ratings, open_rendered_file,
    print_recall_duration, print_summary, submit_rating, suspend_cards, suspend_note, tag_note,
};

mod utils;

#[derive(Args, Debug)]
pub struct ReviewArgs {
    // Using `Option<FilterArgs>` here instead won't work since they `query` becomes a required parameter.
    #[command(flatten)]
    pub filter_args: FilterArgs,
    #[arg(short, long, default_value = "fsrs")]
    pub scheduler_name: String,
    #[arg(long, env = "SPARES_RENDERED_FILE_OPENER")]
    pub opener: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct FilterArgs {
    /// Filter the cards due today with the supplied query
    #[arg(short, long)]
    pub query: Option<String>,
    /// Study a filtered tag with the supplied id
    #[arg(long, conflicts_with_all = ["query", "tag_name"])]
    pub tag_id: Option<TagId>,
    /// Study a filtered tag with the supplied name
    #[arg(short, long, conflicts_with_all = ["query", "tag_id"])]
    pub tag_name: Option<String>,
}

#[derive(Clone, Debug, Display, EnumIter, EnumString, PartialEq)]
enum ReviewAction {
    Flip,
    #[strum(to_string = "Rate: {description} ({id})")]
    Rate {
        id: RatingId,
        description: String,
    },
    #[strum(serialize = "Open Note")]
    OpenNote,
    #[strum(serialize = "Suspend Card")]
    SuspendCard,
    #[strum(serialize = "Suspend Note (card + siblings)")]
    SuspendNote,
    #[strum(serialize = "Tag to modify later")]
    TagNote,
    // Undo,
    Bury,
    Exit,
}

// NoteId is for each action
// enum UndoReviewAction {
//     Rate,
//     Suspend,
//     TagNote { tag_name: String },
// }

async fn get_review_card(
    filter_args: &FilterArgs,
    opener: Option<&str>,
    base_url: &str,
    client: &Client,
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
            let child = open_rendered_file(review_card.card_front_rendered_path.as_ref(), opener)?;

            println!("Note Id: {}", &review_card.note_id);
            println!("Card Id: {}", &review_card.card_id);
            println!(
                "Card Front File Name: {:?}",
                &review_card.card_front_rendered_path.file_name().unwrap()
            );
            // println!("Note Raw Path: {}", &review_card.note_raw_path.display());

            Ok(Some((review_card, child)))
        }
        // No cards left to review
        None => Ok(None),
    }
}

#[allow(clippy::too_many_lines)]
pub async fn review_cards(
    review_args: ReviewArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let opener = review_args.opener.as_deref();
    let scheduler_name = &review_args.scheduler_name;
    let tag_id = review_args.filter_args.tag_id;

    // Get scheduler ratings
    let mut all_options = ReviewAction::iter()
        .filter(|x| !matches!(*x, ReviewAction::Rate { .. }))
        .collect::<Vec<_>>();
    // We want to keep the rating near the top so they are all visible
    all_options.splice(
        1..1,
        get_scheduler_ratings(scheduler_name, base_url, client).await?,
    );

    let session_start = Instant::now();
    let mut session_recall = Duration::default();
    let mut reviewed_cards_count = 0;
    let mut card_back_rendered_child: Option<Child> = None;
    let mut card_flipped = false;
    let mut advance_review_card = false;

    let review_card_opt =
        get_review_card(&review_args.filter_args, opener, base_url, client).await?;
    let mut recall_start = Instant::now();
    // let mut recall_duration = std::time::Duration::MAX;
    let mut recall_duration = None;
    if review_card_opt.is_none() {
        println!("Done");
        return Ok(());
    }
    let (mut review_card_response, mut card_front_rendered_child) = review_card_opt.unwrap();
    let config = read_external_config().map_err(|e| format!("{}", e))?;
    let flagged_tag_name = config.flagged_tag_name;
    // let mut action_history = Vec::new();
    loop {
        if advance_review_card {
            println!();
            // Opening the card's raw file is not useful since edits must be made to the note, not the
            // card. Opening the note's raw file and the card's rendered file is more useful.
            let review_card_opt =
                get_review_card(&review_args.filter_args, opener, base_url, client).await?;
            recall_start = Instant::now();
            recall_duration = None;
            if review_card_opt.is_none() {
                println!("Done");
                print_summary(session_start, session_recall, reviewed_cards_count);
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
                    !matches!(*x, ReviewAction::Rate { .. })
                }
            })
            .collect::<Vec<_>>();
        let mut select = Select::new("Action:", options);
        select.vim_mode = true;
        select.page_size = 10;
        let chosen_action_res = select.prompt();
        if chosen_action_res.is_err() {
            // The user exited. (Probably pressed Escape).
            print_summary(session_start, session_recall, reviewed_cards_count);
            return Ok(());
        }
        let chosen_action = chosen_action_res.as_ref().unwrap();
        advance_review_card = false;
        match chosen_action {
            ReviewAction::Rate {
                description: _,
                id: rating_id,
            } => {
                card_flipped = false;

                // Close card back
                close_rendered_file(&mut card_back_rendered_child.take().unwrap())?;

                reviewed_cards_count += 1;
                submit_rating(
                    recall_duration.unwrap(),
                    scheduler_name,
                    review_card_response.card_id,
                    tag_id,
                    *rating_id,
                    base_url,
                    client,
                )
                .await?;

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
                close_rendered_file(&mut card_front_rendered_child)?;

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
                    session_recall += recall_duration.unwrap();
                    print_recall_duration(recall_duration.unwrap());
                }
                let card_back_rendered_path = match &review_card_response.card_back_rendered_path {
                    CardBackRenderedPath::CardBack(path_buf)
                    | CardBackRenderedPath::Note(path_buf) => path_buf,
                };
                card_back_rendered_child =
                    Some(open_rendered_file(card_back_rendered_path, opener)?);
            }
            ReviewAction::OpenNote => {
                // If the note is viewed before the card is flipped, then the answer is revealed.
                // This means that the user already recalled (or failed to recall) the card at this
                // point.
                if !card_flipped {
                    recall_duration = Some(recall_start.elapsed());
                    session_recall += recall_duration.unwrap();
                    print_recall_duration(recall_duration.unwrap());
                }
                let open_note_res = open::that_detached(&review_card_response.note_raw_path);
                if let Err(e) = open_note_res {
                    println!("{}", e);
                }
            }
            ReviewAction::Bury | ReviewAction::SuspendCard | ReviewAction::SuspendNote => {
                match chosen_action {
                    ReviewAction::Bury => {
                        bury_card(
                            scheduler_name,
                            review_card_response.card_id,
                            base_url,
                            client,
                        )
                        .await?;
                    }
                    ReviewAction::SuspendCard => {
                        suspend_cards(&[review_card_response.card_id], base_url, client).await?;
                    }
                    ReviewAction::SuspendNote => {
                        suspend_note(review_card_response.note_id, base_url, client).await?;
                    }
                    _ => unreachable!(),
                }
                if card_flipped {
                    // Close card back
                    close_rendered_file(&mut card_back_rendered_child.take().unwrap())?;
                } else {
                    // Close card front
                    close_rendered_file(&mut card_front_rendered_child)?;
                }
                card_flipped = false;

                // Advance to next review card
                advance_review_card = true;
            }
            ReviewAction::TagNote => {
                // action_history.push()
                tag_note(
                    review_card_response.note_id,
                    &flagged_tag_name,
                    base_url,
                    client,
                )
                .await?;
            }
            // ReviewAction::Undo => {
            //     if card_flipped {
            //         // Close card back
            //         close_rendered_file(&mut card_back_rendered_child.take().unwrap())?;
            //     } else {
            //         // Close card front
            //         close_rendered_file(&mut card_front_rendered_child)?;
            //     }
            //     // Remove previously submitted rating
            //     // TODO
            //
            //     // Get review card (which should be the same as the previous card)
            //     // TODO
            // }
            ReviewAction::Exit => {
                close_rendered_file(&mut card_front_rendered_child)?;
                if let Some(mut child) = card_back_rendered_child {
                    close_rendered_file(&mut child)?;
                }
                print_summary(session_start, session_recall, reviewed_cards_count);
                return Ok(());
            }
        }
    }
}
