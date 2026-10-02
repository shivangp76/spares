use std::process::Command;

use spares_core::config::get_data_dir;
use spares_core::config::read_external_config;
use spares_core::parsers::get_all_parsers;

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

    // Pass 2: Everything else (cards, image occlusions, render assets). The server reads notes from
    // the database, so raw note files are not needed. Neither are the raw cards of parsers that
    // render in the browser, since they are generated from the database on demand.
    println!("Syncing files → {}", remote_host);
    let mut excludes = vec![
        "--exclude=*.sqlite".to_string(),
        "--exclude=/notes/".to_string(),
    ];
    excludes.extend(
        get_all_parsers()
            .into_iter()
            .map(|parser| parser())
            .filter(|parser| parser.renders_in_browser())
            .map(|parser| format!("--exclude=/cards/{}/", parser.get_parser_name())),
    );
    let source = format!("{}/", local);
    let destination = format!("{}:{}/", remote_host, local);
    let mut args = vec!["-avz"];
    args.extend(excludes.iter().map(String::as_str));
    args.extend([source.as_str(), destination.as_str()]);
    run_rsync(&args)?;

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
