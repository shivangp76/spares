//! `spares migrate ids`: renumbers notes from numeric ids to letter ids, once. See
//! [`spares_core::ids::migrate`].

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;

use chrono::Local;
use miette::Error;
use miette::miette;
use spares_core::config::REPLICA_ENV;
use spares_core::config::get_data_dir;
use spares_core::ids::migrate::IdMapping;
use spares_core::ids::migrate::MigrationPlan;
use spares_core::ids::migrate::backup_database;
use spares_core::ids::migrate::count_files;
use spares_core::ids::migrate::estimate_render_time;
use spares_core::ids::migrate::files_pending;
use spares_core::ids::migrate::id_map_path;
use spares_core::ids::migrate::is_migrated;
use spares_core::ids::migrate::mismatched_note_files;
use spares_core::ids::migrate::open_copy;
use spares_core::ids::migrate::plan;
use spares_core::ids::migrate::read_id_map;
use spares_core::ids::migrate::regenerate_files;
use spares_core::ids::migrate::rewrite_database;
use spares_core::ids::migrate::write_id_map;
use spares_core::notes_vcs;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

pub(crate) async fn migrate_ids(
    database_url: &str,
    socket_address: &str,
    dry_run: bool,
) -> Result<(), Error> {
    if let Ok(replica) = std::env::var(REPLICA_ENV)
        && replica.trim() != "0"
    {
        return Err(miette!(
            "{REPLICA_ENV} is {replica}. Migrate on the laptop (replica 0), before a second \
             replica exists."
        ));
    }
    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| miette!("{e}"))?
        .with_regexp();
    let db = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .map_err(|e| miette!("Failed to open the database {database_url}: {e}"))?;

    if is_migrated(&db).await.map_err(to_miette)? {
        if !files_pending(&db).await.map_err(to_miette)? {
            println!("Note ids are already letters. Nothing to do.");
            return Ok(());
        }
        if dry_run {
            println!(
                "The database is migrated, but its files were not regenerated yet. Run \
                 `spares migrate ids` to finish."
            );
            return Ok(());
        }
        ensure_server_stopped(socket_address).await?;
        println!("The database is migrated. Finishing its files.");
        let mapping = read_id_map(&id_map_path()).map_err(to_miette)?;
        finish(&db, &mapping).await?;
        return Ok(());
    }

    let problems = preconditions(socket_address).await;
    if !problems.is_empty() {
        let list = problems
            .iter()
            .map(|p| format!("  - {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        if !dry_run {
            return Err(miette!("Not migrating:\n{list}"));
        }
        println!("A real run would stop here:\n{list}\n");
    }

    // Everything is tried on a scratch copy first, including checking that the note files describe
    // the database, so a real run changes nothing if that fails.
    let scratch = std::env::temp_dir().join(format!("spares-migrate-ids-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).map_err(|e| miette!("{e}"))?;
    }
    std::fs::create_dir_all(&scratch).map_err(|e| miette!("{e}"))?;
    let result = try_on_copy(&db, &scratch, dry_run).await;
    let _ = std::fs::remove_dir_all(&scratch);
    let plan = result?;
    if dry_run {
        return Ok(());
    }

    migrate(&db, &plan).await
}

/// Migrates a copy of `db` in `scratch` and checks the note files against it. With `dry_run`,
/// prints what a real run would do.
async fn try_on_copy(
    db: &SqlitePool,
    scratch: &Path,
    dry_run: bool,
) -> Result<MigrationPlan, Error> {
    let copy = open_copy(db, &scratch.join("copy.sqlite"))
        .await
        .map_err(to_miette)?;
    let plan = plan(&copy).await.map_err(to_miette)?;
    rewrite_database(&copy, &plan).await.map_err(to_miette)?;
    let mismatches = mismatched_note_files(&copy, &plan.mapping)
        .await
        .map_err(to_miette)?;
    let card_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card")
        .fetch_one(&copy)
        .await
        .map_err(|e| miette!("{e}"))?;
    copy.close().await;

    if dry_run {
        print_plan(&plan, usize::try_from(card_count).unwrap_or(0), scratch);
    }
    if !mismatches.is_empty() {
        let shown = mismatches
            .iter()
            .take(20)
            .map(|m| format!("  - {m}"))
            .collect::<Vec<_>>()
            .join("\n");
        let more = mismatches.len().saturating_sub(20);
        let message = format!(
            "The note files do not describe the database, so renaming them would lose changes. \
             Import or discard them first (`spares vcs check` with the previous version).\n\
             {shown}{}",
            if more > 0 {
                format!("\n  … and {more} more")
            } else {
                String::new()
            }
        );
        if !dry_run {
            return Err(miette!("{message}"));
        }
        println!("\nA real run would stop here: {message}");
    } else if dry_run {
        println!("\nEvery note file matches the database apart from its id.");
    }
    Ok(plan)
}

fn print_plan(plan: &MigrationPlan, card_count: usize, scratch: &Path) {
    let mapping = &plan.mapping;
    let (Some(first), Some(last)) = (mapping.first(), mapping.last()) else {
        println!("There are no notes, so there is nothing to renumber.");
        return;
    };
    println!(
        "{} notes get ids {} to {} (old {} to {}).",
        mapping.len(),
        first.new,
        last.new,
        first.old,
        last.old
    );
    println!("Sample of the mapping (old -> new):");
    let step = (mapping.len() / 8).max(1);
    let mut shown: Vec<&IdMapping> = mapping.iter().step_by(step).collect();
    if shown.last().map(|m| m.old) != Some(last.old) {
        shown.push(last);
    }
    for m in shown {
        println!("  {:>6} -> {}  ({})", m.old, m.new, m.parser_name);
    }
    if plan.rewrites.is_empty() {
        println!("No `inh:` references or filtered tag queries name notes.");
    } else {
        println!("Rewritten references:");
        for rewrite in &plan.rewrites {
            println!(
                "  {}:\n    before: {}\n    after:  {}",
                rewrite.owner, rewrite.before, rewrite.after
            );
        }
    }
    for (what, count) in &plan.orphans {
        println!("Rows naming notes that no longer exist, to clean up: {count} {what}.");
    }
    println!(
        "Undo history to drop: {} events. Image occlusion render assets to drop: {}.",
        plan.events, plan.image_occlusion_assets
    );
    let counts = count_files(mapping, card_count);
    println!(
        "Files: {} note files to rename and regenerate, {} card files and {} rendered files to \
         delete, {} files to render again.",
        counts.note_files, counts.card_files, counts.rendered_files, counts.to_render
    );
    println!("Timing a few renders to estimate how long rendering takes…");
    match estimate_render_time(scratch, counts.to_render) {
        Some(estimate) => println!(
            "Rendering everything should take about {}.",
            minutes(estimate)
        ),
        None => println!("Could not time a render, so there is no estimate."),
    }
}

fn minutes(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 90 {
        format!("{secs} s")
    } else {
        format!("{} min", secs.div_ceil(60))
    }
}

async fn migrate(db: &SqlitePool, plan: &MigrationPlan) -> Result<(), Error> {
    let data_dir = get_data_dir();
    let stamp = Local::now().format("%Y%m%d-%H%M%S");

    println!("Backing up the database…");
    let backup = data_dir.join(format!("spares-main.before-note-ids-{stamp}.sqlite"));
    backup_database(db, &backup).await.map_err(to_miette)?;
    let repo = notes_vcs::server_repo_dir();
    let repo_head = notes_vcs::enabled()
        .then(|| {
            output(
                Command::new("git")
                    .current_dir(&repo)
                    .args(["rev-parse", "HEAD"]),
            )
        })
        .transpose()?;
    if let Some(head) = &repo_head {
        let path = data_dir.join(format!("notes-spares.before-note-ids-{stamp}.head"));
        std::fs::write(&path, format!("{head}\n")).map_err(|e| miette!("{e}"))?;
    }
    write_id_map(&id_map_path(), &plan.mapping).map_err(to_miette)?;
    println!("  database: {}", backup.display());
    if let Some(head) = &repo_head {
        println!("  {} was at {head}", repo.display());
    }
    println!("  old -> new ids: {}", id_map_path().display());

    // A plan made on the copy could be stale if anything changed since, which `rewrite_database`
    // checks again under its lock.
    sqlx::migrate!("../spares_core/migrations")
        .run(db)
        .await
        .map_err(|e| miette!("Failed to migrate the database: {e}"))?;
    println!("Renumbering notes in the database…");
    rewrite_database(db, plan).await.map_err(to_miette)?;

    finish(db, &plan.mapping).await?;
    println!("\nTo roll back:");
    println!(
        "  - stop the server and copy {} over the database",
        backup.display()
    );
    if let Some(head) = &repo_head {
        println!("  - git -C {} reset --hard {head}", repo.display());
    }
    println!("  - run the previous version of spares");
    Ok(())
}

/// Regenerates files from the migrated database and updates the notes clone.
async fn finish(db: &SqlitePool, mapping: &[IdMapping]) -> Result<(), Error> {
    let report = regenerate_files(db, mapping, |step| println!("{step}…"))
        .await
        .map_err(to_miette)?;
    if report.notes_commit {
        println!("Committed the renamed note files.");
    }
    println!(
        "Deleted {} old card and rendered files; rekeyed {} entries of the sync hash index; \
         rendered in {}.",
        report.deleted,
        report.hash_index_keys,
        minutes(report.render_duration)
    );
    if let Some(error) = &report.render_error {
        println!(
            "Some notes failed to render: {error}\nThey are rendered again when they change, or \
             with `spares note generate --render`."
        );
    }
    update_clone()?;
    println!(
        "\nDone. Set {REPLICA_ENV}=0 and SPARES_SYNC_ROLE=client for the server, and open the web \
         app once online so it drops offline copies with old file names."
    );
    Ok(())
}

/// Things that must hold before migrating. Empty if all do.
async fn preconditions(socket_address: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(e) = ensure_server_stopped(socket_address).await {
        problems.push(e.to_string());
    }
    if notes_vcs::enabled() {
        let repo = notes_vcs::server_repo_dir();
        match output(Command::new("git").current_dir(&repo).args([
            "status",
            "--porcelain",
            "--untracked-files=all",
        ])) {
            Ok(status) if status.is_empty() => {}
            Ok(_) => problems.push(format!(
                "{} has uncommitted changes. Start the previous server once to commit or revert \
                 them.",
                repo.display()
            )),
            Err(e) => problems.push(e.to_string()),
        }
        if notes_vcs::status().receive_in_progress {
            problems.push(
                "A push into spares did not finish. Start the previous server once to settle it."
                    .to_string(),
            );
        }
        if let Err(e) = check_clone(&repo) {
            problems.push(e.to_string());
        }
    }
    problems
}

async fn ensure_server_stopped(socket_address: &str) -> Result<(), Error> {
    let connect = tokio::net::TcpStream::connect(socket_address);
    if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_secs(2), connect).await {
        return Err(miette!(
            "A spares server is running at {socket_address}. Stop it first."
        ));
    }
    Ok(())
}

/// Your clone of the notes repository, if there is one.
fn clone_dir() -> Option<PathBuf> {
    let dir = get_data_dir().join("notes");
    dir.join(".git").exists().then_some(dir)
}

/// The clone must have nothing that is not in the server's repository, since its files are about
/// to be renamed there.
fn check_clone(repo: &Path) -> Result<(), Error> {
    let Some(clone) = clone_dir() else {
        return Ok(());
    };
    let head = output(
        Command::new("git")
            .current_dir(repo)
            .args(["rev-parse", "HEAD"]),
    )?;
    if clone.join(".jj").exists() {
        let jj = |args: &[&str]| output(Command::new("jj").current_dir(&clone).args(args));
        jj(&["git", "fetch"])?;
        let empty = jj(&["log", "--no-graph", "-r", "@", "-T", "empty"])?;
        let parent = jj(&["log", "--no-graph", "-r", "@-", "-T", "commit_id"])?;
        if empty != "true" || parent != head {
            return Err(miette!(
                "{} has changes that spares does not have yet (its working copy must be empty, \
                 on main). Push or abandon them first.",
                clone.display()
            ));
        }
        let other = jj(&[
            "log",
            "--no-graph",
            "-r",
            "mutable() ~ @",
            "-T",
            "change_id.short() ++ \" \"",
        ])?;
        if !other.is_empty() {
            return Err(miette!(
                "{} has other unpushed changes ({other}), whose file names would no longer match. \
                 Push or abandon them first.",
                clone.display()
            ));
        }
    } else {
        let git = |args: &[&str]| output(Command::new("git").current_dir(&clone).args(args));
        let status = git(&["status", "--porcelain", "--untracked-files=all"])?;
        if !status.is_empty() || git(&["rev-parse", "HEAD"])? != head {
            return Err(miette!(
                "{} has changes that spares does not have yet. Push or discard them first.",
                clone.display()
            ));
        }
    }
    Ok(())
}

/// Brings the clone up to the renumbering commit, as `jj spares-pull` (or `git pull`) would.
fn update_clone() -> Result<(), Error> {
    let Some(clone) = clone_dir() else {
        return Ok(());
    };
    println!("Updating {}…", clone.display());
    if clone.join(".jj").exists() {
        output(
            Command::new("jj")
                .current_dir(&clone)
                .args(["git", "fetch"]),
        )?;
        output(Command::new("jj").current_dir(&clone).args([
            "rebase",
            "-b",
            "@",
            "-d",
            "main@origin",
        ]))?;
    } else {
        output(
            Command::new("git")
                .current_dir(&clone)
                .args(["pull", "--ff-only", "-q"]),
        )?;
    }
    Ok(())
}

/// Runs `command`, returning its trimmed standard output, or an error with its standard error.
fn output(command: &mut Command) -> Result<String, Error> {
    let out = command
        .output()
        .map_err(|e| miette!("Failed to run {command:?}: {e}"))?;
    if !out.status.success() {
        return Err(miette!(
            "{command:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[expect(clippy::needless_pass_by_value, reason = "for `map_err`")]
fn to_miette(e: spares_core::Error) -> Error {
    miette!("{e}")
}
