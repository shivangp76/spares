//! `spares sync devices`: the copies of the collection enrolled with the root of its sync chain
//! (see `spares_core::sync::devices`), through the host the CLI uses, which asks its upstream if
//! it has one.

use chrono::DateTime;
use chrono::Local;
use clap::Subcommand;
use reqwest::Client;
use serde_json::Value;
use serde_json::json;
use spares_core::sync::devices::Device;

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum DevicesAction {
    /// Free a device's number, e.g. one whose browser deleted its copy. It can no longer sync,
    /// and what it hasn't synced yet never reaches the host. A host's copy is removed by revoking
    /// its token instead (`spares host revoke`, or Settings on the upstream).
    Remove { replica: u8 },
}

async fn answer(response: reqwest::Result<reqwest::Response>) -> Result<Value, String> {
    let response = response.map_err(|e| format!("Is the spares host running? {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if status.is_success() {
        Ok(body)
    } else {
        Err(body["message"]
            .as_str()
            .map_or_else(|| status.to_string(), str::to_string))
    }
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

pub(crate) async fn devices(
    base_url: &str,
    client: &Client,
    action: Option<DevicesAction>,
) -> Result<(), String> {
    match action {
        None => {
            let devices: Vec<Device> = serde_json::from_value(
                answer(
                    client
                        .get(format!("{base_url}/api/sync/devices"))
                        .send()
                        .await,
                )
                .await?,
            )
            .map_err(|e| e.to_string())?;
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
        Some(DevicesAction::Remove { replica }) => {
            answer(
                client
                    .post(format!("{base_url}/api/sync/unenroll"))
                    .json(&json!({ "replica": replica }))
                    .send()
                    .await,
            )
            .await?;
            println!("Removed replica {replica}.");
        }
    }
    Ok(())
}
