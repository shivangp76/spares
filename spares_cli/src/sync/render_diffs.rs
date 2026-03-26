use super::{SyncImportAction, get_import_data, replace_action, utils::clear_dir};
use spares_core::parsers::{find_parser, generate_files::RenderOutputType, get_all_parsers};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Render diffs in `/tmp/spares/{from_source_name}/diffs`
//   - `from_output_dir` is `/tmp/spares/{from_source_name}/notes/`
//   - `to_output_dir` is `/tmp/spares/{to_source_name}/notes/`
//   - Add `action: add` and `action: delete`, if needed.
pub(super) fn generate_diffs(
    from_output_dir: &Path,
    to_output_dir: &Path,
) -> Result<PathBuf, String> {
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
