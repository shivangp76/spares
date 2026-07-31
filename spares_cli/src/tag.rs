use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schema::tag::CreateTagRequest;
use spares_core::schema::tag::TagResponse;
use spares_core::schema::tag::TagSelector;
use spares_core::schema::tag::UpdateTagRequest;

use crate::args::TagArgs;
use crate::args::TagCommands;
use crate::tree::build_tree;
use crate::tree::tree_to_string;
use crate::utils::ensure_ok;
use crate::utils::page_limit_queries;

#[expect(clippy::too_many_lines)]
pub(crate) async fn handle(
    tag_args: TagArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match tag_args.command {
        TagCommands::Add {
            name,
            description,
            query,
            auto_delete,
        } => {
            let request = CreateTagRequest {
                name,
                description,
                query,
                auto_delete,
            };
            let url = format!("{}/api/tags", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: TagResponse = response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&response).unwrap());
        }
        TagCommands::Edit {
            id: tag_id_opt,
            tag_name: tag_name_opt,
            name,
            description,
            query,
            auto_delete,
            rebuild,
        } => {
            if rebuild {
                let tag_id = if let Some(tag_id) = tag_id_opt {
                    tag_id
                } else {
                    let tag_name =
                        tag_name_opt.ok_or_else(|| miette!("--id or --tag-name is required"))?;
                    let url = format!("{}/api/tags/name/{}", base_url, tag_name);
                    let response = client
                        .get(&url)
                        .send()
                        .await
                        .map_err(|e| miette!("{}", e))?;
                    let response = ensure_ok(response).await?;
                    let tag_response: TagResponse =
                        response.json().await.map_err(|e| miette!("{}", e))?;
                    tag_response.id
                };
                let url = format!("{}/api/tags/{}/rebuild", base_url, tag_id);
                let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
                let _ = ensure_ok(response).await?;
                println!("Done");
            } else {
                let tag_to_modify = if let Some(tag_id) = tag_id_opt {
                    TagSelector::Id(tag_id)
                } else if let Some(tag_name) = tag_name_opt {
                    TagSelector::Name(tag_name)
                } else {
                    unreachable!("required by clap");
                };
                let request = UpdateTagRequest {
                    tag_to_modify,
                    name,
                    description,
                    query,
                    auto_delete,
                };
                let url = format!("{}/api/tags", base_url);
                let response = client
                    .patch(url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| miette!("{}", e))?;
                let response = ensure_ok(response).await?;
                let response: TagResponse = response.json().await.map_err(|e| miette!("{}", e))?;
                println!("{}", serde_json::to_string_pretty(&response).unwrap());
            }
        }
        TagCommands::Delete { id } => {
            let url = format!("{}/api/tags/{}", base_url, id);
            let response = client
                .delete(url)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let _ = ensure_ok(response).await?;
            println!("Done");
        }
        TagCommands::Get { id, name } => {
            let url = if let Some(id) = id {
                format!("{}/api/tags/{}", base_url, id)
            } else if let Some(name) = name {
                format!("{}/api/tags/name/{}", base_url, name)
            } else {
                unreachable!("by clap required_unless_present");
            };
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let tag_response: TagResponse = response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", serde_json::to_string_pretty(&tag_response).unwrap());
        }
        TagCommands::List {
            page,
            limit,
            long: _,
            short,
            tree,
        } => {
            let url = format!("{}/api/tags", base_url);
            let response = client
                .get(url)
                .query(&page_limit_queries(page, limit))
                .send()
                .await
                .map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let tag_responses: Vec<TagResponse> =
                response.json().await.map_err(|e| miette!("{}", e))?;
            if short {
                let tag_names = tag_responses
                    .into_iter()
                    .map(|x| x.name)
                    .collect::<Vec<_>>()
                    .join("\n");
                println!("{}", tag_names);
            } else if tree {
                let tag_names = tag_responses
                    .into_iter()
                    .map(|r| r.name)
                    .collect::<Vec<_>>();
                let tree = build_tree(tag_names);
                let output = tree_to_string(&tree, 0);
                println!("{}", output);
            } else {
                println!("{}", serde_json::to_string_pretty(&tag_responses).unwrap());
            }
        }
    }
    Ok(())
}
