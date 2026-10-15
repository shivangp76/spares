//! `spares deploy init`: the first deployment, and setting up this machine to sync with it.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

use colored::Colorize;
use inquire::CustomType;
use inquire::Password;
use inquire::PasswordDisplayMode;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;
use reqwest::StatusCode;
use serde_json::Value;
use spares_core::config::EnvironmentConfig;
use spares_core::config::get_config_dir;
use spares_core::config::get_data_dir;
use spares_core::config::get_external_config_file;
use spares_core::config::local_api_key;
use spares_core::config::read_external_config;
use spares_core::config::set_sync_values;
use spares_core::sync::run::SyncMode;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePool;

use super::bundle::Bundle;
use super::bundle::REMOTE_DIR;
use super::bundle::Target;
use super::local_timezone;
use super::progress::Progress;
use super::progress::without_footer;
use super::repo_dir;
use super::shell::Ssh;
use super::shell::command_exists;
use super::shell::confirm;
use super::shell::output;
use super::shell::pause;
use super::shell::quote;
use super::shell::run;
use super::shell::warn;
use super::upload;
use crate::sync::sync_cloud;

/// Where the local server's own API key is kept, if it has one and `SPARES_API_KEY` is not set
const LOCAL_KEYCHAIN_SERVICE: &str = "spares-api-key";
/// The Keychain item holding the server's API key
const KEYCHAIN_SERVICE: &str = "spares-sync";

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
            "No database at {}. Set SPARES_DATA_DIR if it is elsewhere.",
            db.display()
        ));
    }
    for command in ["ssh", "scp", "rsync", "security"] {
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
        "Connecting",
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

    progress.step("Connecting", &format!("to {root}"));
    // For uploading the bundle, before setup_vps.sh installs it. Fresh Debian images lack it.
    ssh.output(
        &root,
        "command -v rsync >/dev/null || \
         { apt-get update -qq && apt-get install -y -qq rsync; } >&2",
    )?;
    let target = Target::of(&ssh, &root)?;
    let bundle = Bundle::build(&progress, &repo, target, !no_latex).await?;

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
        "--data-dir".to_string(),
        data_dir.to_string_lossy().into_owned(),
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
    progress.run(command)?;

    let url = ssh.output(&root, "cat /etc/spares/url")?;
    let api_key = ssh.output(
        &root,
        "sed -n 's/^SPARES_API_KEY=//p' /etc/spares/spares.env",
    )?;
    let Some(host) = url.strip_prefix("https://").filter(|_| !api_key.is_empty()) else {
        return Err(miette!("Could not read the server's address and API key."));
    };
    let host = host.to_string();
    println!("The server is at {url}");

    progress.step("Setting up this machine", "");
    run(Command::new("security").args([
        "add-generic-password",
        "-U",
        "-s",
        KEYCHAIN_SERVICE,
        "-a",
        &me,
        "-w",
        &api_key,
    ]))?;
    println!("Stored the server's API key in the Keychain (service \"{KEYCHAIN_SERVICE}\").");
    let interval = set_up_config(&user, &host, &url)?;
    set_up_shell_file(&url)?;

    progress.step("Restarting the local server", "");
    println!("Restart spares_serve in a new terminal, so it picks up the new settings.");
    let local_address = env_config.socket_address.replace("0.0.0.0", "127.0.0.1");
    let base_url = format!("http://{local_address}");
    let local_key = wait_for_local_server(&base_url, &me, &url).await?;
    let client = crate::client_with_key(local_key.as_deref())?;

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
    let server_db = get_data_dir().join("spares-main.sqlite");
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
        "\n{}\nOn your phone, with Tailscale on, open {url}, log in with the API key
(security find-generic-password -s {KEYCHAIN_SERVICE} -w) and add it to the home screen.

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

/// Sets `remote_host`, `url` and `interval_minutes` (asked) in the [sync] table of config.toml.
/// Returns the interval.
fn set_up_config(user: &str, host: &str, url: &str) -> Result<u64, Error> {
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
            ("url", url.into()),
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
            "Set remote_host, url and interval_minutes in the [sync] table of {}.",
            file.display()
        );
        if had_file {
            println!("The old one is config.toml.bak.");
        }
    }
    Ok(interval)
}

/// Offers to add what the local server needs in its environment to spares.zsh.
fn set_up_shell_file(url: &str) -> Result<(), Error> {
    let shell_file = get_config_dir().join("spares.zsh");
    let contents = fs::read_to_string(&shell_file).unwrap_or_default();
    let missing: Vec<String> = [
        ("SPARES_REPLICA=", "export SPARES_REPLICA=0".to_string()),
        (
            "SPARES_SYNC_ROLE=",
            "export SPARES_SYNC_ROLE=client".to_string(),
        ),
        (
            "SPARES_SYNC_API_KEY=",
            format!(
                "export SPARES_SYNC_API_KEY=\"$(security find-generic-password -s \
                 {KEYCHAIN_SERVICE} -w 2>/dev/null)\""
            ),
        ),
    ]
    .into_iter()
    .filter(|(name, _)| !contents.contains(name))
    .map(|(_, line)| line)
    .collect();
    if missing.is_empty() {
        return Ok(());
    }
    println!("The local server needs these where it starts:");
    for line in &missing {
        println!("  {line}");
    }
    if confirm(&format!("Add them to {}?", shell_file.display()))? {
        let mut new = contents;
        let _ = write!(
            new,
            "\n# Two-way sync with the server ({url}). This machine is replica 0; the key is in the \
             Keychain.\n{}\n",
            missing.join("\n")
        );
        fs::write(&shell_file, new).into_diagnostic()?;
    } else {
        println!("Add them yourself, then continue.");
    }
    Ok(())
}

/// Waits until the local server runs as the client of the server at `url`. Returns the local
/// server's API key, if it wants one.
async fn wait_for_local_server(
    base_url: &str,
    me: &str,
    url: &str,
) -> Result<Option<String>, Error> {
    let mut key = local_api_key();
    let mut tried_keychain = false;
    let mut wait = true;
    loop {
        if wait {
            pause("Press Enter once it is running. ")?;
        }
        wait = true;
        let client = crate::client_with_key(key.as_deref())?;
        let response = client
            .get(format!("{base_url}/api/sync/status"))
            .send()
            .await;
        let Ok(response) = response else {
            warn(&format!("No spares server answers at {base_url}."));
            continue;
        };
        if response.status() == StatusCode::UNAUTHORIZED {
            if !tried_keychain
                && let Ok(found) = output(
                    Command::new("security")
                        .args([
                            "find-generic-password",
                            "-a",
                            me,
                            "-s",
                            LOCAL_KEYCHAIN_SERVICE,
                            "-w",
                        ])
                        .stderr(std::process::Stdio::null()),
                )
            {
                println!(
                    "Using the local server's API key from the Keychain (service \
                     \"{LOCAL_KEYCHAIN_SERVICE}\")."
                );
                key = Some(found);
            } else {
                warn("The local server wants its API key (its SPARES_API_KEY).");
                let pasted = without_footer(|| {
                    Password::new("Paste it here:")
                        .without_confirmation()
                        .with_display_mode(PasswordDisplayMode::Hidden)
                        .prompt()
                })
                .map_err(|e| miette!("{e}"))?;
                key = Some(pasted.split_whitespace().collect());
            }
            tried_keychain = true;
            wait = false;
            continue;
        }
        let status: Value = match response.error_for_status() {
            Ok(response) => response.json().await.unwrap_or_default(),
            Err(_) => Value::Null,
        };
        let field = |name: &str| status.get(name).cloned().unwrap_or_default();
        match (field("role").as_str(), field("sync_url").as_str()) {
            (None, _) => warn(&format!("No spares server answers at {base_url}.")),
            (Some(role), _) if role != "client" => {
                warn(&format!(
                    "It runs as the {role}, not the client. Check SPARES_SYNC_ROLE."
                ));
            }
            (_, sync_url) if sync_url != Some(url) => warn(&format!(
                "Its sync_url is \"{}\", not {url}.",
                sync_url.unwrap_or_default()
            )),
            _ if field("sync_api_key_set") != Value::Bool(true) => {
                warn("It has no SPARES_SYNC_API_KEY. Start it from a new terminal.");
            }
            _ => return Ok(key),
        }
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
