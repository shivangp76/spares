//! A host: one `spares_server` process serving the collections of its users, from one root
//! directory.
//!
//! The laptop, a personal VPS and a public host are all hosts, and run the same code. They differ
//! only in `host.toml` ([`HostConfig`]): the laptop's has one account and signups closed, so it only
//! ever holds its owner's collection.
//!
//! ```text
//! <SPARES_ROOT>/               ~/.local/share/spares: what can't be recreated
//!   host.toml          bind address, signup policy, frontend directory, limits
//!   accounts.sqlite    users, sessions, device tokens (see [`accounts`])
//!   users/<user_id>/   a collection (see `spares_core::collection`)
//! <cache_dir>/                 ~/.cache/spares: what spares recreates (see [`cache_root`])
//!   users/<user_id>/   the collection's rendered files, raw card files and other caches
//! ```
//!
//! Backing up the root is enough to recreate the rest: a collection's cache is filled again by
//! rendering its notes.
//!
//! Whoever can read the root is the host's admin: `spares host` commands work on these files
//! directly, without logging in.
//!
//! **Collections are opened on demand** ([`Host::collection`]) and kept in a least-recently-used
//! cache of at most `max_open_collections`. One that nothing has used for `idle_minutes` is closed
//! ([`Host::close_idle`]), as is the least recently used one when another must open; never one a
//! request is still using. A collection is migrated when it is first opened, under a lock of its
//! own, so two requests opening it at once migrate it once.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use log::info;
use log::warn;
use spares_core::Error;
use spares_core::collection::Collection;
use spares_core::collection::CollectionPaths;
use spares_core::config::Replica;
use spares_core::config::SparesExternalConfig;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::Mutex;

pub mod accounts;
pub mod admin;
pub mod auth;
pub mod config;
pub mod export;
pub mod limits;
pub mod lock;
pub mod signup;

pub use config::HostConfig;
pub use config::Signups;

use self::accounts::invalid;
use self::accounts::sqlx_error;

/// The environment variable naming a host's root.
pub const ROOT_ENV: &str = "SPARES_ROOT";
/// Where users' directories are, in the root and in the cache root.
pub const USERS_DIR_NAME: &str = "users";
/// The cache root's name in the root, for a host outside the default root (see [`cache_root`]).
pub const CACHE_DIR_NAME: &str = "cache";

/// The host's root: `SPARES_ROOT`, else `$XDG_DATA_HOME/spares`
/// (`~/.local/share/spares`).
pub fn default_root() -> PathBuf {
    if let Some(root) = std::env::var_os(ROOT_ENV).filter(|root| !root.is_empty()) {
        return PathBuf::from(root);
    }
    xdg_dir("XDG_DATA_HOME", ".local/share").join("spares")
}

/// `$<var>`, else `~/<fallback>`.
fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|dir| !dir.is_empty())
        .map_or_else(
            || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback),
            PathBuf::from,
        )
}

/// Where the host at `root` keeps users' caches: `host.toml`'s `cache_dir`, else
/// `$XDG_CACHE_HOME/spares` (`~/.cache/spares`) if `root` is `$XDG_DATA_HOME/spares`, else
/// `<root>/cache`. Only the host in the default root gets the default cache, so a scratch host in
/// another root never shares (or clears) its caches.
pub fn cache_root(root: &Path, config: &HostConfig) -> PathBuf {
    if let Some(dir) = &config.cache_dir {
        return dir.clone();
    }
    let data_root = xdg_dir("XDG_DATA_HOME", ".local/share").join("spares");
    // A VPS names its root by the laptop's path, a link to the same directory.
    let same = root == data_root
        || matches!(
            (root.canonicalize(), data_root.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        );
    if same {
        xdg_dir("XDG_CACHE_HOME", ".cache").join("spares")
    } else {
        root.join(CACHE_DIR_NAME)
    }
}

/// The cache of user `id` in the cache root `cache_root`.
pub fn user_cache_dir(cache_root: &Path, id: &str) -> PathBuf {
    cache_root.join(USERS_DIR_NAME).join(id)
}

/// The directory of user `id` in the host at `root`.
pub fn user_dir(root: &Path, id: &str) -> PathBuf {
    root.join(USERS_DIR_NAME).join(id)
}

struct OpenCollection {
    collection: Arc<Collection>,
    last_used: Instant,
}

/// A running host.
pub struct Host {
    pub root: PathBuf,
    /// Where users' caches are (see [`cache_root`])
    pub cache_root: PathBuf,
    pub config: HostConfig,
    pub accounts: SqlitePool,
    open: Mutex<HashMap<String, OpenCollection>>,
    /// One per user, held while their collection opens (and migrates)
    opening: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// The automatic syncs of collections with an upstream
    pub sync: crate::auto_sync::Scheduler,
    /// Requests and failed logins (see [`limits`])
    pub counters: limits::Counters,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("root", &self.root)
            .field("cache_root", &self.cache_root)
            .finish_non_exhaustive()
    }
}

impl Host {
    /// Opens the host at `root`, which `spares host init` created.
    pub async fn open(root: &Path) -> Result<Arc<Self>, Error> {
        let accounts_path = root.join(accounts::ACCOUNTS_FILE_NAME);
        if !accounts_path.is_file() {
            return Err(invalid(format!(
                "{} is not a spares host: it has no {}. Create one with `spares host init`.",
                root.display(),
                accounts::ACCOUNTS_FILE_NAME
            )));
        }
        let config = HostConfig::read(root)?;
        let accounts = accounts::open(&accounts_path).await?;
        Ok(Arc::new(Self {
            root: root.to_path_buf(),
            cache_root: cache_root(root, &config),
            sync: crate::auto_sync::Scheduler::new(config.sync_at_once),
            counters: limits::Counters::default(),
            config,
            accounts,
            open: Mutex::default(),
            opening: std::sync::Mutex::default(),
        }))
    }

    /// Creates a host at `root` with `host.toml`'s defaults (`bind` aside) and no accounts.
    pub async fn create(root: &Path, bind: &str) -> Result<Arc<Self>, Error> {
        let io = |description: String| {
            move |source| Error::Io {
                description,
                source,
            }
        };
        std::fs::create_dir_all(root.join(USERS_DIR_NAME))
            .map_err(io(format!("Failed to create {}", root.display())))?;
        let config_path = root.join(config::HOST_CONFIG_FILE_NAME);
        if !config_path.exists() {
            std::fs::write(&config_path, HostConfig::template(bind))
                .map_err(io(format!("Failed to write {}", config_path.display())))?;
        }
        accounts::open(&root.join(accounts::ACCOUNTS_FILE_NAME)).await?;
        restrict_to_owner(root);
        Self::open(root).await
    }

    pub fn user_dir(&self, id: &str) -> PathBuf {
        user_dir(&self.root, id)
    }

    /// User `id`'s cache
    pub fn user_cache_dir(&self, id: &str) -> PathBuf {
        user_cache_dir(&self.cache_root, id)
    }

    pub fn paths(&self, id: &str) -> CollectionPaths {
        CollectionPaths::new(self.user_dir(id), self.user_cache_dir(id))
    }

    /// Adds an account, with an empty collection: its directory, database and notes repository.
    pub async fn create_user(&self, new: accounts::NewUser<'_>) -> Result<accounts::User, Error> {
        // A directory without a collection (what a setup that stopped halfway leaves) is fine.
        if let Some(id) = new.id
            && self.paths(id).database().exists()
        {
            return Err(invalid(format!(
                "{} already exists. Import an export into it with `spares host import`, or \
                 remove it first.",
                self.user_dir(id).display()
            )));
        }
        let user = accounts::create_user(&self.accounts, new).await?;
        let created = async {
            let collection = self.collection(&user.id).await?;
            collection
                .scope(async {
                    // Without git, notes are kept as plain files until it is installed and the
                    // repository is made (`spares vcs init`).
                    let has_git = std::process::Command::new("git")
                        .arg("--version")
                        .output()
                        .is_ok_and(|o| o.status.success());
                    if !has_git {
                        warn!(
                            "git isn't installed, so {} has no notes repository",
                            user.id
                        );
                    } else if !spares_core::notes_vcs::enabled() {
                        spares_core::notes_vcs::init_server_repo(&collection.db).await?;
                    }
                    Ok::<_, Error>(())
                })
                .await
        }
        .await;
        if let Err(e) = created {
            // No account without its collection
            self.close(&user.id).await;
            let _ = accounts::delete_user(&self.accounts, &user.id).await;
            let _ = std::fs::remove_dir_all(self.user_dir(&user.id));
            let _ = std::fs::remove_dir_all(self.user_cache_dir(&user.id));
            return Err(e);
        }
        Ok(user)
    }

    /// Deletes an account, its directory and its cache.
    pub async fn delete_user(&self, id: &str) -> Result<(), Error> {
        self.close(id).await;
        accounts::delete_user(&self.accounts, id).await?;
        for dir in [self.user_dir(id), self.user_cache_dir(id)] {
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| Error::Io {
                    description: format!("Failed to remove {}", dir.display()),
                    source: e,
                })?;
            }
        }
        Ok(())
    }

    /// User `id`'s collection, opening it if it isn't open.
    pub async fn collection(&self, id: &str) -> Result<Arc<Collection>, Error> {
        if let Some(open) = self.open.lock().await.get_mut(id) {
            open.last_used = Instant::now();
            return Ok(Arc::clone(&open.collection));
        }
        let opening = {
            let mut opening = self
                .opening
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(opening.entry(id.to_string()).or_default())
        };
        let _opening = opening.lock().await;
        // Another request may have opened it meanwhile.
        if let Some(open) = self.open.lock().await.get_mut(id) {
            open.last_used = Instant::now();
            return Ok(Arc::clone(&open.collection));
        }
        let paths = self.paths(id);
        let collection =
            open_collection(id, paths, self.config.max_connections_per_collection).await?;
        let mut open = self.open.lock().await;
        if open.len() >= self.config.max_open_collections {
            evict_least_recently_used(&mut open).await;
        }
        open.insert(
            id.to_string(),
            OpenCollection {
                collection: Arc::clone(&collection),
                last_used: Instant::now(),
            },
        );
        Ok(collection)
    }

    /// The collections that are open, for the host's background work.
    pub async fn open_collections(&self) -> Vec<Arc<Collection>> {
        self.open
            .lock()
            .await
            .values()
            .map(|open| Arc::clone(&open.collection))
            .collect()
    }

    /// Closes the collections nothing has used for `idle_minutes`.
    pub async fn close_idle(&self) {
        let idle = Duration::from_secs(self.config.idle_minutes * 60);
        let mut open = self.open.lock().await;
        let idle_ids: Vec<String> = open
            .iter()
            .filter(|(_, open)| open.last_used.elapsed() >= idle && in_use(open) == 0)
            .map(|(id, _)| id.clone())
            .collect();
        for id in idle_ids {
            if let Some(closed) = open.remove(&id) {
                info!("Closing the idle collection of {id}");
                closed.collection.db.close().await;
            }
        }
    }

    /// Closes user `id`'s collection, if it is open.
    pub async fn close(&self, id: &str) {
        let closed = self.open.lock().await.remove(id);
        if let Some(closed) = closed {
            closed.collection.db.close().await;
        }
    }

    /// User `id`'s limits: the host's, with their account's overrides.
    pub async fn limits_for(&self, id: &str) -> Result<limits::Limits, Error> {
        Ok(self
            .config
            .limits
            .with(&limits::overrides(&self.accounts, id).await?))
    }

    /// Measures the directories of the open collections, and writes what requests were counted,
    /// to `usage`.
    pub async fn record_usage(&self) {
        for collection in self.open_collections().await {
            let dirs = collection.paths.dirs();
            let bytes = tokio::task::spawn_blocking(move || {
                dirs.iter().map(|dir| limits::directory_size(dir)).sum()
            })
            .await
            .unwrap_or_default();
            if let Err(e) = limits::record_storage(&self.accounts, &collection.id, bytes).await {
                warn!("Failed to record the storage of {}: {e}", collection.id);
            }
        }
        if let Err(e) = self.counters.record(&self.accounts).await {
            warn!("Failed to record requests: {e}");
        }
    }

    /// Removes what renders of the open collections left in their scratch directories over an
    /// hour ago (see `spares_core::render`).
    pub async fn clean_render_scratch(&self) {
        let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        for collection in self.open_collections().await {
            let tmp = collection.paths.render.join("tmp");
            let Ok(entries) = std::fs::read_dir(&tmp) else {
                continue;
            };
            for entry in entries.flatten() {
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|modified| modified < hour_ago);
                if old {
                    let path = entry.path();
                    let _ = if path.is_dir() {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                }
            }
        }
    }

    /// Closes every collection, e.g. when the host stops.
    pub async fn close_all(&self) {
        let all: Vec<OpenCollection> = self.open.lock().await.drain().map(|(_, o)| o).collect();
        for open in all {
            open.collection.db.close().await;
        }
    }

    /// Opens and migrates every user's collection in turn, for `spares host migrate --all`.
    pub async fn migrate_all(&self) -> Result<Vec<String>, Error> {
        let mut migrated = Vec::new();
        for user in accounts::users(&self.accounts).await? {
            self.collection(&user.id).await?;
            self.close(&user.id).await;
            migrated.push(user.id);
        }
        Ok(migrated)
    }
}

/// Requests holding the collection, besides the cache.
fn in_use(open: &OpenCollection) -> usize {
    Arc::strong_count(&open.collection) - 1
}

async fn evict_least_recently_used(open: &mut HashMap<String, OpenCollection>) {
    let unused = open
        .iter()
        .filter(|(_, open)| in_use(open) == 0)
        .min_by_key(|(_, open)| open.last_used)
        .map(|(id, _)| id.clone());
    match unused {
        Some(id) => {
            if let Some(closed) = open.remove(&id) {
                info!("Closing the collection of {id} to open another");
                closed.collection.db.close().await;
            }
        }
        // Every open collection is in use: one more is open for now.
        None => warn!(
            "Every one of the {} open collections is in use; opening one more",
            open.len()
        ),
    }
}

/// Opens the collection in `paths`, creating its directories and an empty database if it has
/// none, and migrates it.
pub async fn open_collection(
    id: &str,
    paths: CollectionPaths,
    max_connections: u32,
) -> Result<Arc<Collection>, Error> {
    paths.create().map_err(|e| Error::Io {
        description: format!("Failed to create {}", paths.root.display()),
        source: e,
    })?;
    let options = SqliteConnectOptions::from_str(&paths.database_url())
        .map_err(sqlx_error)?
        .create_if_missing(true)
        .with_regexp()
        // Reads go through memory shared by every connection, rather than through each
        // connection's own page cache of 2 MB, so the statistics' queries over the whole review
        // log run at the same speed on whichever connection they get.
        .pragma("mmap_size", "268435456");
    let db = SqlitePoolOptions::new()
        .max_connections(max_connections.max(1))
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_with(options)
        .await
        .map_err(|e| {
            Error::Sync(format!(
                "Failed to open {}: {e}",
                paths.database().display()
            ))
        })?;
    sqlx::migrate!("../spares_core/migrations")
        .run(&db)
        .await
        .map_err(|e| {
            Error::Sync(format!(
                "Failed to migrate {}: {e}",
                paths.database().display()
            ))
        })?;
    spares_core::ids::ensure_note_ids_migrated(&db).await?;
    let collection = Arc::new(Collection::new(id, db, paths));
    collection
        .scope(async {
            collection.set_replica(Some(collection_replica()?));
            // Before serving: settle a push the host stopped in the middle of, and commit or
            // revert note files left uncommitted.
            spares_core::notes_vcs::startup(&collection.db).await;
            Ok::<_, Error>(())
        })
        .await?;
    Ok(collection)
}

/// The current collection's replica on this host, from its config (see
/// [`spares_core::sync::SyncConfig::replica`]).
pub fn collection_replica() -> Result<Replica, Error> {
    let config: SparesExternalConfig = spares_core::config::read_external_config()?;
    match (config.sync.replica, &config.sync.url) {
        (Some(number), _) => Replica::try_from(number)
            .map_err(|e| invalid(format!("`[sync] replica` in config.toml: {e}"))),
        (None, None) => Ok(Replica::ROOT),
        (None, Some(url)) => Err(invalid(format!(
            "this collection syncs with {url} but has no `[sync] replica` in its config.toml. \
             Log in to {url} with `spares login`, which enrolls this host's copy."
        ))),
    }
}

/// Keeps the host's files (users' collections, password hashes) from other accounts on the
/// machine.
fn restrict_to_owner(root: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)) {
            warn!("Failed to make {} private: {e}", root.display());
        }
    }
    #[cfg(not(unix))]
    let _ = root;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("spares-host-{}", accounts::random_string(10)))
    }

    /// A host in a temporary directory, with one account, `me`, whose password is `password1`.
    pub(crate) async fn test_host() -> (Arc<Host>, TempRoot) {
        let root = temp_root();
        let host = Host::create(&root, "127.0.0.1:0").await.unwrap();
        host.create_user(accounts::NewUser {
            id: Some("me"),
            name: "me",
            email: None,
            password: "password1",
            admin: true,
            email_verified: true,
        })
        .await
        .unwrap();
        (host, TempRoot(root))
    }

    /// Removes a test host's root when dropped.
    pub(crate) struct TempRoot(pub(crate) PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn new_user<'a>(id: &'a str, name: &'a str) -> accounts::NewUser<'a> {
        accounts::NewUser {
            id: Some(id),
            name,
            email: None,
            password: "password1",
            admin: false,
            email_verified: false,
        }
    }

    /// Each user's collection is in their own directory, and closing and reopening keeps it.
    #[tokio::test]
    async fn a_host_keeps_collections_apart() {
        let root = temp_root();
        let host = Host::create(&root, "127.0.0.1:0").await.unwrap();
        assert!(root.join("host.toml").is_file());
        host.create_user(new_user("alice", "alice")).await.unwrap();
        host.create_user(new_user("bob", "bob")).await.unwrap();
        assert!(host.create_user(new_user("alice", "alice2")).await.is_err());
        // A directory left without a collection takes an account.
        std::fs::create_dir_all(root.join("users/carol/config")).unwrap();
        host.create_user(new_user("carol", "carol")).await.unwrap();
        assert!(root.join("users/carol/collection.sqlite").is_file());
        let alice = host.collection("alice").await.unwrap();
        let bob = host.collection("bob").await.unwrap();
        assert_eq!(alice.replica(), Some(Replica::ROOT));
        assert!(root.join("users/alice/collection.sqlite").is_file());
        assert!(root.join("users/alice/notes.git/.git").is_dir());
        sqlx::query("INSERT INTO app_state (key, value) VALUES ('who', 'alice')")
            .execute(&alice.db)
            .await
            .unwrap();
        let who = |c: Arc<Collection>| async move {
            sqlx::query_scalar::<_, String>("SELECT value FROM app_state WHERE key = 'who'")
                .fetch_optional(&c.db)
                .await
                .unwrap()
        };
        assert_eq!(who(Arc::clone(&bob)).await, None);
        alice
            .scope(async {
                assert_eq!(
                    spares_core::config::get_files_dir(),
                    root.join("cache/users/alice/files")
                );
            })
            .await;
        drop(alice);
        host.close("alice").await;
        assert_eq!(host.open_collections().await.len(), 2, "bob's and carol's");
        let alice = host.collection("alice").await.unwrap();
        assert_eq!(who(alice).await.as_deref(), Some("alice"));
        host.delete_user("bob").await.unwrap();
        assert!(!root.join("users/bob").exists());
        assert!(!root.join("cache/users/bob").exists());
        host.close_all().await;
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The default root's caches are in the default cache, and another root's in itself.
    #[test]
    fn caches_are_out_of_the_default_root() {
        let config = HostConfig::default();
        assert_eq!(
            cache_root(Path::new("/tmp/scratch-host"), &config),
            Path::new("/tmp/scratch-host/cache")
        );
        assert_eq!(
            cache_root(
                &xdg_dir("XDG_DATA_HOME", ".local/share").join("spares"),
                &config
            ),
            xdg_dir("XDG_CACHE_HOME", ".cache").join("spares")
        );
        let config = HostConfig {
            cache_dir: Some(PathBuf::from("/var/cache/spares")),
            ..HostConfig::default()
        };
        assert_eq!(
            cache_root(Path::new("/tmp/scratch-host"), &config),
            Path::new("/var/cache/spares")
        );
    }

    /// The least recently used collection that nothing holds is closed to open another.
    #[tokio::test]
    async fn a_host_closes_collections_past_its_limit() {
        let root = temp_root();
        Host::create(&root, "127.0.0.1:0").await.unwrap();
        std::fs::write(
            root.join("host.toml"),
            "max_open_collections = 2\nidle_minutes = 0\n",
        )
        .unwrap();
        let host = Host::open(&root).await.unwrap();
        for id in ["a", "b", "c"] {
            host.create_user(new_user(id, id)).await.unwrap();
        }
        let a = host.collection("a").await.unwrap();
        drop(host.collection("b").await.unwrap());
        drop(host.collection("c").await.unwrap());
        // `a` is held, so `b` was closed.
        let mut open: Vec<String> = host
            .open_collections()
            .await
            .iter()
            .map(|c| c.id.clone())
            .collect();
        open.sort();
        assert_eq!(open, ["a", "c"]);
        host.close_idle().await;
        let open: Vec<String> = host
            .open_collections()
            .await
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(open, ["a"]);
        drop(a);
        host.close_all().await;
        std::fs::remove_dir_all(&root).unwrap();
    }
}
