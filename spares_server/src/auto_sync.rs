//! Automatic sync, the host's one scheduler for it: a collection with an upstream (`[sync] url`)
//! and `sync.interval_minutes` syncs with it when the host starts, on waking from sleep, every
//! that many minutes, and `sync.push_delay_seconds` after local changes stop (see [`SyncConfig`]).
//! A failed attempt (the upstream being unreachable, for one) is logged and tried again a minute
//! later if it was for local changes or a wake.
//!
//! A wake syncs once the Mac has stayed awake for [`WAKE_GRACE`]. macOS wakes for a few seconds
//! at a time while the lid is closed (dark wakes, in `pmset -g log`), before Wi-Fi and Tailscale
//! are back, and a sync then would only fail.
//!
//! At most `sync_at_once` collections (in `host.toml`) sync at once; the others wait their turn.
//! A collection that syncs automatically stays open.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use log::debug;
use log::info;
use log::warn;
use spares_core::Error;
use spares_core::collection::Collection;
use spares_core::config::get_cache_dir;
use spares_core::config::read_external_config;
use spares_core::notes_vcs::run_exclusive;
use spares_core::sync::SyncConfig;
use spares_core::sync::dirty_count;
use spares_core::sync::files::sync_now;
use spares_core::sync::run::SyncMode;
use sqlx::SqlitePool;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
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
/// How long the Mac must stay awake after a wake before syncing. Most dark wakes are over by then,
/// and the network is up in those that last longer.
const WAKE_GRACE: Duration = Duration::from_secs(15);

/// The host's automatic syncs, by user.
#[derive(Debug)]
pub struct Scheduler {
    at_once: Arc<Semaphore>,
    running: std::sync::Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Scheduler {
    pub fn new(at_once: usize) -> Self {
        Self {
            at_once: Arc::new(Semaphore::new(at_once.max(1))),
            running: std::sync::Mutex::default(),
        }
    }

    /// (Re)starts syncing `collection` automatically, as its config now says, or stops it if its
    /// config says not to.
    pub async fn start(&self, collection: Arc<Collection>) {
        let id = collection.id.clone();
        let inner = Arc::clone(&collection);
        let at_once = Arc::clone(&self.at_once);
        let handle = collection
            .scope(async move { spawn_current(inner, at_once) })
            .await;
        let previous = {
            let mut running = self
                .running
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match handle {
                Some(handle) => running.insert(id, handle),
                None => running.remove(&id),
            }
        };
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    /// Stops a user's automatic sync, e.g. when their account goes.
    pub fn stop(&self, id: &str) {
        let handle = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    /// Stops every automatic sync, e.g. when the host stops.
    pub fn stop_all(&self) {
        let running = std::mem::take(
            &mut *self
                .running
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for handle in running.into_values() {
            handle.abort();
        }
    }
}

/// Whether the collection in `paths` syncs automatically, from its config file, without opening
/// it.
pub fn syncs_automatically(paths: &spares_core::collection::CollectionPaths) -> bool {
    let Ok(contents) = std::fs::read_to_string(paths.config_file()) else {
        return false;
    };
    toml_edit::de::from_str::<spares_core::config::SparesExternalConfig>(&contents)
        .is_ok_and(|config| config.sync.url.is_some() && config.sync.interval_minutes > 0)
}

fn spawn_current(collection: Arc<Collection>, at_once: Arc<Semaphore>) -> Option<JoinHandle<()>> {
    let config = read_external_config().ok()?;
    let SyncConfig {
        url: Some(url),
        interval_minutes,
        push_delay_seconds,
        ..
    } = config.sync
    else {
        return None;
    };
    if interval_minutes == 0 {
        return None;
    }
    let who = &collection.id;
    if push_delay_seconds == 0 {
        info!("{who} syncs with {url} on startup, on wake and every {interval_minutes} minutes");
    } else {
        info!(
            "{who} syncs with {url} on startup, on wake, every {interval_minutes} minutes and \
             {push_delay_seconds} seconds after local changes"
        );
    }
    let interval = Duration::from_secs(interval_minutes * 60);
    let push_delay = (push_delay_seconds > 0).then(|| Duration::from_secs(push_delay_seconds));
    let db = collection.db.clone();
    Some(spares_core::collection::spawn(async move {
        // Held, so the collection stays open while it syncs automatically
        let _collection = collection;
        run(db, interval, push_delay, at_once).await;
    }))
}

async fn run(
    db: SqlitePool,
    interval: Duration,
    push_delay: Option<Duration>,
    at_once: Arc<Semaphore>,
) {
    let mut check = tokio::time::interval(CHECK_EVERY);
    check.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut changes = Changes::default();
    let mut last_attempt: Option<Instant> = None;
    let mut retry_at: Option<Instant> = None;
    // When the last wake was seen, until a sync works
    let mut woke_at: Option<Instant> = None;
    let mut last_check = (Instant::now(), SystemTime::now());
    let mut warned_full_sync = false;
    let mut failing = false;
    loop {
        check.tick().await;
        let now = (Instant::now(), SystemTime::now());
        let wall = now.1.duration_since(last_check.1).unwrap_or_default();
        if wall.saturating_sub(now.0 - last_check.0) >= MIN_SLEEP {
            debug!("Woke from sleep; syncing if awake for {WAKE_GRACE:?}");
            woke_at = Some(now.0);
        }
        last_check = now;
        let now = now.0;
        if woke_at.is_some_and(|at| now - at < WAKE_GRACE) {
            continue;
        }

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
        if !(interval_due || woke_at.is_some() || changes_due)
            || retry_at.is_some_and(|at| now < at)
        {
            continue;
        }

        last_attempt = Some(now);
        let Ok(_turn) = at_once.acquire().await else {
            return;
        };
        match sync(&db).await {
            Ok(()) => {
                if failing {
                    info!("Automatic sync works again");
                }
                failing = false;
                warned_full_sync = false;
                woke_at = None;
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
