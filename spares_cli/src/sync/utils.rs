use super::SyncSource;
use indexmap::IndexMap;
use inquire::Select;
use std::{fs, hash::Hash, path::Path};

pub trait GroupByInsertion<A, B> {
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

pub fn apply_select_settings<T>(select: &mut Select<T>) {
    select.vim_mode = true;
}

pub fn clear_dir(path: &Path) -> std::io::Result<()> {
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

pub fn hub_spoke_error(sync_source_from: SyncSource, sync_source_to: SyncSource) -> String {
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
