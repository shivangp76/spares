use clap::Args;
use log::info;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use spares::{
    adapters::{SrsAdapter, impls::anki::AnkiAdapter, migration::MigrationData},
    parsers::{NotePart, find_parser, get_all_parsers, get_cards},
    schema::{
        note::RenderNotesRequest,
        tag::{TagResponse, UpdateTagRequest},
    },
};
use sqlx::SqlitePool;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Args, Debug)]
pub struct MigrateArgs {
    #[arg(short, long)]
    pub adapter: String,
    #[arg(short, long, default_value_t = false)]
    pub initial_migration: bool,
    #[arg(short, long, default_value_t = false)]
    pub run: bool,
    #[arg(short, long, help = "Path to JSON file containing tag relations")]
    pub tag_relations_file_path: Option<PathBuf>,
}

fn migration_func(
    MigrationData {
        front,
        back,
        parser_name,
        is_suspended,
    }: MigrationData,
) -> (String, String) {
    // let new_front = parse_side(&front);
    // let new_back = parse_side(&back);
    let new_front = front;
    let new_back = back;

    let parser = find_parser(&parser_name, &get_all_parsers()).unwrap();

    if new_back.is_empty() {
        return (new_front, new_back);
    }

    // Try to parse note by joining front and back
    let temp_note_data = format!("{}{}", new_front, new_back);
    let mut cards =
        get_cards(parser.as_ref(), None, temp_note_data.as_str(), false, false).unwrap();
    if cards.is_empty() {
        let note_settings_keys = parser.note_settings_keys();
        let cloze_settings_keys = parser.cloze_settings_keys();
        // Since no cards were parsed, the cloze is missing.
        // Add cloze wrapper and ordering to `back`.
        let cloze_settings_string = if is_suspended {
            format!(
                "{}{}",
                cloze_settings_keys.is_suspended, note_settings_keys.settings_key_value_delim
            )
        } else {
            String::new()
        };
        let (cloze_prefix, cloze_suffix) =
            parser.construct_cloze(cloze_settings_string.as_str(), &new_back);
        let note_data = format!("{}{}{}{}", new_front, cloze_prefix, new_back, cloze_suffix);
        // Get cards again, adding the order as well
        cards = get_cards(parser.as_ref(), None, &note_data, true, false).unwrap();
    }
    assert!(!cards.is_empty());
    let card = cards.first().unwrap();
    let first_cloze_index = card
        .data
        .iter()
        .position(|p| matches!(*p, NotePart::ClozeStart(_)))
        .unwrap_or(cards.len());
    let new_front =
        AnkiAdapter::note_parts_to_data(&card.data[..first_cloze_index], parser.as_ref());
    let new_back =
        AnkiAdapter::note_parts_to_data(&card.data[first_cloze_index..], parser.as_ref());
    (new_front, new_back)
}

#[derive(Debug, Deserialize)]
struct TagRelation {
    parent_tag: String,
    child_tag: String,
}

#[allow(clippy::too_many_lines)]
async fn create_tag_relations(
    client: &Client,
    base_url: &str,
    run: bool,
    tag_relations_file_path: &Path,
) -> Result<(), String> {
    println!("Creating tag relations...");
    let start = Instant::now();
    let content =
        fs::read_to_string(tag_relations_file_path).expect("Failed to read tag relations file");
    let relations: Vec<TagRelation> =
        serde_json::from_str(&content).expect("Failed to parse tag relations JSON");
    let tag_relations = relations
        .into_iter()
        .map(|r| (r.parent_tag, r.child_tag))
        .collect::<Vec<_>>();
    if !run {
        return Ok(());
    }
    let url = format!("{}/api/tags?limit=999", base_url);
    let response = client.get(url).send().await.map_err(|e| format!("{}", e))?;
    let status = response.status();
    if status != StatusCode::OK {
        let body: Value = response.json().await.map_err(|e| format!("{}", e))?;
        dbg!(&body);
        return Err("Failed to get all tags.".to_string());
    }
    let tag_responses: Vec<TagResponse> = response.json().await.map_err(|e| format!("{}", e))?;
    let tag_relations_with_responses = tag_relations
        .into_iter()
        .filter_map(|(parent_tag_name, child_tag_name)| {
            // Try to get parent and child tag.
            let parent_tag = tag_responses
                .iter()
                .find(|tag_response| tag_response.name == parent_tag_name);
            let child_tag = tag_responses
                .iter()
                .find(|tag_response| tag_response.name == child_tag_name);
            if parent_tag.is_none() {
                info!("Could not find parent tag named {}", parent_tag_name);
                return None;
            }
            if child_tag.is_none() {
                info!("Could not find child tag named {}", child_tag_name);
                return None;
            }
            Some((parent_tag.unwrap(), child_tag.unwrap()))
        })
        .collect::<Vec<_>>();
    for (parent_tag, child_tag) in tag_relations_with_responses {
        // Edit parent of child tag to be parent tag
        let url = format!("{}/api/tags/{}", base_url, child_tag.id,);
        let request = UpdateTagRequest {
            parent_id: Some(Some(parent_tag.id)),
            name: None,
            description: None,
            query: None,
            auto_delete: None,
        };
        if run {
            let response = client
                .patch(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| format!("{}", e))?;
            let status = response.status();
            if status != StatusCode::OK {
                let body: Value = response.json().await.map_err(|e| format!("{}", e))?;
                dbg!(&body);
                return Err(format!(
                    "Failed to set the parent id of tag {} to {}.",
                    child_tag.id, parent_tag.id
                ));
            }
            let _tag_response: TagResponse = response.json().await.map_err(|e| format!("{}", e))?;
        } else {
            dbg!(&request);
        }
    }
    let duration = start.elapsed();
    println!("Tag relations duration: {:?}", duration);
    Ok(())
}

async fn call_render_notes(client: &Client, base_url: &str, run: bool) -> Result<(), String> {
    println!("Rendering notes...");
    let start = Instant::now();
    let url = format!("{}/api/notes/generate_files", base_url);
    let request = RenderNotesRequest {
        generate_files_note_ids: None,
        overridden_output_raw_dir: None,
        include_linked_notes: true,
        include_cards: true,
        generate_rendered: false,
        force_generate_rendered: false,
    };
    if run {
        let response = client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("{}", e))?;
        let status = response.status();
        if status != StatusCode::OK {
            let body: Value = response.json().await.map_err(|e| format!("{}", e))?;
            dbg!(&body);
            return Err(body.to_string());
        }
    } else {
        dbg!(&request);
    }
    let duration = start.elapsed();
    println!("Notes render duration: {:?}", duration);
    Ok(())
}

pub async fn migrate_from_adapter(
    base_url: &str,
    spares_pool: &SqlitePool,
    client: &Client,
    adapter: &mut dyn SrsAdapter,
    initial_migration: bool,
    run: bool,
    tag_relations_file_path: Option<&Path>,
) -> Result<(), String> {
    let start = Instant::now();
    adapter
        .migrate(
            base_url,
            spares_pool,
            Some(migration_func),
            initial_migration,
            run,
        )
        .await
        .map_err(|e| format!("{}", e))?;

    if let Some(tag_relations_file_path) = tag_relations_file_path {
        create_tag_relations(client, base_url, run, tag_relations_file_path).await?;
    }

    // Render notes after adding spares id, so in case the migration is aborted, the data can still be recovered.
    call_render_notes(client, base_url, run).await?;

    let duration = start.elapsed();
    println!("\nTotal duration: {:?}", duration);

    Ok(())
}
