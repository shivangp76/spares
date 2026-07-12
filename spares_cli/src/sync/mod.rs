mod cloud;
mod interactive;
mod utils;

use std::collections::HashMap;
use std::fs;
use std::fs::remove_dir_all;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use clap::Args;
use clap::Subcommand;
use clap::ValueEnum;
use interactive::SyncMode;
use interactive::sync_notes_interactive;
use itertools::Itertools;
use log::info;
use rayon::prelude::*;
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::Value;
use spares_core::adapters::SrsAdapter;
use spares_core::adapters::impls::anki::AnkiAdapter;
use spares_core::adapters::impls::spares::SparesAdapter;
use spares_core::adapters::impls::spares::SparesRequestProcessor;
use spares_core::config::get_cache_dir;
use spares_core::config::get_data_dir;
use spares_core::model::NoteId;
use spares_core::parsers::NoteFilepathData;
use spares_core::parsers::find_parser;
use spares_core::parsers::generate_files::GenerateNoteFilesRequests;
use spares_core::parsers::generate_files::RenderOutputType;
use spares_core::parsers::generate_files::create_note_files_bulk;
use spares_core::parsers::get_all_parsers;
use spares_core::parsers::get_note_info_from_filepath;
use spares_core::parsers::get_output_raw_dir;
use spares_core::schema::note::NotesSelector;
use spares_core::schema::note::RenderNotesRequest;
use strum::EnumIter;
use strum_macros::Display;
use strum_macros::EnumString;
use utils::GroupByInsertion as _;
use utils::hub_spoke_error;

use crate::import::import_from_files;
use crate::sync::utils::blake3_hex;
use crate::sync::utils::build_file_map;
use crate::sync::utils::clear_dir;
use crate::sync::utils::load_hash_index;
use crate::sync::utils::replace_action;
use crate::sync::utils::save_hash_index;

#[derive(Args, Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct SyncArgs {
    #[command(subcommand)]
    pub(crate) subcommand: Option<SyncSubcommand>,

    // Having `from` and `to` is clearer than just specifying `source` to sync with the Hub (SparesDb). This also allows the git diffs to be highlighted appropriately.
    /// Sync Source
    #[arg(short, long, default_value = "spares-local-files")]
    pub(crate) from: SyncSource,
    /// Sync Destination
    #[arg(short, long, default_value = "spares")]
    pub(crate) to: SyncSource,
    #[arg(short, long, default_value_t = false)]
    pub(crate) dry_run: bool,
    /// Sync all files
    #[arg(long, default_value_t = false)]
    pub(crate) all: bool,
    /// Review all changes together as one group [default]
    #[arg(long, default_value_t = true, overrides_with_all = ["individual"])]
    pub(crate) bulk: bool,
    /// Review each change one at a time
    #[arg(long, overrides_with_all = ["bulk"])]
    pub(crate) individual: bool,
    /// Limit sync to specific note IDs
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    pub(crate) ids: Option<Vec<NoteId>>,
    /// Limit sync to notes matching the given file paths (accepts cache-rendered paths)
    #[arg(long, value_delimiter = ' ', num_args = 1..)]
    pub(crate) files: Option<Vec<PathBuf>>,
    /// Print changed note file paths to stdout (non-interactive). Use with fzf for batch selection.
    #[arg(long, default_value_t = false, conflicts_with_all = ["dry_run", "individual", "bulk"])]
    pub(crate) print_files: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum SyncSubcommand {
    /// Push local DB and files to a remote server via rsync.
    ///
    /// Reads `remote_host` from config.toml (e.g. "user@myserver.com").
    /// Runs two rsync passes: `SQLite` DB first, then all other files.
    Cloud,
}

/// Follows the hub-spoke model, where [`SyncSource::default()`] is the hub.
#[derive(Clone, Copy, Debug, Default, Display, PartialEq, ValueEnum)]
pub(crate) enum SyncSource {
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
#[expect(clippy::too_many_lines)]
async fn update_changes(
    original_sync_source_from: SyncSource,
    original_sync_source_to: SyncSource,
    import_datas: &mut [SyncImportData],
    direction: &UpdateDirection,
    dry_run: bool,
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
            // Overwrite note file with cache file
            for import_data in &mut *import_datas {
                match import_data.action {
                    SyncImportAction::Add { .. } | SyncImportAction::Update { .. } => {
                        if dry_run {
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

        // Collect delete filepaths before consuming import_data_filepaths
        let delete_filepaths: Vec<PathBuf> = import_data_filepaths
            .iter()
            .filter(|(_, action, _)| matches!(action, SyncImportAction::Delete { .. }))
            .map(|(fp, _, _)| (*fp).clone())
            .collect();

        // Import
        if let Some(ref mut adapter) = adapter_opt {
            let filepaths = import_data_filepaths
                .into_iter()
                .map(|(filepath, _, _)| filepath)
                .collect::<Vec<_>>();
            if dry_run {
                for filepath in filepaths {
                    println!("{} will be imported", filepath.display());
                }
            } else {
                import_from_files(
                    adapter.as_mut(),
                    Some(parser.as_ref()),
                    None,
                    filepaths.as_slice(),
                    false,
                    false, // not quiet
                )
                .await
                .map_err(|e| format!("{}", e))?;

                // Remove deleted notes from cache so next sync is up to date
                for filepath in &delete_filepaths {
                    let _ = fs::remove_file(filepath);
                }
            }
        }
    }
    Ok(import_datas.iter().map(|x| x.note_id).collect::<Vec<_>>())
}

pub(crate) async fn sync_notes(
    base_url: &str,
    client: &Client,
    sync_args: SyncArgs,
) -> Result<(), String> {
    let filter_note_ids = resolve_filter(sync_args.ids.as_ref(), sync_args.files.as_ref())?;

    match sync_args.subcommand {
        Some(SyncSubcommand::Cloud) => cloud::sync_cloud(),
        None => {
            // Non-interactive: print changed note file paths and exit
            if sync_args.print_files {
                let (from_output_dir, to_output_dir) =
                    generate_notes(base_url, client, sync_args.from, sync_args.to).await?;

                let import_data = get_import_data(
                    &from_output_dir,
                    &to_output_dir,
                    sync_args.dry_run,
                    sync_args.all,
                    filter_note_ids.as_ref(),
                )?;

                for import in &import_data {
                    let path = match &import.action {
                        SyncImportAction::Add { to } | SyncImportAction::Delete { to } => to,
                        SyncImportAction::Update { from, .. } => from,
                    };
                    println!("{}", path.display());
                }
                return Ok(());
            }

            let sync_mode = if sync_args.individual {
                SyncMode::Individual
            } else {
                SyncMode::Bulk
            };
            sync_notes_interactive(
                base_url,
                client,
                sync_args.from,
                sync_args.to,
                sync_args.dry_run,
                sync_args.all,
                sync_mode,
                filter_note_ids,
            )
            .await
        }
    }
}

/// Resolve `--ids` and `--files` flags into a set of note IDs for filtering.
/// Returns `None` if no filter is active.
fn resolve_filter(
    ids: Option<&Vec<NoteId>>,
    files: Option<&Vec<PathBuf>>,
) -> Result<Option<std::collections::HashSet<NoteId>>, String> {
    let mut set: std::collections::HashSet<NoteId> = std::collections::HashSet::new();

    if let Some(ids) = ids {
        set.extend(ids);
    }
    if let Some(files) = files {
        for f in files {
            let data = get_note_info_from_filepath(f)
                .map_err(|e| format!("Failed to parse file {}: {}", f.display(), e))?;
            set.insert(data.note_id);
        }
    }

    if set.is_empty() {
        Ok(None)
    } else {
        Ok(Some(set))
    }
}

/// Compute hashes for all paths, reusing mtime-based cached hashes where possible.
/// Loads and saves the persistent hash index.
fn compute_file_hashes(all_paths: &[PathBuf]) -> Result<HashMap<PathBuf, String>, String> {
    // Collect mtimes (sequential metadata reads)
    let path_mtimes: Vec<(PathBuf, u64)> = all_paths
        .iter()
        .map(|p| {
            let mtime = fs::metadata(p).and_then(|m| m.modified()).map_or(0, |t| {
                t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
            });
            (p.clone(), mtime)
        })
        .collect();

    // Find which paths have stale or missing cache entries
    let mut hash_index = load_hash_index();
    let needs_hashing: Vec<(PathBuf, u64)> = path_mtimes
        .iter()
        .filter(|(p, mtime)| {
            let key = p.to_string_lossy().to_string();
            !matches!(hash_index.get(&key), Some((cached_mtime, _)) if cached_mtime == mtime)
        })
        .cloned()
        .collect();

    // Hash stale/new files in parallel
    let new_hashes: Vec<(PathBuf, u64, String)> = needs_hashing
        .par_iter()
        .map(|(p, mtime)| {
            let contents =
                fs::read(p).map_err(|e| format!("Failed to read {}: {}", p.display(), e))?;
            let hash = blake3_hex(contents.as_slice());
            Ok((p.clone(), *mtime, hash))
        })
        .collect::<Result<_, String>>()?;

    for (p, mtime, hash) in &new_hashes {
        hash_index.insert(p.to_string_lossy().to_string(), (*mtime, hash.clone()));
    }

    let file_hashes: HashMap<PathBuf, String> = path_mtimes
        .iter()
        .filter_map(|(p, _)| {
            let key = p.to_string_lossy().to_string();
            hash_index.get(&key).map(|(_, h)| (p.clone(), h.clone()))
        })
        .collect();

    save_hash_index(&hash_index);
    Ok(file_hashes)
}

/// Find add/update/delete actions by comparing `from_files` against `to_files`.
fn find_diff_actions(
    from_files: &HashMap<PathBuf, PathBuf>,
    to_files: &HashMap<PathBuf, PathBuf>,
    file_hashes: &HashMap<PathBuf, String>,
    from_output_base_dir: &Path,
) -> Vec<SyncImportData> {
    let mut import_data = Vec::new();

    // Files in 'from' only → Add; files in both but differing → Update
    for (relative_path, from_path) in from_files {
        let Some(NoteFilepathData {
            parser_name,
            note_id,
        }) = get_note_info_from_filepath(from_path).ok()
        else {
            continue;
        };

        if let Some(to_path) = to_files.get(relative_path) {
            let hashes_match = match (file_hashes.get(to_path), file_hashes.get(from_path)) {
                (Some(h1), Some(h2)) => h1 == h2,
                _ => false,
            };
            if !hashes_match {
                let mut note_from_filepath = get_output_raw_dir(
                    &parser_name,
                    RenderOutputType::Note,
                    Some(from_output_base_dir),
                );
                note_from_filepath.push(from_path.file_name().unwrap());
                import_data.push(SyncImportData {
                    note_id,
                    parser_name,
                    action: SyncImportAction::Update {
                        from: note_from_filepath,
                        to: to_path.clone(),
                    },
                });
            }
        } else {
            // Note: for Add, the 'to' field contains the source file path (in 'from')
            import_data.push(SyncImportData {
                note_id,
                parser_name,
                action: SyncImportAction::Add {
                    to: from_path.clone(),
                },
            });
        }
    }

    // Files in 'to' only → Delete
    import_data.extend(
        to_files
            .iter()
            .filter(|(relative_path, _)| !from_files.contains_key(*relative_path))
            .filter_map(|(_, to_path)| {
                get_note_info_from_filepath(to_path)
                    .ok()
                    .map(|info| (to_path, info.parser_name, info.note_id))
            })
            .map(|(to_path, parser_name, note_id)| SyncImportData {
                note_id,
                parser_name,
                action: SyncImportAction::Delete {
                    to: to_path.clone(),
                },
            }),
    );

    import_data
}

/// When `sync_all_notes` is set, append Update actions for all `to` files not already covered.
fn expand_sync_all(
    import_data: &mut Vec<SyncImportData>,
    to_files: &HashMap<PathBuf, PathBuf>,
    from_output_base_dir: &Path,
) {
    let existing_to_paths: std::collections::HashSet<_> = import_data
        .iter()
        .filter_map(|d| match &d.action {
            SyncImportAction::Update { to, .. } | SyncImportAction::Delete { to } => {
                Some(to.clone())
            }
            SyncImportAction::Add { .. } => None,
        })
        .collect();

    for to_path in to_files.values() {
        if existing_to_paths.contains(to_path) {
            continue;
        }
        let Some(NoteFilepathData {
            parser_name,
            note_id,
        }) = get_note_info_from_filepath(to_path).ok()
        else {
            continue;
        };
        let mut note_from_filepath = get_output_raw_dir(
            &parser_name,
            RenderOutputType::Note,
            Some(from_output_base_dir),
        );
        note_from_filepath.push(to_path.file_name().unwrap());
        import_data.push(SyncImportData {
            note_id,
            parser_name,
            action: SyncImportAction::Update {
                from: note_from_filepath,
                to: to_path.clone(),
            },
        });
    }
}

/// Fuse pairs of (Add, Delete) for the same note id into a single Update.
/// This handles notes whose parser was changed, which appears as a delete from the old
/// parser path and an add to the new parser path.
fn fuse_parser_changes(import_data: Vec<SyncImportData>) -> Vec<SyncImportData> {
    // Use into_group_by_insertion instead of into_group_map for deterministic output order
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
                SyncImportAction::Delete { to } => {
                    note_to_filepath = Some(to.clone());
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
    import_data_map.into_iter().flat_map(|(_, v)| v).collect()
}

fn get_import_data(
    from_output_dir: &Path,
    to_output_dir: &Path,
    dry_run: bool,
    sync_all_notes: bool,
    filter_note_ids: Option<&std::collections::HashSet<NoteId>>,
) -> Result<Vec<SyncImportData>, String> {
    let from_output_base_dir = from_output_dir.parent().unwrap();

    if dry_run {
        info!(
            "Comparing directories: {} vs {}",
            to_output_dir.display(),
            from_output_dir.display()
        );
    }

    let to_files = build_file_map(to_output_dir)?;
    let from_files = build_file_map(from_output_dir)?;

    let all_paths: Vec<PathBuf> = from_files
        .values()
        .chain(to_files.values())
        .cloned()
        .collect();
    let file_hashes = compute_file_hashes(&all_paths)?;

    let mut import_data =
        find_diff_actions(&from_files, &to_files, &file_hashes, from_output_base_dir);

    if sync_all_notes {
        expand_sync_all(&mut import_data, &to_files, from_output_base_dir);
    }

    let mut import_data = fuse_parser_changes(import_data);

    if let Some(filter) = filter_note_ids {
        import_data.retain(|d| filter.contains(&d.note_id));
    }

    Ok(import_data)
}

/// Generate all notes (not cards) in cache directory
async fn generate_notes(
    base_url: &str,
    client: &Client,
    sync_source_from: SyncSource,
    sync_source_to: SyncSource,
) -> Result<(PathBuf, PathBuf), String> {
    // Use persistent cache directory so rendered notes survive reboots
    let mut base_dir = get_cache_dir();
    base_dir.push("sync");
    fs::create_dir_all(&base_dir).map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let mut output_dirs: Vec<PathBuf> = Vec::with_capacity(2);

    for source in [sync_source_from, sync_source_to] {
        match source {
            SyncSource::Spares => {
                let mut output_dir = base_dir.clone();
                output_dir.push("spares");
                let mut returned_output_dir = output_dir.clone();
                returned_output_dir.push("notes");
                output_dirs.push(returned_output_dir.clone());

                // Clear directory first
                if output_dir.exists() {
                    clear_dir(&output_dir).map_err(|e| format!("{}", e))?;
                }

                info!("Rendering notes from Spares...");
                let include_linked_notes = sync_source_from == SyncSource::SparesLocalFiles
                    || sync_source_to == SyncSource::SparesLocalFiles;
                let request = RenderNotesRequest {
                    selector: NotesSelector::All,
                    immutable_note_ids: None,
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
    let (from_output_dir, to_output_dir) = (output_dirs[0].clone(), output_dirs[1].clone());

    Ok((from_output_dir, to_output_dir))
}
