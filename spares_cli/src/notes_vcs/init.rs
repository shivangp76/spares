//! `spares note init-vcs`: turns the notes directory into a clone of the server's notes repository.
//! Every step checks for the state an interrupted run leaves, so rerunning continues it.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use chrono::Utc;
use colored::Colorize;
use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::config::get_data_dir;
use spares_core::notes_vcs;
use spares_core::notes_vcs::InitResponse;
use spares_core::notes_vcs::StatusResponse;
use spares_core::notes_vcs::git::GIT_LOCATION_VARIABLES;
use walkdir::WalkDir;

use crate::utils::ensure_ok;

/// Name of the branch (git) or description (jj) holding edits found in the notes directory before
/// it became a repository.
const LOCAL_EDITS_BRANCH: &str = "pre-vcs-local-edits";
const LOCAL_EDITS_MESSAGE: &str =
    "Local edits from before spares used git (review before pushing: some may be outdated)";
/// jj 0.30 is the first version that pushes by running `git`, so the server's hooks run.
const MIN_JJ_VERSION: (u32, u32) = (0, 30);

/// Delimit the aliases in the clone's jj config, so `--repair-hooks` can replace them.
const JJ_ALIASES_BEGIN: &str =
    "# BEGIN spares aliases (rewritten by `spares note init-vcs --repair-hooks`)";
const JJ_ALIASES_END: &str = "# END spares aliases";

/// The aliases as versions before [`JJ_ALIASES_BEGIN`] wrote them, to replace
const OLD_JJ_ALIASES: &str = r#"
[aliases]
# Fetch spares' changes and rebase your work (the current branch, or the given revision's) onto them
spares-pull = ["util", "exec", "--", "sh", "-c", '''
  jj git fetch && jj rebase -b "${1:-@}" -d main@origin
''', "spares-pull"]
# Push a revision (default: @, or @- when @ is empty) to spares
spares-push = ["util", "exec", "--", "sh", "-c", '''
  set -e
  rev="${1:-@}"
  if [ "$rev" = @ ] && [ "$(jj log --no-graph -r @ -T empty)" = true ]; then rev=@-; fi
  jj git fetch
  if [ -z "$(jj log --no-graph -r "main@origin & ::($rev)" -T '"x"')" ]; then
    echo "spares has newer notes; run 'jj spares-pull' first" >&2; exit 1
  fi
  if [ -n "$(jj log --no-graph -r "conflicts() & (main@origin..($rev))" -T '"x"')" ]; then
    echo "resolve the conflicts first (jj log -r 'conflicts()')" >&2; exit 1
  fi
  # Checked above to be on top of spares' `main`, so moving the local bookmark anywhere is safe
  jj bookmark set main --allow-backwards -r "$rev"
  # A rejected push must not leave the local `main` on the rejected commit
  jj git push -b main || { jj bookmark set main --allow-backwards -r main@origin >/dev/null 2>&1; exit 1; }
''', "spares-push"]
"#;

/// Aliases added to the clone's jj config, running `spares` as `spares_command`. See the workflow
/// docs.
fn jj_aliases(spares_command: &str) -> String {
    let aliases = r#"[aliases]
# Fetch spares' changes and rebase your work (the current branch, or the given revision's) onto them
spares-pull = ["util", "exec", "--", "sh", "-c", '''
  jj git fetch && jj rebase -b "${1:-@}" -d main@origin
''', "spares-pull"]
# Push a revision (default: @, or @- when @ is empty and not a merge) to spares
spares-push = ["util", "exec", "--", "sh", "-c", '''
  set -e
  rev="${1:-@}"
  # An empty merge, as `jj new main@origin anki@origin` makes, is pushed itself
  if [ "$rev" = @ ] && [ -n "$(jj log --no-graph -r '@ & empty() & ~merges()' -T '"x"')" ]; then rev=@-; fi
  jj git fetch
  if [ -z "$(jj log --no-graph -r "main@origin & ::($rev)" -T '"x"')" ]; then
    echo "spares has newer notes; run 'jj spares-pull' first" >&2; exit 1
  fi
  if [ -n "$(jj log --no-graph -r "conflicts() & (main@origin..($rev))" -T '"x"')" ]; then
    echo "resolve the conflicts first (jj log -r 'conflicts()')" >&2; exit 1
  fi
  # Checked above to be on top of spares' `main`, so moving the local bookmark anywhere is safe
  jj bookmark set main --allow-backwards -r "$rev"
  # A rejected push must not leave the local `main` on the rejected commit
  jj git push -b main || { jj bookmark set main --allow-backwards -r main@origin >/dev/null 2>&1; exit 1; }
''', "spares-push"]
# Update `anki` with Anki's notes and fetch it
spares-anki-pull = ["util", "exec", "--", "sh", "-c", '''
  set -e
  @SPARES@ note anki-refresh
  jj git fetch
  jj bookmark track anki@origin >/dev/null 2>&1 || true
''', "spares-anki-pull"]
# Write a revision (default: @, or @- when @ is empty and not a merge) to Anki by pushing it to `anki`
spares-anki-push = ["util", "exec", "--", "sh", "-c", '''
  set -e
  rev="${1:-@}"
  # An empty merge, as `jj new main@origin anki@origin` makes, is pushed itself
  if [ "$rev" = @ ] && [ -n "$(jj log --no-graph -r '@ & empty() & ~merges()' -T '"x"')" ]; then rev=@-; fi
  @SPARES@ note anki-refresh
  jj git fetch
  jj bookmark track anki@origin >/dev/null 2>&1 || true
  if [ -z "$(jj log --no-graph -r "anki@origin & ::($rev)" -T '"x"')" ]; then
    echo "the revision is not on top of anki@origin, or Anki has newer notes; merge anki@origin into it first" >&2; exit 1
  fi
  if [ -n "$(jj log --no-graph -r "conflicts() & (anki@origin..($rev))" -T '"x"')" ]; then
    echo "resolve the conflicts first (jj log -r 'conflicts()')" >&2; exit 1
  fi
  jj bookmark set anki --allow-backwards -r "$rev"
  jj git push -b anki || { jj bookmark set anki --allow-backwards -r anki@origin >/dev/null 2>&1; exit 1; }
''', "spares-anki-push"]
"#;
    format!(
        "{JJ_ALIASES_BEGIN}\n{}{JJ_ALIASES_END}\n",
        aliases.replace("@SPARES@", spares_command)
    )
}

/// `config` with the spares aliases replaced by `aliases`, or added. `None` if it has aliases
/// spares did not write, which are left alone.
fn with_jj_aliases(config: &str, aliases: &str) -> Option<String> {
    if let (Some(start), Some(end)) = (config.find(JJ_ALIASES_BEGIN), config.find(JJ_ALIASES_END))
        && start < end
    {
        let end = end + JJ_ALIASES_END.len();
        let end = if config[end..].starts_with('\n') {
            end + 1
        } else {
            end
        };
        let mut config = config.to_string();
        config.replace_range(start..end, aliases);
        return Some(config);
    }
    if config.contains(OLD_JJ_ALIASES) {
        return Some(config.replacen(OLD_JJ_ALIASES, &format!("\n{aliases}"), 1));
    }
    if config.contains("spares-push") {
        return None;
    }
    Some(format!("{config}\n{aliases}"))
}

/// `--repair-hooks`: rewrites the hooks, and the jj aliases of a jj clone.
fn repair(dirs: &Dirs, base_url: &str) -> Result<(), Error> {
    if !dirs.repo.join(".git").exists() {
        return Err(miette!(
            "{} does not exist; run `spares note init-vcs` first",
            dirs.repo.display()
        ));
    }
    install_hooks(&dirs.repo, base_url)?;
    println!("Rewrote the hooks in {}", dirs.repo.display());
    if dirs.notes.join(".jj").exists() {
        setup_jj(&dirs.notes, &spares_command(base_url)?)?;
        println!("Rewrote the jj aliases of {}", dirs.notes.display());
    }
    Ok(())
}

struct Dirs {
    data: PathBuf,
    repo: PathBuf,
    notes: PathBuf,
    clone_tmp: PathBuf,
}

impl Dirs {
    fn new() -> Self {
        let data = get_data_dir();
        Self {
            repo: notes_vcs::server_repo_dir(),
            notes: data.join("notes"),
            clone_tmp: data.join(".notes-clone-tmp"),
            data,
        }
    }
}

pub(crate) async fn init_vcs(
    base_url: &str,
    client: &Client,
    jj: bool,
    repair_hooks: bool,
) -> Result<(), Error> {
    let dirs = Dirs::new();
    require_tool("git", &["--version"])?;
    if jj {
        check_jj_version()?;
    }
    check_typst_root(&dirs.data)?;
    let status = server_status(base_url, client).await?;
    let data_dir = dirs
        .data
        .canonicalize()
        .unwrap_or_else(|_| dirs.data.clone());
    if status.data_dir != data_dir {
        return Err(miette!(
            "The spares server at {base_url} uses the data directory {}, but this command uses {}. Run it with the same SPARES_* settings as the server.",
            status.data_dir.display(),
            data_dir.display()
        ));
    }

    if repair_hooks {
        return repair(&dirs, base_url);
    }

    if let Some(origin) = clone_origin(&dirs.notes) {
        if same_path(&origin, &dirs.repo) {
            install_hooks(&dirs.repo, base_url)?;
            println!(
                "{} is already a clone of {}.",
                dirs.notes.display(),
                dirs.repo.display()
            );
            return Ok(());
        }
        return Err(miette!(
            "{} is already a repository, cloned from {}. Move it away to set up spares' repository.",
            dirs.notes.display(),
            origin.display()
        ));
    }

    // Steps 2-4 happen in the server, under its lock, so no change can land in between.
    if dirs.repo.exists() {
        println!("Using the existing repository {}", dirs.repo.display());
    } else {
        println!("Creating {} from the database…", dirs.repo.display());
        let response = client
            .post(format!("{base_url}/api/notes/vcs/init"))
            .send()
            .await
            .map_err(|e| miette!("{}", e))?;
        let init: InitResponse = ensure_ok(response)
            .await?
            .json()
            .await
            .map_err(|e| miette!("{}", e))?;
        println!(
            "Committed {} notes ({} rendered files stay valid).",
            init.note_count, init.render_hashes_kept
        );
    }

    install_hooks(&dirs.repo, base_url)?;

    println!("Cloning it…");
    if dirs.clone_tmp.exists() {
        std::fs::remove_dir_all(&dirs.clone_tmp).map_err(|e| miette!("{}", e))?;
    }
    if jj {
        run(Command::new("jj")
            .args(["git", "clone", "--colocate"])
            .arg(&dirs.repo)
            .arg(&dirs.clone_tmp))?;
        setup_jj(&dirs.clone_tmp, &spares_command(base_url)?)?;
    } else {
        run(git_command()
            .args(["clone", "-q"])
            .arg(&dirs.repo)
            .arg(&dirs.clone_tmp))?;
    }

    // Swap the clone in. The `~/notes` symlink keeps pointing at the same path.
    let backup = if dirs.notes.exists() {
        let backup = dirs.data.join(format!(
            "notes.pre-vcs-{}",
            Utc::now().format("%Y%m%dT%H%M%SZ")
        ));
        std::fs::rename(&dirs.notes, &backup).map_err(|e| miette!("{}", e))?;
        Some(backup)
    } else {
        latest_backup(&dirs.data)
    };
    std::fs::rename(&dirs.clone_tmp, &dirs.notes).map_err(|e| miette!("{}", e))?;

    if let Some(backup) = &backup {
        carry_over(backup, &dirs.notes, jj)?;
    }
    print_workflow(&dirs, backup.as_deref(), jj);
    Ok(())
}

pub(crate) async fn server_status(
    base_url: &str,
    client: &Client,
) -> Result<StatusResponse, Error> {
    let response = client
        .get(format!("{base_url}/api/notes/vcs/status"))
        .send()
        .await
        .map_err(|_| miette!("The spares server at {base_url} is not running; start it first"))?;
    ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{}", e))
}

fn require_tool(tool: &str, args: &[&str]) -> Result<String, Error> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .map_err(|_| miette!("`{tool}` was not found on the PATH"))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn check_jj_version() -> Result<(), Error> {
    let version = require_tool("jj", &["--version"])?;
    let numbers: Vec<u32> = version
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .split(['.', '-'])
        .filter_map(|n| n.parse().ok())
        .collect();
    let found = (
        numbers.first().copied().unwrap_or(0),
        numbers.get(1).copied().unwrap_or(0),
    );
    if found < MIN_JJ_VERSION {
        return Err(miette!(
            "{version} is too old: jj {}.{} or newer is needed, so pushes run spares' hooks",
            MIN_JJ_VERSION.0,
            MIN_JJ_VERSION.1
        ));
    }
    Ok(())
}

/// Typst can only read files under its root, and notes are compiled inside the data directory.
fn check_typst_root(data_dir: &Path) -> Result<(), Error> {
    let Some(root) = std::env::var_os("TYPST_ROOT") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    let root = root.canonicalize().unwrap_or(root);
    let data = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    if data.starts_with(&root) {
        Ok(())
    } else {
        Err(miette!(
            "TYPST_ROOT ({}) does not contain the data directory ({}), where spares compiles notes from now on. Set TYPST_ROOT to a directory containing both, e.g. `/`.",
            root.display(),
            data.display()
        ))
    }
}

/// git, isolated from the git environment of a caller such as a hook.
fn git_command() -> Command {
    let mut command = Command::new("git");
    for variable in GIT_LOCATION_VARIABLES {
        command.env_remove(variable);
    }
    command
}

fn run(command: &mut Command) -> Result<String, Error> {
    let output = command
        .output()
        .map_err(|e| miette!("Failed to run {:?}: {e}", command.get_program()))?;
    if !output.status.success() {
        return Err(miette!(
            "{:?} failed: {}",
            command,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The origin of `dir` if it is a git repository (also colocated with jj).
fn clone_origin(dir: &Path) -> Option<PathBuf> {
    if !dir.join(".git").exists() {
        return None;
    }
    run(git_command()
        .current_dir(dir)
        .args(["config", "--get", "remote.origin.url"]))
    .ok()
    .map(PathBuf::from)
    .or_else(|| Some(PathBuf::from("(no origin)")))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn latest_backup(data_dir: &Path) -> Option<PathBuf> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(data_dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("notes.pre-vcs-"))
        })
        .collect();
    backups.sort();
    backups.pop()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// This spares binary, run against this instance: its address and data directory are fixed, so a
/// shell with other settings cannot reach another spares instance.
fn spares_command(base_url: &str) -> Result<String, Error> {
    let exe = std::env::current_exe().map_err(|e| miette!("{}", e))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let socket_address = base_url.trim_start_matches("http://");
    let data_dir = get_data_dir();
    let data_dir = data_dir.canonicalize().unwrap_or(data_dir);
    Ok(format!(
        "env SPARES_SOCKET_ADDRESS={} SPARES_DATA_DIR={} {}",
        shell_quote(socket_address),
        shell_quote(&data_dir.to_string_lossy()),
        shell_quote(&exe.to_string_lossy()),
    ))
}

/// Writes the hooks and the repository settings that make a push an import. The hooks carry this
/// instance's address and data directory, so a push from a shell with other settings cannot reach
/// another spares instance.
fn install_hooks(repo: &Path, base_url: &str) -> Result<(), Error> {
    let spares = spares_command(base_url)?;
    let hooks_dir = repo.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).map_err(|e| miette!("{}", e))?;
    for (hook, phase) in [
        ("pre-receive", "pre-receive"),
        ("update", "update"),
        ("push-to-checkout", "push-to-checkout"),
        ("post-receive", "post-receive"),
    ] {
        let script = format!(
            "#!/bin/sh\n\
             # Written by `spares note init-vcs`. Rewrite with `spares note init-vcs --repair-hooks`.\n\
             exec {spares} note vcs-hook {phase} \"$@\"\n",
        );
        let path = hooks_dir.join(hook);
        std::fs::write(&path, script).map_err(|e| miette!("{}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| miette!("{}", e))?;
        }
    }
    let hooks_dir = hooks_dir.canonicalize().unwrap_or(hooks_dir);
    for (key, value) in [
        // A global `core.hooksPath` would otherwise make git skip these hooks, and check out the
        // pushed tree without importing it.
        ("core.hooksPath", hooks_dir.to_string_lossy().into_owned()),
        ("receive.denyCurrentBranch", "updateInstead".to_string()),
        ("receive.denyNonFastForwards", "true".to_string()),
        ("receive.denyDeletes", "true".to_string()),
    ] {
        run(git_command()
            .current_dir(repo)
            .args(["config", "--local", key, &value]))?;
    }
    // Checked with the user's own configuration, which a push runs with.
    let effective =
        run(git_command()
            .current_dir(repo)
            .args(["rev-parse", "--git-path", "hooks"]))?;
    let effective = repo.join(effective);
    if !same_path(&effective, &hooks_dir) {
        return Err(miette!(
            "git would run hooks from {} instead of {}, so pushes would not be imported",
            effective.display(),
            hooks_dir.display()
        ));
    }
    Ok(())
}

/// Repository settings for a jj clone. jj keeps them outside the repository, keyed by an id stored
/// in it, so they survive moving the clone into place.
fn setup_jj(clone: &Path, spares_command: &str) -> Result<(), Error> {
    // Only files spares wrote can be pushed, so new files are not tracked automatically. Otherwise
    // the next jj command would commit an editor's backup file, and the push would be rejected.
    run(Command::new("jj").current_dir(clone).args([
        "config",
        "set",
        "--repo",
        "snapshot.auto-track",
        "none()",
    ]))?;
    let config_path = PathBuf::from(run(Command::new("jj")
        .current_dir(clone)
        .args(["config", "path", "--repo"]))?);
    let config = std::fs::read_to_string(&config_path).unwrap_or_default();
    match with_jj_aliases(&config, &jj_aliases(spares_command)) {
        Some(new_config) => {
            if new_config != config {
                std::fs::write(&config_path, new_config).map_err(|e| miette!("{}", e))?;
            }
        }
        None => println!(
            "{} has spares aliases (`spares-push`) without the markers spares writes, so they were left alone. Delete them and run `spares note init-vcs --repair-hooks` to get the current ones.",
            config_path.display()
        ),
    }
    Ok(())
}

/// Brings over what the old notes directory had that the clone does not: edited notes go into a
/// separate commit to review, and other files are copied as untracked files.
#[expect(clippy::too_many_lines)]
fn carry_over(backup: &Path, notes: &Path, jj: bool) -> Result<(), Error> {
    let mut local_edits: Vec<PathBuf> = Vec::new();
    let mut untracked: Vec<PathBuf> = Vec::new();
    let mut same_note = 0;
    for entry in WalkDir::new(backup)
        .into_iter()
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | ".jj")))
    {
        let entry = entry.map_err(|e| miette!("{}", e))?;
        if !entry.file_type().is_file() || entry.file_name() == ".DS_Store" {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(backup)
            .map_err(|e| miette!("{}", e))?
            .to_path_buf();
        let clone_file = notes.join(&rel);
        if !clone_file.exists() {
            untracked.push(rel);
            continue;
        }
        let old = std::fs::read(entry.path()).map_err(|e| miette!("{}", e))?;
        let new = std::fs::read(&clone_file).map_err(|e| miette!("{}", e))?;
        if old == new {
            continue;
        }
        // Files that differ only in the hash line, the linked-notes section or formatting hold
        // the same note.
        if let Some((parser, id)) = notes_vcs::parse_note_rel_path(&rel)
            && let (Ok(old), Ok(new)) = (std::str::from_utf8(&old), std::str::from_utf8(&new))
            && let Some(old_fields) = notes_vcs::file_note_fields(parser.as_ref(), old, id)
            && let Some(new_fields) = notes_vcs::file_note_fields(parser.as_ref(), new, id)
            && notes_vcs::same_note(parser.as_ref(), id, &old_fields, &new_fields)
        {
            same_note += 1;
            continue;
        }
        local_edits.push(rel);
    }

    let copy = |rel: &Path| -> Result<(), Error> {
        let target = notes.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| miette!("{}", e))?;
        }
        std::fs::copy(backup.join(rel), &target).map_err(|e| miette!("{}", e))?;
        Ok(())
    };

    let mut local_edits_ref = None;
    if !local_edits.is_empty() {
        if jj {
            run(Command::new("jj")
                .current_dir(notes)
                .args(["new", "main@origin"]))?;
            for rel in &local_edits {
                copy(rel)?;
            }
            run(Command::new("jj").current_dir(notes).args([
                "describe",
                "-m",
                LOCAL_EDITS_MESSAGE,
            ]))?;
            let change = run(Command::new("jj").current_dir(notes).args([
                "log",
                "--no-graph",
                "-r",
                "@",
                "-T",
                "change_id.short()",
            ]))?;
            run(Command::new("jj")
                .current_dir(notes)
                .args(["new", "main@origin"]))?;
            local_edits_ref = Some(format!("jj change {change}"));
        } else {
            run(git_command()
                .current_dir(notes)
                .args(["switch", "-q", "-c", LOCAL_EDITS_BRANCH]))?;
            for rel in &local_edits {
                copy(rel)?;
            }
            run(git_command()
                .current_dir(notes)
                .args(["add", "-A", "--"])
                .args(&local_edits))?;
            let committed = run(git_command().current_dir(notes).args([
                "commit",
                "-q",
                "-m",
                LOCAL_EDITS_MESSAGE,
            ]));
            if committed.is_err() {
                // No identity configured: commit as spares rather than lose the edits.
                run(git_command().current_dir(notes).args([
                    "-c",
                    "user.name=spares",
                    "-c",
                    "user.email=spares@localhost",
                    "commit",
                    "-q",
                    "-m",
                    LOCAL_EDITS_MESSAGE,
                ]))?;
            }
            run(git_command()
                .current_dir(notes)
                .args(["switch", "-q", "main"]))?;
            local_edits_ref = Some(format!("git branch {LOCAL_EDITS_BRANCH}"));
        }
    }
    // Copied after switching back, so they stay untracked on `main`.
    for rel in &untracked {
        copy(rel)?;
    }

    println!();
    if same_note > 0 {
        println!(
            "{same_note} note file(s) in the old directory only differed in formatting, the hash line or linked notes."
        );
    }
    if let Some(local_edits_ref) = local_edits_ref {
        println!(
            "{} file(s) differed from the database, and are in {}:",
            local_edits.len(),
            local_edits_ref.bold()
        );
        for rel in &local_edits {
            println!("  {}", rel.display());
        }
        println!(
            "{}",
            "A file differs either because you edited it without syncing, or because spares changed the note without rewriting the file (undo, tag rename). Pushing the second kind would revert that change, so review them first:".yellow()
        );
        if jj {
            println!("  jj diff -r <change>      # then rebase what you want onto main and push");
        } else {
            println!(
                "  git diff main {LOCAL_EDITS_BRANCH}    # then cherry-pick what you want and push"
            );
        }
    }
    if !untracked.is_empty() {
        println!(
            "{} other file(s) were copied into the clone as untracked files:",
            untracked.len()
        );
        for rel in &untracked {
            println!("  {}", rel.display());
        }
    }
    Ok(())
}

fn print_workflow(dirs: &Dirs, backup: Option<&Path>, jj: bool) {
    println!();
    println!(
        "{} is now a {} clone of {}.",
        dirs.notes.display(),
        if jj { "jj" } else { "git" },
        dirs.repo.display()
    );
    if jj {
        println!(
            "\n  jj spares-pull        # get spares' changes, rebase your work onto them\n  jj new main@origin    # start a change (e.g. for an LLM to edit)\n  jj diff               # review\n  jj abandon            # discard it\n  jj describe -m '…' && jj spares-push   # import it into spares"
        );
    } else {
        println!(
            "\n  git pull --rebase                 # get spares' changes\n  git switch -c edit origin/main    # start a branch (e.g. for an LLM to edit)\n  git diff origin/main              # review\n  git commit -am '…' && git push origin HEAD:main   # import it into spares"
        );
    }
    println!(
        "\nCreate notes by pushing `<parser>/new-<name>.<ext>` files, and delete them by deleting their files. Never edit {} by hand.",
        dirs.repo.display()
    );
    if let Some(backup) = backup {
        println!(
            "The old notes directory is kept at {}. Delete it once you have reviewed the above.",
            backup.display()
        );
    }
    let gitignore = dirs.data.join(".gitignore");
    if std::fs::read_to_string(&gitignore).is_ok_and(|s| s.contains("notes/")) {
        println!(
            "{} lists `notes/`; you may want to add `{}/` and `notes.pre-vcs-*/` too.",
            gitignore.display(),
            notes_vcs::SERVER_REPO_DIR_NAME
        );
    }
}

#[cfg(test)]
mod tests {
    use super::JJ_ALIASES_BEGIN;
    use super::OLD_JJ_ALIASES;
    use super::jj_aliases;
    use super::with_jj_aliases;

    #[test]
    fn test_with_jj_aliases() {
        let aliases = jj_aliases("spares");
        assert!(aliases.contains("spares-anki-push"));
        assert!(aliases.contains("  spares note anki-refresh\n"));
        let user = "[user]\nname = \"x\"\n";

        // Added once, then replaced in place.
        let added = with_jj_aliases(user, &aliases).unwrap();
        assert!(added.starts_with(user));
        let newer = jj_aliases("/new/spares");
        let replaced = with_jj_aliases(&added, &newer).unwrap();
        assert_eq!(replaced, format!("{user}\n{newer}"));
        assert_eq!(with_jj_aliases(&replaced, &newer).unwrap(), replaced);

        // The unmarked aliases of earlier versions are replaced.
        let old = format!("{user}{OLD_JJ_ALIASES}");
        let migrated = with_jj_aliases(&old, &aliases).unwrap();
        assert_eq!(migrated, format!("{user}\n{aliases}"));
        assert_eq!(migrated.matches("spares-push =").count(), 1);

        // Aliases someone edited are left alone.
        let edited = format!("{user}[aliases]\nspares-push = [\"x\"]\n");
        assert_eq!(with_jj_aliases(&edited, &aliases), None);
        assert!(!edited.contains(JJ_ALIASES_BEGIN));
    }
}
