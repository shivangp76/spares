//! `spares deploy`: deploys spares to a VPS as a host with your account, the root of your
//! collection's sync chain, and makes this machine's host's copy (the laptop, replica 0) sync with
//! it as one of your devices. `scripts/setup_vps.sh` does the server's side; this uploads it and
//! runs it there.

mod bundle;
mod init;
mod progress;
mod shell;

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use bundle::Bundle;
use bundle::REMOTE_DIR;
use bundle::Server;
use bundle::Target;
use clap::Args;
use clap::Subcommand;
use colored::Colorize;
use miette::Error;
use miette::miette;
use progress::Progress;
use shell::Ssh;
use shell::quote;
use spares_core::config::read_external_config;

use crate::local::LocalEnv as EnvironmentConfig;
use crate::utils::local_timezone;

#[derive(Args, Debug)]
pub(crate) struct DeployArgs {
    #[command(subcommand)]
    pub(crate) command: DeployCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DeployCommands {
    /// First deployment, to a fresh Debian or Ubuntu VPS that `ssh root@<server>` reaches.
    ///
    /// <server> is its IP with your SSH key, or its Tailscale name if it joined your tailnet when
    /// created (needed when this machine cannot reach its address, e.g. an IPv6-only server). It
    /// can be re-run, and asks before replacing the server's database or closing its ports:
    ///
    /// 1. backs up the database;
    ///
    /// 2. builds spares for the server and runs `setup_vps.sh` there as root, which creates the
    ///    user (with sudo there without a password) and, if needed, prints a link to join your
    ///    tailnet;
    ///
    /// 3. creates the host there, at the same root as this machine's, with your account (asking
    ///    for its password there);
    ///
    /// 4. sets `remote_host` and `interval_minutes` in the [sync] table of config.toml, logs this
    ///    host's copy of your collection in to the server's (`spares sync login`), uploads it
    ///    (`spares sync cloud --full-upload`) and checks that both sides have the same notes;
    ///
    /// 5. closes the server's public ports, leaving only Tailscale (not with --domain).
    ///
    /// The server is reached over Tailscale unless --domain is given, so install Tailscale on
    /// this machine and your phone, logged in to the same account, first.
    Init {
        /// root@<server> is what SSH logs in to
        server: String,
        /// The user spares runs as on the server [default: yours]
        #[arg(long)]
        user: Option<String>,
        /// Serve over HTTPS at this domain with Caddy, instead of on your tailnet
        #[arg(long)]
        domain: Option<String>,
        /// Skip LaTeX and pandoc on the server, which only markdown and LaTeX notes need
        #[arg(long)]
        no_latex: bool,
    },
    /// Builds this checkout and installs it on the server (with sudo there, which `init` lets
    /// your user use without a password), upgrading the server's packages and Tailscale first.
    ///
    /// It removes what the upgrade no longer needs, such as old kernels, and says when the server
    /// needs a reboot to finish it.
    ///
    /// Restart the local server at the same time: the two refuse to sync across database
    /// versions.
    Update,
    /// Prints a script for the VPS provider's "cloud-init user data" (or "startup script") field.
    ///
    /// It joins the new server to your tailnet as <name>, so `init` can reach it as root@<name>
    /// over Tailscale SSH. Use it when this machine cannot reach the server's address, e.g. an
    /// IPv6-only server and no IPv6 here.
    CloudInit {
        #[arg(default_value = "spares")]
        name: String,
    },
}

pub(crate) async fn handle(args: DeployArgs, env_config: &EnvironmentConfig) -> Result<(), Error> {
    match args.command {
        DeployCommands::Init {
            server,
            user,
            domain,
            no_latex,
        } => {
            init::init(
                env_config,
                init::InitOptions {
                    server,
                    user,
                    domain,
                    no_latex,
                },
            )
            .await
        }
        DeployCommands::Update => update().await,
        DeployCommands::CloudInit { name } => {
            cloud_init(&name);
            Ok(())
        }
    }
}

async fn update() -> Result<(), Error> {
    let repo = repo_dir()?;
    let config = read_external_config().map_err(|e| miette!("{e}"))?;
    let remote_host = config.sync.remote_host.ok_or_else(|| {
        miette!("sync.remote_host is not set in config.toml. Run `spares deploy init` first.")
    })?;
    let (root, account_id, account) = crate::local::account().await?;
    let ssh = Ssh::new();
    if ssh.output(&remote_host, "sudo -n true 2>/dev/null && echo yes || true")? != "yes" {
        return Err(miette!(
            "Your user on {remote_host} can't use sudo without a password. Run `spares deploy \
             init <server>` once, which lets it."
        ));
    }
    let has_host = ssh.output(
        &remote_host,
        &format!(
            "sudo -n sqlite3 {} \"SELECT COUNT(*) FROM user WHERE id = '{}'\" 2>/dev/null || true",
            quote(&root.join("accounts.sqlite").to_string_lossy()),
            account_id.replace('\'', "")
        ),
    )? == "1";
    if !has_host {
        return Err(miette!(
            "The server has no spares host with your account {account}. Run `spares deploy init \
             <server>`, which creates it."
        ));
    }
    let progress = Progress::start("update", &["Building", "Uploading", "Installing"]);
    let bundle = Bundle::build(&progress, &repo, &remote_host, || {
        let output = ssh.output(
            &remote_host,
            "uname -m; sed -n 's/^NO_LATEX=//p' /etc/spares/setup.conf",
        )?;
        let mut lines = output.lines();
        Ok(Server {
            target: Target::of_arch(lines.next().unwrap_or_default())?,
            with_pandoc: lines.next() != Some("true"),
        })
    })
    .await?;
    progress.step("Uploading", &format!("to {remote_host}"));
    upload(&ssh, &bundle, &remote_host)?;
    drop(bundle);
    // Kept in step with this machine's, e.g. after moving
    let timezone = local_timezone()
        .map(|zone| format!(" --timezone {}", quote(&zone)))
        .unwrap_or_default();
    progress.step("Installing", &format!("on {remote_host}"));
    let mut command = ssh.command();
    command.args([
        &remote_host,
        &format!(
            "sudo -n bash {REMOTE_DIR}/setup_vps.sh --bundle \"$HOME/{REMOTE_DIR}\"{timezone} \
             --root {} --account {} --account-id {}",
            quote(&root.to_string_lossy()),
            quote(&account),
            quote(&account_id)
        ),
    ]);
    // Closed standard input, so the script never waits on this terminal
    progress.run(command, Some(""))?;
    progress.end_step();
    progress.finish();
    let needs_reboot = wait_for_tailscale_upgrade(&ssh, &remote_host)?;
    // This host's copy needs a token there, after this machine moved into a host
    let url = match config.sync.url.clone() {
        Some(url) => url,
        None => ssh.output(&remote_host, "cat /etc/spares/url")?,
    };
    if config.sync.token.is_none() {
        let base_url = spares_core::config::host_client().base_url;
        let client = crate::local_client()?;
        println!("Logging this host in to {url}…");
        crate::sync::upstream_login(&base_url, &client, &url, &account, None)
            .await
            .map_err(|e| miette!("{e}"))?;
    }
    println!(
        "\n{}",
        "Restart spares_server here too, so both sides run the same version.".yellow()
    );
    if needs_reboot {
        println!(
            "{}",
            format!("Reboot the server to finish its upgrade: ssh {remote_host} sudo reboot")
                .yellow()
        );
    }
    Ok(())
}

/// Waits for the upgrade of Tailscale that `setup_vps.sh` leaves running on `host` (as
/// user@host), which restarts it and so ends SSH connections over the tailnet. Returns whether
/// the server then needs a reboot to finish upgrading.
fn wait_for_tailscale_upgrade(ssh: &Ssh, host: &str) -> Result<bool, Error> {
    const CHECK: &str = "systemctl is-active --quiet spares-tailscale-upgrade && echo upgrading; \
                         test -f /var/run/reboot-required && echo reboot; true";
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut said = false;
    loop {
        // Fails while Tailscale restarts
        match ssh.output(host, CHECK) {
            Ok(output) if !output.contains("upgrading") => return Ok(output.contains("reboot")),
            _ if Instant::now() > deadline => {
                return Err(miette!(
                    "Tailscale on {host} did not finish upgrading in 3 minutes. See `journalctl -u \
                     spares-tailscale-upgrade` there."
                ));
            }
            _ => {
                if !said {
                    println!("Waiting for Tailscale to upgrade on {host}…");
                    said = true;
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

/// Copies `bundle` to [`REMOTE_DIR`] on `server` (as user@host), sending only what changed since
/// the last deploy.
fn upload(ssh: &Ssh, bundle: &Bundle, server: &str) -> Result<(), Error> {
    let sent = ssh.sync(bundle.path(), &format!("{server}:{REMOTE_DIR}"))?;
    #[allow(clippy::cast_precision_loss)]
    let megabytes = sent as f64 / 1_000_000.0;
    println!("Sent {megabytes:.1} MB");
    Ok(())
}

fn cloud_init(name: &str) {
    eprintln!(
        "Create an auth key at https://login.tailscale.com/admin/settings/keys (not reusable, not
ephemeral), put it in place of the placeholder, and paste this into the VPS's cloud-init user
data. Once the server is up, run: spares deploy init {name}
If `ssh root@{name}` is refused, allow root in the \"ssh\" section of your tailnet policy file.
"
    );
    println!(
        "#!/bin/sh
curl -fsSL https://tailscale.com/install.sh | sh
tailscale up --ssh --hostname={name} --authkey=TSKEY-PLACEHOLDER"
    );
}

/// The spares repository to build the server from: the one this runs in, or else the one this
/// binary was built from.
fn repo_dir() -> Result<PathBuf, Error> {
    let is_repo = |dir: &Path| {
        dir.join("scripts/setup_vps.sh").is_file() && dir.join("spares_server/Cargo.toml").is_file()
    };
    let current = std::env::current_dir().ok();
    let built = Path::new(env!("CARGO_MANIFEST_DIR")).parent();
    current
        .iter()
        .flat_map(|dir| dir.ancestors())
        .chain(built)
        .find(|dir| is_repo(dir))
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            miette!("Run this in the spares repository, which it builds the server from.")
        })
}

/// Where `spares deploy` keeps what it downloads and the times of its steps
fn cache_dir() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))?;
    Some(cache.join("spares-deploy"))
}
