use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use spares_core::api::browser_render::get_render_assets_dir;
use spares_core::api::browser_render::read_render_dependencies;
use spares_core::config::get_cache_dir;
use spares_core::config::get_config_dir;
use spares_core::config::get_data_dir;
use spares_core::config::get_external_config_file;
use spares_core::config::get_files_dir;
use spares_core::config::get_media_dir;
use spares_core::config::read_external_config;
use spares_core::ids::NoteId;
use spares_core::parsers::Parseable;
use spares_core::parsers::RenderOutputDirectoryType;
use spares_core::parsers::get_all_parsers;
use spares_core::parsers::get_image_occlusion_rendered_directory;
use spares_core::sync::http::HttpRemote;
use spares_core::sync::http::RemoteAuth;
use spares_core::sync::push::PushTarget;
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

/// What [`push_files`] did, for after the sync.
pub(crate) struct Pushed {
    remote: HttpRemote,
    /// Whether templates, settings or the files notes read changed on the server, so its rendered
    /// files may be out of date
    render_inputs_changed: bool,
}

impl Pushed {
    /// Asks the server to render again the notes whose files are out of date, if what rendering
    /// reads changed. Its render cache tells which, from the note file each renders (made from
    /// the template).
    pub(crate) async fn render_if_changed(&self) -> Result<(), String> {
        if !self.render_inputs_changed {
            return Ok(());
        }
        self.remote
            .render_all()
            .await
            .map_err(|e| format!("Failed to ask the server to render: {e}"))?;
        println!("The server renders the notes these changes affect.");
        Ok(())
    }
}

/// Copies to `remote_host` what the server needs besides the database and media, which the sync
/// carries: the render assets, the templates and settings, the files the notes read, and the
/// rendered files of parsers the server cannot render. First asks the server at `url` where they
/// go, and checks over SSH that `remote_host` is that server (see [`check_target`]).
///
/// Runs before the sync, so that the notes the server renders after it are rendered with these.
pub(crate) async fn push_files(
    remote_host: &str,
    url: &str,
    token: Option<String>,
) -> Result<Pushed, String> {
    let remote = HttpRemote::new(url, RemoteAuth::token(token)).map_err(|e| e.to_string())?;
    let target = remote
        .push_target()
        .await
        .map_err(|e| format!("Failed to ask {url} where to copy files: {e}"))?;
    let home = std::env::home_dir().ok_or("Failed to find the home directory")?;
    let note_ids = server_rendered_note_ids(&spares_core::collection::paths().database()).await?;
    let dependencies = plan_dependency_sync(&home, render_dependency_files(&note_ids));
    let facts = remote_facts(remote_host, &target, &home)?;
    let local = LocalSide {
        root: get_data_dir(),
        replica: read_external_config()
            .map_err(|e| e.to_string())?
            .sync
            .replica,
        home: home.clone(),
    };
    check_target(
        remote_host,
        url,
        &local,
        &target,
        &facts,
        !dependencies.files.is_empty(),
    )?;

    // Render assets: the stored copies of the files Typst cards read, which web clients compile
    // them with. Of the collection's directory, only these: the database and media go with the
    // sync, settings and templates below, and the rest (notes, backups, ...) stay here.
    println!("Copying render assets → {remote_host}");
    let assets = render_assets_to_copy(&spares_core::collection::paths().database()).await?;
    if !assets.is_empty() {
        run_rsync_files_from(
            &assets.join("\n"),
            &["-az", "--ignore-existing"],
            &dir_source(&get_render_assets_dir())?,
            &remote_dir(remote_host, &target.render_assets)?,
        )?;
    }

    // Templates, and config.toml, which the server merges with its own (keeping its `[sync]`
    // table), so reviews there are scheduled with the same settings.
    println!("Copying templates and settings → {remote_host}");
    let templates = run_rsync(&[
        "-az",
        "--exclude=/config.toml",
        "--exclude=/config.toml.*",
        // Symlinks out of the config directory, such as one to pandoc's data directory, name
        // paths of this machine. The files notes read from there are copied below.
        "--safe-links",
        &dir_source(&get_config_dir())?,
        &remote_dir(remote_host, &target.config)?,
    ])?;
    let config_changed = push_config(&remote).await?;

    // Files the notes read, so the server can render notes edited there.
    println!("Copying files read by notes → {remote_host}");
    let read = sync_render_dependencies(remote_host, &home, &dependencies)?;

    // The rendered files of parsers the server can't render, for want of their programs (see
    // `setup_vps.sh --no-latex`), but not over its newer ones (see `sync_rendered_files`). The
    // server renders the others itself after the sync.
    let unrendered: Vec<String> = server_rendered_parsers()
        .map(|parser| parser.get_parser_name().to_string())
        .filter(|name| !target.renders.contains(name))
        .collect();
    if !unrendered.is_empty() {
        println!(
            "Copying rendered files of {} notes, which the server can't render → {remote_host}",
            unrendered.join(" and ")
        );
        sync_rendered_files(
            &remote_dir(remote_host, &target.files)?,
            &note_ids,
            &unrendered,
        )?;
    }

    Ok(Pushed {
        remote,
        render_inputs_changed: !templates.is_empty() || config_changed || !read.is_empty(),
    })
}

/// The names of the render assets to copy: every stored copy a `render_asset` row names, but
/// those only of media, which the sync gives the server and it stores itself when asked for them
/// (see `spares_core::api::browser_render::get_render_asset`). Stored copies are named by their
/// hash, so one already there is the same, and isn't copied again.
async fn render_assets_to_copy(database: &Path) -> Result<Vec<String>, String> {
    let mut connection = SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .connect()
        .await
        .map_err(|e| format!("Failed to open {}: {e}", database.display()))?;
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT path, hash FROM render_asset")
        .fetch_all(&mut connection)
        .await
        .map_err(|e| {
            format!(
                "Failed to read render assets from {}: {e}",
                database.display()
            )
        })?;
    let store = get_render_assets_dir();
    Ok(assets_to_copy(rows, &get_media_dir())
        .into_iter()
        .filter(|hash| store.join(hash).is_file())
        .collect())
}

/// The hashes of `rows` (`render_asset`'s paths and hashes) whose files are not in `media`.
fn assets_to_copy(rows: Vec<(String, String)>, media: &Path) -> BTreeSet<String> {
    rows.into_iter()
        .filter(|(path, _)| !Path::new(path).starts_with(media))
        .map(|(_, hash)| hash)
        .filter(|hash| !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect()
}

/// `dir` on `remote_host`, as an rsync destination for a directory's contents.
fn remote_dir(remote_host: &str, dir: &Path) -> Result<String, String> {
    let dir = dir
        .to_str()
        .filter(|dir| !dir.contains('\n') && dir.starts_with('/'))
        .ok_or_else(|| format!("The server named {} to copy files to", dir.display()))?;
    Ok(format!("{remote_host}:{}/", dir.trim_end_matches('/')))
}

/// This machine's side of [`check_target`].
#[derive(Debug)]
struct LocalSide {
    /// The collection's directory
    root: PathBuf,
    /// The collection's replica here, from `[sync] replica`
    replica: Option<u8>,
    home: PathBuf,
}

/// What `remote_host` says over SSH, to compare with the server's [`PushTarget`].
#[derive(Debug, Default, PartialEq, Eq)]
struct RemoteFacts {
    /// The contents of [`PushTarget::check_file`]
    check: Option<String>,
    /// The SSH user's home directory, without symlinks: where the files notes read are copied
    home: Option<PathBuf>,
    /// This machine's home directory as a path there, without symlinks, if it exists there
    local_home: Option<PathBuf>,
    /// Whether [`PushTarget::root`] is a directory there
    root_exists: bool,
}

/// Asks `remote_host` over SSH for the [`RemoteFacts`] about `target`.
fn remote_facts(
    remote_host: &str,
    target: &PushTarget,
    local_home: &Path,
) -> Result<RemoteFacts, String> {
    let script = facts_script(target, local_home)?;
    let output = Command::new("ssh")
        .arg(remote_host)
        .arg(script)
        .output()
        .map_err(|e| format!("Failed to run ssh: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ssh to {remote_host} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_remote_facts(&String::from_utf8_lossy(&output.stdout)))
}

/// The shell script whose output [`parse_remote_facts`] reads.
fn facts_script(target: &PushTarget, local_home: &Path) -> Result<String, String> {
    let quoted = |path: &Path| {
        path.to_str()
            .map(shell_quote)
            .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))
    };
    Ok(format!(
        "printf 'check=%s\\n' \"$(cat -- {check} 2>/dev/null)\"; \
         printf 'home=%s\\n' \"$(cd && pwd -P)\"; \
         printf 'local_home=%s\\n' \"$(cd -- {local_home} 2>/dev/null && pwd -P)\"; \
         if [ -d {root} ]; then echo root=yes; fi",
        check = quoted(&target.check_file)?,
        local_home = quoted(local_home)?,
        root = quoted(&target.root)?,
    ))
}

fn parse_remote_facts(output: &str) -> RemoteFacts {
    let mut facts = RemoteFacts::default();
    let non_empty = |value: &str| (!value.is_empty()).then(|| value.to_string());
    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "check" => facts.check = non_empty(value),
            "home" => facts.home = non_empty(value).map(PathBuf::from),
            "local_home" => facts.local_home = non_empty(value).map(PathBuf::from),
            "root" => facts.root_exists = value == "yes",
            _ => {}
        }
    }
    facts
}

/// Checks, before anything is copied, that `remote_host` reaches the collection the server at
/// `url` serves, at the paths the copies assume:
///
/// - the check value the server just wrote is in its check file there, so `remote_host` is the
///   server's machine and its user can read the collection's files;
/// - the collection is in the same directory as here, since notes name their images and the files
///   they read by absolute path;
/// - the server's replica isn't this one;
/// - the directories the server named are in the collection's;
/// - if files the notes read are copied (to the same paths relative to the SSH user's home),
///   this machine's home directory leads there, as `setup_vps.sh --local-home` sets up.
fn check_target(
    remote_host: &str,
    url: &str,
    local: &LocalSide,
    target: &PushTarget,
    facts: &RemoteFacts,
    copies_home_files: bool,
) -> Result<(), String> {
    if facts.check.as_deref() != Some(target.check.as_str()) {
        return Err(format!(
            "{remote_host} doesn't reach the collection {url} serves: {} there doesn't hold the \
             value the server just wrote to it. Set `sync.remote_host` in config.toml to the \
             server's machine, as the user the server runs as.",
            target.check_file.display()
        ));
    }
    if !facts.root_exists {
        return Err(format!(
            "{} is not a directory on {remote_host}",
            target.root.display()
        ));
    }
    if target.root != local.root {
        return Err(format!(
            "The server keeps this collection in {}, but here it is in {}. Notes name their \
             images and the files they read by absolute path, so it must be the same path on \
             both (`spares deploy` sets the server up so).",
            target.root.display(),
            local.root.display()
        ));
    }
    if local.replica == Some(target.replica) {
        return Err(format!(
            "The server's copy of the collection is replica {}, like this one. Log in to it \
             again with `spares sync login`, which gives this copy a number of its own.",
            target.replica
        ));
    }
    for dir in [&target.config, &target.media, &target.render_assets] {
        if !dir.starts_with(&target.root) {
            return Err(format!(
                "The server named {} to copy files to, which is not in its collection's \
                 directory {}",
                dir.display(),
                target.root.display()
            ));
        }
    }
    if copies_home_files && (facts.local_home.is_none() || facts.local_home != facts.home) {
        let leads_to = facts.local_home.as_ref().map_or_else(
            || "doesn't exist".to_string(),
            |path| format!("leads to {}", path.display()),
        );
        let home = facts
            .home
            .as_ref()
            .map_or_else(|| "?".to_string(), |path| path.display().to_string());
        return Err(format!(
            "On {remote_host}, {} {leads_to}, not to {home}, the home directory the files your \
             notes read are copied to, so the server would not find them. Make it a symlink to \
             it, as `setup_vps.sh --local-home {}` does.",
            local.home.display(),
            local.home.display()
        ));
    }
    Ok(())
}

/// Sends this machine's config.toml to the server, which keeps its own `[sync]` table, so the
/// token, replica and addresses here never reach it.
async fn push_config(remote: &HttpRemote) -> Result<bool, String> {
    let path = get_external_config_file();
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let changed = remote
        .push_config(contents)
        .await
        .map_err(|e| format!("Failed to send {} to the server: {e}", path.display()))?;
    if changed {
        println!("  config.toml");
    }
    Ok(changed)
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
        .flat_map(|parser| parser_rendered_dirs(parser.as_ref()))
        .collect()
}

fn parser_rendered_dirs(parser: &dyn Parseable) -> [PathBuf; 2] {
    [
        RenderOutputDirectoryType::Note,
        RenderOutputDirectoryType::Card,
    ]
    .map(|output_type| parser.get_output_rendered_dir(output_type))
}

/// The notes of each parser the server renders, by parser name.
async fn server_rendered_note_ids(
    database: &Path,
) -> Result<HashMap<&'static str, HashSet<NoteId>>, String> {
    let mut connection = SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .connect()
        .await
        .map_err(|e| format!("Failed to open {}: {e}", database.display()))?;
    let rows: Vec<(NoteId, String)> = sqlx::query_as(
        "SELECT note.id, parser.name FROM note JOIN parser ON parser.id = note.parser_id",
    )
    .fetch_all(&mut connection)
    .await
    .map_err(|e| format!("Failed to read notes from {}: {e}", database.display()))?;
    let mut note_ids: HashMap<&'static str, HashSet<NoteId>> = server_rendered_parsers()
        .map(|parser| (parser.get_parser_name(), HashSet::new()))
        .collect();
    for (note_id, parser_name) in rows {
        if let Some(ids) = note_ids.get_mut(parser_name.as_str()) {
            ids.insert(note_id);
        }
    }
    Ok(note_ids)
}

/// The note a rendered file belongs to. Rendered files are named `<note id>.<ext>` or
/// `<note id>-<order>-<side>.<ext>`.
fn rendered_file_note_id(file_name: &str) -> Option<NoteId> {
    file_name.split(['-', '.']).next()?.parse().ok()
}

/// `dir` as an rsync source that copies its contents.
fn dir_source(dir: &Path) -> Result<String, String> {
    let dir = dir
        .to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", dir.display()))?;
    Ok(format!("{dir}/"))
}

/// Copies the rendered files of the notes of `parsers` into `destination`, the server's files
/// directory, with their rendered image occlusions.
///
/// Files newer on the server are kept (`--update`), in case it rendered a note edited there. This
/// needs the two clocks to roughly agree.
fn sync_rendered_files(
    destination: &str,
    note_ids: &HashMap<&'static str, HashSet<NoteId>>,
    parsers: &[String],
) -> Result<(), String> {
    // Rendered files can share a directory with other files (`aux/`, files rendered outside
    // spares), so only those of existing notes are copied, by name.
    let mut dirs: BTreeMap<PathBuf, HashSet<NoteId>> = BTreeMap::new();
    let mut all_ids: HashSet<NoteId> = HashSet::new();
    for parser in server_rendered_parsers() {
        if !parsers.iter().any(|name| name == parser.get_parser_name()) {
            continue;
        }
        let ids = &note_ids[parser.get_parser_name()];
        all_ids.extend(ids);
        for dir in parser_rendered_dirs(parser.as_ref()) {
            dirs.entry(dir).or_default().extend(ids);
        }
    }
    dirs.entry(get_image_occlusion_rendered_directory())
        .or_default()
        .extend(all_ids);
    for (dir, ids) in dirs {
        let names = rendered_files_of(&dir, &ids);
        if names.is_empty() {
            continue;
        }
        let destination = match dir.strip_prefix(get_files_dir()) {
            Ok(relative) if !relative.as_os_str().is_empty() => {
                format!("{destination}{}/", relative.display())
            }
            _ => destination.to_string(),
        };
        run_rsync_files_from(
            &names.join("\n"),
            &["-az", "--update"],
            &dir_source(&dir)?,
            &destination,
        )?;
    }
    Ok(())
}

/// The names of the rendered files in `dir` of the notes `ids`.
fn rendered_files_of(dir: &Path, ids: &HashSet<NoteId>) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            !name.ends_with(".synctex.gz")
                && !name.contains('\n')
                && rendered_file_note_id(name).is_some_and(|id| ids.contains(&id))
        })
        .collect();
    names.sort();
    names
}

/// Copies the files that rendering recorded as read by the notes of parsers the server renders
/// (preambles, figures, pandoc defaults, ...) to the same paths relative to the remote home
/// directory, recreating the symlinks on the way to them. Absolute paths in notes also work if
/// the local home directory is a symlink to the remote one (see `setup_vps.sh --local-home`).
///
/// Only files read by a local render are known, so a file a note starts reading after being edited
/// on the server is not copied until the note is rendered locally.
fn sync_render_dependencies(
    remote_host: &str,
    home: &Path,
    plan: &DependencySync,
) -> Result<Vec<String>, String> {
    for path in &plan.skipped {
        eprintln!(
            "Not syncing {}, since it is outside {}",
            path.display(),
            home.display()
        );
    }
    if plan.files.is_empty() {
        return Ok(Vec::new());
    }
    let contents = plan
        .files
        .iter()
        .filter_map(|path| path.to_str())
        .collect::<Vec<_>>()
        .join("\n");
    let copied = run_rsync_files_from(
        &contents,
        &["-azR"],
        &dir_source(home)?,
        &format!("{remote_host}:"),
    )?;
    if !plan.symlinks.is_empty() {
        run_ssh(remote_host, &symlinks_command(&plan.symlinks))
            .map_err(|e| format!("Creating symlinks failed: {e}"))?;
    }
    Ok(copied)
}

/// Existing files read by rendered notes of parsers the server renders, except those synced by
/// other passes or written by rendering. Records left by deleted notes are ignored.
fn render_dependency_files(note_ids: &HashMap<&'static str, HashSet<NoteId>>) -> BTreeSet<PathBuf> {
    let excluded_dirs: Vec<PathBuf> = [
        get_data_dir(),
        get_cache_dir(),
        get_image_occlusion_rendered_directory(),
    ]
    .into_iter()
    .chain(rendered_dirs())
    .collect();
    server_rendered_parsers()
        .flat_map(|parser| {
            read_render_dependencies(parser.as_ref(), Some(&note_ids[parser.get_parser_name()])).0
        })
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

/// Quotes `s` as a single POSIX shell word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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

/// Runs rsync on the files listed in `list` (one path per line, relative to `source`). Returns
/// the files it copied (see [`run_rsync`]).
fn run_rsync_files_from(
    list: &str,
    args: &[&str],
    source: &str,
    destination: &str,
) -> Result<Vec<String>, String> {
    let path = std::env::temp_dir().join(format!("spares-sync-files-{}", std::process::id()));
    let result = std::fs::write(&path, list)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
        .and_then(|()| {
            let files_from = format!("--files-from={}", path.display());
            let mut all = args.to_vec();
            all.extend([files_from.as_str(), source, destination]);
            run_rsync(&all)
        });
    let _ = std::fs::remove_file(&path);
    result
}

/// Runs rsync, printing each file it copies. Returns them, relative to the source, without the
/// directories it created or updated.
fn run_rsync(args: &[&str]) -> Result<Vec<String>, String> {
    let output = Command::new("rsync")
        .arg("--out-format=%n")
        .args(args)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("Failed to run rsync: {e}"))?;
    if !output.status.success() {
        return Err(format!("rsync exited with {}", output.status));
    }
    let copied = copied_files(&String::from_utf8_lossy(&output.stdout));
    for file in &copied {
        println!("  {file}");
    }
    Ok(copied)
}

/// The files in rsync's `--out-format=%n` output: one per line, directories ending in `/`.
fn copied_files(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| !line.is_empty() && !line.ends_with('/'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use spares_core::ids::NoteId;

    use super::LocalSide;
    use super::PushTarget;
    use super::RemoteFacts;
    use super::SymlinkTarget;
    use super::assets_to_copy;
    use super::check_target;
    use super::copied_files;
    use super::parse_remote_facts;
    use super::plan_dependency_sync;
    use super::rendered_file_note_id;
    use super::rendered_files_of;
    use super::symlinks_command;

    fn target() -> PushTarget {
        let root = PathBuf::from("/home/me/.local/share/spares/users/me");
        PushTarget {
            config: root.join("config"),
            media: root.join("media"),
            files: PathBuf::from("/home/me/.cache/spares/users/me/files"),
            render_assets: root.join("render_assets"),
            root,
            replica: 1,
            renders: vec!["markdown".to_string()],
            check_file: PathBuf::from("/home/me/.cache/spares/users/me/sync/push-check"),
            check: "abc".to_string(),
        }
    }

    fn local() -> LocalSide {
        LocalSide {
            root: target().root,
            replica: Some(0),
            home: PathBuf::from("/Users/me"),
        }
    }

    fn facts() -> RemoteFacts {
        RemoteFacts {
            check: Some("abc".to_string()),
            home: Some(PathBuf::from("/home/me")),
            local_home: Some(PathBuf::from("/home/me")),
            root_exists: true,
        }
    }

    fn check(local: &LocalSide, target: &PushTarget, facts: &RemoteFacts) -> Result<(), String> {
        check_target("me@vps", "https://vps", local, target, facts, true)
    }

    #[test]
    fn the_remote_host_must_reach_the_collection_the_server_serves() {
        check(&local(), &target(), &facts()).unwrap();
        for check_value in [None, Some("other".to_string())] {
            let facts = RemoteFacts {
                check: check_value,
                ..facts()
            };
            let error = check(&local(), &target(), &facts).unwrap_err();
            assert!(error.contains("doesn't reach the collection"), "{error}");
        }
        let facts = RemoteFacts {
            root_exists: false,
            ..facts()
        };
        assert!(check(&local(), &target(), &facts).is_err());
    }

    #[test]
    fn the_collection_must_be_at_the_same_path_with_another_replica() {
        let moved = LocalSide {
            root: PathBuf::from("/Users/me/.local/share/spares/users/me"),
            ..local()
        };
        let error = check(&moved, &target(), &facts()).unwrap_err();
        assert!(error.contains("same path"), "{error}");
        let same_replica = LocalSide {
            replica: Some(1),
            ..local()
        };
        let error = check(&same_replica, &target(), &facts()).unwrap_err();
        assert!(error.contains("replica 1"), "{error}");
        // A collection that has no number yet
        let unnumbered = LocalSide {
            replica: None,
            ..local()
        };
        check(&unnumbered, &target(), &facts()).unwrap();
    }

    #[test]
    fn directories_outside_the_collection_are_refused() {
        let target = PushTarget {
            render_assets: PathBuf::from("/etc"),
            ..target()
        };
        let error = check(&local(), &target, &facts()).unwrap_err();
        assert!(error.contains("/etc"), "{error}");
    }

    #[test]
    fn files_in_the_home_directory_need_the_local_home_to_lead_there() {
        for local_home in [None, Some(PathBuf::from("/home/other"))] {
            let facts = RemoteFacts {
                local_home,
                ..facts()
            };
            let error = check(&local(), &target(), &facts).unwrap_err();
            assert!(error.contains("--local-home /Users/me"), "{error}");
            // Nothing is copied there
            check_target("me@vps", "https://vps", &local(), &target(), &facts, false).unwrap();
        }
    }

    #[test]
    fn remote_facts_are_read_from_the_script_output() {
        let facts =
            parse_remote_facts("check=abc\nhome=/home/me\nlocal_home=\nroot=yes\nunrelated line\n");
        assert_eq!(
            facts,
            RemoteFacts {
                check: Some("abc".to_string()),
                home: Some(PathBuf::from("/home/me")),
                local_home: None,
                root_exists: true,
            }
        );
        assert_eq!(parse_remote_facts(""), RemoteFacts::default());
    }

    #[test]
    fn the_facts_script_runs_in_sh() {
        let dir = std::env::temp_dir().join(format!("spares-facts-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home me");
        let root = home.join("it's the root");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&home, dir.join("local")).unwrap();
        let check_file = home.join("check");
        std::fs::write(&check_file, "abc").unwrap();
        let target = PushTarget {
            root,
            check_file,
            ..target()
        };
        let run = |local_home: &std::path::Path| {
            let output = Command::new("sh")
                .arg("-c")
                .arg(super::facts_script(&target, local_home).unwrap())
                .env("HOME", &home)
                .output()
                .unwrap();
            assert!(output.status.success());
            parse_remote_facts(&String::from_utf8(output.stdout).unwrap())
        };
        let home = home.canonicalize().unwrap();
        assert_eq!(
            run(&dir.join("local")),
            RemoteFacts {
                check: Some("abc".to_string()),
                home: Some(home.clone()),
                local_home: Some(home.clone()),
                root_exists: true,
            }
        );
        assert_eq!(run(&dir.join("missing")).local_home, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn render_assets_of_media_are_not_copied() {
        let media = std::path::Path::new("/home/me/collection/media");
        let row = |path: &str, hash: &str| (path.to_string(), hash.to_string());
        let hashes = assets_to_copy(
            vec![
                row("/home/me/collection/media/images/a.png", "aa"),
                row("/home/me/figures/b.png", "bb"),
                row("@image-occlusions/aab-1-front-io-1.png", "cc"),
                // The same contents as a media file, but read from elsewhere too
                row("/home/me/collection/media/images/d.png", "dd"),
                row("/home/me/figures/d.png", "dd"),
                row("/home/me/figures/bad.png", "../x"),
            ],
            media,
        );
        assert_eq!(hashes.into_iter().collect::<Vec<_>>(), ["bb", "cc", "dd"]);
    }

    #[test]
    fn rsync_reports_the_files_it_copied_without_directories() {
        assert_eq!(
            copied_files("./\nmarkdown/\nmarkdown/templates/note_template.md\n\nconfig.toml\n"),
            ["markdown/templates/note_template.md", "config.toml"]
        );
        assert_eq!(copied_files(""), Vec::<String>::new());
    }

    #[test]
    fn rendered_files_of_some_notes() {
        let dir = std::env::temp_dir().join(format!("spares-rendered-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("aab")).unwrap();
        for name in [
            "aab.pdf",
            "aab-1-front.pdf",
            "aab-1-front-io-1.png",
            "aab.synctex.gz",
            "aac.pdf",
            "539_HW1.pdf",
        ] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let ids = [NoteId::from_letters("aab").unwrap()].into_iter().collect();
        assert_eq!(
            rendered_files_of(&dir, &ids),
            ["aab-1-front-io-1.png", "aab-1-front.pdf", "aab.pdf"]
        );
        assert_eq!(
            rendered_files_of(&dir.join("missing"), &ids),
            Vec::<String>::new()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_files_named_after_a_note_are_rendered_files() {
        let note = |letters| NoteId::from_letters(letters).ok();
        assert_eq!(rendered_file_note_id("aab.pdf"), note("aab"));
        assert_eq!(rendered_file_note_id("aab-1-front.pdf"), note("aab"));
        assert_eq!(rendered_file_note_id("aab.deps.json"), note("aab"));
        // Files rendered outside spares, which `push_files` then checks against the notes
        assert_eq!(rendered_file_note_id("539_HW1.pdf"), None);
        assert_eq!(rendered_file_note_id("1780433902-chat.html"), None);
        assert_eq!(rendered_file_note_id("0001.pdf"), None);
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
