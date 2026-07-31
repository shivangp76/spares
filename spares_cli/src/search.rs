use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::parsers::RenderOutputDirectoryType;
use spares_core::parsers::find_parser;
use spares_core::parsers::generate_files::CardSide;
use spares_core::parsers::generate_files::RenderOutputType;
use spares_core::parsers::get_all_parsers;
use spares_core::parsers::get_output_raw_dir;
use spares_core::schema::card::CardResponse;
use spares_core::schema::note::SearchNotesRequest;
use spares_core::schema::note::SearchNotesResponse;
use spares_core::search::QueryReturnItemType;

use crate::args::OutputFormat;
use crate::utils::compute_note_raw_path;
use crate::utils::compute_note_rendered_path;
use crate::utils::ensure_ok;

async fn search_notes(
    query: String,
    output_type: QueryReturnItemType,
    base_url: &str,
    client: &Client,
) -> Result<SearchNotesResponse, Error> {
    let request = SearchNotesRequest { query, output_type };
    let url = format!("{}/api/notes/search", base_url);
    let response = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|e| miette!("{}", e))?;
    let response = ensure_ok(response).await?;
    response.json().await.map_err(|e| miette!("{}", e))
}

pub(crate) async fn search_cards(
    query: String,
    base_url: &str,
    client: &Client,
) -> Result<Vec<(CardResponse, String)>, Error> {
    match search_notes(query, QueryReturnItemType::Cards, base_url, client).await? {
        SearchNotesResponse::Cards(card_responses) => Ok(card_responses),
        SearchNotesResponse::Notes(_) => {
            unreachable!("search with Cards output type returns cards")
        }
    }
}

pub(crate) async fn search(
    query: String,
    output_type: QueryReturnItemType,
    output_format: OutputFormat,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    let response = search_notes(query, output_type, base_url, client).await?;
    match response {
        SearchNotesResponse::Notes(note_responses) => {
            for (note_response, parser_name) in note_responses {
                match output_format {
                    OutputFormat::RawFilepath => {
                        match compute_note_raw_path(parser_name.as_str(), note_response.id) {
                            Ok(p) => println!("{}", p.display()),
                            Err(e) => eprintln!("Error: {}", e),
                        }
                    }
                    OutputFormat::RenderedFilepath => {
                        match compute_note_rendered_path(parser_name.as_str(), note_response.id) {
                            Ok(p) => println!("{}", p.display()),
                            Err(e) => eprintln!("Error: {}", e),
                        }
                    }
                }
            }
        }
        SearchNotesResponse::Cards(card_responses) => {
            let all_parsers = get_all_parsers();
            for (card_response, parser_name) in card_responses {
                let parser = find_parser(parser_name.as_str(), &all_parsers)?;
                match output_format {
                    OutputFormat::RawFilepath => {
                        let mut card_raw_path = get_output_raw_dir(
                            parser.get_parser_name(),
                            RenderOutputType::Card(card_response.order as usize, CardSide::Front),
                            None,
                        );
                        card_raw_path.push(parser.get_output_filename(
                            RenderOutputType::Card(card_response.order as usize, CardSide::Front),
                            card_response.note_id,
                        ));
                        card_raw_path.set_extension(parser.file_extension());
                        println!("{}", card_raw_path.display());
                    }
                    OutputFormat::RenderedFilepath => {
                        let mut card_rendered_path =
                            parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
                        card_rendered_path.push(parser.get_output_filename(
                            RenderOutputType::Card(card_response.order as usize, CardSide::Front),
                            card_response.note_id,
                        ));
                        println!("{}", card_rendered_path.display());
                    }
                }
            }
        }
    }
    Ok(())
}
