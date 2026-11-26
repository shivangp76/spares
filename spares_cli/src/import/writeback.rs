//! Writing back the files an import rewrote (its [`ImportOutcome::rewritten`]), without losing
//! what the user or another import wrote meanwhile.
//!
//! - **Other imports.** An import locks each of its files (an advisory lock on the file, see
//!   [`SourceLocks`]) before reading it, and holds the locks until its rewrites are written back.
//!   A second `spares import` of one of them waits, then reads the file as the first one left it.
//!   Two imports of overlapping files lock them in the same order, so neither waits on the other
//!   forever.
//! - **Editors.** An editor doesn't take the lock, so each file's [`Fingerprint`] is taken as it is
//!   read, and the file is compared with it before it is replaced. If it changed, it is left as it
//!   is and the rewrite is written next to it (see [`WriteBack::Diverged`]).
//! - **The replacement.** The rewrite goes to a temporary file created for it alone in the file's
//!   directory (never one that is there already), with the file's permissions, and is synced to
//!   disk before it is renamed over the file, so the file is always either the old one or the new
//!   one, whole.
//! - **Interruptions.** Nothing is written back before the import has changed the database, and
//!   rewriting is idempotent, so importing the file again after an interrupted import writes the
//!   rewrite then. A temporary file a killed import left behind is removed by the next import of
//!   the file, which holds the lock that every writer of such files holds.
//!
//! [`ImportOutcome::rewritten`]: spares_core::api::note::import::ImportOutcome::rewritten

use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError;
use std::io::ErrorKind;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use spares_core::Error;

/// How often a file locked by another import is tried again
const LOCK_POLL: Duration = Duration::from_millis(100);

/// What identifies a file on disk, whatever its path: its device and inode on Unix
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct FileId(u64, u64);

impl FileId {
    #[cfg(unix)]
    fn of(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self(metadata.dev(), metadata.ino())
    }

    /// Elsewhere, files are told apart by path, as [`SourceLocks::lock`] gives them
    #[cfg(not(unix))]
    fn of(_metadata: &std::fs::Metadata) -> Self {
        Self(0, 0)
    }
}

fn io_error(description: String) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io {
        description,
        source,
    }
}

/// Advisory locks on the files of an import, held until it is dropped. A file given twice (by
/// two paths, say) is locked once.
pub(crate) struct SourceLocks {
    /// The locked files, by path as given, each the open file holding its lock
    files: HashMap<PathBuf, File>,
}

impl SourceLocks {
    /// Locks `paths`, waiting while another import holds one of them, and saying so unless
    /// `quiet`. They are locked in the order of their [`FileId`]s, so two imports of overlapping
    /// files take their common locks in the same order.
    pub(crate) async fn lock(paths: &[&Path], quiet: bool) -> Result<Self, Error> {
        let mut by_id: Vec<(FileId, PathBuf)> = Vec::with_capacity(paths.len());
        for (index, path) in paths.iter().enumerate() {
            let metadata = std::fs::metadata(path)
                .map_err(io_error(format!("Failed to read {}", path.display())))?;
            let id = if cfg!(unix) {
                FileId::of(&metadata)
            } else {
                FileId(0, index as u64)
            };
            by_id.push((id, path.to_path_buf()));
        }
        by_id.sort();
        let mut files: HashMap<PathBuf, File> = HashMap::new();
        let mut locked: HashMap<FileId, PathBuf> = HashMap::new();
        for (id, path) in by_id {
            if let Some(first) = locked.get(&id) {
                let file = files[first]
                    .try_clone()
                    .map_err(io_error(format!("Failed to open {} again", path.display())))?;
                files.insert(path, file);
                continue;
            }
            let file = lock_file(&path, quiet).await?;
            locked.insert(id, path.clone());
            files.insert(path, file);
        }
        Ok(Self { files })
    }

    /// Whether `path` is one of the locked files
    fn holds(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }
}

/// Opens and locks the file at `path`. If another import replaced it while this one waited for
/// the lock, the lock is on the file it replaced, so the new file is locked instead.
async fn lock_file(path: &Path, quiet: bool) -> Result<File, Error> {
    let mut said = false;
    loop {
        let file =
            File::open(path).map_err(io_error(format!("Failed to open {}", path.display())))?;
        // On Windows a lock keeps other handles from reading the file, this import's own too
        if !cfg!(unix) {
            return Ok(file);
        }
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => {
                    if !quiet && !said {
                        eprintln!(
                            "Waiting for another spares import of {} to finish...",
                            path.display()
                        );
                        said = true;
                    }
                    tokio::time::sleep(LOCK_POLL).await;
                }
                Err(TryLockError::Error(e)) => {
                    return Err(io_error(format!("Failed to lock {}", path.display()))(e));
                }
            }
        }
        let locked = file
            .metadata()
            .map_err(io_error(format!("Failed to read {}", path.display())))?;
        let current = std::fs::metadata(path)
            .map_err(io_error(format!("Failed to read {}", path.display())))?;
        if FileId::of(&locked) == FileId::of(&current) {
            return Ok(file);
        }
    }
}

/// A file as an import read it, to tell whether it changed before its rewrite is written back
pub(crate) struct Fingerprint {
    contents: Vec<u8>,
}

impl Fingerprint {
    pub(crate) fn new(contents: Vec<u8>) -> Self {
        Self { contents }
    }

    pub(crate) fn contents(&self) -> &[u8] {
        &self.contents
    }
}

/// What [`write_back`] did with a rewrite
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WriteBack {
    /// The file was replaced by the rewrite
    Written,
    /// The file already was the rewrite, such as after another import of it
    Unchanged,
    /// The file changed (or was removed) after the import read it, so it was left as it is now,
    /// and the rewrite was written to `kept`, a new file next to it
    Diverged { kept: PathBuf },
}

/// Replaces the file at `path`, which the import read as `read`, with `contents`, unless it
/// changed since (see [`WriteBack`]). `locks` has to hold `path`: under its lock, no other import
/// writes the file or its temporary files.
pub(crate) fn write_back(
    locks: &SourceLocks,
    path: &Path,
    read: &Fingerprint,
    contents: &[u8],
) -> Result<WriteBack, Error> {
    assert!(locks.holds(path), "{} is not locked", path.display());
    // A link is followed, so that the file it names is replaced rather than the link. Other
    // paths stay as given, for the paths reported.
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let target = if is_link {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    let (dir, name) = split(&target)?;
    remove_stale_temporary_files(dir, name);

    let current = match std::fs::read(&target) {
        Ok(current) => Some(current),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(io_error(format!("Failed to read {}", target.display()))(e)),
    };
    match current {
        Some(current) if current == contents => return Ok(WriteBack::Unchanged),
        Some(current) if current == read.contents() => {}
        _ => return keep_beside(&target, contents),
    }
    let permissions = std::fs::metadata(&target)
        .map_err(io_error(format!("Failed to read {}", target.display())))?
        .permissions();

    let (temporary, mut file) = create_new_beside(dir, &format!(".{name}.spares-import"), "tmp")?;
    let written = (|| {
        file.write_all(contents)?;
        file.set_permissions(permissions)?;
        file.sync_all()
    })();
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(io_error(format!("Failed to write {}", temporary.display()))(e));
    }
    // An editor may have saved the file while the rewrite was being written
    if std::fs::read(&target).ok().as_deref() != Some(read.contents()) {
        let _ = std::fs::remove_file(&temporary);
        return keep_beside(&target, contents);
    }
    if let Err(e) = std::fs::rename(&temporary, &target) {
        let _ = std::fs::remove_file(&temporary);
        return Err(io_error(format!(
            "Failed to rename {} to {}",
            temporary.display(),
            target.display()
        ))(e));
    }
    // The rename itself is on disk once the directory is
    #[cfg(unix)]
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(WriteBack::Written)
}

/// The directory of `path` and its file name
fn split(path: &Path) -> Result<(&Path, &str), Error> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        io_error(format!("Can't write back {}", path.display()))(std::io::Error::new(
            ErrorKind::InvalidInput,
            "not a file name",
        ))
    })?;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok((dir, name))
}

/// Writes `contents` to a new file next to `path`, named after it, for [`WriteBack::Diverged`]
fn keep_beside(path: &Path, contents: &[u8]) -> Result<WriteBack, Error> {
    let (dir, name) = split(path)?;
    let (kept, mut file) = create_new_beside(dir, name, "spares-import")?;
    let permissions = std::fs::metadata(path).map(|m| m.permissions());
    let written = (|| {
        file.write_all(contents)?;
        if let Ok(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&kept);
        return Err(io_error(format!("Failed to write {}", kept.display()))(e));
    }
    Ok(WriteBack::Diverged { kept })
}

/// Creates a file in `dir` named `{stem}.{extension}`, or `{stem}.{n}.{extension}` for the first
/// `n` that is free: never one that is there already, nor one that another process creates at the
/// same time.
fn create_new_beside(dir: &Path, stem: &str, extension: &str) -> Result<(PathBuf, File), Error> {
    let pid = std::process::id();
    for n in 0..1000 {
        let name = if extension == "tmp" {
            format!("{stem}-{pid}-{n}.{extension}")
        } else if n == 0 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem}.{n}.{extension}")
        };
        let path = dir.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_error(format!("Failed to create {}", path.display()))(e)),
        }
    }
    Err(io_error(format!(
        "Failed to create a file next to {}",
        dir.join(stem).display()
    ))(std::io::Error::new(
        ErrorKind::AlreadyExists,
        "too many files with its name",
    )))
}

/// Removes the temporary files an earlier import of `name` left in `dir` when it was killed. Only
/// an import holding the file's lock writes them, and this one holds it, so none is in use.
fn remove_stale_temporary_files(dir: &Path, name: &str) {
    let prefix = format!(".{name}.spares-import-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let ours = file_name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .is_some_and(|middle| {
                let mut parts = middle.split('-');
                parts.next().is_some_and(|p| p.parse::<u32>().is_ok())
                    && parts.next().is_some_and(|n| n.parse::<u32>().is_ok())
                    && parts.next().is_none()
            });
        if ours {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new empty directory for a test, removed when dropped
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "spares-writeback-{name}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }

        /// The names of the files in the directory, sorted
        fn names(&self) -> Vec<String> {
            let mut names = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect::<Vec<_>>();
            names.sort();
            names
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn contents_of(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// Reads `path` under its lock, as an import does
    async fn start_import(path: &Path) -> (SourceLocks, Fingerprint) {
        let locks = SourceLocks::lock(&[path], true).await.unwrap();
        let read = Fingerprint::new(std::fs::read(path).unwrap());
        (locks, read)
    }

    /// Files that only look like the temporary file (`notes.tmp` and `notes.md.tmp`, which the
    /// rewrite was once written to) are left alone, and no file is left over.
    #[tokio::test]
    async fn a_rewrite_goes_through_a_temporary_file_of_its_own() {
        let dir = TestDir::new("own-temporary");
        let path = dir.file("notes.md", "old");
        for name in [
            "notes.tmp",
            "notes.md.tmp",
            ".notes.md.spares-import.tmp",
            ".notes.md.spares-import-mine.tmp",
        ] {
            dir.file(name, "the user's");
        }
        let before = dir.names();
        let (locks, read) = start_import(&path).await;
        assert_eq!(
            write_back(&locks, &path, &read, b"new").unwrap(),
            WriteBack::Written
        );
        assert_eq!(contents_of(&path), "new");
        assert_eq!(dir.names(), before);
        for name in before.iter().filter(|name| *name != "notes.md") {
            assert_eq!(contents_of(&dir.0.join(name)), "the user's", "{name}");
        }
        // Again: the file already is the rewrite
        let read = Fingerprint::new(b"new".to_vec());
        assert_eq!(
            write_back(&locks, &path, &read, b"new").unwrap(),
            WriteBack::Unchanged
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_rewrite_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("permissions");
        for mode in [0o640, 0o600, 0o755] {
            let path = dir.file(&format!("{mode:o}.md"), "old");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let (locks, read) = start_import(&path).await;
            write_back(&locks, &path, &read, b"new").unwrap();
            let written = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(written, mode, "{mode:o}");
        }
    }

    /// The file is edited while it is imported: the edit stays, and the rewrite is kept next to
    /// it, under a name of its own each time.
    #[tokio::test]
    async fn an_edit_made_during_the_import_is_kept() {
        let dir = TestDir::new("edited");
        let path = dir.file("notes.md", "old");
        let (locks, read) = start_import(&path).await;
        std::fs::write(&path, "edited").unwrap();
        let WriteBack::Diverged { kept } = write_back(&locks, &path, &read, b"new").unwrap() else {
            panic!("the edit was overwritten");
        };
        assert_eq!(contents_of(&path), "edited");
        assert_eq!(kept, dir.0.join("notes.md.spares-import"));
        assert_eq!(contents_of(&kept), "new");

        let WriteBack::Diverged { kept } = write_back(&locks, &path, &read, b"newer").unwrap()
        else {
            panic!("the edit was overwritten");
        };
        assert_eq!(kept, dir.0.join("notes.md.1.spares-import"));
        assert_eq!(contents_of(&kept), "newer");
        assert_eq!(contents_of(&dir.0.join("notes.md.spares-import")), "new");
        assert_eq!(contents_of(&path), "edited");
    }

    /// The file is removed while it is imported: it isn't created again.
    #[tokio::test]
    async fn a_file_removed_during_the_import_stays_removed() {
        let dir = TestDir::new("removed");
        let path = dir.file("notes.md", "old");
        let (locks, read) = start_import(&path).await;
        std::fs::remove_file(&path).unwrap();
        let written = write_back(&locks, &path, &read, b"new").unwrap();
        assert!(matches!(written, WriteBack::Diverged { .. }), "{written:?}");
        assert!(!path.exists());
        assert_eq!(contents_of(&dir.0.join("notes.md.spares-import")), "new");
    }

    /// An import killed while it wrote its temporary file left it behind: the next import of the
    /// file removes it. Another file's are left alone.
    #[tokio::test]
    async fn a_temporary_file_left_by_a_killed_import_is_removed() {
        let dir = TestDir::new("killed");
        let path = dir.file("notes.md", "old");
        dir.file(".notes.md.spares-import-99999-0.tmp", "half a rewri");
        dir.file(".other.md.spares-import-99999-0.tmp", "another file's");
        let (locks, read) = start_import(&path).await;
        write_back(&locks, &path, &read, b"new").unwrap();
        assert_eq!(contents_of(&path), "new");
        assert_eq!(
            dir.names(),
            [".other.md.spares-import-99999-0.tmp", "notes.md"]
        );
    }

    /// A write that fails leaves the file as it was, and no temporary file.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_write_leaves_the_file_whole() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("failed");
        let path = dir.file("notes.md", "old");
        let (locks, read) = start_import(&path).await;
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(write_back(&locks, &path, &read, b"new").is_err());
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(contents_of(&path), "old");
        assert_eq!(dir.names(), ["notes.md"]);
        // The next attempt writes it
        assert_eq!(
            write_back(&locks, &path, &read, b"new").unwrap(),
            WriteBack::Written
        );
    }

    /// A link to the file is kept, and the file it names is rewritten.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_is_followed() {
        let dir = TestDir::new("link");
        let target = dir.file("notes.md", "old");
        let link = dir.0.join("link.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (locks, read) = start_import(&link).await;
        write_back(&locks, &link, &read, b"new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(contents_of(&target), "new");
    }

    /// A second import of a file waits for the first, and then locks the file the first one
    /// wrote, not the one it replaced.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn imports_of_one_file_wait_for_each_other() {
        let dir = TestDir::new("wait");
        let path = dir.file("notes.md", "old");
        let (first, read) = start_import(&path).await;
        let second = tokio::spawn({
            let path = path.clone();
            async move { SourceLocks::lock(&[path.as_path()], true).await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!second.is_finished(), "the second import didn't wait");
        write_back(&first, &path, &read, b"new").unwrap();
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(5), second)
            .await
            .expect("the second import is still waiting")
            .unwrap();
        let locked = FileId::of(&second.files[&path].metadata().unwrap());
        assert_eq!(locked, FileId::of(&std::fs::metadata(&path).unwrap()));
    }

    /// A file given twice, by the same path or another, is locked once rather than waiting for
    /// itself.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_file_given_twice_is_locked_once() {
        let dir = TestDir::new("twice");
        let path = dir.file("notes.md", "old");
        let link = dir.0.join("link.md");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let locks = tokio::time::timeout(
            Duration::from_secs(5),
            SourceLocks::lock(&[&path, &link, &path], true),
        )
        .await
        .expect("waited for itself")
        .unwrap();
        assert!(locks.holds(&path) && locks.holds(&link));
    }
}
