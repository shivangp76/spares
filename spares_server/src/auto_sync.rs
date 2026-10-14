//! Automatic sync: a sync client with `sync_url` and `sync_interval_minutes` set syncs on startup
//! and then every that many minutes. Failures (the server being unreachable, for one) are logged
//! and the next attempt goes ahead as planned.

use std::time::Duration;

use log::info;
use log::warn;
use spares_core::Error;
use spares_core::config::SyncRole;
use spares_core::config::get_cache_dir;
use spares_core::config::get_sync_role;
use spares_core::config::read_external_config;
use spares_core::notes_vcs::run_exclusive;
use spares_core::sync::files::sync_now;
use spares_core::sync::run::SyncMode;
use sqlx::SqlitePool;
use tokio::time::MissedTickBehavior;

pub(crate) fn spawn(db: SqlitePool) {
    if !matches!(get_sync_role(), Ok(SyncRole::Client)) {
        return;
    }
    let Ok(config) = read_external_config() else {
        return;
    };
    let (Some(url), minutes) = (config.sync_url, config.sync_interval_minutes) else {
        return;
    };
    if minutes == 0 {
        return;
    }
    info!("Syncing with {url} on startup and every {minutes} minutes");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(minutes * 60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut warned_full_sync = false;
        loop {
            interval.tick().await;
            let db = db.clone();
            let temp_dir = get_cache_dir().join("sync");
            if let Err(e) = std::fs::create_dir_all(&temp_dir) {
                warn!("Automatic sync failed: cannot create {}: {e}", temp_dir.display());
                continue;
            }
            let result =
                run_exclusive(async move { sync_now(&db, SyncMode::Normal, &temp_dir).await })
                    .await;
            match result {
                Ok(_) => warned_full_sync = false,
                // Only a person can choose which side to keep, so this is said once.
                Err(Error::FullSyncRequired(reason)) => {
                    if !warned_full_sync {
                        warn!(
                            "Automatic sync is paused: a full sync is required: {reason} Run \
                             `spares sync cloud`."
                        );
                        warned_full_sync = true;
                    }
                }
                Err(e) => warn!("Automatic sync failed: {e}"),
            }
        }
    });
}
