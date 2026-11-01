//! `spares host import`: moves a collection kept the way spares kept one before hosts (a data
//! directory, a config directory and rendered files, wherever `SPARES_DATA_DIR`,
//! `SPARES_CONFIG_DIR`, `SPARES_CACHE_DIR`, `SPARES_FILES_DIR` and the parsers' `*_OUT_DIR` put
//! them) into a user's directory on a host.
//!
//! | Before | After, in `users/<id>/` | in the user's cache (see [`super::cache_root`]) |
//! |---|---|---|
//! | `<data>/spares-main.sqlite` | `collection.sqlite` | |
//! | `<data>/notes-spares` | `notes.git` | |
//! | `<data>/images`, `audio`, `image_occlusions` | `media/…` | |
//! | `<data>/notes`, `render_assets`, `id-map.tsv`, … | the same, at the top | |
//! | `<config>/config.toml`, `<config>/<parser>/` | `config/…` | |
//! | pandoc's user data directory | `config/pandoc`, a link to it, which `[render] read_dirs` lets renders follow | |
//! | `<data>/cards` | | `cards` |
//! | `<cache>/note-hashes` | | `note-hashes` |
//! | `<cache>/image_occlusions`, rendered files in the `*_OUT_DIR`s | | `files/…` |
//!
//! The old cache directory may be the host's cache root (`~/.cache/spares`): its `users/` stays.
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
const DATA_ENTRIES: [&str; 5] = [
    "notes",
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
    /// What `config/pandoc` links to, and the directories added to `[render] read_dirs` for it
    pub pandoc: Option<(PathBuf, Vec<PathBuf>)>,
}

/// Imports into user `user_id`'s directory, whose collection must be empty. The host must not be
/// running (the CLI checks with [`super::lock::HostLock`]).
#[allow(clippy::too_many_lines, reason = "one step after another")]
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
        // The host's own users, when an old directory is its root or its cache root
        keep: vec![
            host.root.join(super::USERS_DIR_NAME),
            host.cache_root.join(super::USERS_DIR_NAME),
        ],
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
    mover.entry(&data.join("cards"), &paths.cards(), &mut report)?;
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
        let link = paths.config.join("pandoc");
        if link.exists() || link.is_symlink() {
            report.skipped.push(pandoc.clone());
        } else {
            let read_dirs = link_pandoc(pandoc, &link)?;
            add_read_dirs(&host.root, &read_dirs)?;
            report.pandoc = Some((pandoc.clone(), read_dirs));
        }
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
            // The host's own files, when the old data directory is its root
            let host_file = entry.parent() == Some(host.root.as_path())
                && entry.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name == super::USERS_DIR_NAME
                        || name == super::config::HOST_CONFIG_FILE_NAME
                        || name == super::lock::LOCK_FILE_NAME
                        || name.starts_with(super::accounts::ACCOUNTS_FILE_NAME)
                });
            if !host_file
                && !mover.keep.contains(&entry)
                && !report.moved.iter().any(|(from, _)| from == &entry)
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
pub(super) async fn ensure_empty_collection(
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
    // Its database, notes repository and default config.toml go: the import's replace them. So
    // does what its cache has, which may be another collection's.
    for dir in [&paths.root, &paths.cache] {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| io(dir, e))?;
        }
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
    /// What is never moved, even from a directory that is
    keep: Vec<PathBuf>,
}

impl Mover {
    /// Moves (or copies) `from` to `to`, which must not exist: a file or directory, merged into
    /// `to` if that is a directory already.
    fn entry(&self, from: &Path, to: &Path, report: &mut ImportReport) -> Result<(), Error> {
        if !from.exists() || self.keep.iter().any(|keep| keep == from) {
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
            copy_recursively(from, to)?;
        } else if std::fs::rename(from, to).is_err() {
            // Another file system
            copy_recursively(from, to)?;
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

fn copy_recursively(from: &Path, to: &Path) -> Result<(), Error> {
    let metadata = std::fs::symlink_metadata(from).map_err(|e| io(from, e))?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(from).map_err(|e| io(from, e))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, to).map_err(|e| io(to, e))?;
        #[cfg(not(unix))]
        let _ = target;
    } else if metadata.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
        for entry in std::fs::read_dir(from).map_err(|e| io(from, e))?.flatten() {
            copy_recursively(&entry.path(), &to.join(entry.file_name()))?;
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

/// Makes `link` a link to pandoc's user data directory `pandoc`, so renders use the same defaults
/// files and templates as pandoc does everywhere else, and returns the directories renders must
/// read to follow it: `pandoc` itself and where the links in it lead.
fn link_pandoc(pandoc: &Path, link: &Path) -> Result<Vec<PathBuf>, Error> {
    let real = |path: &Path| path.canonicalize().map_err(|e| io(path, e));
    let mut read_dirs = vec![real(pandoc)?];
    for entry in list(pandoc) {
        if entry.is_symlink()
            && let Ok(target) = entry.canonicalize()
        {
            let dir = if target.is_dir() {
                target
            } else {
                target.parent().map(Path::to_path_buf).unwrap_or(target)
            };
            if !read_dirs.iter().any(|known| dir.starts_with(known)) {
                read_dirs.push(dir);
            }
        }
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(pandoc, link).map_err(|e| io(link, e))?;
    #[cfg(not(unix))]
    return Err(invalid("linking pandoc's directory needs a Unix"));
    Ok(read_dirs)
}

/// Adds `dirs` to `[render] read_dirs` in the host's `host.toml`, keeping the rest as written.
pub fn add_read_dirs(root: &Path, dirs: &[PathBuf]) -> Result<(), Error> {
    let path = root.join(super::config::HOST_CONFIG_FILE_NAME);
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = contents
        .parse()
        .map_err(|e: toml_edit::TomlError| invalid(format!("{}: {e}", path.display())))?;
    let render = doc
        .entry("render")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| invalid("`render` in host.toml is not a table"))?;
    let read_dirs = render
        .entry("read_dirs")
        .or_insert(toml_edit::value(toml_edit::Array::new()))
        .as_array_mut()
        .ok_or_else(|| invalid("`render.read_dirs` in host.toml is not a list"))?;
    for dir in dirs {
        let dir = dir.to_string_lossy();
        if !read_dirs.iter().any(|known| known.as_str() == Some(&*dir)) {
            read_dirs.push(dir.as_ref());
        }
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| io(&path, e))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::tests::TempRoot;
    use crate::host::tests::temp_root;

    /// A VPS's old cache directory is its new cache root: rendered files move into the user's
    /// cache, and the cache root's `users/` stays.
    #[tokio::test]
    async fn an_import_from_the_cache_root_keeps_it() {
        let temp = TempRoot(temp_root());
        let root = temp.0.join("root");
        let cache = temp.0.join("cache");
        Host::create(&root, "127.0.0.1:0").await.unwrap();
        std::fs::write(
            root.join("host.toml"),
            format!("cache_dir = \"{}\"\n", cache.display()),
        )
        .unwrap();
        let host = Host::open(&root).await.unwrap();
        host.create_user(crate::host::accounts::NewUser {
            id: Some("me"),
            name: "me",
            email: None,
            password: "password1",
            admin: true,
            email_verified: true,
        })
        .await
        .unwrap();
        host.close_all().await;

        let data = temp.0.join("data");
        std::fs::create_dir_all(data.join("cards/typst")).unwrap();
        std::fs::write(data.join("cards/typst/a-1-front.typ"), "card").unwrap();
        let options = SqliteConnectOptions::from_str(&format!(
            "sqlite://{}",
            data.join(OLD_DATABASE_FILE_NAME).display()
        ))
        .unwrap()
        .create_if_missing(true);
        let db = SqlitePoolOptions::new()
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("../spares_core/migrations")
            .run(&db)
            .await
            .unwrap();
        db.close().await;
        std::fs::create_dir_all(cache.join("note-hashes")).unwrap();
        std::fs::write(cache.join("a.pdf"), "pdf").unwrap();

        import(
            &host,
            "me",
            &ImportOptions {
                data_dir: data.clone(),
                cache_dir: Some(cache.clone()),
                rendered_dirs: vec![cache.clone()],
                ..ImportOptions::default()
            },
        )
        .await
        .unwrap();
        host.close_all().await;
        let paths = host.paths("me");
        assert_eq!(paths.cache, cache.join("users/me"));
        assert!(paths.files.join("a.pdf").is_file());
        assert!(paths.cache.join("note-hashes").is_dir());
        assert!(paths.cards().join("typst/a-1-front.typ").is_file());
        assert!(!paths.files.join("users").exists());
        assert!(paths.database().is_file());
        assert!(!root.join("users/me/cards").exists());
    }
}
