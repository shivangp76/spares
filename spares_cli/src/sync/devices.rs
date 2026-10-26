//! `spares sync devices`: the browsers (and other laptops) enrolled with the server as replicas
//! `2` to `4` (see `spares_core::sync::devices`).

use chrono::DateTime;
use chrono::Local;
use clap::Subcommand;
use spares_core::config::read_external_config;
use spares_core::sync::http::HttpRemote;
use spares_core::sync::http::SYNC_API_KEY_ENV;

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum DevicesAction {
    /// Enroll another laptop, and print the `SPARES_REPLICA` to set on it. A browser enrolls
    /// itself when it starts keeping a copy (Settings → This Device).
    Add {
        /// What it is called in the list, e.g. "work laptop"
        name: String,
    },
    /// Free a device's number, e.g. one whose browser deleted its copy. It can no longer sync,
    /// and what it hasn't synced yet never reaches the server.
    Remove { replica: u8 },
}

fn remote() -> Result<HttpRemote, String> {
    let url = read_external_config()
        .map_err(|e| e.to_string())?
        .sync
        .url
        .ok_or("`sync.url` is not set in config.toml")?;
    HttpRemote::new(&url, std::env::var(SYNC_API_KEY_ENV).ok()).map_err(|e| e.to_string())
}

fn local_time(unix: i64) -> String {
    DateTime::from_timestamp(unix, 0).map_or_else(
        || unix.to_string(),
        |time| {
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

pub(crate) async fn devices(action: Option<DevicesAction>) -> Result<(), String> {
    let remote = remote()?;
    match action {
        None => {
            let devices = remote.devices().await.map_err(|e| e.to_string())?;
            if devices.is_empty() {
                println!("No devices are enrolled.");
            }
            for device in devices {
                let synced = device
                    .last_sync_at
                    .map_or_else(|| "never".to_string(), local_time);
                println!(
                    "{}  {}  (enrolled {}, last synced {synced})",
                    device.replica,
                    device.name,
                    local_time(device.enrolled_at)
                );
            }
        }
        Some(DevicesAction::Add { name }) => {
            let enrolled = remote.enroll(&name).await.map_err(|e| e.to_string())?;
            println!(
                "Enrolled as replica {0}. On that laptop, set SPARES_REPLICA={0} and do a full \
                 download (`spares sync cloud --full-download`).",
                enrolled.replica
            );
        }
        Some(DevicesAction::Remove { replica }) => {
            remote.unenroll(replica).await.map_err(|e| e.to_string())?;
            println!("Removed replica {replica}.");
        }
    }
    Ok(())
}
