//! `spares config`: the collection's settings (see `spares_core::settings`), through the host, as
//! the web app's Settings page shows them. This machine's settings (`[sync]`, `audio_player`,
//! `tts_command`, `render_browser_parsers`) stay in its `config.toml`, edited by hand.

use std::path::Path;
use std::process::Command;

use colored::Colorize as _;
use miette::Error;
use miette::miette;
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::Value;
use spares_core::settings::ReplaceSettingsRequest;
use spares_core::settings::SettingValue;
use spares_core::settings::SettingsChangeResponse;
use spares_core::settings::SettingsView;
use spares_core::settings::UpdateSettingsRequest;
use spares_core::settings::parse_key;

use crate::args::ConfigArgs;
use crate::args::ConfigCommands;
use crate::utils::ensure_ok;

pub(crate) async fn handle(args: ConfigArgs, base_url: &str, client: &Client) -> Result<(), Error> {
    match args.command {
        ConfigCommands::Show { all } => {
            let view = view(base_url, client).await?;
            if all {
                print!("{}", json_as_toml(&view.values)?);
            } else {
                print!("{}", view.text);
                if !view.local.trim().is_empty() {
                    eprintln!(
                        "\n{}\n{}",
                        "This machine's settings, in its config.toml:".dimmed(),
                        view.local.trim_end().dimmed()
                    );
                }
            }
            Ok(())
        }
        ConfigCommands::Get { key } => {
            let path = parse_key(&key).map_err(|e| miette!("{e}"))?;
            let view = view(base_url, client).await?;
            let value = path
                .iter()
                .try_fold(&view.values, |value, key| value.get(key))
                .ok_or_else(|| miette!("There is no setting `{key}`"))?;
            println!("{}", value_as_toml(value)?);
            Ok(())
        }
        ConfigCommands::Set { key, value } => {
            let key = parse_key(&key).map_err(|e| miette!("{e}"))?;
            update(base_url, client, key, Some(value)).await
        }
        ConfigCommands::Unset { key } => {
            let key = parse_key(&key).map_err(|e| miette!("{e}"))?;
            update(base_url, client, key, None).await
        }
        ConfigCommands::Edit => edit(base_url, client).await,
    }
}

async fn view(base_url: &str, client: &Client) -> Result<SettingsView, Error> {
    let response = client
        .get(format!("{base_url}/api/settings"))
        .send()
        .await
        .map_err(|e| miette!("{e}"))?;
    ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{e}"))
}

async fn update(
    base_url: &str,
    client: &Client,
    key: Vec<String>,
    toml: Option<String>,
) -> Result<(), Error> {
    let request = UpdateSettingsRequest {
        values: vec![SettingValue {
            key,
            value: None,
            toml,
        }],
    };
    let response = client
        .patch(format!("{base_url}/api/settings"))
        .json(&request)
        .send()
        .await
        .map_err(|e| miette!("{e}"))?;
    let response: SettingsChangeResponse = ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{e}"))?;
    report(&response);
    Ok(())
}

fn report(response: &SettingsChangeResponse) {
    if response.changed_keys.is_empty() {
        println!("Nothing changed.");
        return;
    }
    println!("Changed {}.", response.changed_keys.join(", "));
    if let Some(event_id) = response.event_id {
        println!(
            "{}",
            format!("Undo it with `spares event undo --event-id {event_id}`.").dimmed()
        );
    }
}

/// Opens the settings in `$VISUAL` or `$EDITOR`, and saves them once they are valid, from the
/// text they were opened from: if they changed meanwhile (on another device, say), nothing is
/// saved, and the edited text stays in its file.
async fn edit(base_url: &str, client: &Client) -> Result<(), Error> {
    let base = view(base_url, client).await?.text;
    let path = std::env::temp_dir().join(format!("spares-settings-{}.toml", std::process::id()));
    std::fs::write(&path, &base).map_err(|e| miette!("{e}"))?;
    loop {
        run_editor(&path)?;
        let text = std::fs::read_to_string(&path).map_err(|e| miette!("{e}"))?;
        if text == base {
            let _ = std::fs::remove_file(&path);
            println!("Nothing changed.");
            return Ok(());
        }
        let response = client
            .put(format!("{base_url}/api/settings"))
            .json(&ReplaceSettingsRequest {
                text: text.clone(),
                base: Some(base.clone()),
            })
            .send()
            .await
            .map_err(|e| miette!("{e}"))?;
        match response.status() {
            StatusCode::OK => {
                let response: SettingsChangeResponse =
                    response.json().await.map_err(|e| miette!("{e}"))?;
                let _ = std::fs::remove_file(&path);
                report(&response);
                return Ok(());
            }
            StatusCode::BAD_REQUEST => {
                let body: Value = response.json().await.map_err(|e| miette!("{e}"))?;
                let message = body["message"].as_str().unwrap_or("Invalid settings");
                println!("{}", message.red());
                let again = inquire::Confirm::new("Edit them again?")
                    .with_default(true)
                    .prompt()
                    .unwrap_or(false);
                if !again {
                    return Err(kept(&path, "Not saved"));
                }
            }
            StatusCode::CONFLICT => {
                let body: Value = response.json().await.map_err(|e| miette!("{e}"))?;
                println!(
                    "{}",
                    body["message"]
                        .as_str()
                        .unwrap_or("The settings changed since.")
                        .yellow()
                );
                return Err(kept(&path, "Run `spares config edit` again"));
            }
            _ => {
                ensure_ok(response).await?;
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
        }
    }
}

/// Says where the edited file is, left so the edit isn't lost.
fn kept(path: &Path, what: &str) -> Error {
    miette!("{what}. Your edit is in {}", path.display())
}

fn run_editor(path: &Path) -> Result<(), Error> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    // An editor given with arguments, such as `code --wait`
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let status = Command::new(program)
        .args(parts)
        .arg(path)
        .status()
        .map_err(|e| miette!("Failed to run {editor}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(miette!("{editor} exited with {status}"))
    }
}

/// A JSON value written as TOML, as `spares config get` prints it.
fn value_as_toml(value: &Value) -> Result<String, Error> {
    use serde::Serialize as _;
    if let Value::Object(_) = value {
        return json_as_toml(value);
    }
    value
        .serialize(toml_edit::ser::ValueSerializer::new())
        .map(|value| value.to_string().trim().to_string())
        .map_err(|e| miette!("{e}"))
}

/// A table written as a TOML document, leaving out what TOML can't write (unset values).
fn json_as_toml(value: &Value) -> Result<String, Error> {
    toml_edit::ser::to_string_pretty(&without_nulls(value)).map_err(|e| miette!("{e}"))
}

fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key.clone(), without_nulls(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_nulls).collect()),
        value => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_print_as_toml() {
        assert_eq!(value_as_toml(&serde_json::json!(20)).unwrap(), "20");
        assert_eq!(value_as_toml(&serde_json::json!("due")).unwrap(), "\"due\"");
        assert_eq!(
            value_as_toml(&serde_json::json!([60, 600])).unwrap(),
            "[60, 600]"
        );
        let table = value_as_toml(&serde_json::json!({ "days": 2, "tag": null })).unwrap();
        assert_eq!(table, "days = 2\n");
    }
}
