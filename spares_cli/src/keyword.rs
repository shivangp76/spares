use itertools::Itertools;
use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::model::NoteId;
use spares_core::schema::note::MatchedKeywordResponse;
use spares_core::schema::note::SearchKeywordRequest;
use spares_core::schema::note::UnmatchedKeywordResponse;

use crate::args::KeywordArgs;
use crate::args::KeywordCommands;
use crate::utils::ensure_ok;

pub(crate) async fn handle(
    keyword_args: KeywordArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match keyword_args.command {
        KeywordCommands::List { short } => {
            let url = format!("{}/api/notes/keywords", base_url);
            let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<(NoteId, String)> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            if short {
                println!("{}", response.iter().map(|(_, kw)| kw).unique().join("\n"));
            } else {
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
        }
        KeywordCommands::Search { keyword } => {
            let request = SearchKeywordRequest { keyword };
            let url = format!("{}/api/notes/search/keyword", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<MatchedKeywordResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            match response.first() {
                Some(matched) => {
                    println!("{}", serde_json::to_string_pretty(matched).unwrap());
                }
                None => return Err(miette!("No matching keyword found")),
            }
        }
        KeywordCommands::Ranking { keyword } => {
            let request = SearchKeywordRequest { keyword };
            let url = format!("{}/api/notes/search/keyword", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<MatchedKeywordResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        KeywordCommands::Unmatched => {
            let url = format!("{}/api/notes/unmatched-keywords", base_url);
            let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<UnmatchedKeywordResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        KeywordCommands::Duplicate => {
            let url = format!("{}/api/notes/duplicate-keywords", base_url);
            let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<(String, Vec<NoteId>)> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
    }
    Ok(())
}
