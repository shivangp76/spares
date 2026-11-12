//! A collection: one user's database, files and settings, and the state spares keeps for it while
//! it is open.
//!
//! A host (`spares_server`) serves many collections from one process. Each has a directory of
//! what can't be recreated, which is what to back up, and a directory of what spares recreates
//! (see `spares_server::host`):
//!
//! ```text
//! <root>/users/<user_id>/      CollectionPaths::root
//!   collection.sqlite   the database
//!   config/             config.toml and the parser templates (`<parser>/templates`)
//!   media/              images, audio and image occlusions that notes name
//!   notes.git/          the notes repository (see `crate::notes_vcs`)
//!   notes/              raw note files, a clone of the notes repository
//!   render_assets/, …
//! <cache>/users/<user_id>/     CollectionPaths::cache
//!   files/              rendered notes and cards, served at /files
//!   cards/              raw card files, which renders write from notes
//!   render/             scratch directories of renders
//!   sync/               sync's snapshots
//! ```
//!
//! [`CollectionPaths`] names them. The same shape on every host makes a collection portable:
//! moving one between hosts is copying its directory. Its cache is recreated by rendering again.
//!
//! # The current collection
//!
//! Most of `spares_core` reads its directories, config and replica through functions such as
//! [`crate::config::get_files_dir`] and [`crate::config::read_external_config`], deep inside code
//! that only gets a database pool. Those answer for the **current collection**: the one a request
//! runs in ([`Collection::scope`]), which the host sets from the request's session or token. It
//! follows the work onto other tasks and threads through [`spawn`], [`run_blocking`] and
//! [`Context`], which is how rendering on Rayon's threads finds it.
//!
//! Nothing falls back to another collection: code that asks for the current collection outside of
//! one panics, rather than read or write some other user's files. Only tests (see
//! [`crate::config::is_test_mode`]) have a default, the directories under `/tmp/spares` they have
//! always used, and a process that only ever has one collection (the CLI acting on one directly,
//! and a browser's copy) can set it with [`set_default`].

use std::cell::RefCell;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::RwLock;

use sqlx::SqlitePool;

use crate::config::Replica;

/// The file name of a collection's database in its directory.
pub const DATABASE_FILE_NAME: &str = "collection.sqlite";
/// The notes repository's directory, in a collection's directory.
pub const NOTES_REPO_DIR_NAME: &str = "notes.git";

/// The directories of a collection (see the [module docs](self)).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionPaths {
    /// The collection's own directory, `users/<user_id>` on a host: what can't be recreated. Raw
    /// note files and the other files that were in the data directory are under it.
    pub root: PathBuf,
    /// `config.toml` and parser templates
    pub config: PathBuf,
    /// Rendered files, served at `/files`, in [`Self::cache`]. API responses name files relative
    /// to it.
    pub files: PathBuf,
    /// Images, audio and image occlusions that notes name
    pub media: PathBuf,
    /// Renders' scratch directories, in [`Self::cache`]
    pub render: PathBuf,
    /// What spares recreates: rendered files, raw card files, render hashes and sync's snapshots.
    /// Outside [`Self::root`] on a host, so that backing up the root leaves it out.
    pub cache: PathBuf,
    /// The notes repository, whether or not it exists
    pub notes_repo: PathBuf,
}

impl CollectionPaths {
    /// The directories of the collection in `root`, whose cache is in `cache`.
    pub fn new(root: impl Into<PathBuf>, cache: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let cache = cache.into();
        Self {
            config: root.join("config"),
            files: cache.join("files"),
            media: root.join("media"),
            render: cache.join("render"),
            notes_repo: root.join(NOTES_REPO_DIR_NAME),
            cache,
            root,
        }
    }

    /// The directories of a collection that keeps its cache in `root/cache`.
    pub fn in_dir(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let cache = root.join("cache");
        Self::new(root, cache)
    }

    pub fn database(&self) -> PathBuf {
        self.root.join(DATABASE_FILE_NAME)
    }

    /// `sqlite://` URL of [`Self::database`]
    pub fn database_url(&self) -> String {
        format!("sqlite://{}", self.database().display())
    }

    /// Raw card files, which renders write from notes
    pub fn cards(&self) -> PathBuf {
        self.cache.join("cards")
    }

    /// The fewest directories that hold every one of these: [`Self::root`] and [`Self::cache`] on
    /// a host. What renders may read.
    pub fn dirs(&self) -> Vec<PathBuf> {
        let all = [
            &self.root,
            &self.cache,
            &self.config,
            &self.files,
            &self.media,
            &self.render,
        ];
        let mut dirs: Vec<PathBuf> = Vec::new();
        for dir in all {
            if all
                .iter()
                .any(|other| *other != dir && dir.starts_with(other))
            {
                continue;
            }
            if !dirs.contains(dir) {
                dirs.push(dir.clone());
            }
        }
        dirs
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    /// Creates the directories that don't exist yet. The notes repository is created by
    /// `crate::notes_vcs::init_server_repo` instead.
    #[cfg(not(target_family = "wasm"))]
    pub fn create(&self) -> std::io::Result<()> {
        for dir in [
            &self.root,
            &self.config,
            &self.files,
            &self.media,
            &self.render,
            &self.cache,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Whether `path` is in one of the collection's directories, without `..` on the way. Paths a
    /// request names are checked with it before they are read (see [`Self::resolve_request_path`]).
    pub fn contains(&self, path: &Path) -> bool {
        path.is_absolute()
            && self.dirs().iter().any(|dir| path.starts_with(dir))
            && !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
    }

    /// A path a request names, relative to `base` (one of these directories): refused if it is
    /// absolute, has `..`, or is empty, so a request never reaches another collection's files.
    pub fn resolve_request_path(base: &Path, relative: &str) -> Option<PathBuf> {
        let relative = Path::new(relative);
        let safe = !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
        safe.then(|| base.join(relative))
    }
}

/// What spares keeps for an open collection besides its database: its replica, the renders it is
/// running and the notes repository's locks and sessions. Process-wide statics before collections.
#[derive(Default)]
pub struct CollectionState {
    replica: RwLock<Option<Replica>>,
    pub(crate) renders: Mutex<crate::parsers::RunningRenders>,
    /// Its turns at rendering (see [`crate::render`])
    pub(crate) render_slots: crate::render::RenderSlots,
    /// Its renders at once, if its user's limits say other than the host's `[render]`; `0` if not
    pub renders_at_once: std::sync::atomic::AtomicUsize,
    pub(crate) vcs: crate::notes_vcs::VcsState,
    /// Its idle Typst workers (see `parsers::impls::typst::worker`)
    #[cfg(all(feature = "typst-render", not(target_family = "wasm")))]
    pub(crate) typst_workers: crate::parsers::impls::typst::worker::TypstWorkers,
}

impl std::fmt::Debug for CollectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectionState")
            .field("replica", &self.replica())
            .finish_non_exhaustive()
    }
}

impl CollectionState {
    pub fn replica(&self) -> Option<Replica> {
        *self
            .replica
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_replica(&self, replica: Option<Replica>) {
        *self
            .replica
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = replica;
    }
}

/// An open collection.
#[derive(Debug)]
pub struct Collection {
    /// Its user's id, which names its directory on a host
    pub id: String,
    pub db: SqlitePool,
    pub paths: Arc<CollectionPaths>,
    pub state: Arc<CollectionState>,
}

impl Collection {
    pub fn new(id: impl Into<String>, db: SqlitePool, paths: CollectionPaths) -> Self {
        Self {
            id: id.into(),
            db,
            paths: Arc::new(paths),
            state: Arc::default(),
        }
    }

    /// This host's replica for the collection (see [`crate::ids`]).
    pub fn replica(&self) -> Option<Replica> {
        self.state.replica()
    }

    pub fn set_replica(&self, replica: Option<Replica>) {
        self.state.set_replica(replica);
    }

    /// Runs `future` with this as the current collection.
    pub async fn scope<F: Future>(self: &Arc<Self>, future: F) -> F::Output {
        CURRENT.scope(Context(Some(Arc::clone(self))), future).await
    }

    /// Makes this the current collection on this thread until the guard is dropped, for code that
    /// doesn't run on a task, such as `#[test]`s and command-line tools.
    pub fn enter(self: &Arc<Self>) -> EnterGuard {
        Context(Some(Arc::clone(self))).enter()
    }
}

/// The current collection, if any, as a value that can be sent to another thread or task and
/// entered there ([`Context::enter`], [`Context::run`]).
#[derive(Clone, Debug, Default)]
pub struct Context(Option<Arc<Collection>>);

impl Context {
    /// Makes this the current collection on this thread until the guard is dropped.
    pub fn enter(&self) -> EnterGuard {
        let previous = THREAD_CURRENT.with(|current| current.replace(self.0.clone()));
        EnterGuard { previous }
    }

    /// Runs `f` in this context.
    pub fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        let _guard = self.enter();
        f()
    }

    /// Runs `future` in this context.
    pub async fn scope<F: Future>(self, future: F) -> F::Output {
        CURRENT.scope(self, future).await
    }

    pub fn collection(&self) -> Option<&Arc<Collection>> {
        self.0.as_ref()
    }
}

/// Restores the thread's previous collection when dropped (see [`Context::enter`]).
pub struct EnterGuard {
    previous: Option<Arc<Collection>>,
}

impl Drop for EnterGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        THREAD_CURRENT.with(|current| *current.borrow_mut() = previous);
    }
}

tokio::task_local! {
    static CURRENT: Context;
}

thread_local! {
    static THREAD_CURRENT: RefCell<Option<Arc<Collection>>> = const { RefCell::new(None) };
}

static DEFAULT: RwLock<Option<Arc<Collection>>> = RwLock::new(None);

/// Sets the collection that is current wherever no other is, for a process that only has one: a
/// browser's copy, and the CLI acting on a collection directly. A host never sets one.
pub fn set_default(collection: Option<Arc<Collection>>) {
    *DEFAULT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = collection;
}

/// The current collection: the task's ([`Collection::scope`]), else the thread's
/// ([`Collection::enter`]), else the default ([`set_default`]).
pub fn current() -> Option<Arc<Collection>> {
    if let Ok(Some(collection)) = CURRENT.try_with(|context| context.0.clone()) {
        return Some(collection);
    }
    if let Some(collection) = THREAD_CURRENT.with(|current| current.borrow().clone()) {
        return Some(collection);
    }
    DEFAULT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The current collection, to carry to another task or thread.
pub fn capture() -> Context {
    Context(current())
}

/// The directories under `/tmp/spares` that tests use when they open no collection.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn test_paths() -> Arc<CollectionPaths> {
    static PATHS: std::sync::LazyLock<Arc<CollectionPaths>> = std::sync::LazyLock::new(|| {
        let root = PathBuf::from("/tmp/spares");
        let cache = root.join("cache");
        let data = root.join("data");
        let paths = CollectionPaths {
            config: root.join("config"),
            // As before collections: rendered files in the cache directory, which is the files
            // directory, and images and the like in the data directory.
            files: cache.clone(),
            media: data.clone(),
            render: cache.join("render"),
            cache,
            notes_repo: data.join(NOTES_REPO_DIR_NAME),
            root: data,
        };
        Arc::new(paths)
    });
    Arc::clone(&PATHS)
}

fn test_state() -> Arc<CollectionState> {
    static STATE: std::sync::LazyLock<Arc<CollectionState>> =
        std::sync::LazyLock::new(Arc::default);
    Arc::clone(&STATE)
}

/// A browser's directories: paths under `/spares` in its own files (see [`crate::platform`]).
#[cfg(target_family = "wasm")]
pub fn device_paths() -> CollectionPaths {
    let root = PathBuf::from("/spares");
    CollectionPaths {
        config: root.join("config"),
        files: root.join("cache"),
        media: root.join("data"),
        render: root.join("cache").join("render"),
        cache: root.join("cache"),
        notes_repo: root.join("data").join(NOTES_REPO_DIR_NAME),
        root: root.join("data"),
    }
}

#[track_caller]
fn no_collection() -> ! {
    panic!(
        "no collection is current here. Requests run in their user's collection \
         (`Collection::scope`); work moved to another task or thread takes it along \
         (`collection::spawn`, `collection::capture`)."
    )
}

/// The current collection's directories. Panics without one, except under tests and in a
/// browser.
#[track_caller]
pub fn paths() -> Arc<CollectionPaths> {
    if let Some(collection) = current() {
        return Arc::clone(&collection.paths);
    }
    #[cfg(target_family = "wasm")]
    {
        static PATHS: std::sync::LazyLock<Arc<CollectionPaths>> =
            std::sync::LazyLock::new(|| Arc::new(device_paths()));
        Arc::clone(&PATHS)
    }
    #[cfg(not(target_family = "wasm"))]
    {
        if crate::config::is_test_mode() {
            let paths = test_paths();
            for dir in [&paths.config, &paths.files, &paths.root] {
                let _ = std::fs::create_dir_all(dir);
            }
            return paths;
        }
        no_collection()
    }
}

/// The current collection's state. As [`paths`].
#[track_caller]
pub fn state() -> Arc<CollectionState> {
    if let Some(collection) = current() {
        return Arc::clone(&collection.state);
    }
    if cfg!(target_family = "wasm") || crate::config::is_test_mode() {
        return test_state();
    }
    no_collection()
}

/// The current collection's id, if there is one.
pub fn current_id() -> Option<String> {
    current().map(|collection| collection.id.clone())
}

/// [`tokio::spawn`], in the current collection.
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(capture().scope(future))
}

/// Runs CPU-heavy `f` on a blocking thread, in the current collection, so it does not stall other
/// requests on the async runtime. A panic in `f` is resumed in the caller. In a browser, which has
/// neither threads nor a Tokio runtime, `f` runs on the caller's thread.
pub async fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    if cfg!(target_family = "wasm") {
        return f();
    }
    let context = capture();
    tokio::task::spawn_blocking(move || context.run(f))
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
}

/// A lock that only guards in-memory state, where a panic while holding it leaves the value whole.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collection(root: &Path) -> Arc<Collection> {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        Arc::new(Collection::new("u1", db, CollectionPaths::in_dir(root)))
    }

    #[tokio::test]
    async fn the_current_collection_follows_tasks_and_threads() {
        let a = collection(Path::new("/srv/spares/users/a")).await;
        let b = collection(Path::new("/srv/spares/users/b")).await;
        assert!(current().is_none());
        a.scope(async {
            assert_eq!(paths().root, Path::new("/srv/spares/users/a"));
            b.scope(async { assert_eq!(current_id().as_deref(), Some("u1")) })
                .await;
            assert_eq!(paths().files, Path::new("/srv/spares/users/a/cache/files"));
            let spawned = spawn(async { paths().root.clone() }).await.unwrap();
            assert_eq!(spawned, Path::new("/srv/spares/users/a"));
            let blocking = run_blocking(|| paths().root.clone()).await;
            assert_eq!(blocking, Path::new("/srv/spares/users/a"));
            let context = capture();
            let threaded = std::thread::spawn(move || context.run(|| paths().root.clone()))
                .join()
                .unwrap();
            assert_eq!(threaded, Path::new("/srv/spares/users/a"));
        })
        .await;
        assert!(current().is_none());
        // Tests have the old directories when no collection is current.
        assert_eq!(paths().files, Path::new("/tmp/spares/cache"));
        {
            let _guard = b.enter();
            assert_eq!(paths().root, Path::new("/srv/spares/users/b"));
        }
        assert!(current().is_none());
    }

    #[test]
    fn request_paths_stay_in_their_directory() {
        let base = Path::new("/srv/spares/users/a/files");
        assert_eq!(
            CollectionPaths::resolve_request_path(base, "typst/abc.pdf"),
            Some(base.join("typst/abc.pdf"))
        );
        for bad in [
            "",
            "../b/files/x.pdf",
            "/etc/passwd",
            "typst/../../x",
            "./x",
        ] {
            assert_eq!(
                CollectionPaths::resolve_request_path(base, bad),
                None,
                "{bad}"
            );
        }
        let paths = CollectionPaths::new("/srv/spares/users/a", "/var/cache/spares/users/a");
        assert_eq!(
            paths.dirs(),
            [
                PathBuf::from("/srv/spares/users/a"),
                PathBuf::from("/var/cache/spares/users/a")
            ]
        );
        assert_eq!(
            CollectionPaths::in_dir("/srv/spares/users/a").dirs(),
            [PathBuf::from("/srv/spares/users/a")]
        );
        assert_eq!(
            test_paths().dirs(),
            [
                PathBuf::from("/tmp/spares/data"),
                PathBuf::from("/tmp/spares/cache"),
                PathBuf::from("/tmp/spares/config")
            ]
        );
        assert!(paths.contains(Path::new("/srv/spares/users/a/media/images/x.png")));
        assert!(paths.contains(Path::new("/var/cache/spares/users/a/files/x.pdf")));
        assert!(!paths.contains(Path::new("/var/cache/spares/users/b/files/x.pdf")));
        assert!(!paths.contains(Path::new("/srv/spares/users/b/media/images/x.png")));
        assert!(!paths.contains(Path::new("/srv/spares/users/a/../b/x.png")));
        assert!(!paths.contains(Path::new("users/a/x.png")));
    }
}
