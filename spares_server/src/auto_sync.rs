//! Automatic sync: a sync client with `sync.url` and `sync.interval_minutes` set syncs on startup,
//! on waking from sleep, every that many minutes, and `sync.push_delay_seconds` after local
//! changes stop (see [`SyncConfig`]). A failed attempt (the server being unreachable, for one) is
//! logged and tried again a minute later if it was for local changes or a wake.

use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use log::debug;
use log::info;
use log::warn;
use spares_core::Error;
use spares_core::config::SyncRole;
use spares_core::config::get_cache_dir;
use spares_core::config::get_sync_role;
use spares_core::config::read_external_config;
use spares_core::notes_vcs::run_exclusive;
use spares_core::sync::SyncConfig;
use spares_core::sync::dirty_count;
use spares_core::sync::files::sync_now;
use spares_core::sync::run::SyncMode;
use sqlx::SqlitePool;
use tokio::time::MissedTickBehavior;

/// How often to look for local changes and for having slept.
const CHECK_EVERY: Duration = Duration::from_secs(15);
/// The longest local changes wait while they keep coming.
const MAX_PUSH_WAIT: Duration = Duration::from_mins(2);
/// How long after a failed attempt to try again.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// How far the wall clock must run ahead of the monotonic clock, which stops while macOS sleeps,
/// to count as a wake.
const MIN_SLEEP: Duration = Duration::from_secs(60);

pub(crate) fn spawn(db: SqlitePool) {
    if !matches!(get_sync_role(), Ok(SyncRole::Client)) {
        return;
    }
    let Ok(config) = read_external_config() else {
        return;
    };
    let SyncConfig {
        url: Some(url),
        interval_minutes,
        push_delay_seconds,
        ..
    } = config.sync
    else {
        return;
    };
    if interval_minutes == 0 {
        return;
    }
    if push_delay_seconds == 0 {
        info!("Syncing with {url} on startup, on wake and every {interval_minutes} minutes");
    } else {
        info!(
            "Syncing with {url} on startup, on wake, every {interval_minutes} minutes and \
             {push_delay_seconds} seconds after local changes"
        );
    }
    let interval = Duration::from_secs(interval_minutes * 60);
    let push_delay = (push_delay_seconds > 0).then(|| Duration::from_secs(push_delay_seconds));
    tokio::spawn(run(db, interval, push_delay));
}

async fn run(db: SqlitePool, interval: Duration, push_delay: Option<Duration>) {
    let mut check = tokio::time::interval(CHECK_EVERY);
    check.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut changes = Changes::default();
    let mut last_attempt: Option<Instant> = None;
    let mut retry_at: Option<Instant> = None;
    let mut woke = false;
    let mut last_check = (Instant::now(), SystemTime::now());
    let mut warned_full_sync = false;
    let mut failing = false;
    loop {
        check.tick().await;
        let now = (Instant::now(), SystemTime::now());
        let wall = now.1.duration_since(last_check.1).unwrap_or_default();
        if wall.saturating_sub(now.0 - last_check.0) >= MIN_SLEEP {
            debug!("Woke from sleep; syncing");
            woke = true;
        }
        last_check = now;
        let now = now.0;

        let changes_due = match push_delay {
            Some(delay) => match dirty_count(&db).await {
                Ok(count) => changes.due(count, now, delay),
                Err(e) => {
                    warn!("Automatic sync cannot look for local changes: {e}");
                    false
                }
            },
            None => false,
        };
        let interval_due = last_attempt.is_none_or(|at| now - at >= interval);
        if !(interval_due || woke || changes_due) || retry_at.is_some_and(|at| now < at) {
            continue;
        }

        last_attempt = Some(now);
        match sync(&db).await {
            Ok(()) => {
                if failing {
                    info!("Automatic sync works again");
                }
                failing = false;
                warned_full_sync = false;
                woke = false;
                retry_at = None;
                changes = Changes::default();
            }
            // Only a person can choose which side to keep, so this is said once, and the next
            // try waits for the interval.
            Err(Error::FullSyncRequired(reason)) => {
                if !warned_full_sync {
                    warn!(
                        "Automatic sync is paused: a full sync is required: {reason} Run \
                         `spares sync cloud`."
                    );
                    warned_full_sync = true;
                }
                retry_at = Some(now + interval);
            }
            // Said once while it keeps failing, since it is tried again every minute.
            Err(e) => {
                if failing {
                    debug!("Automatic sync failed again: {e}");
                } else {
                    warn!("Automatic sync failed: {e}");
                }
                failing = true;
                retry_at = Some(now + RETRY_AFTER);
            }
        }
    }
}

async fn sync(db: &SqlitePool) -> Result<(), Error> {
    let db = db.clone();
    let temp_dir = get_cache_dir().join("sync");
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| Error::Sync(format!("cannot create {}: {e}", temp_dir.display())))?;
    run_exclusive(async move { sync_now(&db, SyncMode::Normal, &temp_dir).await }).await?;
    Ok(())
}

/// Local changes waiting to be synced, as seen by [`dirty_count`].
#[derive(Default)]
struct Changes {
    count: i64,
    /// When the count was first seen above zero.
    since: Option<Instant>,
    /// When the count last changed.
    changed_at: Option<Instant>,
}

impl Changes {
    /// Records `count` and tells whether to sync: once it has stayed the same for `delay`, or
    /// changes have been waiting for [`MAX_PUSH_WAIT`].
    fn due(&mut self, count: i64, now: Instant, delay: Duration) -> bool {
        if count == 0 {
            *self = Self::default();
            return false;
        }
        if count != self.count {
            self.count = count;
            self.changed_at = Some(now);
        }
        let since = *self.since.get_or_insert(now);
        let changed_at = self.changed_at.unwrap_or(now);
        now - changed_at >= delay || now - since >= MAX_PUSH_WAIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_sync_once_quiet_or_after_waiting_too_long() {
        let delay = Duration::from_secs(30);
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        let mut changes = Changes::default();
        assert!(!changes.due(0, at(0), delay));
        assert!(!changes.due(1, at(0), delay));
        assert!(!changes.due(1, at(15), delay));
        assert!(changes.due(1, at(30), delay));

        // Changes every 15 seconds wait for MAX_PUSH_WAIT.
        let mut changes = Changes::default();
        for secs in (0..120_u64).step_by(15) {
            assert!(
                !changes.due(secs.cast_signed() + 1, at(secs), delay),
                "{secs}"
            );
        }
        assert!(changes.due(100, at(120), delay));

        // Nothing left to sync starts over.
        assert!(!changes.due(0, at(135), delay));
        assert!(!changes.due(1, at(150), delay));
    }
}
