//! `spares deploy init`: the first deployment, and setting up this machine to sync with it.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use colored::Colorize;
use inquire::CustomType;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;
use reqwest::StatusCode;
use serde_json::Value;
use spares_core::config::get_data_dir;
use spares_core::config::get_external_config_file;
use spares_core::config::read_external_config;
use spares_core::config::set_sync_values;
use spares_core::sync::run::SyncMode;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePool;

use super::bundle::Bundle;
use super::bundle::REMOTE_DIR;
use super::bundle::Server;
use super::bundle::Target;
use super::progress::Progress;
use super::progress::without_footer;
use super::repo_dir;
use super::shell::Ssh;
use super::shell::command_exists;
use super::shell::confirm;
use super::shell::pause;
use super::shell::quote;
use super::shell::run;
use super::shell::warn;
use super::upload;
use crate::local::LocalEnv as EnvironmentConfig;
use crate::sync::sync_cloud;
use crate::utils::local_timezone;

pub(super) struct InitOptions {
    pub(super) server: String,
    pub(super) user: Option<String>,
    pub(super) domain: Option<String>,
    pub(super) no_latex: bool,
}

#[allow(clippy::too_many_lines)]
pub(super) async fn init(
    env_config: &EnvironmentConfig,
    options: InitOptions,
) -> Result<(), Error> {
    let InitOptions {
        server,
        user,
        domain,
        no_latex,
    } = options;
    let me = std::env::var("USER").map_err(|_| miette!("USER is not set."))?;
    let home = std::env::var("HOME").map_err(|_| miette!("HOME is not set."))?;
    let user = user.unwrap_or_else(|| me.clone());
    let repo = repo_dir()?;
    let data_dir = get_data_dir();
    let db = database_path(&env_config.database_url)?;
    if !db.is_file() {
        return Err(miette!(
            "No database at {}. Create a host with `spares host init`, and import a collection \
             with `spares host import`.",
            db.display()
        ));
    }
    let (host_root, account_id, account) = crate::local::account().await?;
    for command in ["ssh", "scp", "rsync"] {
        if !command_exists(command) {
            return Err(miette!("{command} is needed."));
        }
    }
    if domain.is_none()
        && !command_exists("tailscale")
        && !Path::new("/Applications/Tailscale.app").is_dir()
    {
        warn(
            "Tailscale is not installed here. Install it (https://tailscale.com/download) and log \
             in, as this machine reaches the server over it.",
        );
        if !confirm("Continue anyway?")? {
            return Ok(());
        }
    }
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::from_str(&env_config.database_url).into_diagnostic()?,
    )
    .await
    .map_err(|e| miette!("Failed to open the database: {e}"))?;
    let ssh = Ssh::new();
    let root = format!("root@{server}");
    let mut steps = vec![
        "Backing up the database",
        "Building",
        "Uploading",
        "Setting up the server",
        "Setting up this machine",
        "Restarting the local server",
        "Uploading the database",
        "Checking",
    ];
    if domain.is_none() {
        steps.push("Closing the server's public ports");
    }
    let progress = Progress::start("init", &steps);

    progress.step("Backing up the database", "");
    let backup = data_dir.join(format!(
        "spares-main.before-deploy-{}.sqlite",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    sqlx::query("VACUUM INTO ?")
        .bind(backup.to_string_lossy())
        .execute(&pool)
        .await
        .map_err(|e| miette!("Failed to back up the database: {e}"))?;
    println!("Saved {}", backup.display());

    let bundle = Bundle::build(&progress, &repo, &root, || {
        // rsync uploads the bundle, before setup_vps.sh installs it. Fresh Debian images lack it.
        let arch = ssh.output(
            &root,
            "command -v rsync >/dev/null || \
             { apt-get update -qq && apt-get install -y -qq rsync; } >&2; uname -m",
        )?;
        Ok(Server {
            target: Target::of_arch(&arch)?,
            with_pandoc: !no_latex,
        })
    })
    .await?;

    progress.step("Uploading", &format!("to {root}"));
    upload(&ssh, &bundle, &root)?;
    drop(bundle);

    progress.step("Setting up the server", &format!("as {root}"));
    if !no_latex {
        println!("Installing LaTeX takes a while.");
    }
    match &domain {
        Some(domain) => println!("Point {domain}'s DNS at the server first."),
        None => {
            println!("If it prints a Tailscale link, open it to add the server to your tailnet.");
        }
    }
    let mut args = vec![
        "--user".to_string(),
        user.clone(),
        "--root".to_string(),
        host_root.to_string_lossy().into_owned(),
        "--account".to_string(),
        account.clone(),
        "--account-id".to_string(),
        account_id.clone(),
        "--local-home".to_string(),
        home.clone(),
    ];
    match &domain {
        Some(domain) => args.extend(["--domain".to_string(), domain.clone()]),
        None => args.push("--tailscale".to_string()),
    }
    if no_latex {
        args.push("--no-latex".to_string());
    }
    match local_timezone() {
        Some(zone) => args.extend(["--timezone".to_string(), zone]),
        None => warn("Could not tell this machine's time zone, so the server keeps its own."),
    }
    let args = args
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let mut command = ssh.command();
    command.args([
        "-t",
        &root,
        &format!("bash {REMOTE_DIR}/setup_vps.sh --bundle \"$HOME/{REMOTE_DIR}\" {args}"),
    ]);
    progress.run(command, None)?;

    let url = ssh.output(&root, "cat /etc/spares/url")?;
    let Some(host) = url.strip_prefix("https://") else {
        return Err(miette!("Could not read the server's address."));
    };
    let host = host.to_string();
    println!("The server is at {url}");

    progress.step("Setting up this machine", "");
    let interval = set_up_config(&user, &host)?;

    progress.step("Logging in to the server", "");
    let base_url = spares_core::config::host_client().base_url;
    let client = crate::local_client()?;
    wait_for_local_host(&base_url, &client).await?;
    let status: Value = client
        .get(format!("{base_url}/api/sync/status"))
        .send()
        .await
        .into_diagnostic()?
        .json()
        .await
        .into_diagnostic()?;
    if status["sync_url"].as_str() == Some(url.as_str())
        && status["sync_token_set"].as_bool() == Some(true)
    {
        println!("This host's copy syncs with {url} already.");
    } else {
        println!("Log in as {account}, with the password you gave the server.");
        crate::sync::upstream_login(&base_url, &client, &url, &account, None)
            .await
            .map_err(|e| miette!("{e}"))?;
    }

    progress.step("Uploading the database", "");
    let user_host = format!("{user}@{host}");
    run(ssh
        .command()
        .args(["-o", "StrictHostKeyChecking=accept-new", &user_host, "true"]))?;
    if confirm("This replaces the server's database with this one. Upload?")? {
        sync_cloud(&base_url, &client, SyncMode::FullUpload)
            .await
            .map_err(|e| miette!("{e}"))?;
    }

    progress.step("Checking", "");
    let here: String = sqlx::query_scalar(NOTE_COUNTS)
        .fetch_one(&pool)
        .await
        .map_err(|e| miette!("{e}"))?;
    // The same path as here: the host there has the same root
    let server_db = spares_core::collection::paths().database();
    let there = ssh.output(
        &user_host,
        &format!(
            "sqlite3 {} {}",
            quote(&server_db.to_string_lossy()),
            quote(NOTE_COUNTS)
        ),
    )?;
    println!("Here:   {here}");
    println!("Server: {there}");
    if here != there {
        warn("They differ. Run `spares sync cloud` and compare again.");
    }
    sync_cloud(&base_url, &client, SyncMode::Normal)
        .await
        .map_err(|e| miette!("{e}"))?;

    if domain.is_none() {
        progress.step("Closing the server's public ports", "");
        println!(
            "You reached the server over Tailscale above. This allows only Tailscale in, so SSH \
             to its\npublic address stops working; log in with `ssh {user_host}` instead. If \
             Tailscale ever\nbreaks, use the VPS provider's web console, which can also reset \
             root's password."
        );
        if confirm("Close them?")? {
            let closed = ssh.output(
                &root,
                "command -v ufw >/dev/null || apt-get install -y -q ufw >/dev/null
                 ufw default deny incoming >/dev/null && ufw default allow outgoing >/dev/null
                 ufw allow in on tailscale0 >/dev/null
                 ufw allow 41641/udp >/dev/null  # direct Tailscale connections
                 ufw --force enable",
            );
            match closed {
                Ok(out) => println!("{out}"),
                Err(e) => warn(&format!("Could not close them: {e}")),
            }
        }
    }

    progress.end_step();
    println!(
        "\n{}\nOn your phone, with Tailscale on, open {url}, log in as {account} and add it to the home
screen.

  spares sync cloud       sync both ways and copy files (sync.interval_minutes = {interval}
                          syncs in the background, without copying files)
  spares sync conflicts   resolve notes tagged sync-conflict
  spares deploy update    update the server",
        "Next".bold()
    );
    progress.finish();
    Ok(())
}

const NOTE_COUNTS: &str = "SELECT (SELECT COUNT(*) FROM note) || ' notes, ' || \
                           (SELECT COUNT(*) FROM card) || ' cards, ' || \
                           (SELECT COUNT(*) FROM review_log) || ' reviews';";

/// The file of a `sqlite:` database URL.
fn database_path(database_url: &str) -> Result<PathBuf, Error> {
    let path = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .ok_or_else(|| miette!("Not a SQLite database: {database_url}"))?;
    Ok(PathBuf::from(path.split('?').next().unwrap_or(path)))
}

/// Sets `remote_host` and `interval_minutes` (asked) in the [sync] table of config.toml. Returns
/// the interval.
fn set_up_config(user: &str, host: &str) -> Result<u64, Error> {
    let config = read_external_config().map_err(|e| miette!("{e}"))?;
    // 0, the default, means never, so it is only kept once syncing was set up
    let default = if config.sync.url.is_some() {
        config.sync.interval_minutes
    } else {
        15
    };
    let interval = without_footer(|| {
        CustomType::<u64>::new("Sync in the background every how many minutes (0 for never)?")
            .with_default(default)
            .with_error_message("Type a whole number.")
            .prompt()
    })
    .map_err(|e| miette!("{e}"))?;
    let file = get_external_config_file();
    let old = fs::read_to_string(&file).unwrap_or_default();
    let new = set_sync_values(
        &old,
        [
            ("remote_host", format!("{user}@{host}").into()),
            #[allow(clippy::cast_possible_wrap)]
            ("interval_minutes", (interval as i64).into()),
        ],
    )
    .map_err(|e| miette!("{e}"))?;
    if new != old {
        let had_file = file.exists();
        if had_file {
            fs::copy(&file, file.with_extension("toml.bak")).into_diagnostic()?;
        }
        fs::write(&file, new).into_diagnostic()?;
        println!(
            "Set remote_host and interval_minutes in the [sync] table of {}.",
            file.display()
        );
        if had_file {
            println!("The old one is config.toml.bak.");
        }
    }
    Ok(interval)
}

/// Waits until this machine's host answers, which `spares host init` set up.
async fn wait_for_local_host(base_url: &str, client: &reqwest::Client) -> Result<(), Error> {
    loop {
        match client.get(format!("{base_url}/api/me")).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                warn(&format!(
                    "The host at {base_url} doesn't know this machine's CLI. Run `spares login`."
                ));
            }
            _ => warn(&format!(
                "No spares host answers at {base_url}. Start it (`spares_server`)."
            )),
        }
        pause("Press Enter once it is running. ")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_path_drops_the_scheme_and_options() {
        assert_eq!(
            database_path("sqlite:///Users/me/spares-main.sqlite").unwrap(),
            PathBuf::from("/Users/me/spares-main.sqlite")
        );
        assert_eq!(
            database_path("sqlite:/tmp/db.sqlite?mode=rwc").unwrap(),
            PathBuf::from("/tmp/db.sqlite")
        );
        assert!(database_path("postgres://x").is_err());
    }
}
