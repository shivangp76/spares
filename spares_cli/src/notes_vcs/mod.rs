//! Keeping notes in a git or jj repository synced with spares by push and fetch. See
//! `spares_core::notes_vcs`.

mod hook;
mod init;

pub(crate) use hook::run_hook;
pub(crate) use init::init_vcs;
use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::notes_vcs::CheckRequest;
use spares_core::notes_vcs::CheckResponse;

use crate::utils::ensure_ok;

/// `spares note vcs-check`
pub(crate) async fn check(base_url: &str, client: &Client, fix: bool) -> Result<(), Error> {
    let response = client
        .post(format!("{base_url}/api/notes/vcs/check"))
        .json(&CheckRequest { fix })
        .send()
        .await
        .map_err(|e| miette!("{}", e))?;
    let check: CheckResponse = ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{}", e))?;
    let status = &check.status;
    if !status.enabled {
        println!("Notes are not kept in a git repository (see `spares note init-vcs`).");
        return Ok(());
    }
    if let Some(notes_dir) = &status.notes_dir {
        println!("Repository: {}", notes_dir.display());
    }
    if status.needs_reconcile {
        println!("A commit failed since the server started; the next push reconciles first.");
    }
    if status.receive_in_progress {
        println!("A push is being imported.");
    }
    if let Some(incident) = &status.last_incident {
        println!("Last push incident: {incident}");
    }
    let print_paths = |label: &str, paths: &[std::path::PathBuf]| {
        if !paths.is_empty() {
            println!("{label}:");
            for path in paths {
                println!("  {}", path.display());
            }
        }
    };
    print_paths("Uncommitted paths", &check.dirty);
    let report = &check.report;
    if fix {
        print_paths("Rewritten from the database", &report.rewritten);
        print_paths("Removed", &report.removed);
        print_paths("Restored", &report.restored);
        print_paths("Saved before overwriting", &report.saved);
        if report.committed {
            println!("Committed the changes.");
        }
    } else {
        print_paths("Differ from the database", &report.rewritten);
        print_paths("Should not exist", &report.removed);
        print_paths("Modified outside spares", &report.restored);
        if !report.is_empty() {
            println!("Run `spares note vcs-check --fix` to rewrite them from the database.");
        }
    }
    if report.is_empty() && check.dirty.is_empty() {
        println!("The repository matches the database.");
    }
    print_paths("Copies saved before overwriting", &check.overwritten);
    Ok(())
}
