//! `spares sync cloud` with two-way sync (`sync.url` set), and `spares sync conflicts`. The local
//! server does the syncing (`POST /api/sync/run`); see `spares_core::sync`.

use std::path::PathBuf;
use std::process::Command;

use colored::Colorize;
use inquire::Select;
use reqwest::Client;
use reqwest::StatusCode;
use serde_json::Value;
use serde_json::json;
use spares_core::config::read_external_config;
use spares_core::notes_vcs;
use spares_core::sync::conflicts::ConflictKind;
use spares_core::sync::conflicts::ConflictView;
use spares_core::sync::conflicts::Resolution;
use spares_core::sync::merge::merge_text_with_markers;
use spares_core::sync::run::SyncMode;
use spares_core::sync::run::SyncReport;

use super::cloud::push_files;
use super::cloud::push_to_remote_host;
use crate::sync::utils::apply_select_settings;

/// Syncs with the server, asking which way to go if only a full sync is possible, then copies the
/// files the server cannot generate itself to `sync.remote_host`.
pub(crate) async fn sync_cloud(
    base_url: &str,
    client: &Client,
    mode: SyncMode,
) -> Result<(), String> {
    let config = read_external_config().map_err(|e| e.to_string())?;
    if config.sync.url.is_none() {
        println!(
            "{}",
            "`sync.url` is not set, so this replaces the server's database (one-way).".yellow()
        );
        return push_to_remote_host().await;
    }
    if !notes_vcs::enabled() {
        println!(
            "{}",
            "Edits to note files not imported yet are overwritten if the sync changes those \
             notes. Import them with `spares sync` first."
                .yellow()
        );
    }
    let mut mode = mode;
    let report = loop {
        println!("Syncing ({})...", mode_name(mode));
        let response = client
            .post(format!("{base_url}/api/sync/run"))
            .json(&json!({ "mode": mode }))
            .send()
            .await
            .map_err(|e| format!("Is the spares server running? {e}"))?;
        let status = response.status();
        let body: Value = response.json().await.map_err(|e| e.to_string())?;
        if status.is_success() {
            break serde_json::from_value::<SyncReport>(body).map_err(|e| e.to_string())?;
        }
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status != StatusCode::PRECONDITION_FAILED || mode != SyncMode::Normal {
            return Err(message);
        }
        println!("{} {message}", "A full sync is required:".yellow());
        let choices = vec![
            "Upload: replace the server's database with this one",
            "Download: replace this database with the server's",
            "Cancel",
        ];
        let mut select = Select::new("Which way?", choices.clone());
        apply_select_settings(&mut select);
        let choice = select.prompt().map_err(|e| e.to_string())?;
        mode = match choices.iter().position(|c| *c == choice) {
            Some(0) => SyncMode::FullUpload,
            Some(1) => SyncMode::FullDownload,
            _ => return Ok(()),
        };
    };
    print_report(&report);
    if let Some(remote_host) = &config.sync.remote_host {
        push_files(remote_host, &config.sync.remote_rendered_dir).await?;
    }
    Ok(())
}

fn mode_name(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Normal => "two-way",
        SyncMode::FullUpload => "full upload",
        SyncMode::FullDownload => "full download",
    }
}

fn print_report(report: &SyncReport) {
    match report.mode {
        None => println!("Already in sync."),
        Some(SyncMode::FullUpload) => println!("Uploaded this database to the server."),
        Some(SyncMode::FullDownload) => println!(
            "Downloaded the server's database ({} notes).",
            report.notes_changed.len()
        ),
        Some(SyncMode::Normal) => println!(
            "Received {} changes and sent {}. {} notes updated here, {} deleted.",
            report.pulled,
            report.pushed,
            report.notes_changed.len(),
            report.notes_deleted.len()
        ),
    }
    if !report.conflicts.is_empty() {
        println!(
            "{} {} notes need a look (tagged `sync-conflict`): {}. Run `spares sync conflicts`.",
            "Conflicts:".red(),
            report.conflicts.len(),
            report
                .conflicts
                .iter()
                .map(|id| id.letters())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Goes through the conflicts one at a time.
pub(crate) async fn resolve_conflicts(base_url: &str, client: &Client) -> Result<(), String> {
    let conflicts: Vec<ConflictView> = client
        .get(format!("{base_url}/api/sync/conflicts"))
        .send()
        .await
        .map_err(|e| format!("Is the spares server running? {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if conflicts.is_empty() {
        println!("No conflicts.");
        return Ok(());
    }
    println!(
        "{} conflicts. Card orders are shown as card ids (`o:c40` is card 40), so the same card \
         reads the same on both sides.",
        conflicts.len()
    );
    let dir = std::env::temp_dir().join(format!("spares-conflicts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let result = resolve_each(base_url, client, &conflicts, &dir).await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

async fn resolve_each(
    base_url: &str,
    client: &Client,
    conflicts: &[ConflictView],
    dir: &std::path::Path,
) -> Result<(), String> {
    for conflict in conflicts {
        let note = conflict.note_id.letters();
        let resolution = match conflict.kind {
            ConflictKind::Text => {
                let (Some(local), Some(remote)) = (&conflict.local, &conflict.remote) else {
                    continue;
                };
                println!(
                    "\n{} {}: both sides changed the same lines. Here is this side's text \
                     against the other's:",
                    "Note".bold(),
                    note.black().on_yellow()
                );
                let local_path = dir.join(format!("{note}.here"));
                let remote_path = dir.join(format!("{note}.there"));
                std::fs::write(&local_path, local).map_err(|e| e.to_string())?;
                std::fs::write(&remote_path, remote).map_err(|e| e.to_string())?;
                show_diff(&local_path, &remote_path)?;
                let choices = vec![
                    "Keep this side's text",
                    "Take the other side's text",
                    "Edit a merge of both",
                    "Skip",
                    "Quit",
                ];
                match choose(&choices)? {
                    0 => Resolution::Keep,
                    1 => Resolution::Text(remote.clone()),
                    2 => {
                        let base = conflict.base.as_deref().unwrap_or(local);
                        let merged = merge_text_with_markers(base, local, remote);
                        Resolution::Text(edit(&dir.join(format!("{note}.merge")), &merged)?)
                    }
                    3 => continue,
                    _ => return Ok(()),
                }
            }
            // The note has been deleted since, so nothing is left to decide.
            ConflictKind::DeletedThere | ConflictKind::DeletedHere if conflict.local.is_none() => {
                Resolution::Keep
            }
            ConflictKind::DeletedThere | ConflictKind::DeletedHere => {
                let local = conflict.local.as_deref().unwrap_or_default();
                let what = if conflict.kind == ConflictKind::DeletedThere {
                    "was deleted on the other side but edited here, so it was kept"
                } else {
                    "was deleted here but edited on the other side, so it came back"
                };
                println!("\n{} {} {what}:", "Note".bold(), note.black().on_yellow());
                println!("{local}");
                let choices = vec!["Keep the note", "Delete it", "Skip", "Quit"];
                match choose(&choices)? {
                    0 => Resolution::Keep,
                    1 => Resolution::Delete,
                    2 => continue,
                    _ => return Ok(()),
                }
            }
        };
        let response = client
            .post(format!("{base_url}/api/sync/conflicts/{}", conflict.id))
            .json(&resolution)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            let body: Value = response.json().await.unwrap_or_default();
            return Err(body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Failed to resolve the conflict")
                .to_string());
        }
        println!("Resolved {note}.");
    }
    Ok(())
}

fn choose(choices: &[&'static str]) -> Result<usize, String> {
    let mut select = Select::new("Action:", choices.to_vec());
    apply_select_settings(&mut select);
    let choice = select.prompt().map_err(|e| e.to_string())?;
    Ok(choices
        .iter()
        .position(|c| *c == choice)
        .unwrap_or(choices.len()))
}

fn show_diff(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["diff", "--no-index", "--color=always"])
        .arg(from)
        .arg(to)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .map_err(|e| format!("Failed to diff: {e}"))?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// Opens `contents` in `$EDITOR` until no conflict markers are left, and returns the result.
fn edit(path: &PathBuf, contents: &str) -> Result<String, String> {
    std::fs::write(path, contents).map_err(|e| e.to_string())?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    loop {
        let status = Command::new(&editor)
            .arg(path)
            .status()
            .map_err(|e| format!("Failed to run {editor}: {e}"))?;
        if !status.success() {
            return Err(format!("{editor} exited with {status}"));
        }
        let edited = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        if !edited.lines().any(|line| {
            line.starts_with("<<<<<<<") || line.starts_with(">>>>>>>") || line == "======="
        }) {
            return Ok(edited);
        }
        println!("{}", "The text still has conflict markers.".yellow());
    }
}
