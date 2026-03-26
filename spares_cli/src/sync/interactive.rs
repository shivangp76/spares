use super::{SyncImportAction, SyncImportData, replace_action};
use crate::sync::{
    SyncSource, UpdateDirection, generate_notes, get_import_data, hub_spoke_error, update_changes,
    utils::apply_select_settings,
};
use colored::Colorize;
use inquire::Select;
use log::info;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares_core::{
    model::NoteId,
    parsers::{find_parser, get_all_parsers},
    schema::note::{NotesSelector, RenderNotesRequest},
};
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter, EnumString};

#[derive(Debug, Display, EnumIter, EnumString, PartialEq)]
enum SyncAction {
    #[strum(serialize = "Push Changes")]
    PushChanges,
    #[strum(serialize = "Pull Changes")]
    PullChanges,
    Exit,
    // Previous,
    Next,
}

#[derive(Debug, Display, EnumIter, EnumString, PartialEq)]
pub(crate) enum SyncMode {
    Individual,
    Bulk,
}

fn print_import_data(import_data: &SyncImportData, dry_run: bool) -> Result<(), String> {
    match import_data.action {
        SyncImportAction::Add { to: ref from } | SyncImportAction::Delete { to: ref from } => {
            let file_contents = fs::read_to_string(from).map_err(|e| format!("{}", e))?;
            let parser = find_parser(import_data.parser_name.as_str(), &get_all_parsers())
                .map_err(|e| format!("{:?}", e))?;
            let replaced_file_contents = replace_action(
                file_contents.as_str(),
                &import_data.action,
                parser.as_ref(),
                import_data.note_id,
            )
            .unwrap();
            if matches!(import_data.action, SyncImportAction::Add { .. }) {
                println!("{}", replaced_file_contents.green());
            } else if matches!(import_data.action, SyncImportAction::Delete { .. }) {
                println!("{}", replaced_file_contents.red());
            }
        }
        SyncImportAction::Update {
            from: ref note_from_filepath,
            to: ref note_to_filepath,
        } => {
            let base_command = "git";
            let args = vec![
                "diff",
                "--no-index",
                "--color=always",
                // "--word-diff",
                "--ws-error-highlight=new,old",
                // "--ws-error-highlight=all" // doesn't work with no-index
                // This is inverted on purpose since we want to diff against the source we are pushing data to.
                note_to_filepath.to_str().unwrap(),
                note_from_filepath.to_str().unwrap(),
            ];
            if dry_run {
                let command_str = format!("{} {}", base_command, args.join(" "));
                println!("Running command: {}", command_str.purple());
            }
            let diff_output = Command::new(base_command)
                .args(&args)
                // Ignore user's git config. This fixes the issue where the command does not work when the user overrides the `git diff` command.
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .map_err(|e| format!("Failed to diff notes: {}", e))?;
            println!();
            io::stdout()
                .write_all(&diff_output.stdout)
                .map_err(|e| format!("Failed to write stdout: {}", e))?;
        }
    }
    Ok(())
}

async fn sync_notes_between_files(
    sync_mode: &SyncMode,
    sync_source_from: SyncSource,
    sync_source_to: SyncSource,
    actions: Vec<SyncImportData>,
    dry_run: bool,
) -> Result<(Vec<NoteId>, Option<Vec<NoteId>>), String> {
    let mut modified_notes = Vec::new();
    let mut immutable_note_ids = Vec::new();
    // The inner vector represents all files you want to act on at once. One action will be selected for all of these items.
    let groupings: Vec<Vec<_>> = match sync_mode {
        SyncMode::Bulk => vec![actions],
        SyncMode::Individual => actions.into_iter().map(|x| vec![x]).collect::<Vec<_>>(),
    };

    for mut group in groupings {
        for import_data in &group {
            println!(
                "{} [{} -> {}]: {}",
                import_data.action.to_string().blue(),
                sync_source_from.to_string().black().on_green(),
                sync_source_to.to_string().black().on_bright_blue(),
                &import_data.note_id.to_string().black().on_yellow()
            );
            print_import_data(import_data, dry_run)?;
            println!();
        }

        // Prompt for action
        let mut options = SyncAction::iter().collect::<Vec<_>>();
        if matches!(sync_mode, SyncMode::Bulk) {
            options.retain(|x| !matches!(x, SyncAction::Next));
        }
        let mut select = Select::new("Action:", options);
        apply_select_settings(&mut select);
        let chosen_action_res = select.prompt();
        if chosen_action_res.is_err() {
            // The user exited. (Probably pressed Escape).
            return Ok((modified_notes, None));
        }
        match chosen_action_res.as_ref().unwrap() {
            SyncAction::PullChanges => {
                let new_modified_notes = update_changes(
                    sync_source_from,
                    sync_source_to,
                    &mut group,
                    &UpdateDirection::Pull,
                    dry_run,
                )
                .await?;
                modified_notes.extend(new_modified_notes);
            }
            SyncAction::PushChanges => {
                let new_modified_notes = update_changes(
                    sync_source_from,
                    sync_source_to,
                    &mut group,
                    &UpdateDirection::Push,
                    dry_run,
                )
                .await?;
                modified_notes.extend(new_modified_notes);
                // Notes and cards files are generated at the very end
            }
            SyncAction::Next => {
                immutable_note_ids.extend(
                    group
                        .iter()
                        .map(|sync_import_data| sync_import_data.note_id)
                        .collect::<Vec<_>>(),
                );
            }
            SyncAction::Exit => {
                return Ok((
                    modified_notes,
                    (!immutable_note_ids.is_empty()).then_some(immutable_note_ids),
                ));
            }
        }
        println!();
    }
    Ok((
        modified_notes,
        (!immutable_note_ids.is_empty()).then_some(immutable_note_ids),
    ))
}

pub(crate) async fn sync_notes_interactive(
    base_url: &str,
    client: &Client,
    sync_source_from: SyncSource,
    sync_source_to: SyncSource,
    dry_run: bool,
    sync_all_notes: bool,
    sync_mode: SyncMode,
) -> Result<(), String> {
    let sync_source_hub = SyncSource::default();
    if sync_source_from != sync_source_hub && sync_source_to != sync_source_hub {
        return Err(hub_spoke_error(sync_source_from, sync_source_to));
    }
    if dry_run {
        println!("{}\n", "DRY RUN".black().on_bright_yellow());
    }
    println!("Syncing from {} to {}.", sync_source_from, sync_source_to);

    // Render notes in cache directory
    let (from_output_dir, to_output_dir) =
        generate_notes(base_url, client, sync_source_from, sync_source_to).await?;

    // See which notes changed
    info!(
        "Diffing notes from {} to {}...",
        &from_output_dir.display(),
        &to_output_dir.display()
    );
    let import_data = get_import_data(&from_output_dir, &to_output_dir, dry_run, sync_all_notes)?;
    println!();
    if import_data.is_empty() {
        println!("All notes are up to date.");
        return Ok(());
    }
    println!("Found {} note(s) with differences\n", import_data.len());
    let (modified_notes, immutable_note_ids) = sync_notes_between_files(
        &sync_mode,
        sync_source_from,
        sync_source_to,
        import_data,
        dry_run,
    )
    .await?;

    regenerate_notes(
        base_url,
        client,
        modified_notes,
        immutable_note_ids,
        dry_run,
    )
    .await?;

    println!("Done");
    Ok(())
}

async fn regenerate_notes(
    base_url: &str,
    client: &Client,
    modified_notes: Vec<NoteId>,
    immutable_note_ids: Option<Vec<NoteId>>,
    dry_run: bool,
) -> Result<(), String> {
    // Regenerate linked notes and generate files
    // This will also ensure that updated notes will have their clozes renumbered sequentially so the note is ready to be edited again.
    if !modified_notes.is_empty() {
        println!("Rerendering notes...");
        let request = RenderNotesRequest {
            // Note that all notes can not have their files generated since some notes may still not be synced. For example, a couple notes may be skipped over.
            // Instead, all notes will have their linked notes regenerated, but only the specified notes will have their files regenerated.
            // See `render_notes()`.
            selector: NotesSelector::Ids(modified_notes),
            immutable_note_ids,
            overridden_output_raw_dir: None,
            include_linked_notes: true,
            include_cards: true,
            generate_rendered: true,
            force_generate_rendered: false,
        };
        if !dry_run {
            let url = format!("{}/api/notes/generate_files", base_url);
            let response = client
                .post(url)
                .json(&request)
                .send()
                .await
                .map_err(|e| format!("{}", e))?;
            let status = response.status();
            if status != StatusCode::OK {
                let response: Value = response.json().await.map_err(|e| format!("{}", e))?;
                return Err(response.to_string());
            }
        }
    }
    Ok(())
}
