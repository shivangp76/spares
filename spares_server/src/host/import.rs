//! `spares host import`: moves a collection kept the way spares kept one before hosts (a data
//! directory, a config directory and rendered files, wherever `SPARES_DATA_DIR`,
//! `SPARES_CONFIG_DIR`, `SPARES_CACHE_DIR`, `SPARES_FILES_DIR` and the parsers' `*_OUT_DIR` put
//! them) into a user's directory on a host.
//!
//! | Before | After, in `users/<id>/` |
//! |---|---|
//! | `<data>/spares-main.sqlite` | `collection.sqlite` |
//! | `<data>/notes-spares` | `notes.git` |
//! | `<data>/images`, `audio`, `image_occlusions` | `media/…` |
//! | `<data>/notes`, `cards`, `render_assets`, `id-map.tsv`, … | the same, at the top |
//! | `<config>/config.toml`, `<config>/<parser>/` | `config/…` |
//! | pandoc's user data directory | `config/pandoc` (copied: pandoc's own) |
//! | `<cache>/note-hashes` | `cache/note-hashes` |
//! | `<cache>/image_occlusions`, rendered files in the `*_OUT_DIR`s | `files/…` |
//!
//! Notes name images, audio and image occlusions by their absolute path, so their text is
//! rewritten to the new paths, as an ordinary change that syncs. So are `render_asset`'s paths. The
//! user's clone of the notes repository (`notes`) is pointed at `notes.git`.
//!
//! Other files are left where they are, and listed. With `copy`, nothing is moved: the old
//! directories stay as they were, so the import can be tried first.

use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use spares_core::Error;
use spares_core::collection::CollectionPaths;
use spares_core::config::Replica;
use spares_core::ids::ID_MAP_FILE_NAME;
use spares_core::parsers::get_all_parsers;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

use super::Host;
use super::accounts::invalid;
use super::accounts::sqlx_error;

/// The data directory's file name of the database, before hosts.
pub const OLD_DATABASE_FILE_NAME: &str = "spares-main.sqlite";
/// The notes repository's name in the data directory, before hosts.
pub const OLD_NOTES_REPO_DIR_NAME: &str = "notes-spares";

/// Entries of the data directory that move to the top of the user's directory.
const DATA_ENTRIES: [&str; 6] = [
    "notes",
    "cards",
    "render_assets",
    ID_MAP_FILE_NAME,
    ".notes-spares-init",
    ".gitignore",
];
/// Entries of the data directory that move to `media/`.
const MEDIA_ENTRIES: [&str; 3] = ["images", "audio", "image_occlusions"];

#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// The old data directory (`SPARES_DATA_DIR`, by default `~/.local/share/spares`)
    pub data_dir: PathBuf,
    /// The database, if not `spares-main.sqlite` in the data directory (`DATABASE_URL`)
    pub database: Option<PathBuf>,
    /// The old config directory (`SPARES_CONFIG_DIR`, by default `~/.config/spares`)
    pub config_dir: Option<PathBuf>,
    /// The old cache directory (`SPARES_CACHE_DIR`, by default `~/.cache/spares`), which was also
    /// the files directory unless `SPARES_FILES_DIR` said otherwise
    pub cache_dir: Option<PathBuf>,
    /// Directories of rendered files: `SPARES_FILES_DIR` and the parsers' `*_OUT_DIR`s
    pub rendered_dirs: Vec<PathBuf>,
    /// Pandoc's user data directory, whose defaults files notes are rendered with
    pub pandoc_dir: Option<PathBuf>,
    /// This host's replica for the collection: what `SPARES_REPLICA` was. `None` for `1` without
    /// an upstream.
    pub replica: Option<Replica>,
    /// Copy instead of moving, leaving the old directories as they were
    pub copy: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    /// What went where
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Directories whose entries went into one that was there already, and how many
    pub merged: Vec<(PathBuf, PathBuf, usize)>,
    /// What was already there and kept
    pub skipped: Vec<PathBuf>,
    /// Entries of the old directories that weren't moved
    pub left: Vec<PathBuf>,
    pub notes_rewritten: u64,
    pub replica: Option<u8>,
}

/// Imports into user `user_id`'s directory, whose collection must be empty. The host must not be
/// running (the CLI checks with [`super::lock::HostLock`]).
pub async fn import(
    host: &Host,
    user_id: &str,
    options: &ImportOptions,
) -> Result<ImportReport, Error> {
    let user = super::accounts::user(&host.accounts, user_id)
        .await?
        .ok_or_else(|| invalid(format!("no account has id {user_id}")))?;
    let paths = host.paths(&user.id);
    let old_database = options
        .database
        .clone()
        .unwrap_or_else(|| options.data_dir.join(OLD_DATABASE_FILE_NAME));
    if !old_database.is_file() {
        return Err(invalid(format!(
            "there is no database at {}",
            old_database.display()
        )));
    }
    ensure_empty_collection(host, &user.id, &paths).await?;
    let mut report = ImportReport::default();

    // The database first: if it fails, nothing has moved.
    copy_database(&old_database, &paths.database()).await?;
    report.moved.push((old_database.clone(), paths.database()));

    let mover = Mover {
        copy: options.copy,
        follow_links: false,
    };
    let data = &options.data_dir;
    let old_repo = data.join(OLD_NOTES_REPO_DIR_NAME);
    mover.entry(&old_repo, &paths.notes_repo, &mut report)?;
    for name in MEDIA_ENTRIES {
        mover.entry(&data.join(name), &paths.media.join(name), &mut report)?;
    }
    for name in DATA_ENTRIES {
        mover.entry(&data.join(name), &paths.root.join(name), &mut report)?;
    }
    for entry in list(data) {
        let name = entry
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // Backups of the database, and the notes repository's from before note ids
        if name.starts_with("spares-main.before-") || name.starts_with("notes-spares.before-") {
            mover.entry(&entry, &paths.root.join("backups").join(&name), &mut report)?;
        } else if name.starts_with("notes.pre-vcs-") {
            mover.entry(&entry, &paths.root.join(&name), &mut report)?;
        }
    }

    if let Some(config_dir) = &options.config_dir {
        mover.entry(
            &config_dir.join("config.toml"),
            &paths.config_file(),
            &mut report,
        )?;
        for parser in get_all_parsers() {
            let name = parser().get_parser_name().to_string();
            mover.entry(
                &config_dir.join(&name),
                &paths.config.join(&name),
                &mut report,
            )?;
        }
    }
    if let Some(pandoc) = &options.pandoc_dir {
        // Its own files, even those it links to elsewhere, so the collection's directory is whole
        let copier = Mover {
            copy: true,
            follow_links: true,
        };
        copier.entry(pandoc, &paths.config.join("pandoc"), &mut report)?;
    }
    if let Some(cache) = &options.cache_dir {
        mover.entry(
            &cache.join("note-hashes"),
            &paths.cache.join("note-hashes"),
            &mut report,
        )?;
        mover.entry(
            &cache.join("image_occlusions"),
            &paths.files.join("image_occlusions"),
            &mut report,
        )?;
    }
    let mut rendered: Vec<&PathBuf> = options.rendered_dirs.iter().collect();
    rendered.sort();
    rendered.dedup();
    for dir in rendered {
        if dir.is_dir() {
            mover.merge_into(dir, &paths.files, &mut report)?;
        }
    }

    let replica = options.replica.unwrap_or(Replica::ROOT);
    set_replica(&paths, replica)?;
    report.replica = Some(replica.number());
    report.notes_rewritten = rewrite_paths(&paths, options).await?;
    point_clone_at(&paths.root.join("notes"), &old_repo, &paths.notes_repo);
    point_hooks_at(&paths.notes_repo);

    for dir in [
        Some(data),
        options.config_dir.as_ref(),
        options.cache_dir.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        for entry in list(dir) {
            if !report.moved.iter().any(|(from, _)| from == &entry)
                && !report.merged.iter().any(|(from, _, _)| from == &entry)
                && !report.skipped.contains(&entry)
                && entry != old_database
                && !entry
                    .to_string_lossy()
                    .starts_with(&*old_database.to_string_lossy())
            {
                report.left.push(entry);
            }
        }
    }
    if !options.copy {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", old_database.display()));
        }
    }
    // Opens it, which checks the import.
    host.close(&user.id).await;
    host.collection(&user.id).await?;
    host.close(&user.id).await;
    Ok(report)
}

/// The account's collection may only be the empty one it got when it was created, which the
/// import replaces.
async fn ensure_empty_collection(
    host: &Host,
    user_id: &str,
    paths: &CollectionPaths,
) -> Result<(), Error> {
    if paths.database().exists() {
        let collection = host.collection(user_id).await?;
        let notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note")
            .fetch_one(&collection.db)
            .await
            .map_err(sqlx_error)?;
        drop(collection);
        host.close(user_id).await;
        if notes > 0 {
            return Err(invalid(format!(
                "{} already has a collection with {notes} notes. Import into a new account.",
                paths.root.display()
            )));
        }
    }
    // Its database, notes repository and default config.toml go: the import's replace them.
    if paths.root.exists() {
        std::fs::remove_dir_all(&paths.root).map_err(|e| io(&paths.root, e))?;
    }
    paths.create().map_err(|e| io(&paths.root, e))?;
    Ok(())
}

/// A consistent copy of a database that may be in use, with its WAL.
async fn copy_database(from: &Path, to: &Path) -> Result<(), Error> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io {
            description: format!("Failed to create {}", parent.display()),
            source: e,
        })?;
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", from.display()))
        .map_err(sqlx_error)?
        .read_only(true);
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(sqlx_error)?;
    sqlx::query("VACUUM INTO ?")
        .bind(to.to_string_lossy())
        .execute(&db)
        .await
        .map_err(|e| Error::Sync(format!("Failed to copy {}: {e}", from.display())))?;
    db.close().await;
    Ok(())
}

fn list(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.retain(|path| path.file_name().is_none_or(|name| name != ".DS_Store"));
    entries.sort();
    entries
}

struct Mover {
    copy: bool,
    /// Copy what symbolic links point to, rather than the links
    follow_links: bool,
}

impl Mover {
    /// Moves (or copies) `from` to `to`, which must not exist: a file or directory, merged into
    /// `to` if that is a directory already.
    fn entry(&self, from: &Path, to: &Path, report: &mut ImportReport) -> Result<(), Error> {
        if !from.exists() {
            return Ok(());
        }
        if to.is_dir() && from.is_dir() {
            return self.merge_into(from, to, report);
        }
        if to.exists() {
            report.skipped.push(from.to_path_buf());
            return Ok(());
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        if self.copy {
            copy_recursively(from, to, self.follow_links)?;
        } else if std::fs::rename(from, to).is_err() {
            // Another file system
            copy_recursively(from, to, self.follow_links)?;
            remove(from)?;
        }
        report.moved.push((from.to_path_buf(), to.to_path_buf()));
        Ok(())
    }

    /// Moves every entry of `from` into `to`.
    fn merge_into(&self, from: &Path, to: &Path, report: &mut ImportReport) -> Result<(), Error> {
        std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
        let mut inner = ImportReport::default();
        for entry in list(from) {
            let name = entry.file_name().unwrap_or_default().to_os_string();
            self.entry(&entry, &to.join(name), &mut inner)?;
        }
        let count = inner.moved.len()
            + inner
                .merged
                .iter()
                .map(|(_, _, count)| count)
                .sum::<usize>();
        report
            .merged
            .push((from.to_path_buf(), to.to_path_buf(), count));
        report.skipped.extend(inner.skipped);
        if !self.copy {
            // Empty now, unless something was skipped
            let _ = std::fs::remove_dir(from);
        }
        Ok(())
    }
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        description: path.display().to_string(),
        source,
    }
}

fn copy_recursively(from: &Path, to: &Path, follow_links: bool) -> Result<(), Error> {
    let metadata = if follow_links {
        std::fs::metadata(from)
    } else {
        std::fs::symlink_metadata(from)
    }
    .map_err(|e| io(from, e))?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(from).map_err(|e| io(from, e))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, to).map_err(|e| io(to, e))?;
        #[cfg(not(unix))]
        let _ = target;
    } else if metadata.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
        for entry in std::fs::read_dir(from).map_err(|e| io(from, e))?.flatten() {
            copy_recursively(&entry.path(), &to.join(entry.file_name()), follow_links)?;
        }
    } else {
        std::fs::copy(from, to).map_err(|e| io(from, e))?;
    }
    Ok(())
}

fn remove(path: &Path) -> Result<(), Error> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|e| io(path, e))
}

/// Writes `[sync] replica` into the collection's config.toml.
pub fn set_replica(paths: &CollectionPaths, replica: Replica) -> Result<(), Error> {
    let path = paths.config_file();
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    let contents = spares_core::config::set_sync_values(
        &contents,
        [("replica", i64::from(replica.number()).into())],
    )?;
    std::fs::create_dir_all(&paths.config).map_err(|e| io(&paths.config, e))?;
    std::fs::write(&path, contents).map_err(|e| io(&path, e))
}

/// Rewrites the old directories' paths in notes, and in the render assets' paths.
async fn rewrite_paths(paths: &CollectionPaths, options: &ImportOptions) -> Result<u64, Error> {
    let mut prefixes: Vec<(String, String)> = Vec::new();
    let mut add = |old: PathBuf, new: PathBuf| {
        let mut old = old.to_string_lossy().into_owned();
        let mut new = new.to_string_lossy().into_owned();
        if old != new {
            old.push('/');
            new.push('/');
            prefixes.push((old, new));
        }
    };
    for name in MEDIA_ENTRIES {
        add(options.data_dir.join(name), paths.media.join(name));
    }
    if let Some(config) = &options.config_dir {
        add(config.clone(), paths.config.clone());
    }
    // Last, since the media directories are in it
    add(options.data_dir.clone(), paths.root.clone());
    let options = SqliteConnectOptions::from_str(&paths.database_url()).map_err(sqlx_error)?;
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(sqlx_error)?;
    let mut rewritten = 0;
    for (old, new) in &prefixes {
        // Only the media directories in notes: other paths in them are the user's own files.
        if new.starts_with(&*paths.media.to_string_lossy()) {
            rewritten += sqlx::query(
                "UPDATE note SET data = replace(data, ?1, ?2) WHERE instr(data, ?1) > 0",
            )
            .bind(old)
            .bind(new)
            .execute(&db)
            .await
            .map_err(sqlx_error)?
            .rows_affected();
        }
        sqlx::query(
            "UPDATE OR IGNORE render_asset SET path = ?2 || substr(path, length(?1) + 1)
             WHERE substr(path, 1, length(?1)) = ?1",
        )
        .bind(old)
        .bind(new)
        .execute(&db)
        .await
        .map_err(sqlx_error)?;
    }
    db.close().await;
    Ok(rewritten)
}

/// Points the notes repository's `core.hooksPath` at its own hooks, wherever it was moved from.
/// The hooks themselves name the host they report to, and are rewritten by `spares vcs init
/// --repair-hooks`.
fn point_hooks_at(repo: &Path) {
    let hooks = repo.join(".git").join("hooks");
    if hooks.is_dir() {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["config", "--local", "core.hooksPath"])
            .arg(&hooks)
            .output();
    }
}

/// Points the user's clone of the notes repository at its new place, if it was a clone of the old
/// one.
fn point_clone_at(clone: &Path, old_repo: &Path, new_repo: &Path) {
    if !clone.join(".git").exists() {
        return;
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(clone)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let Some(origin) = git(&["remote", "get-url", "origin"]) else {
        return;
    };
    if Path::new(origin.trim_end_matches('/')) == old_repo {
        let _ = git(&["remote", "set-url", "origin", &new_repo.to_string_lossy()]);
    }
}
