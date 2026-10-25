use inquire::Confirm;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;
use reqwest::Client;
use serde_json::Map;
use spares_core::model::NoteId;
use spares_core::schema::note::CreateNoteRequest;
use spares_core::schema::note::CreateNotesRequest;
use spares_core::schema::note::DeleteNotesRequest;
use spares_core::schema::note::ExportNotesRequest;
use spares_core::schema::note::NoteResponse;
use spares_core::schema::note::NotesResponse;
use spares_core::schema::note::NotesSelector;
use spares_core::schema::note::RenderNotesRequest;
use spares_core::schema::note::UpdateNotesRequest;
use spares_core::schema::note::UpdateNotesResponse;
use spares_core::schema::note::UpdateTags;
use spares_core::search::QueryReturnItemType;

use crate::args::GenerateArgs;
use crate::args::NoteArgs;
use crate::args::NoteCommands;
use crate::args::SearchArgs;
use crate::graph::chart;
use crate::saved_search::saved_query;
use crate::search::print_examples;
use crate::search::print_fields;
use crate::search::search;
use crate::search::search_note_responses;
use crate::utils::ensure_ok;
use crate::utils::page_limit_queries;
use crate::utils::parse_custom_data;
use crate::utils::parse_list;
use crate::view::view_notes;

#[expect(clippy::too_many_lines)]
pub(crate) async fn handle(
    note_args: NoteArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match note_args.command {
        NoteCommands::Add {
            data,
            parser_id,
            keywords,
            tags,
            is_suspended,
            custom_data,
        } => {
            let custom_data = match custom_data {
                Some(s) => parse_custom_data(&s)?,
                None => Map::new(),
            };
            let create_note_request = CreateNoteRequest {
                data,
                keywords: parse_list(keywords.as_str()),
                tags,
                is_suspended,
                custom_data,
            };
            let request = CreateNotesRequest {
                parser_id,
                requests: vec![create_note_request],
            };
            let url = format!("{}/api/notes", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: NotesResponse = response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        NoteCommands::Edit {
            selector,
            data,
            parser_id,
            keywords,
            tags_to_remove,
            tags_to_add,
            remove_all_tags,
            custom_data,
        } => {
            let selector = selector
                .get_notes_selector()
                .map_err(|e| miette!("{}", e))?;
            // A live note's data comes from its source file, which the next `spares import` of it
            // would restore. Its tags and keywords stay editable, as on the web.
            if data.is_some() || parser_id.is_some() || custom_data.is_some() {
                let live = live_notes(&selector, base_url, client).await?;
                if !live.is_empty() {
                    return Err(miette!(
                        "Only the tags and keywords of a live note can be edited here: {}. Edit \
                         the note in its source file and run `spares import` instead.",
                        live.iter()
                            .map(|(id, lsn)| format!("note {id} is live (live-sync-name `{lsn}`)"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            let tags = if remove_all_tags {
                // If `tags_to_add` is empty, this will just set all tags to nothing which will remove all tags. Otherwise, it will remove all tags and then add `tags_to_add`
                UpdateTags::SetTags(tags_to_add.unwrap_or_default())
            } else {
                UpdateTags::ModifyTags {
                    tags_to_remove,
                    tags_to_add,
                }
            };
            let custom_data = match custom_data {
                Some(s) => Some(parse_custom_data(&s)?),
                None => None,
            };
            let request = UpdateNotesRequest {
                selector,
                data,
                parser_id,
                keywords: keywords.as_deref().map(parse_list),
                tags,
                custom_data,
            };
            let url = format!("{}/api/notes", base_url);
            let response = client
                .patch(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let update_response: UpdateNotesResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&update_response.notes).unwrap()
            );
        }
        NoteCommands::Delete { selector } => {
            let selector = selector
                .get_notes_selector()
                .map_err(|e| miette!("{}", e))?;
            let request = DeleteNotesRequest { selector };
            let url = format!("{}/api/notes", base_url);
            let response = client
                .delete(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
        NoteCommands::Get { id } => {
            let url = format!("{}/api/notes/{}", base_url, id.packed());
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let mut note_response: NoteResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            note_response.linked_notes = None;
            println!("{}", serde_json::to_string_pretty(&note_response).unwrap());
        }
        NoteCommands::List { page, limit, graph } => {
            let url = format!("{}/api/notes", base_url);
            let response = client
                .get(url)
                .query(&page_limit_queries(page, limit))
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let note_responses: Vec<NoteResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            if graph {
                chart(note_responses);
            } else {
                println!("{}", serde_json::to_string_pretty(&note_responses).unwrap());
            }
        }
        NoteCommands::View(view_args) => {
            view_notes(view_args, base_url, client)
                .await
                .map_err(|e| miette!("{}", e))?;
        }
        NoteCommands::Search(SearchArgs {
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
                    spares_core::search::explain(&query, QueryReturnItemType::Notes)?
                );
                return Ok(());
            }
            search(
                query,
                QueryReturnItemType::Notes,
                output_format,
                base_url,
                client,
            )
            .await?;
        }
        NoteCommands::Export(export_args) => {
            let request = ExportNotesRequest {
                query: export_args.query,
            };
            let url = format!("{}/api/notes/export", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let result: std::collections::HashMap<String, String> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            tokio::fs::create_dir_all(&export_args.output_dir)
                .await
                .into_diagnostic()?;
            for (extension, content) in &result {
                let filename = format!("export.{}", extension);
                let path = export_args.output_dir.join(&filename);
                tokio::fs::write(&path, content).await.into_diagnostic()?;
                println!("Wrote {}", filename);
            }
        }
        NoteCommands::Generate(GenerateArgs {
            query,
            overridden_output_raw_dir,
            include_linked_notes,
            include_cards,
            render,
            force_render,
        }) => {
            // With a notes repository, files are written to spares' copy, never over your edits.
            if !spares_core::notes_vcs::enabled() {
                let prompt = "Note that this will overwrite any unsaved changes you have. Please sync your notes before proceeding. Are you sure you want to continue?";
                let ans = Confirm::new(prompt).with_default(false).prompt();
                if !ans.unwrap_or(false) {
                    return Ok(());
                }
            }
            let request = RenderNotesRequest {
                selector: query.map_or(NotesSelector::All, NotesSelector::Query),
                immutable_note_ids: None,
                overridden_output_raw_dir,
                include_linked_notes,
                include_cards,
                generate_rendered: render,
                force_generate_rendered: force_render,
            };
            let url = format!("{}/api/notes/generate_files", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
    }
    Ok(())
}

/// The live notes among those `selector` selects, with their `live_sync_name`.
async fn live_notes(
    selector: &NotesSelector,
    base_url: &str,
    client: &Client,
) -> Result<Vec<(NoteId, String)>, Error> {
    let notes = match selector {
        NotesSelector::Ids(ids) => {
            let mut notes = Vec::with_capacity(ids.len());
            for id in ids {
                let url = format!("{}/api/notes/{}", base_url, id.packed());
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let note: NoteResponse = response.json().await.map_err(|e| miette!("{}", e))?;
                notes.push(note);
            }
            notes
        }
        NotesSelector::Query(query) => {
            search_note_responses(query.clone(), base_url, client).await?
        }
        NotesSelector::All => unreachable!("`NotesSelectorLocal` never selects all notes"),
    };
    Ok(notes
        .into_iter()
        .filter_map(|note| {
            let lsn = note
                .custom_data
                .get("live_sync_name")?
                .as_str()?
                .to_string();
            Some((note.id, lsn))
        })
        .collect())
}
