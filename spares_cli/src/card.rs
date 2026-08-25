use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schema::card::CardResponse;
use spares_core::schema::card::CardsSelector;
use spares_core::schema::card::GetLeechesRequest;
use spares_core::schema::card::UnburyRequest;
use spares_core::schema::card::UpdateCardsRequest;
use spares_core::schema::card::UpdateCardsResponse;
use spares_core::schema::review::StatisticsRequest;
use spares_core::schema::review::StatisticsResponse;
use spares_core::schema::review::StudyAction;
use spares_core::schema::review::SubmitStudyActionRequest;
use spares_core::search::QueryReturnItemType;

use crate::args::AdvanceArgs;
use crate::args::CardArgs;
use crate::args::CardCommands;
use crate::args::ForgetCardArgs;
use crate::args::PostponeArgs;
use crate::args::SearchArgs;
use crate::args::SpecialStateLocal;
use crate::args::StatisticsArgs;
use crate::review::forget_card;
use crate::review::review_cards;
use crate::search::search;
use crate::search::search_cards;
use crate::utils::ensure_ok;
use crate::utils::page_limit_queries;
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
                format!("{}/api/cards/note_id/{}", base_url, note_id)
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
        CardCommands::View(view_args) => {
            view_cards(view_args, base_url, client)
                .await
                .map_err(|e| miette!("{}", e))?;
        }
        CardCommands::Search(SearchArgs {
            query,
            output_format,
        }) => {
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
        CardCommands::Statistics(StatisticsArgs {
            scheduler_name,
            date,
        }) => {
            let request = StatisticsRequest {
                scheduler_name,
                date,
            };
            let url = format!("{}/api/review/statistics", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: StatisticsResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
    }
    Ok(())
}
