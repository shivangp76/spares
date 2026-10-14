use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::hash::RandomState;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use spares_core::api::browser_render::read_render_dependencies;
use spares_core::config::get_cache_dir;
use spares_core::config::get_config_dir;
use spares_core::config::get_data_dir;
use spares_core::config::read_external_config;
use spares_core::parsers::Parseable;
use spares_core::parsers::RenderOutputDirectoryType;
use spares_core::parsers::get_all_parsers;
use spares_core::parsers::get_image_occlusion_rendered_directory;
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

const DATABASE_FILENAME: &str = "spares-main.sqlite";
/// The server's config directory, relative to the remote home directory. Matches `setup_vps.sh`.
const REMOTE_CONFIG_DIR: &str = ".config/spares";

/// Pushes the database and files to `remote_host`, replacing the server's database. Used when
/// two-way sync (`sync_url`) is not set up.
pub(crate) async fn push_to_remote_host() -> Result<(), String> {
    let config = read_external_config().map_err(|e| e.to_string())?;
    let remote_host = config
        .remote_host
        .ok_or("remote_host is not set in config.toml")?;

    // Pass 1: DB only
    println!("Syncing database → {}", remote_host);
    sync_database(&remote_host, &get_data_dir()).await?;

    push_files(&remote_host, &config.remote_rendered_dir)
}

/// Copies everything but the database to `remote_host`: the data files the server does not
/// generate itself, rendered files, templates and the files the notes read.
pub(crate) fn push_files(remote_host: &str, remote_rendered_dir: &str) -> Result<(), String> {
    let data_dir = get_data_dir();
    let local = data_dir.to_str().unwrap();

    // Pass 2: Everything else (cards, image occlusions, render assets). The server reads notes from
    // the database, so raw note files are not needed. Neither are the raw cards of parsers that
    // render in the browser, since they are generated from the database on demand.
    println!("Syncing files → {}", remote_host);
    let mut excludes = vec![
        // The database, its WAL/SHM/journal files and local backups. The database is synced above.
        "--exclude=*.sqlite*".to_string(),
        "--exclude=/notes/".to_string(),
        // The notes repository (see `spares note init-vcs`), what setting it up leaves behind, and
        // its interrupted setup
        format!(
            "--exclude=/{}/",
            spares_core::notes_vcs::SERVER_REPO_DIR_NAME
        ),
        "--exclude=/notes.pre-vcs-*/".to_string(),
        "--exclude=/.notes-spares-init/".to_string(),
        "--exclude=/.notes-clone-tmp/".to_string(),
    ];
    excludes.extend(
        get_all_parsers()
            .into_iter()
            .map(|parser| parser())
            .filter(|parser| parser.renders_in_browser())
            .map(|parser| format!("--exclude=/cards/{}/", parser.get_parser_name())),
    );
    let source = format!("{}/", local);
    let destination = format!("{}:{}/", remote_host, local);
    let mut args = vec!["-avz"];
    args.extend(excludes.iter().map(String::as_str));
    args.extend([source.as_str(), destination.as_str()]);
    run_rsync(&args)?;

    // Pass 3: Rendered files, so the server does not have to render notes again.
    println!("Syncing rendered files → {}", remote_host);
    sync_rendered_files(remote_host, remote_rendered_dir)?;

    // Pass 4: Templates. The remote keeps its own config.toml (e.g. with
    // `render_browser_parsers = false`).
    println!("Syncing templates → {}", remote_host);
    run_rsync(&[
        "-avz",
        "--exclude=/config.toml",
        &dir_source(&get_config_dir())?,
        &format!("{remote_host}:{REMOTE_CONFIG_DIR}/"),
    ])?;

    // Pass 5: Files the notes read, so the server can render notes edited there.
    println!("Syncing files read by notes → {}", remote_host);
    sync_render_dependencies(remote_host)?;

    println!("Done.");
    Ok(())
}

/// Parsers whose notes the server renders.
fn server_rendered_parsers() -> impl Iterator<Item = Box<dyn Parseable>> {
    get_all_parsers()
        .into_iter()
        .map(|parser| parser())
        .filter(|parser| !parser.renders_in_browser())
}

fn rendered_dirs() -> BTreeSet<PathBuf> {
    server_rendered_parsers()
        .flat_map(|parser| {
            [
                RenderOutputDirectoryType::Note,
                RenderOutputDirectoryType::Card,
            ]
            .map(|output_type| parser.get_output_rendered_dir(output_type))
        })
        .collect()
}

/// `dir` as an rsync source that copies its contents.
fn dir_source(dir: &Path) -> Result<String, String> {
    let dir = dir
        .to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", dir.display()))?;
    Ok(format!("{dir}/"))
}

/// Copies the rendered files of the parsers the server renders into `remote_rendered_dir`, along
/// with rendered image occlusions.
fn sync_rendered_files(remote_host: &str, remote_rendered_dir: &str) -> Result<(), String> {
    let destination = format!("{remote_host}:{remote_rendered_dir}/");
    for dir in rendered_dirs() {
        if !dir.is_dir() {
            continue;
        }
        // Rendered files are named `<note id>...`, so this skips `aux/` and unrelated files in
        // shared directories.
        run_rsync(&[
            "-avz",
            "--exclude=*.synctex.gz",
            "--include=/[0-9]*",
            "--exclude=*",
            &dir_source(&dir)?,
            &destination,
        ])?;
    }
    run_rsync(&[
        "-avz",
        &dir_source(&get_image_occlusion_rendered_directory())?,
        &format!("{destination}image_occlusions/"),
    ])
}

/// Copies the files that rendering recorded as read by the notes of parsers the server renders
/// (preambles, figures, pandoc defaults, ...) to the same paths relative to the remote home
/// directory, recreating the symlinks on the way to them. Absolute paths in notes also work if
/// the local home directory is a symlink to the remote one (see `setup_vps.sh --local-home`).
///
/// Only files read by a local render are known, so a file a note starts reading after being edited
/// on the server is not copied until the note is rendered locally.
fn sync_render_dependencies(remote_host: &str) -> Result<(), String> {
    let home = std::env::home_dir().ok_or("Failed to find the home directory")?;
    let plan = plan_dependency_sync(&home, render_dependency_files());
    for path in &plan.skipped {
        eprintln!(
            "Not syncing {}, since it is outside {}",
            path.display(),
            home.display()
        );
    }
    if plan.files.is_empty() {
        return Ok(());
    }
    let list = std::env::temp_dir().join(format!("spares-sync-files-{}", std::process::id()));
    let contents = plan
        .files
        .iter()
        .filter_map(|path| path.to_str())
        .collect::<Vec<_>>()
        .join("\n");
    let result = std::fs::write(&list, contents)
        .map_err(|e| format!("Failed to write {}: {e}", list.display()))
        .and_then(|()| {
            run_rsync(&[
                "-avzR",
                &format!("--files-from={}", list.display()),
                &dir_source(&home)?,
                &format!("{remote_host}:"),
            ])
        });
    let _ = std::fs::remove_file(&list);
    result?;
    if !plan.symlinks.is_empty() {
        run_ssh(remote_host, &symlinks_command(&plan.symlinks))
            .map_err(|e| format!("Creating symlinks failed: {e}"))?;
    }
    Ok(())
}

/// Existing files read by rendered notes of parsers the server renders, except those synced by
/// other passes or written by rendering.
fn render_dependency_files() -> BTreeSet<PathBuf> {
    let excluded_dirs: Vec<PathBuf> = [
        get_data_dir(),
        get_cache_dir(),
        get_image_occlusion_rendered_directory(),
    ]
    .into_iter()
    .chain(rendered_dirs())
    .collect();
    server_rendered_parsers()
        .flat_map(|parser| read_render_dependencies::<RandomState>(parser.as_ref(), None).0)
        .filter(|path| path.is_file() && !excluded_dirs.iter().any(|dir| path.starts_with(dir)))
        .collect()
}

#[derive(Debug, PartialEq)]
enum SymlinkTarget {
    /// Relative to the home directory.
    InHome(PathBuf),
    Other(PathBuf),
}

#[derive(Debug, Default)]
struct DependencySync {
    /// Paths relative to the home directory, without symlinks. rsync turns symlinks on the way to
    /// a file into directories, which would break files found relative to the symlink's target
    /// (e.g. pandoc's `${.}`), so the files are copied to their real paths instead.
    files: BTreeSet<PathBuf>,
    /// Symlinks on the way to the files, relative to the home directory. Parents come before
    /// their children.
    symlinks: BTreeMap<PathBuf, SymlinkTarget>,
    /// Files outside the home directory, or with paths that cannot be passed to rsync.
    skipped: Vec<PathBuf>,
}

fn plan_dependency_sync(home: &Path, files: impl IntoIterator<Item = PathBuf>) -> DependencySync {
    let canonical_home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let in_home = |path: &Path| {
        path.strip_prefix(home)
            .or_else(|_| path.strip_prefix(&canonical_home))
            .ok()
            .map(Path::to_path_buf)
    };
    let mut plan = DependencySync::default();
    for file in files {
        let relative = file
            .canonicalize()
            .ok()
            .and_then(|canonical| {
                canonical
                    .strip_prefix(&canonical_home)
                    .ok()
                    .map(Path::to_path_buf)
            })
            .filter(|relative| relative.to_str().is_some_and(|s| !s.contains('\n')));
        let Some(relative) = relative else {
            plan.skipped.push(file);
            continue;
        };
        plan.files.insert(relative);
        let Ok(alias) = file.strip_prefix(home) else {
            continue;
        };
        let mut link = home.to_path_buf();
        for component in alias.components() {
            link.push(component);
            let Ok(target) = std::fs::read_link(&link) else {
                continue;
            };
            // Relative targets stay valid, since the structure around them is copied.
            let target = match target.is_absolute().then(|| in_home(&target)).flatten() {
                Some(relative) => SymlinkTarget::InHome(relative),
                None => SymlinkTarget::Other(target),
            };
            let link = link.strip_prefix(home).unwrap_or(&link).to_path_buf();
            plan.symlinks.insert(link, target);
        }
    }
    plan
}

/// Shell command that creates `symlinks`, run from the remote home directory. Paths that exist
/// and are not symlinks are left alone.
fn symlinks_command(symlinks: &BTreeMap<PathBuf, SymlinkTarget>) -> String {
    symlinks
        .iter()
        .filter_map(|(link, target)| {
            let parent = link
                .parent()
                .and_then(Path::to_str)
                .filter(|p| !p.is_empty());
            let link = shell_quote(link.to_str()?);
            let target = match target {
                SymlinkTarget::InHome(target) => {
                    format!("\"$HOME\"/{}", shell_quote(target.to_str()?))
                }
                SymlinkTarget::Other(target) => shell_quote(target.to_str()?),
            };
            let mkdir = parent.map_or_else(String::new, |parent| {
                format!("mkdir -p {} && ", shell_quote(parent))
            });
            Some(format!(
                "if [ -L {link} ] || [ ! -e {link} ]; then {mkdir}ln -sfn {target} {link}; \
                else echo \"Not replacing ~/\"{link}\" with a symlink, since it exists\" >&2; fi"
            ))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Replaces the remote database with a consistent snapshot of the local one.
///
/// Copying the database file directly is unsafe: changes still in the `-wal` file would be
/// missing, and the remote's stale `-wal`/`-shm` files could be applied to the new file, corrupting
/// it. Instead a snapshot is taken with `VACUUM INTO` (which includes the WAL and is safe while the
/// local server is running), uploaded next to the remote database, and loaded with `sqlite3`'s
/// `.restore`. That uses SQLite's backup API, so it is safe while the remote server has the
/// database open.
async fn sync_database(remote_host: &str, data_dir: &Path) -> Result<(), String> {
    // The remote keeps its data in the same directory as the local machine.
    let database = data_dir.join(DATABASE_FILENAME);
    let database_str = database
        .to_str()
        .ok_or("Database path is not valid UTF-8")?;
    let incoming = format!("{database_str}.incoming");
    let snapshot =
        std::env::temp_dir().join(format!("{DATABASE_FILENAME}.sync-{}", std::process::id()));
    let snapshot_str = snapshot
        .to_str()
        .ok_or("Snapshot path is not valid UTF-8")?;
    let result = async {
        snapshot_database(&database, &snapshot).await?;
        run_rsync(&["-avz", snapshot_str, &format!("{remote_host}:{incoming}")])?;
        run_ssh(remote_host, &restore_command(database_str, &incoming)).map_err(|e| {
            format!("Restoring the database failed: {e}. `sqlite3` must be installed there.")
        })
    }
    .await;
    let _ = std::fs::remove_file(&snapshot);
    result
}

/// Writes a consistent copy of `database`, including changes still in its WAL, to `snapshot`.
async fn snapshot_database(database: &Path, snapshot: &Path) -> Result<(), String> {
    // `VACUUM INTO` refuses to overwrite a file.
    let _ = std::fs::remove_file(snapshot);
    let mut connection = SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .connect()
        .await
        .map_err(|e| format!("Failed to open {}: {e}", database.display()))?;
    sqlx::query("VACUUM INTO ?")
        .bind(
            snapshot
                .to_str()
                .ok_or("Snapshot path is not valid UTF-8")?,
        )
        .execute(&mut connection)
        .await
        .map_err(|e| format!("Failed to snapshot {}: {e}", database.display()))?;
    Ok(())
}

/// Shell command that loads `incoming` into `database` and then deletes `incoming`. `sqlite3` runs
/// each argument after the database as a separate command. The timeout waits for the server to
/// release its locks instead of failing immediately.
fn restore_command(database: &str, incoming: &str) -> String {
    format!(
        "sqlite3 {} {} {} && rm -f {}",
        shell_quote(database),
        shell_quote(".timeout 30000"),
        shell_quote(&format!(".restore {}", sqlite_quote(incoming))),
        shell_quote(incoming),
    )
}

/// Quotes `s` as a single POSIX shell word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quotes `s` as an argument to a `sqlite3` dot-command.
fn sqlite_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

fn run_ssh(remote_host: &str, command: &str) -> Result<(), String> {
    let status = Command::new("ssh")
        .arg(remote_host)
        .arg(command)
        .status()
        .map_err(|e| format!("Failed to run ssh: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ssh to {remote_host} exited with {status}"))
    }
}

fn run_rsync(args: &[&str]) -> Result<(), String> {
    let status = Command::new("rsync")
        .args(args)
        .status()
        .map_err(|e| format!("Failed to run rsync: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rsync exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use sqlx::ConnectOptions;
    use sqlx::Connection;
    use sqlx::SqliteConnection;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::sqlite::SqliteJournalMode;

    use super::SymlinkTarget;
    use super::plan_dependency_sync;
    use super::restore_command;
    use super::snapshot_database;
    use super::symlinks_command;

    async fn open(path: &PathBuf) -> SqliteConnection {
        let mut connection = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .connect()
            .await
            .unwrap();
        // Keep writes in the WAL so the test covers uncheckpointed changes.
        sqlx::query("PRAGMA wal_autocheckpoint = 0")
            .execute(&mut connection)
            .await
            .unwrap();
        connection
    }

    async fn values(connection: &mut SqliteConnection) -> Vec<String> {
        sqlx::query_scalar("SELECT value FROM t ORDER BY value")
            .fetch_all(connection)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_sync_database_snapshot_and_restore() {
        let dir =
            std::env::temp_dir().join(format!("spares-cloud-sync-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("local's db.sqlite");
        let remote = dir.join("remote's db.sqlite");
        let incoming = dir.join("remote's db.sqlite.incoming");

        // Local database with a change that is only in its WAL.
        let mut local_connection = open(&local).await;
        for statement in [
            "CREATE TABLE t (value TEXT)",
            "INSERT INTO t VALUES ('a'), ('b')",
        ] {
            sqlx::query(statement)
                .execute(&mut local_connection)
                .await
                .unwrap();
        }
        assert!(
            std::fs::metadata(local.with_extension("sqlite-wal"))
                .unwrap()
                .len()
                > 0
        );

        // Remote database held open by the "server", with stale changes in its own WAL.
        let mut server_connection = open(&remote).await;
        for statement in [
            "CREATE TABLE t (value TEXT)",
            "INSERT INTO t VALUES ('stale')",
        ] {
            sqlx::query(statement)
                .execute(&mut server_connection)
                .await
                .unwrap();
        }

        snapshot_database(&local, &incoming).await.unwrap();
        let status = Command::new("sh")
            .arg("-c")
            .arg(restore_command(
                remote.to_str().unwrap(),
                incoming.to_str().unwrap(),
            ))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!incoming.exists());

        assert_eq!(values(&mut server_connection).await, ["a", "b"]);
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&mut server_connection)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");

        local_connection.close().await.unwrap();
        server_connection.close().await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_sync_render_dependencies_symlinks() {
        let dir =
            std::env::temp_dir().join(format!("spares-cloud-deps-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let local = dir.join("local");
        let remote = dir.join("remote");
        let write = |path: PathBuf| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
            path
        };
        // Like `~/.local/share/pandoc/defaults -> ~/.config/pandoc/defaults`.
        let defaults = write(local.join(".config/pandoc/defaults/defaults.yaml"));
        let header = write(local.join(".config/pandoc/defaults/header.tex"));
        std::fs::create_dir_all(local.join(".local/share/pandoc")).unwrap();
        std::os::unix::fs::symlink(
            local.join(".config/pandoc/defaults"),
            local.join(".local/share/pandoc/defaults"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "defaults.yaml",
            local.join(".config/pandoc/defaults/d.yaml"),
        )
        .unwrap();
        let outside = write(dir.join("outside.tex"));

        let plan = plan_dependency_sync(
            &local,
            [
                local.join(".local/share/pandoc/defaults/d.yaml"),
                header,
                outside.clone(),
            ],
        );
        assert_eq!(
            plan.files.iter().collect::<Vec<_>>(),
            [
                &PathBuf::from(".config/pandoc/defaults/defaults.yaml"),
                &PathBuf::from(".config/pandoc/defaults/header.tex"),
            ]
        );
        assert_eq!(
            plan.symlinks.iter().collect::<Vec<_>>(),
            [
                (
                    &PathBuf::from(".local/share/pandoc/defaults"),
                    &SymlinkTarget::InHome(PathBuf::from(".config/pandoc/defaults"))
                ),
                (
                    &PathBuf::from(".local/share/pandoc/defaults/d.yaml"),
                    &SymlinkTarget::Other(PathBuf::from("defaults.yaml"))
                ),
            ]
        );
        assert_eq!(plan.skipped, [outside]);

        // Copy the files like rsync, then create the symlinks in the remote home.
        for file in &plan.files {
            write(remote.join(file));
        }
        let mut blocked = remote.join(".local/share/pandoc/blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let mut symlinks = plan.symlinks;
        symlinks.insert(
            PathBuf::from(".local/share/pandoc/blocked"),
            SymlinkTarget::InHome(PathBuf::from(".config")),
        );
        for _ in 0..2 {
            let output = Command::new("sh")
                .arg("-c")
                .arg(symlinks_command(&symlinks))
                .current_dir(&remote)
                .env("HOME", &remote)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("blocked"));
        }
        assert_eq!(
            std::fs::read_link(remote.join(".local/share/pandoc/defaults")).unwrap(),
            remote.join(".config/pandoc/defaults")
        );
        assert!(remote.join(".local/share/pandoc/defaults/d.yaml").is_file());
        assert!(
            remote
                .join(".local/share/pandoc/defaults/header.tex")
                .is_file()
        );
        blocked.push("x");
        assert!(!blocked.exists());
        assert!(defaults.is_file());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
