use std::process::Command;

use spares_core::config::get_data_dir;
use spares_core::config::read_external_config;

pub(crate) fn sync_cloud() -> Result<(), String> {
    let config = read_external_config().map_err(|e| e.to_string())?;
    let remote_host = config
        .remote_host
        .ok_or("remote_host is not set in config.toml")?;

    let data_dir = get_data_dir();
    let local = data_dir.to_str().unwrap();

    // Pass 1: DB only
    println!("Syncing database → {}", remote_host);
    run_rsync(&[
        "-avz",
        &format!("{}/spares-main.sqlite", local),
        &format!("{}:{}/spares-main.sqlite", remote_host, local),
    ])?;

    // Pass 2: Everything else (notes, cards, PDFs)
    println!("Syncing files → {}", remote_host);
    run_rsync(&[
        "-avz",
        "--exclude=*.sqlite",
        &format!("{}/", local),
        &format!("{}:{}/", remote_host, local),
    ])?;

    println!("Done.");
    Ok(())
}

fn run_rsync(args: &[&str]) -> Result<(), String> {
    let status = Command::new("rsync")
        .args(args)
        .status()
        .map_err(|e| format!("Failed to run rsync: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rsync exited with {status}"))
    }
}
