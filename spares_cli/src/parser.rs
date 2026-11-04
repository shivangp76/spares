use std::path::PathBuf;

use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::parsers::TemplateType;
use spares_core::parsers::find_parser;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::parser::CreateParserRequest;
use spares_core::schema::parser::ParserResponse;
use spares_core::schema::parser::UpdateParserRequest;

use crate::args::ParserArgs;
use crate::args::ParserCommands;
use crate::args::TemplateKind;
use crate::utils::ensure_ok;
use crate::utils::page_limit_queries;

pub(crate) async fn handle(
    parser_args: ParserArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match parser_args.command {
        ParserCommands::Add { name } => {
            let request = CreateParserRequest { name };
            let url = format!("{}/api/parsers", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: ParserResponse = response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        ParserCommands::Edit { id, name } => {
            let request = UpdateParserRequest { name: Some(name) };
            let url = format!("{}/api/parsers/{}", base_url, id);
            let response = client
                .patch(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: ParserResponse = response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        ParserCommands::Delete { id } => {
            let url = format!("{}/api/parsers/{}", base_url, id);
            let response = client
                .delete(url)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
        ParserCommands::Get { id } => {
            let url = format!("{}/api/parsers/{}", base_url, id);
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let parser_response: ParserResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&parser_response).unwrap()
            );
        }
        ParserCommands::List { page, limit } => {
            let url = format!("{}/api/parsers", base_url);
            let response = client
                .get(url)
                .query(&page_limit_queries(page, limit))
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let parser_responses: Vec<ParserResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&parser_responses).unwrap()
            );
        }
        ParserCommands::Template { name, kind } => {
            let path = write_default_template(&name, kind)?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

/// Writes the default template of `kind` for the parser `name`, unless it has a file already, and
/// returns the file's path.
fn write_default_template(name: &str, kind: TemplateKind) -> Result<PathBuf, Error> {
    let parser = find_parser(name, &get_all_parsers()).map_err(|e| miette!("{e}"))?;
    let template_type = match kind {
        TemplateKind::Note => TemplateType::Note,
        TemplateKind::Card => TemplateType::Card,
        TemplateKind::Export => TemplateType::Export,
    };
    let path = parser.template_filepath(template_type);
    if path.exists() {
        eprintln!("The template already has a file, which is left as it is.");
        return Ok(path);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| miette!("Failed to create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, parser.default_template())
        .map_err(|e| miette!("Failed to write {}: {e}", path.display()))?;
    Ok(path)
}
