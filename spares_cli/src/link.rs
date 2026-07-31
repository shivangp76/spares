use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::model::NoteLink;
use spares_core::schema::note::NoteLinksRequest;

use crate::args::LinkArgs;
use crate::args::LinkCommands;
use crate::utils::ensure_ok;

pub(crate) async fn handle(
    link_args: LinkArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match link_args.command {
        LinkCommands::List { score_threshold } => {
            let url = format!("{}/api/notes/search/note-links", base_url);
            let request = NoteLinksRequest { score_threshold };
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: Vec<NoteLink> = response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
    }
    Ok(())
}
