use std::fmt::Write;

use inquire::Confirm;
use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schema::card::CardResponse;
use spares_core::schema::card::CardsSelector;
use spares_core::schema::card::GetLeechesRequest;
use spares_core::schema::card::ScheduleBreakRequest;
use spares_core::schema::card::ScheduleBreakResponse;
use spares_core::schema::card::UnburyRequest;
use spares_core::schema::card::UpdateCardsRequest;
use spares_core::schema::card::UpdateCardsResponse;
use spares_core::schema::review::OptimizeRequest;
use spares_core::schema::review::OptimizeResponse;
use spares_core::schema::review::ReschedulePreviewResponse;
use spares_core::schema::review::RescheduleResponse;
use spares_core::schema::review::StudyAction;
use spares_core::schema::review::SubmitStudyActionRequest;
use spares_core::search::QueryReturnItemType;

use crate::args::AdvanceArgs;
use crate::args::BreakArgs;
use crate::args::CardArgs;
use crate::args::CardCommands;
use crate::args::ForgetCardArgs;
use crate::args::OptimizeArgs;
use crate::args::PostponeArgs;
use crate::args::RescheduleArgs;
use crate::args::SearchArgs;
use crate::args::SpecialStateLocal;
use crate::args::StepsArgs;
use crate::card_info::fetch_card_reviews;
use crate::card_info::print_card_info;
use crate::history::print_history;
use crate::review::forget_card;
use crate::review::queue::print_queue_positions;
use crate::review::review_cards;
use crate::saved_search::saved_query;
use crate::search::print_examples;
use crate::search::print_fields;
use crate::search::search;
use crate::search::search_cards;
use crate::statistics::print_statistics;
use crate::steps::print_steps_stats;
use crate::utils::ensure_ok;
use crate::utils::page_limit_queries;
use crate::utils::with_spinner;
use crate::view::view_cards;

#[expect(clippy::too_many_lines)]
pub(crate) async fn handle(
    card_args: CardArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match card_args.command {
        CardCommands::Edit {
            selector: selector_local,
            desired_retention,
            special_state: special_state_local,
            due,
        } => {
            let selector = if let Some(ids) = selector_local.ids {
                CardsSelector::Ids(ids)
            } else if let Some(query) = selector_local.query {
                CardsSelector::Query(query)
            } else {
                unreachable!("by clap conflicts_with")
            };
            let special_state = special_state_local.map(|x| match x {
                SpecialStateLocal::Suspended => {
                    Some(spares_core::schema::card::SpecialStateUpdate::Suspended)
                }
                SpecialStateLocal::Buried => {
                    Some(spares_core::schema::card::SpecialStateUpdate::Buried)
                }
                SpecialStateLocal::None => None,
            });
            let request = UpdateCardsRequest {
                selector,
                desired_retention,
                special_state,
                due,
            };
            let url = format!("{}/api/cards", base_url);
            let response = client
                .patch(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let update_response: UpdateCardsResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&update_response.cards).unwrap()
            );
        }
        CardCommands::Get { id, note_id } => {
            let url = if let Some(id) = id {
                format!("{}/api/cards/{}", base_url, id)
            } else if let Some(note_id) = note_id {
                format!("{}/api/cards/note_id/{}", base_url, note_id.packed())
            } else {
                unreachable!("by clap required_unless_present")
            };
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            if id.is_some() {
                let card_response: CardResponse =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&card_response).unwrap());
            } else {
                let card_responses: Vec<CardResponse> =
                    response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&card_responses).unwrap());
            }
        }
        CardCommands::List { page, limit } => {
            let url = format!("{}/api/cards", base_url);
            let response = client
                .get(url)
                .query(&page_limit_queries(page, limit))
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let card_responses: Vec<CardResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&card_responses).unwrap());
        }
        CardCommands::Reviews { id, json } => {
            if json {
                let entries = fetch_card_reviews(id, base_url, client)
                    .await
                    .map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&entries).unwrap());
            } else {
                print_card_info(id, base_url, client)
                    .await
                    .map_err(|e| miette!("{}", e))?;
            }
        }
        CardCommands::History { weeks, json } => {
            print_history(weeks, json, base_url, client).await?;
        }
        CardCommands::View(view_args) => {
            view_cards(view_args, base_url, client)
                .await
                .map_err(|e| miette!("{}", e))?;
        }
        CardCommands::Search(SearchArgs {
            query,
            output_format,
            fields: _,
            examples,
            explain,
            saved,
        }) => {
            // Clap requires a query unless `--fields`, `--examples` or `--saved` is given.
            let query = match (query, saved) {
                (Some(query), _) => query,
                (None, Some(name)) => saved_query(&name, base_url, client).await?,
                (None, None) if examples => {
                    print_examples();
                    return Ok(());
                }
                (None, None) => {
                    print_fields();
                    return Ok(());
                }
            };
            if explain {
                print!(
                    "{}",
                    spares_core::search::explain(&query, QueryReturnItemType::Cards)?
                );
                return Ok(());
            }
            search(
                query,
                QueryReturnItemType::Cards,
                output_format,
                base_url,
                client,
            )
            .await?;
        }
        CardCommands::Review(review_args) => {
            review_cards(review_args, base_url, client)
                .await
                .map_err(|e| miette!("{}", e))?;
        }
        CardCommands::Queue(queue_args) => {
            print_queue_positions(queue_args, base_url, client).await?;
        }
        CardCommands::Advance(AdvanceArgs {
            count,
            scheduler_name,
            query,
        }) => {
            let request = SubmitStudyActionRequest {
                scheduler_name,
                action: StudyAction::Advance { count, query },
            };
            let url = format!("{}/api/review/submit", base_url);
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Advanced {} cards.", count);
        }
        CardCommands::Postpone(PostponeArgs {
            count,
            scheduler_name,
            query,
        }) => {
            let request = SubmitStudyActionRequest {
                scheduler_name,
                action: StudyAction::Postpone { count, query },
            };
            let url = format!("{}/api/review/submit", base_url);
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Postponed {} cards.", count);
        }
        CardCommands::Break(args) => {
            schedule_break(args, base_url, client).await?;
        }
        CardCommands::Reschedule(RescheduleArgs {
            scheduler_name,
            yes,
        }) => {
            let url = format!("{}/api/scheduler/{}/reschedule", base_url, scheduler_name);
            if !yes {
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let preview: ReschedulePreviewResponse = ensure_ok(response)
                    .await?
                    .json()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let prompt = format!(
                    "This recomputes the memory state and due date of every card, including suspended and buried ones.{} Are you sure you want to continue?",
                    moved_cards_note(&preview)
                );
                let ans = Confirm::new(&prompt).with_default(false).prompt();
                if !ans.unwrap_or(false) {
                    return Ok(());
                }
            }
            let rescheduled: RescheduleResponse = with_spinner("Rescheduling cards", async {
                let response = client
                    .post(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                response.json().await.map_err(|e| miette!("{}", e))
            })
            .await?;
            print_rescheduled(rescheduled.event_id);
        }
        CardCommands::Optimize(OptimizeArgs {
            scheduler_name,
            apply,
            yes,
        }) => {
            if apply && !yes {
                let prompt = "If the optimized parameters are better, this saves them to the config and recomputes the memory state and due date of every card. Are you sure you want to continue?";
                let ans = Confirm::new(prompt).with_default(false).prompt();
                if !ans.unwrap_or(false) {
                    return Ok(());
                }
            }
            let url = format!("{}/api/scheduler/{}/optimize", base_url, scheduler_name);
            let optimized: OptimizeResponse = with_spinner(
                "Optimizing. This can take a minute on a large collection.",
                async {
                    let response = client
                        .post(&url)
                        .json(&OptimizeRequest { apply })
                        .send()
                        .await
                        .map_err(|e| miette!("{}", e))?;
                    let response = ensure_ok(response).await?;
                    response.json().await.map_err(|e| miette!("{}", e))
                },
            )
            .await?;
            print_optimize_response(&optimized, apply);
        }
        CardCommands::Steps(args @ StepsArgs { .. }) => {
            print_steps_stats(args, base_url, client).await?;
        }
        CardCommands::Forget(ForgetCardArgs { ids, query }) => {
            let card_ids = if let Some(ids_vec) = ids {
                ids_vec
            } else if let Some(q) = query {
                search_cards(q, base_url, client)
                    .await?
                    .into_iter()
                    .map(|(card, _)| card.id)
                    .collect()
            } else {
                unreachable!("--ids or --query is required by clap")
            };
            for card_id in card_ids {
                let forget_response = forget_card(card_id, base_url, client)
                    .await
                    .map_err(|e| miette!("{}", e))?;
                println!("Forgot card: {:#?}", forget_response.card);
            }
        }
        CardCommands::Unbury { query } => {
            let url = format!("{}/api/cards/unbury", base_url);
            let req = UnburyRequest { query };
            let response = client
                .post(&url)
                .json(&req)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
        CardCommands::Leeches { scheduler_name } => {
            let url = format!("{}/api/cards/leeches", base_url);
            let req = GetLeechesRequest { scheduler_name };
            let response = client
                .post(&url)
                .json(&req)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let card_responses: Vec<CardResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&card_responses).unwrap());
        }
        CardCommands::Statistics(args) => {
            print_statistics(args, base_url, client).await?;
        }
    }
    Ok(())
}

/// `spares card break`: prints what the break would do, asks for confirmation, and does it.
async fn schedule_break(args: BreakArgs, base_url: &str, client: &Client) -> Result<(), Error> {
    let BreakArgs {
        from,
        days,
        spread,
        scheduler_name,
        dry_run,
        yes,
    } = args;
    let url = format!("{}/api/cards/break", base_url);
    let send = |dry_run: bool| {
        let request = ScheduleBreakRequest {
            scheduler_name: scheduler_name.clone(),
            start: from,
            days,
            spread,
            dry_run,
        };
        let url = url.clone();
        async move {
            let response = client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            ensure_ok(response)
                .await?
                .json::<ScheduleBreakResponse>()
                .await
                .map_err(|e| miette!("{}", e))
        }
    };
    let preview = send(true).await?;
    println!("{}", describe_break(&preview, true));
    if dry_run {
        return Ok(());
    }
    if !yes {
        let prompt = format!(
            "Move them, and add {} to {} to `easy_days.specific_dates` in config.toml?",
            preview.start,
            preview.end.pred_opt().unwrap_or(preview.end)
        );
        if !Confirm::new(&prompt)
            .with_default(false)
            .prompt()
            .unwrap_or(false)
        {
            return Ok(());
        }
    }
    let done = send(false).await?;
    // Placement draws at random, so the moves can differ a little from the preview.
    println!("{}", describe_break(&done, false));
    if let Some(id) = done.event_id {
        println!("`spares event undo {id}` puts the cards back, but leaves the easy days.");
    }
    Ok(())
}

/// What a break did, or would do (`preview`).
fn describe_break(response: &ScheduleBreakResponse, preview: bool) -> String {
    let last_day = response.end.pred_opt().unwrap_or(response.end);
    let (verb, cards) = match (preview, response.card_count) {
        (true, 1) => ("would move", "card"),
        (true, _) => ("would move", "cards"),
        (false, 1) => ("moved", "card"),
        (false, _) => ("moved", "cards"),
    };
    let mut out = format!(
        "A break from {} to {} {verb} {} {cards} to the days from {}.",
        response.start, last_day, response.card_count, response.end
    );
    if let Some((day, count)) = response.busiest_day
        && count > 0
    {
        let _ = write!(
            out,
            " The busiest day after it is {day}, with {count} cards due."
        );
    }
    if let Some((before, after)) = response.mean_retrievability {
        let _ = write!(
            out,
            " The moved cards' mean predicted recall falls from {:.1}% to {:.1}%.",
            before * 100.0,
            after * 100.0
        );
    }
    out
}

/// What a reschedule does with the cards moved since their last review, as a sentence after a
/// space, or nothing if there are none.
fn moved_cards_note(preview: &ReschedulePreviewResponse) -> String {
    let count = preview.moved_count;
    if count == 0 {
        return String::new();
    }
    let cards = if count == 1 { "card was" } else { "cards were" };
    if preview.moved_cards_kept {
        format!(
            " {count} {cards} moved since their last review (a due date set by hand, a postponement, an advance or a break) and keep their due date; `reschedule_moved_cards = true` in config.toml moves them too."
        )
    } else {
        format!(
            " {count} {cards} moved since their last review, and are moved again, since `reschedule_moved_cards` is set."
        )
    }
}

/// Says that every card was rescheduled, and how to undo it.
fn print_rescheduled(event_id: Option<i64>) {
    match event_id {
        // An automatic event: a plain `spares event undo` skips it
        Some(id) => println!(
            "Rescheduled all cards. `spares event undo {id}` restores their previous schedule."
        ),
        None => println!("Rescheduled all cards. None of them changed."),
    }
}

fn print_optimize_response(response: &OptimizeResponse, apply: bool) {
    println!(
        "Fitted to {} reviews of {} cards. Lower is better:",
        response.review_count, response.card_count
    );
    println!("{:<12}{:>10}{:>10}", "", "Log loss", "RMSE");
    for (name, evaluation) in [
        ("Current", &response.current),
        ("Optimized", &response.optimized),
    ] {
        println!(
            "{:<12}{:>10.4}{:>9.2}%",
            name,
            evaluation.log_loss,
            evaluation.rmse_bins * 100.0
        );
    }
    let parameters = response
        .optimized_parameters
        .iter()
        .map(|parameter| parameter.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    println!("\nfsrs_parameters = [{}]\n", parameters);
    let better = response.optimized.log_loss < response.current.log_loss;
    if response.applied {
        println!("Saved the optimized parameters to the config.");
        print_rescheduled(response.event_id);
    } else if !better {
        println!(
            "Kept the current parameters: the optimized ones do not predict your reviews better."
        );
    } else if !apply {
        println!("Run with `--apply` to save them and reschedule every card.");
    }
}
