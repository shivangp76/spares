//! `spares deploy`: deploys spares to a VPS as the two-way sync server, and sets up this machine
//! (the laptop, replica 0) to sync with it. `scripts/setup_vps.sh` does the server's side; this
//! uploads it and runs it there.

mod bundle;
mod init;
mod shell;
mod status_bar;

use std::path::Path;
use std::path::PathBuf;

use clap::Args;
use clap::Subcommand;
use miette::Error;
use miette::miette;
use spares_core::config::EnvironmentConfig;
use spares_core::config::read_external_config;

use bundle::Bundle;
use shell::Ssh;
use shell::bold;
use shell::quote;
use status_bar::StatusBar;

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
    ///    user, asks for its sudo password and, if needed, prints a link to join your tailnet;
    ///
    /// 3. stores the server's API key in the Keychain, and sets `remote_host`, `url` and
    ///    `interval_minutes` in the [sync] table of config.toml;
    ///
    /// 4. has you restart the local server, uploads the database (`spares sync cloud
    ///    --full-upload`) and checks that both sides have the same notes;
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
    /// Builds this checkout and installs it on the server (with sudo there).
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

/// The steps of `setup_vps.sh` that an update shows in its progress bar, each the start of the
/// title of its "==> " line.
const UPDATE_STEPS: &[&str] = &[
    "Uploading",
    "Setting the time zone",
    "Installing packages",
    "Installing pandoc",
    "Adding",
    "Installing the bundled spares",
    "Configuring",
    "Setting up",
];

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
    let ssh = Ssh::new();
    let no_latex = ssh.output(
        &remote_host,
        "sed -n 's/^NO_LATEX=//p' /etc/spares/setup.conf",
    )?;
    let bundle = Bundle::build(&ssh, &repo, &remote_host, no_latex != "true").await?;
    bold(&format!("Updating {remote_host}"));
    // Kept in step with this machine's, e.g. after moving
    let timezone = local_timezone()
        .map(|zone| format!(" --timezone {}", quote(&zone)))
        .unwrap_or_default();
    let bar = StatusBar::start(UPDATE_STEPS);
    ssh.upload(
        &[&repo.join("scripts/setup_vps.sh"), &bundle.path()],
        &format!("{remote_host}:/tmp/"),
    )?;
    drop(bundle);
    let mut command = ssh.command();
    command.args([
        "-t",
        &remote_host,
        &format!(
            "sudo bash /tmp/setup_vps.sh --bundle /tmp/spares-bundle.tar.gz{timezone}; \
             status=$?; rm -f /tmp/setup_vps.sh /tmp/spares-bundle.tar.gz; exit $status"
        ),
    ]);
    status_bar::run(bar, command)?;
    println!("Restart spares_serve here too, so both sides run the same version.");
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

/// This machine's time zone, e.g. `America/New_York`, which the server is set to so that both count
/// days the same. `None` if it cannot be told.
fn local_timezone() -> Option<String> {
    let zone = std::fs::read_link("/etc/localtime").ok()?;
    let zone = zone.to_str()?;
    zone.split_once("/zoneinfo/")
        .map(|(_, zone)| zone.to_string())
}
