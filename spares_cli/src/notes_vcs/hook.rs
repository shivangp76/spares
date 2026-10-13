//! The phases of the notes repository's git hooks. They only talk to the server, which does all the
//! work while holding the notes lock; see `spares_core::notes_vcs`.

use std::io::BufRead;
use std::path::PathBuf;

use reqwest::Client;
use serde::de::DeserializeOwned;
use spares_core::model::NoteId;
use spares_core::notes_vcs::FinishRequest;
use spares_core::notes_vcs::FinishResponse;
use spares_core::notes_vcs::ReceiveRequest;
use spares_core::notes_vcs::ReceiveResponse;
use spares_core::notes_vcs::StatusResponse;
use spares_core::schema::note::NotesSelector;
use spares_core::schema::note::RenderNotesRequest;

use crate::args::VcsHookPhase;

const ZERO_SHA_CHARS: char = '0';
const MAIN: &str = "refs/heads/main";

/// Runs a hook phase. Returns whether git should go on; messages are printed to stderr, which git
/// shows the pusher.
pub(crate) async fn run_hook(
    base_url: &str,
    client: &Client,
    phase: VcsHookPhase,
    args: &[String],
) -> bool {
    let result = match phase {
        VcsHookPhase::PreReceive => pre_receive(),
        VcsHookPhase::PushToCheckout => push_to_checkout(base_url, client, args).await,
        VcsHookPhase::PostReceive => post_receive(base_url, client).await,
    };
    match result {
        Ok(ok) => ok,
        Err(message) => {
            say(&message);
            false
        }
    }
}

fn say(message: &str) {
    for line in message.lines() {
        eprintln!("spares: {line}");
    }
}

/// `old new ref` lines from stdin
fn read_ref_updates() -> Result<Vec<(String, String, String)>, String> {
    let mut updates = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| format!("Failed to read the pushed refs: {e}"))?;
        let mut fields = line.split_whitespace();
        if let (Some(old), Some(new), Some(name)) = (fields.next(), fields.next(), fields.next()) {
            updates.push((old.to_string(), new.to_string(), name.to_string()));
        }
    }
    Ok(updates)
}

fn is_zero(sha: &str) -> bool {
    sha.chars().all(|c| c == ZERO_SHA_CHARS)
}

/// Only `main` may be pushed: it is the only ref that is imported.
fn pre_receive() -> Result<bool, String> {
    let mut problems = Vec::new();
    for (_, new, name) in read_ref_updates()? {
        if name != MAIN {
            problems.push(format!(
                "{name}: only `main` can be pushed, since it is what spares imports. Move `main` to your commit and push it (`jj spares-push`, or `git push origin HEAD:main`)."
            ));
        } else if is_zero(&new) {
            problems.push("`main` cannot be deleted".to_string());
        }
    }
    if problems.is_empty() {
        Ok(true)
    } else {
        say(&format!(
            "the push was rejected, so nothing was imported:\n{}",
            problems.join("\n")
        ));
        Ok(false)
    }
}

/// The repository the hook runs in. git runs hooks with `GIT_DIR` set; its parent is the worktree.
fn hook_repo() -> Result<PathBuf, String> {
    let git_dir = std::env::var_os("GIT_DIR").ok_or("GIT_DIR is not set; is this run by git?")?;
    let git_dir = PathBuf::from(git_dir);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(git_dir)
    };
    let git_dir = git_dir
        .canonicalize()
        .map_err(|e| format!("Failed to resolve {}: {e}", git_dir.display()))?;
    git_dir
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{} has no parent", git_dir.display()))
}

async fn get_json<T: DeserializeOwned>(client: &Client, url: String) -> Result<T, String> {
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|_| "the spares server is not running; start it to push".to_string())?;
    response_json(response).await
}

async fn post_json<B: serde::Serialize, T: DeserializeOwned>(
    client: &Client,
    url: String,
    body: &B,
) -> Result<T, String> {
    let response = client
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("the spares server could not be reached: {e}"))?;
    response_json(response).await
}

async fn response_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .map_or_else(|| body.to_string(), str::to_string);
        return Err(format!("the spares server failed ({status}): {message}"));
    }
    response
        .json()
        .await
        .map_err(|e| format!("Unexpected response from the spares server: {e}"))
}

async fn push_to_checkout(
    base_url: &str,
    client: &Client,
    args: &[String],
) -> Result<bool, String> {
    let new = args
        .first()
        .ok_or("push-to-checkout was called without the pushed commit")?
        .clone();
    let repo = hook_repo()?;
    let status: StatusResponse =
        get_json(client, format!("{base_url}/api/notes/vcs/status")).await?;
    if status.notes_dir.as_ref() != Some(&repo) {
        return Err(format!(
            "this repository ({}) does not belong to the spares server at {base_url}, which uses {}. Run `spares note init-vcs --repair-hooks` with the settings of this repository's spares instance.",
            repo.display(),
            status.notes_dir.map_or_else(
                || "no notes repository".to_string(),
                |d| d.display().to_string()
            )
        ));
    }
    if let Some(incident) = status.last_incident {
        say(&format!("note about an earlier push: {incident}"));
    }
    say("importing the push…");
    let response: ReceiveResponse = post_json(
        client,
        format!("{base_url}/api/notes/vcs/receive"),
        &ReceiveRequest { repo, new },
    )
    .await?;
    say(&response.message);
    Ok(response.ok)
}

async fn post_receive(base_url: &str, client: &Client) -> Result<bool, String> {
    let updates = read_ref_updates()?;
    let Some((_, new, _)) = updates.into_iter().find(|(_, _, name)| name == MAIN) else {
        return Ok(true);
    };
    let repo = hook_repo()?;
    let session_file = repo.join(".git").join("spares-session");
    let Ok(session) = std::fs::read_to_string(&session_file) else {
        say("spares has no record of this push; run `spares note vcs-check`");
        return Ok(true);
    };
    let finish: FinishResponse = post_json(
        client,
        format!("{base_url}/api/notes/vcs/finish"),
        &FinishRequest {
            session: session.trim().to_string(),
            new: new.clone(),
        },
    )
    .await?;
    say(&finish.message);
    if !finish.imported.is_empty() {
        render(base_url, client, finish.imported).await;
    }
    // spares' own commits on top of the push (normalization, render) are what there is to fetch.
    let head = std::process::Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    if head.as_deref() == Some(new.as_str()) {
        say("spares matches your push; there is nothing to fetch");
    } else {
        say("fetch (`jj spares-pull` / `git pull --rebase`) to get the notes as spares wrote them");
    }
    Ok(true)
}

/// Renders the imported notes, and notes whose linked notes changed because of them. The push has
/// gone through either way, so failures are only reported.
async fn render(base_url: &str, client: &Client, imported: Vec<NoteId>) {
    say(&format!("rendering {} note(s)…", imported.len()));
    let request = RenderNotesRequest {
        selector: NotesSelector::Ids(imported),
        immutable_note_ids: Some(Vec::new()),
        overridden_output_raw_dir: None,
        include_linked_notes: true,
        include_cards: true,
        generate_rendered: true,
        force_generate_rendered: false,
    };
    let response = client
        .post(format!("{base_url}/api/notes/generate_files"))
        .json(&request)
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            let status = response.status();
            let body: serde_json::Value = response.json().await.unwrap_or_default();
            say(&format!(
                "rendering failed ({status}): {}\nThe notes were imported; fix them and push again, or render with `spares note generate`.",
                body.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
            ));
        }
        Err(e) => say(&format!("rendering failed: {e}")),
    }
}
