use clap::Args;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares::{
    adapters::{SrsAdapter, impls::anki::AnkiAdapter, migration::MigrationData},
    parsers::{NotePart, find_parser, get_all_parsers, get_cards},
    schema::note::{NotesSelector, RenderNotesRequest},
};
use sqlx::SqlitePool;
use std::time::Instant;

#[derive(Args, Debug)]
pub(crate) struct MigrateArgs {
    #[arg(short, long)]
    pub(crate) adapter: String,
    #[arg(short, long, default_value_t = false)]
    pub(crate) initial_migration: bool,
    #[arg(short, long, default_value_t = true)]
    pub(crate) dry_run: bool,
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

async fn call_render_notes(client: &Client, base_url: &str, run: bool) -> Result<(), String> {
    println!("Rendering notes...");
    let start = Instant::now();
    let url = format!("{}/api/notes/generate_files", base_url);
    let request = RenderNotesRequest {
        selector: NotesSelector::All,
        immutable_note_ids: None,
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

pub(crate) async fn migrate_from_adapter(
    base_url: &str,
    spares_pool: &SqlitePool,
    client: &Client,
    adapter: &mut dyn SrsAdapter,
    initial_migration: bool,
    run: bool,
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

    // Render notes after adding spares id, so in case the migration is aborted, the data can still be recovered.
    call_render_notes(client, base_url, run).await?;

    let duration = start.elapsed();
    println!("\nTotal duration: {:?}", duration);

    Ok(())
}
