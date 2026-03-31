use super::SyncSource;
use crate::sync::SyncImportAction;
use indexmap::IndexMap;
use inquire::Select;
use spares_core::{
    config::get_cache_dir,
    model::NoteId,
    parsers::{NoteSettingsKeys, Parseable},
};
use std::{
    collections::HashMap,
    fs,
    hash::Hash,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

pub(super) trait GroupByInsertion<A, B> {
    /// Groups the provided elements by A, sorted by the first presence of A. Thus, this is deterministic. Essentially, this is `.into_group_map()` provided by `itertools` if it were to return an `IndexMap` (from the `indexmap` crate) instead of a `HashMap`.
    fn into_group_by_insertion(self) -> Vec<(A, Vec<B>)>;
}

impl<A, B, I> GroupByInsertion<A, B> for I
where
    A: Hash + Eq,
    I: IntoIterator<Item = (A, B)>,
{
    fn into_group_by_insertion(self) -> Vec<(A, Vec<B>)> {
        let mut grouping: IndexMap<A, Vec<B>> = IndexMap::new();
        for (key, item) in self {
            grouping.entry(key).or_default().push(item);
        }
        grouping.into_iter().collect::<Vec<_>>()
    }
}

pub(super) fn apply_select_settings<T>(select: &mut Select<T>) {
    select.vim_mode = true;
}

pub(super) fn clear_dir(path: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

pub(super) fn hub_spoke_error(sync_source_from: SyncSource, sync_source_to: SyncSource) -> String {
    let sync_source_hub = SyncSource::default();
    format!(
        "Bidirectional syncing is only supported with {}. To sync from {} to {}, first sync from {} to {} and then from {} to {}.",
        sync_source_hub,
        sync_source_from,
        sync_source_to,
        sync_source_from,
        sync_source_hub,
        sync_source_hub,
        sync_source_to,
    )
}

pub(super) fn replace_action(
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

/// Build a map of relative paths (from the base directory) to full file paths
/// for all files in the given directory.
pub(super) fn build_file_map(base_dir: &Path) -> Result<HashMap<PathBuf, PathBuf>, String> {
    let mut file_map = HashMap::new();
    for entry in WalkDir::new(base_dir) {
        let entry = entry.map_err(|e| format!("Failed to walk directory: {}", e))?;
        if entry.file_type().is_file() {
            let full_path = entry.path().to_path_buf();
            let relative_path = full_path
                .strip_prefix(base_dir)
                .map_err(|e| format!("Failed to get relative path: {}", e))?
                .to_path_buf();
            file_map.insert(relative_path, full_path);
        }
    }
    Ok(file_map)
}

pub(super) fn blake3_hex(data: impl AsRef<[u8]>) -> String {
    blake3::hash(data.as_ref()).to_hex().to_string()
}

/// Persistent mtime+hash cache keyed by absolute path string → (`mtime_secs`, `blake3_hex`).
type HashIndex = HashMap<String, (u64, String)>;

fn hash_index_path() -> PathBuf {
    let mut p = get_cache_dir();
    p.push("sync");
    p.push("hash_index.json");
    p
}

pub(super) fn load_hash_index() -> HashIndex {
    let path = hash_index_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(super) fn save_hash_index(index: &HashIndex) {
    let path = hash_index_path();
    if let Ok(json) = serde_json::to_string(index) {
        let _ = fs::write(&path, json);
    }
}
