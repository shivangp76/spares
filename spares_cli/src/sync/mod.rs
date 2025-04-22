pub mod interactive;
pub mod utils;

use crate::import::import_from_files;
use clap::{Args, Subcommand, ValueEnum, command};
use colored::Colorize;
use interactive::sync_notes_interactive;
use itertools::Itertools;
use log::info;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use spares::{
    adapters::{
        SrsAdapter,
        impls::{
            anki::AnkiAdapter,
            spares::{SparesAdapter, SparesRequestProcessor},
        },
    },
    config::get_data_dir,
    model::NoteId,
    parsers::{
        NoteFilepathData, NoteSettingsKeys, Parseable, find_parser,
        generate_files::{GenerateNoteFilesRequests, RenderOutputType, create_note_files_bulk},
        get_all_parsers, get_note_info_from_filepath, get_output_raw_dir,
    },
    schema::note::{GenerateFilesNoteIds, RenderNotesRequest},
};
use std::path::{Path, PathBuf};
use std::{fs, process::Command};
use std::{fs::remove_dir_all, io::Write};
use strum::EnumIter;
use strum_macros::{Display, EnumString};
use utils::{GroupByInsertion as _, clear_dir, hub_spoke_error};
use walkdir::WalkDir;

#[derive(Args, Debug, Clone)]
pub struct SyncArgs {
    #[command(subcommand)]
    pub action: SyncMainAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SyncMainAction {
    #[command(arg_required_else_help = false)]
    Interactive {
        // Having `from` and `to` is clearer than just specifying `source` to synce with the Hub (SparesDb). This also allows the git diffs to be highlighted appropriately.
        /// Sync Source
        #[arg(short, long, default_value = "spares-local-files")]
        from: SyncSource,
        /// Sync Destination
        #[arg(short, long, default_value = "spares")]
        to: SyncSource,
        #[arg(short, long, default_value_t = false)]
        run: bool,
        /// Sync all files
        #[arg(long, default_value_t = false)]
        all: bool,
    },
    /// Render all diffs between the source and destination.
    ///
    /// For example, if syncing from Spares to Anki, then `/tmp/spares/spares/markdown/0001.md` will have the diff of the note with id 1 in spares with the note with spares id 1 in Anki. All files in `/tmp/spares/spares/` will contain the diffs for all the notes, sorted by their parser. To sync this note, `spares_cli sync files --from spares --to anki /tmp/spares/spares/markdown/0001.md` can be run.
    ///
    /// Returns the path to the directory containing the diffs.
    #[command(arg_required_else_help = false)]
    RenderDiffs {
        /// Sync Source
        #[arg(short, long, default_value = "spares-local-files")]
        from: SyncSource,
        /// Sync Destination
        #[arg(short, long, default_value = "spares")]
        to: SyncSource,
    },
}

/// Follows the hub-spoke model, where [`SyncSource::default()`] is the hub.
#[derive(Clone, Copy, Debug, Default, Display, PartialEq, ValueEnum)]
pub enum SyncSource {
    #[default]
    Spares,
    SparesLocalFiles,
    Anki,
}

#[derive(Debug)]
struct SyncImportData {
    parser_name: String,
    note_id: NoteId,
    action: SyncImportAction,
}

#[derive(Clone, Debug, Display, EnumIter, EnumString, PartialEq)]
enum SyncImportAction {
    Add { to: PathBuf },
    Update { from: PathBuf, to: PathBuf },
    Delete { to: PathBuf },
}

#[derive(Debug)]
enum UpdateDirection {
    Push,
    Pull,
}

fn replace_action(
    original_note_contents: &str,
    action: &SyncImportAction,
    parser: &dyn Parseable,
    note_id: NoteId,
) -> Option<String> {
    if matches!(action, SyncImportAction::Update { .. }) {
        return None;
    }
    let NoteSettingsKeys {
        action: action_key,
        action_add,
        action_delete,
        settings_key_value_delim,
        settings_delim,
        note_id: note_id_key,
        ..
    } = parser.note_settings_keys();
    let action_str = match action {
        SyncImportAction::Add { .. } => action_add,
        SyncImportAction::Update { .. } => unreachable!(),
        SyncImportAction::Delete { .. } => action_delete,
    };
    let old_note_id_string = format!(
        "{}{} {}",
        note_id_key.get_write(),
        settings_key_value_delim,
        note_id
    );
    let new_action_note_id_string = format!(
        "{}{} {}{} {}",
        action_key.get_write(),
        settings_key_value_delim,
        action_str.get_write(),
        settings_delim,
        old_note_id_string
    );
    let new_content =
        original_note_contents.replacen(&old_note_id_string, new_action_note_id_string.as_str(), 1);
    Some(new_content)
}

/// Note that you can switch entries in this table as long as you flip the direction of syncing.
/// For example, syncing from `SyncSource::A` to `SyncSource::B` the action of a `push` and `add` is equivalent to syncing from `SyncSource::B` to `SyncSource::A` the action of a `pull` and `delete`.
/// This function only handles the `push` equivalent version, so corresponding entry is used if a `pull` action is requested.
///
/// |        | Push | Pull |
/// |--------|------|-------
/// | Add    | 1    | 3    |
/// | Update | 2    | 2    |
/// | Delete | 3    | 1    |
///
/// Returns modified note.
#[allow(clippy::too_many_lines)]
async fn update_changes(
    original_sync_source_from: SyncSource,
    original_sync_source_to: SyncSource,
    import_datas: &mut [SyncImportData],
    direction: &UpdateDirection,
    run: bool,
) -> Result<Vec<NoteId>, String> {
    let (sync_source_from, sync_source_to) = match direction {
        UpdateDirection::Push => (original_sync_source_from, original_sync_source_to),
        UpdateDirection::Pull => (original_sync_source_to, original_sync_source_from),
    };
    // Modify actions based on direction
    for import_data in import_datas.iter_mut() {
        match direction {
            UpdateDirection::Push => {}
            UpdateDirection::Pull => {
                let inverted_action = match import_data.action.clone() {
                    SyncImportAction::Add { to } => SyncImportAction::Delete { to },
                    SyncImportAction::Update { from, to } => {
                        SyncImportAction::Update { from: to, to: from }
                    }
                    SyncImportAction::Delete { to } => SyncImportAction::Add { to },
                };
                import_data.action = inverted_action;
            }
        }
    }
    let mut adapter_opt: Option<Box<dyn SrsAdapter>> = None;
    match (sync_source_from, sync_source_to) {
        (SyncSource::Spares, SyncSource::Spares)
        | (SyncSource::Anki, SyncSource::Anki)
        | (SyncSource::SparesLocalFiles, SyncSource::SparesLocalFiles) => {
            return Err("The sources `from` and `to` must be different.".to_string());
        }
        (SyncSource::Anki, SyncSource::SparesLocalFiles)
        | (SyncSource::SparesLocalFiles, SyncSource::Anki) => {
            return Err(hub_spoke_error(sync_source_from, sync_source_to));
        }
        (SyncSource::Spares, SyncSource::SparesLocalFiles) => {
            // Overwrite note file with temp file
            for import_data in &mut *import_datas {
                match import_data.action {
                    SyncImportAction::Add { .. } | SyncImportAction::Update { .. } => {
                        if !run {
                            println!("This will be handled when render notes is called.");
                        }
                        // fs::copy(note_from_filepath, note_to_filepath).map_err(|e| format!("{}", e))?;
                        // println!(
                        //     "Copied {} to {}",
                        //     &note_from_filepath.display(),
                        //     &note_to_filepath.display()
                        // );
                    }
                    SyncImportAction::Delete { .. } => match direction {
                        UpdateDirection::Push => {
                            return Err("Unsupported. Notes cannot be manually deleted from the Spares database. The Spares API must be used which will ensure local files stay synced.".to_string());
                        }
                        UpdateDirection::Pull => {
                            return Err("Unsupported. Notes cannot be created through files. The Spares import API must be used which will ensure local files stay synced and the proper id is assigned.".to_string());
                        }
                    },
                }
            }
        }
        (SyncSource::SparesLocalFiles | SyncSource::Anki, SyncSource::Spares) => {
            adapter_opt = Some(Box::new(SparesAdapter::new(SparesRequestProcessor::Server)));
        }
        (SyncSource::Spares, SyncSource::Anki) => {
            adapter_opt = Some(Box::new(AnkiAdapter::new()));
        }
    }

    let grouped_import_datas = import_datas
        .iter()
        .map(|x| (x.parser_name.clone(), x))
        .into_group_map();
    for (parser_name, import_datas) in grouped_import_datas {
        let parser = find_parser(parser_name.as_str(), &get_all_parsers())
            .map_err(|e| format!("{:?}", e))?;
        let import_data_filepaths = import_datas
            .iter()
            .map(|import_data| {
                (
                    match import_data.action {
                        SyncImportAction::Add { to: ref from }
                        | SyncImportAction::Delete { to: ref from }
                        | SyncImportAction::Update { ref from, .. } => from,
                    },
                    import_data.action.clone(),
                    import_data.note_id,
                )
            })
            .collect::<Vec<_>>();

        // Update files to match action
        for (note_from_filepath, action, note_id) in &import_data_filepaths {
            let content = fs::read_to_string(note_from_filepath).map_err(|e| format!("{}", e))?;
            let new_content_opt =
                replace_action(content.as_str(), action, parser.as_ref(), *note_id);
            if let Some(new_content) = new_content_opt {
                // Open the file in write mode to overwrite the original content
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(note_from_filepath)
                    .map_err(|e| format!("{}", e))?;
                file.write_all(new_content.as_bytes())
                    .map_err(|e| format!("{}", e))?;
            }
        }

        // Import
        if let Some(ref mut adapter) = adapter_opt {
            if run {
                let filepaths = import_data_filepaths
                    .into_iter()
                    .map(|(filepath, _, _)| filepath.clone())
                    .collect::<Vec<_>>();
                let quiet = false;
                import_from_files(
                    adapter.as_mut(),
                    Some(parser.as_ref()),
                    None,
                    filepaths,
                    true,
                    quiet,
                )
                .await
                .map_err(|e| format!("{}", e))?;
            } else {
                for (note_from_filepath, _, _) in import_data_filepaths {
                    println!("{} will be imported", note_from_filepath.display());
                }
            }
        }
    }
    Ok(import_datas.iter().map(|x| x.note_id).collect::<Vec<_>>())
}

async fn regenerate_notes(
    base_url: &str,
    client: &Client,
    modified_notes: Vec<i64>,
    run: bool,
) -> Result<(), String> {
    // Regenerate linked notes and generate files
    // This will also ensure that updated notes will have their clozes renumbered sequentially so the note is ready to be edited again.
    if !modified_notes.is_empty() {
        println!("Rerendering notes...");
        let request = RenderNotesRequest {
            // Note that all notes can not have their files generated since some notes may still not be synced. For example, a couple notes may be skipped over.
            // Instead, all notes will have their linked notes regenerated, but only the specified notes will have their files regenerated.
            // See `render_notes()`.
            generate_files_note_ids: Some(GenerateFilesNoteIds::NoteIds(modified_notes)),
            overridden_output_raw_dir: None,
            include_linked_notes: true,
            include_cards: true,
            generate_rendered: true,
            force_generate_rendered: false,
        };
        if run {
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
                dbg!(&response);
                return Err(response.to_string());
            }
        }
    }
    Ok(())
}

/// Generate all notes (not cards) in temp directory
async fn generate_notes(
    base_url: &str,
    client: &Client,
    sync_source_from: SyncSource,
    sync_source_to: SyncSource,
    // base_dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    // Create temp directory
    let mut base_dir = PathBuf::from("/tmp");
    base_dir.push(clap::crate_name!());

    let mut output_dirs: Vec<PathBuf> = Vec::with_capacity(2);
    for source in [sync_source_from, sync_source_to] {
        match source {
            SyncSource::Spares => {
                let mut output_dir = base_dir.clone();
                output_dir.push("spares");
                let mut returned_output_dir = output_dir.clone();
                returned_output_dir.push("notes");
                output_dirs.push(returned_output_dir);

                // Clear directory first
                if output_dir.exists() {
                    clear_dir(&output_dir).map_err(|e| format!("{}", e))?;
                }

                info!("Rendering notes from Spares...");
                let include_linked_notes = sync_source_from == SyncSource::SparesLocalFiles
                    || sync_source_to == SyncSource::SparesLocalFiles;
                let request = RenderNotesRequest {
                    generate_files_note_ids: None,
                    overridden_output_raw_dir: Some(output_dir.clone()),
                    include_linked_notes,
                    include_cards: false,
                    generate_rendered: false,
                    force_generate_rendered: false,
                };
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
                    dbg!(&response);
                    return Err(response.to_string());
                }
            }
            SyncSource::SparesLocalFiles => {
                let mut output_dir = base_dir.clone();
                output_dir.push("spares_local_files");
                output_dir.push("notes");
                output_dirs.push(output_dir.clone());

                // Create empty parent directory since `copy_dir` requires that the directory to
                // not exist but its parent directories to exist
                if output_dir.exists() {
                    remove_dir_all(&output_dir).map_err(|e| format!("{}", e))?;
                } else {
                    std::fs::create_dir_all(output_dir.parent().unwrap())
                        .map_err(|e| format!("Failed to create directory: {}", e))?;
                }

                let mut local_notes_dir = get_data_dir();
                local_notes_dir.push("notes");
                copy_dir::copy_dir(local_notes_dir, output_dir).map_err(|e| format!("{}", e))?;
                // output_dirs.push(local_notes_dir);
            }
            SyncSource::Anki => {
                let mut output_dir = base_dir.clone();
                output_dir.push("anki");
                let mut returned_output_dir = output_dir.clone();
                returned_output_dir.push("notes");
                output_dirs.push(returned_output_dir);

                // Clear directory first
                if output_dir.exists() {
                    clear_dir(&output_dir).map_err(|e| format!("{}", e))?;
                }

                info!("Rendering notes from Anki...");
                let anki_db_path = std::env::var("ANKI_DB_PATH")
                    .map_err(|e| format!("ANKI_DB_PATH environment variable is not set: {}", e))?;
                let db_path = PathBuf::from(anki_db_path);
                // let start = std::time::Instant::now();
                let parse_note_requests =
                    AnkiAdapter::database_to_requests(db_path.as_path(), None)
                        .await
                        .map_err(|e| format!("{}", e))?;
                let grouped_notes = parse_note_requests.into_iter().into_group_map();
                for (parser_name, requests) in grouped_notes {
                    let parser =
                        find_parser(&parser_name, &get_all_parsers()).map_err(|e| e.to_string())?;
                    let parse_notes_request = GenerateNoteFilesRequests {
                        requests,
                        overridden_output_raw_dir: Some(output_dir.clone()),
                        include_cards: false,
                        render: false,
                        force_render: false,
                    };
                    let _card_paths = create_note_files_bulk(parser.as_ref(), &parse_notes_request)
                        .map_err(|e| format!("{}", e))?
                        .into_iter()
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| format!("{}", e))?;
                }
            }
        }
    }
    assert_eq!(output_dirs.len(), 2);
    Ok((output_dirs[0].clone(), output_dirs[1].clone()))
}

// Render diffs in `/tmp/spares_cli/{from_source_name}/diffs`
//   - `from_output_dir` is `/tmp/spares_cli/{from_source_name}/notes/`
//   - `to_output_dir` is `/tmp/spares_cli/{to_source_name}/notes/`
//   - Add `action: add` and `action: delete`, if needed.
fn generate_diffs(from_output_dir: &Path, to_output_dir: &Path) -> Result<PathBuf, String> {
    // Get the parent of the 'notes' directory which should be the source directory
    let source_dir = from_output_dir
        .parent()
        .ok_or_else(|| String::from("from_output_dir must have a parent directory"))?;

    // Create the diff directory as a sibling to 'notes'
    let diff_dir = source_dir.join("diffs");
    if diff_dir.exists() {
        clear_dir(&diff_dir).map_err(|e| format!("Failed to clear directory: {}", e))?;
    } else {
        std::fs::create_dir_all(&diff_dir)
            .map_err(|e| format!("Failed to create diff directory: {}", e))?;
    }

    // First, get the list of changed files
    let all_import_data = get_import_data(from_output_dir, to_output_dir, true, false)?;

    let dev_null = PathBuf::from("/dev/null");

    // Generate individual diffs for each file
    for sync_import_data in all_import_data {
        let (from_file_path, to_file_path, import_file_path) = match &sync_import_data.action {
            SyncImportAction::Add { to: from } => (from, &dev_null, from),
            SyncImportAction::Update { from, to } => (from, to, to),
            SyncImportAction::Delete { to } => (&dev_null, to, to),
        };
        let mut diff_file_path = diff_dir.clone();
        diff_file_path.push(&sync_import_data.parser_name);
        let parser = find_parser(sync_import_data.parser_name.as_str(), &get_all_parsers())
            .map_err(|e| format!("{}", e))?;
        diff_file_path
            .push(parser.get_output_filename(RenderOutputType::Note, sync_import_data.note_id));
        let ext = import_file_path
            .extension()
            .ok_or_else(|| format!("Failed to get extension: {}", diff_file_path.display()))?;
        let mut new_ext = ext.to_os_string();
        new_ext.push(".diff");
        diff_file_path.set_extension(new_ext);

        // Replace the action in the note file
        let file_contents = fs::read_to_string(import_file_path).map_err(|e| format!("{}", e))?;
        let replaced_file_contents = replace_action(
            &file_contents,
            &sync_import_data.action,
            parser.as_ref(),
            sync_import_data.note_id,
        )
        .unwrap_or(file_contents);
        std::fs::write(import_file_path, replaced_file_contents).map_err(|e| {
            format!(
                "Failed to write file for {}: {}",
                import_file_path.display(),
                e
            )
        })?;
        // If deleting, then create note file so replacing the `diffs` directory with `notes` makes sense.
        if matches!(sync_import_data.action, SyncImportAction::Delete { .. }) {
            let mut from_file_path = from_output_dir.to_path_buf();

            from_file_path.push(&sync_import_data.parser_name);
            from_file_path
                .push(parser.get_output_filename(RenderOutputType::Note, sync_import_data.note_id));
            from_file_path.set_extension(ext);
            fs::copy(import_file_path, from_file_path)
                .map_err(|e| format!("Failed to copy data: {}", e))?;
        }

        // Generate diff for this specific file
        let output = Command::new("git")
            .arg("diff")
            .arg("--color")
            .arg("--no-index")
            .arg("--patch")
            // This is inverted on purpose since we want to diff against the source we are pushing data to.
            .arg(to_file_path)
            .arg(from_file_path)
            .output()
            .map_err(|e| {
                format!(
                    "Failed to execute git diff for {}: {}",
                    from_file_path.display(),
                    e
                )
            })?;
        let diff_file_contents = String::from_utf8(output.stdout)
            .map_err(|e| format!("Failed to parse git diff output: {}", e))?;

        // Create necessary subdirectories in the diff directory
        if let Some(parent) = diff_file_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory structure for diff: {}", e))?;
        }
        std::fs::write(&diff_file_path, diff_file_contents).map_err(|e| {
            format!(
                "Failed to write diff file for {}: {}",
                from_file_path.display(),
                e
            )
        })?;
    }

    Ok(diff_dir)
}

pub async fn sync_notes(
    base_url: &str,
    client: &Client,
    sync_args: SyncArgs,
) -> Result<(), String> {
    match sync_args.action {
        SyncMainAction::Interactive {
            from: sync_source_from,
            to: sync_source_to,
            run,
            all: sync_all_notes,
        } => {
            sync_notes_interactive(
                base_url,
                client,
                sync_source_from,
                sync_source_to,
                run,
                sync_all_notes,
            )
            .await
        }
        SyncMainAction::RenderDiffs {
            from: sync_source_from,
            to: sync_source_to,
        } => {
            // Render notes in temp directory
            let (from_output_dir, to_output_dir) =
                generate_notes(base_url, client, sync_source_from, sync_source_to).await?;

            // Render diffs
            let diffs_directory_path = generate_diffs(&from_output_dir, &to_output_dir)?;
            println!("{}", diffs_directory_path.display());

            Ok(())
        }
    }
}

#[expect(clippy::too_many_lines)]
fn get_import_data(
    from_output_dir: &Path,
    to_output_dir: &Path,
    run: bool,
    sync_all_notes: bool,
) -> Result<Vec<SyncImportData>, String> {
    let from_output_base_dir = &from_output_dir.parent().unwrap();
    let base_command = "git";
    let args = vec![
        "diff",
        "--no-index",
        "--name-status",
        "--no-renames",
        // Think of `to_output_dir` like the main branch that we want to merge _into_, so it goes first.
        &to_output_dir.to_str().unwrap(),
        &from_output_dir.to_str().unwrap(),
    ];
    if !run {
        let command_str = format!("{} {}", base_command, args.join(" "));
        println!("Running command: {}", command_str.purple());
    }
    let git_diff_output = Command::new(base_command)
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to diff notes: {}", e))?;
    let output_str = String::from_utf8(git_diff_output.stdout)
        .map_err(|e| format!("Failed to parse git diff output: {}", e))?;
    let diff_lines = output_str.lines().collect::<Vec<&str>>();
    let mut note_filepaths_raw = diff_lines
        .iter()
        .filter_map(|line| {
            // Split at the first whitespace, which separates the modifier and the filename
            let mut parts = line.splitn(2, char::is_whitespace);
            let modifier = parts.next()?.to_string();
            let note_filepath_str = parts.next()?.trim().to_string();
            let note_filepath = PathBuf::from(note_filepath_str);
            Some((modifier, note_filepath))
        })
        .collect::<Vec<_>>();
    if sync_all_notes {
        // Get all notes
        let mut all_files = WalkDir::new(to_output_dir)
            .into_iter()
            .filter_map(Result::ok)
            .map(|entry| entry.path().to_path_buf())
            .filter(|x| x.is_file())
            .collect::<Vec<_>>();
        let modified_filepaths = note_filepaths_raw
            .iter()
            .map(|(_, y)| y)
            .collect::<Vec<_>>();
        all_files.retain(|x| !modified_filepaths.contains(&x));
        note_filepaths_raw.extend(
            all_files
                .into_iter()
                .map(|x| ("M".to_string(), x))
                .collect::<Vec<_>>(),
        );
    }
    let import_data = note_filepaths_raw
        .into_iter()
        .map(|(modifier, note_to_filepath)| {
            let note_info_opt = get_note_info_from_filepath(&note_to_filepath).ok();
            if note_info_opt.is_none() {
                return Ok(None);
            }
            let NoteFilepathData {
                parser_name,
                note_id,
            } = note_info_opt.unwrap();
            let parser = find_parser(parser_name.as_str(), &get_all_parsers())
                .map_err(|e| format!("{:?}", e))?;
            let mut note_from_filepath = get_output_raw_dir(
                parser.get_parser_name(),
                RenderOutputType::Note,
                Some(from_output_base_dir),
            );
            let note_filename = note_to_filepath.file_name().unwrap().to_str().unwrap();
            note_from_filepath.push(note_filename);

            let update_action = match modifier.as_str() {
                // For Add, git diff returns note_from_filepath, so this is actually note_from_filepath
                "A" => SyncImportAction::Add {
                    to: note_to_filepath,
                },
                "M" => SyncImportAction::Update {
                    from: note_from_filepath,
                    to: note_to_filepath,
                },
                "D" => SyncImportAction::Delete {
                    to: note_to_filepath,
                },
                key => panic!("Unsupported git diff --name-status action: {}", key),
            };
            Ok(Some(SyncImportData {
                note_id,
                parser_name,
                action: update_action,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    // Identify notes that had their parser changed. This will appear as 2 actions: deleting the note from the old parser and adding the note to the new parser. These should be fused to an update.
    let mut import_data_map = import_data
        .into_iter()
        .map(|d| (d.note_id, d))
        // This is used instead of `.into_group_map()` for consistency in the user output
        .into_group_by_insertion();
    for (note_id, dups) in import_data_map.iter_mut().filter(|(_, v)| v.len() == 2) {
        let mut new_parser_name: Option<String> = None;
        let mut note_from_filepath: Option<PathBuf> = None;
        let mut note_to_filepath: Option<PathBuf> = None;
        for dup in &*dups {
            match &dup.action {
                SyncImportAction::Add { to: from } => {
                    new_parser_name = Some(dup.parser_name.clone());
                    note_from_filepath = Some(from.clone());
                }
                SyncImportAction::Update { .. } => unreachable!(),
                SyncImportAction::Delete { to: from } => {
                    note_to_filepath = Some(from.clone());
                }
            }
        }
        *dups = vec![SyncImportData {
            note_id: *note_id,
            parser_name: new_parser_name.unwrap(),
            action: SyncImportAction::Update {
                from: note_from_filepath.unwrap(),
                to: note_to_filepath.unwrap(),
            },
        }];
    }
    let result = import_data_map
        .into_iter()
        .flat_map(|(_, v)| v)
        .collect::<Vec<_>>();
    Ok(result)
}
