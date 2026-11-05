mod cloud;
mod devices;
mod two_way;
mod upstream;

use clap::Args;
use clap::Subcommand;
use reqwest::Client;
pub(crate) use two_way::sync_cloud;
pub(crate) use upstream::login_with as upstream_login;

#[derive(Args, Debug, Clone)]
pub(crate) struct SyncArgs {
    #[command(subcommand)]
    pub(crate) subcommand: SyncSubcommand,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum SyncSubcommand {
    /// Sync with the server both ways.
    ///
    /// With `sync.url` set in config.toml, exchanges changes with the server, merging notes both
    /// sides changed, and the images, audio and image occlusions either side lacks. If only a
    /// full sync is possible (the first sync, for one), it says why and asks which way.
    ///
    /// Before that, with `sync.remote_host` set (e.g. "user@myserver.com", the server's machine
    /// and the user it runs as), copies there over SSH what the server needs besides: the stored
    /// copies of the files Typst cards read (render assets), the templates, the files rendering
    /// recorded the notes as reading (preambles, pandoc defaults, ...), and the rendered files
    /// of parsers the server has no programs for. It first checks that `remote_host` reaches the
    /// collection `sync.url` serves, at the same path as here. Settings go to the server too,
    /// which keeps its own `[sync]` table. What is gone here goes there too: templates removed
    /// here, and files it copied that no note reads any more. The server itself removes the
    /// render assets no note reads any more, and the files of the notes and cards the sync
    /// deletes. Afterwards, if what rendering reads changed, the server renders again the notes
    /// it affects.
    ///
    /// Images and audio stay, whether or not a note names them.
    Cloud {
        /// Replace the server's database with this one
        #[arg(long, conflicts_with = "full_download")]
        full_upload: bool,
        /// Replace this database with the server's
        #[arg(long)]
        full_download: bool,
        /// Check where the files would go and list what would be copied or removed, without
        /// copying, removing or syncing anything
        #[arg(long, conflicts_with_all = ["full_upload", "full_download"])]
        dry_run: bool,
    },
    /// Resolve the notes a sync could not merge (tagged `sync-conflict`), one at a time
    Conflicts,
    /// List the copies of the collection enrolled with the root of its sync chain, or remove one
    Devices {
        #[command(subcommand)]
        action: Option<devices::DevicesAction>,
    },
    /// Sync this host's copy of the collection with an upstream host, as one of its devices
    ///
    /// Logs in there with a sync-only token, enrolls this copy (keeping its replica number, the
    /// laptop's `0` say, if it has one), and tells this host, which then syncs with it as
    /// `[sync]` in config.toml says.
    Login(upstream::UpstreamLoginArgs),
    /// Stop syncing with the upstream, freeing this copy's number there
    Logout,
}

pub(crate) async fn sync_notes(
    base_url: &str,
    client: &Client,
    sync_args: SyncArgs,
) -> Result<(), String> {
    match sync_args.subcommand {
        SyncSubcommand::Cloud { dry_run: true, .. } => two_way::preview_sync_cloud().await,
        SyncSubcommand::Cloud {
            full_upload,
            full_download,
            dry_run: false,
        } => {
            let mode = if full_upload {
                spares_core::sync::run::SyncMode::FullUpload
            } else if full_download {
                spares_core::sync::run::SyncMode::FullDownload
            } else {
                spares_core::sync::run::SyncMode::Normal
            };
            two_way::sync_cloud(base_url, client, mode).await
        }
        SyncSubcommand::Conflicts => two_way::resolve_conflicts(base_url, client).await,
        SyncSubcommand::Devices { action } => devices::devices(base_url, client, action).await,
        SyncSubcommand::Login(args) => upstream::login(base_url, client, args).await,
        SyncSubcommand::Logout => upstream::logout(base_url, client).await,
    }
}
