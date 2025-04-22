use super::{SyncImportAction, SyncImportData, replace_action};
use crate::sync::{
    SyncSource, UpdateDirection, generate_notes, get_import_data, hub_spoke_error,
    regenerate_notes, update_changes, utils::apply_select_settings,
};
use colored::Colorize;
use inquire::Select;
use reqwest::Client;
use spares::{
    model::NoteId,
    parsers::{find_parser, get_all_parsers},
};
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use strum::{EnumIter, IntoEnumIterator};
use strum_macros::{Display, EnumString};

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
enum SyncMode {
    Individual,
    Bulk,
}

fn print_import_data(import_data: &SyncImportData, run: bool) -> Result<(), String> {
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
            if !run {
                let command_str = format!("{} {}", base_command, args.join(" "));
                println!("Running command: {}", command_str.purple());
            }
            let diff_output = Command::new(base_command)
                .args(&args)
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
    run: bool,
) -> Result<Vec<NoteId>, String> {
    let mut modified_notes = Vec::new();
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
            print_import_data(import_data, run)?;
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
            return Ok(modified_notes);
        }
        match chosen_action_res.as_ref().unwrap() {
            SyncAction::PullChanges => {
                let new_modified_notes = update_changes(
                    sync_source_from,
                    sync_source_to,
                    &mut group,
                    &UpdateDirection::Pull,
                    run,
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
                    run,
                )
                .await?;
                modified_notes.extend(new_modified_notes);
                // Notes and cards files are generated at the very end
            }
            SyncAction::Next => {}
            SyncAction::Exit => {
                return Ok(modified_notes);
            }
        }
        println!();
    }
    Ok(modified_notes)
}

pub async fn sync_notes_interactive(
    base_url: &str,
    client: &Client,
    sync_source_from: SyncSource,
    sync_source_to: SyncSource,
    run: bool,
    sync_all_notes: bool,
) -> Result<(), String> {
    let sync_source_hub = SyncSource::default();
    if sync_source_from != sync_source_hub && sync_source_to != sync_source_hub {
        return Err(hub_spoke_error(sync_source_from, sync_source_to));
    }
    if !run {
        println!("{}\n", "DRY RUN".black().on_bright_yellow());
    }
    println!("Syncing from {} to {}.", sync_source_from, sync_source_to);

    // Render notes in temp directory
    let (from_output_dir, to_output_dir) =
        generate_notes(base_url, client, sync_source_from, sync_source_to).await?;

    // See which notes changed
    println!(
        "Diffing notes from {} to {}...",
        &from_output_dir.display(),
        &to_output_dir.display()
    );
    let import_data = get_import_data(&from_output_dir, &to_output_dir, run, sync_all_notes)?;
    println!();
    if import_data.is_empty() {
        println!("All notes are up to date.");
        return Ok(());
    }
    println!("Found {} note(s) with differences\n", import_data.len());
    let options = SyncMode::iter().collect::<Vec<_>>();
    let mut select = Select::new("Mode:", options);
    apply_select_settings(&mut select);
    let chosen_mode_res = select.prompt();
    if chosen_mode_res.is_err() {
        // The user exited. (Probably pressed Escape).
        return Ok(());
    }
    let sync_mode = chosen_mode_res.unwrap();
    let modified_notes = sync_notes_between_files(
        &sync_mode,
        sync_source_from,
        sync_source_to,
        import_data,
        run,
    )
    .await?;

    regenerate_notes(base_url, client, modified_notes, run).await?;

    println!("Done");
    Ok(())
}
