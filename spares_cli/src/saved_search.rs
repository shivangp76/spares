use miette::Error;
use miette::miette;
use reqwest::Client;
use reqwest::Url;
use spares_core::schema::saved_search::SavedSearch;

use crate::args::SavedSearchArgs;
use crate::args::SavedSearchCommands;
use crate::utils::ensure_ok;

/// The URL of the saved search called `name`, with the name escaped.
fn saved_search_url(base_url: &str, name: &str) -> Result<Url, Error> {
    let mut url = Url::parse(base_url).map_err(|e| miette!("{e}"))?;
    url.path_segments_mut()
        .map_err(|()| miette!("`{base_url}` can't have a path"))?
        .extend(["api", "saved-searches", name]);
    Ok(url)
}

/// The query saved as `name`.
pub(crate) async fn saved_query(
    name: &str,
    base_url: &str,
    client: &Client,
) -> Result<String, Error> {
    let response = client
        .get(saved_search_url(base_url, name)?)
        .send()
        .await
        .map_err(|e| miette!("{e}"))?;
    let search: SavedSearch = ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{e}"))?;
    Ok(search.query)
}

pub(crate) async fn handle(
    args: SavedSearchArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match args.command {
        SavedSearchCommands::List => {
            let response = client
                .get(format!("{base_url}/api/saved-searches"))
                .send()
                .await
                .map_err(|e| miette!("{e}"))?;
            let searches: Vec<SavedSearch> = ensure_ok(response)
                .await?
                .json()
                .await
                .map_err(|e| miette!("{e}"))?;
            for search in searches {
                println!("{}\t{}", search.name, search.query);
            }
        }
        SavedSearchCommands::Save { name, query } => {
            let response = client
                .put(format!("{base_url}/api/saved-searches"))
                .json(&SavedSearch { name, query })
                .send()
                .await
                .map_err(|e| miette!("{e}"))?;
            let saved: SavedSearch = ensure_ok(response)
                .await?
                .json()
                .await
                .map_err(|e| miette!("{e}"))?;
            println!("Saved `{}` as `{}`", saved.query, saved.name);
        }
        SavedSearchCommands::Delete { name } => {
            let response = client
                .delete(saved_search_url(base_url, &name)?)
                .send()
                .await
                .map_err(|e| miette!("{e}"))?;
            let deleted: bool = ensure_ok(response)
                .await?
                .json()
                .await
                .map_err(|e| miette!("{e}"))?;
            if !deleted {
                return Err(miette!("No search is saved as `{name}`"));
            }
        }
    }
    Ok(())
}
