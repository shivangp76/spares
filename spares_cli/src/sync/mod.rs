mod cloud;
mod two_way;

use clap::Args;
use clap::Subcommand;
use reqwest::Client;
pub(crate) use two_way::sync_cloud;

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
    /// sides changed. If only a full sync is possible (the first sync, for one), it says why and
    /// asks which way. Then, with `sync.remote_host` set (e.g. "user@myserver.com"), copies the
    /// files the server cannot generate itself: the rendered files, the templates, and the files
    /// that rendering recorded the notes as reading (preambles, figures, ...).
    ///
    /// Without `sync.url`, pushes the database to `sync.remote_host`, replacing the server's, and
    /// then the files.
    Cloud {
        /// Replace the server's database with this one
        #[arg(long, conflicts_with = "full_download")]
        full_upload: bool,
        /// Replace this database with the server's
        #[arg(long)]
        full_download: bool,
    },
    /// Resolve the notes a sync could not merge (tagged `sync-conflict`), one at a time
    Conflicts,
}

pub(crate) async fn sync_notes(
    base_url: &str,
    client: &Client,
    sync_args: SyncArgs,
) -> Result<(), String> {
    match sync_args.subcommand {
        SyncSubcommand::Cloud {
            full_upload,
            full_download,
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
    }
}
