//! A collection owns its watcher, so closing it releases the watch and its file list.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::{
    ChangedFilesRequest, ChangedFilesResponse, INTERNAL_DIRS, RenderedFile, bucket_digest,
    file_bucket, is_rendered, mtime, rendered_files_in,
};
use crate::Error;
use crate::collection::lock;

const RECONCILE: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Pending {
    paths: BTreeSet<PathBuf>,
    rescan: bool,
}

impl Pending {
    fn record(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        if self.rescan {
            return;
        }
        self.paths.extend(paths);
        if self.paths.len() > 4096 {
            self.rescan = true;
            self.paths.clear();
        }
    }
}

pub(crate) struct Manifest {
    root: PathBuf,
    buckets: BTreeMap<String, Vec<RenderedFile>>,
    digests: BTreeMap<String, String>,
    scanned: Instant,
    pending: Arc<Mutex<Pending>>,
    _watcher: RecommendedWatcher,
}

impl Manifest {
    pub(super) fn watch(root: &Path) -> Result<Option<Self>, Error> {
        let Ok(root) = root.canonicalize() else {
            return Ok(None);
        };
        let pending = Arc::new(Mutex::new(Pending::default()));
        let events = pending.clone();
        let callback = move |event: notify::Result<Event>| {
            let mut pending = lock(&events);
            match event {
                Ok(event) if !event.need_rescan() && !event.paths.is_empty() => {
                    if !matches!(event.kind, EventKind::Access(_)) {
                        pending.record(event.paths);
                    }
                }
                _ => pending.rescan = true,
            }
        };
        let Ok(mut watcher) = RecommendedWatcher::new(
            callback,
            notify::Config::default().with_follow_symlinks(false),
        ) else {
            return Ok(None);
        };
        if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
            return Ok(None);
        }
        // Watch before listing so changes during the initial walk are retained.
        let mut manifest = Self {
            root,
            buckets: BTreeMap::new(),
            digests: BTreeMap::new(),
            pending,
            scanned: Instant::now(),
            _watcher: watcher,
        };
        manifest.rebuild()?;
        Ok(Some(manifest))
    }

    pub(super) fn mark(&self, relative: &Path) {
        lock(&self.pending).record([self.root.join(relative)]);
    }

    fn rebuild(&mut self) -> Result<(), Error> {
        let files = rendered_files_in(&self.root)?;
        self.buckets.clear();
        for file in files {
            self.buckets
                .entry(file.bucket.clone())
                .or_default()
                .push(file);
        }
        self.digests = self
            .buckets
            .iter()
            .map(|(k, v)| (k.clone(), bucket_digest(v)))
            .collect();
        self.scanned = Instant::now();
        Ok(())
    }

    fn update(&mut self, paths: BTreeSet<PathBuf>) -> Result<(), Error> {
        let mut touched = BTreeSet::new();
        for path in paths {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                return self.rebuild();
            };
            if relative
                .components()
                .any(|p| p.as_os_str().to_string_lossy().starts_with('.'))
                || INTERNAL_DIRS.iter().any(|dir| relative.starts_with(dir))
            {
                continue;
            }
            let metadata = path.symlink_metadata().ok();
            let relative = relative.to_string_lossy();
            let prefix = format!("{relative}/");
            // Moving a directory can change an entire subtree with just one event.
            if relative.is_empty()
                || metadata.as_ref().is_some_and(std::fs::Metadata::is_dir)
                || (metadata.is_none()
                    && self.buckets.values().any(|files| {
                        let i = files.partition_point(|f| f.path < prefix);
                        files.get(i).is_some_and(|f| f.path.starts_with(&prefix))
                    }))
            {
                return self.rebuild();
            }
            if !is_rendered(&path) {
                continue;
            }
            let bucket = file_bucket(&relative);
            let files = self.buckets.entry(bucket.clone()).or_default();
            let at = files.binary_search_by(|f| f.path.as_str().cmp(&relative));
            if let Some(metadata) = metadata {
                let file = RenderedFile {
                    path: relative.into_owned(),
                    size: metadata.len(),
                    mtime: mtime(&metadata),
                    bucket: bucket.clone(),
                };
                match at {
                    Ok(i) => files[i] = file,
                    Err(i) => files.insert(i, file),
                }
            } else if let Ok(i) = at {
                files.remove(i);
            }
            touched.insert(bucket);
        }
        for bucket in touched {
            let files = &self.buckets[&bucket];
            if files.is_empty() {
                self.digests.remove(&bucket);
                self.buckets.remove(&bucket);
            } else {
                self.digests.insert(bucket, bucket_digest(files));
            }
        }
        Ok(())
    }

    pub(super) fn changed(
        &mut self,
        request: &ChangedFilesRequest,
    ) -> Result<ChangedFilesResponse, Error> {
        let pending = std::mem::take(&mut *lock(&self.pending));
        if pending.rescan || self.scanned.elapsed() >= RECONCILE {
            self.rebuild()
        } else {
            self.update(pending.paths)
        }
        .inspect_err(|_| lock(&self.pending).rescan = true)?;
        let files = self
            .digests
            .iter()
            .filter(|(key, value)| request.buckets.get(*key) != Some(*value))
            .flat_map(|(key, _)| self.buckets[key].iter().cloned())
            .collect();
        Ok(ChangedFilesResponse {
            buckets: self.digests.clone(),
            files,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_and_reconciled_manifests_match_a_walk() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one.html"), "one").unwrap();
        std::fs::create_dir(root.path().join("notes")).unwrap();
        std::fs::write(root.path().join("notes/two.svg"), "two").unwrap();
        let mut manifest = Manifest::watch(root.path()).unwrap().unwrap();
        let first = manifest.changed(&ChangedFilesRequest::default()).unwrap();
        assert_eq!(first.files.len(), 2);
        let request = ChangedFilesRequest {
            buckets: first.buckets,
        };
        assert!(manifest.changed(&request).unwrap().files.is_empty());
        std::fs::write(root.path().join("one.html"), "edited").unwrap();
        std::fs::remove_file(root.path().join("notes/two.svg")).unwrap();
        manifest.mark(Path::new("one.html"));
        manifest.mark(Path::new("notes/two.svg"));
        let changed = manifest.changed(&request).unwrap();
        assert_eq!(changed.files, rendered_files_in(root.path()).unwrap());
        std::fs::write(root.path().join("notes/new.html"), "added").unwrap();
        manifest.scanned -= RECONCILE;
        let reconciled = manifest.changed(&ChangedFilesRequest::default()).unwrap();
        let mut files = reconciled.files;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(files, rendered_files_in(root.path()).unwrap());
    }
}
