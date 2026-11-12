use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OwnedRwLockReadGuard;
use tokio::sync::OwnedRwLockWriteGuard;
use tokio::sync::RwLock;

use crate::Error;

/// Serializes changes to a collection's notes against pushes. Requests that change notes hold it
/// shared for their whole duration (database write, file write and commit). A push holds it
/// exclusively, from checking `HEAD` until git has updated the ref, so no other change can slip in
/// between. Each collection has its own (`VcsState::lock`).
fn notes_lock() -> Arc<RwLock<()>> {
    Arc::clone(&crate::collection::state().vcs.lock)
}

/// How long a request that changes notes waits for a push before giving up with
/// [`Error::NotesVcsBusy`]. Nothing has been written at that point, so it can simply be retried.
pub const WRITER_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum NotesGuard {
    Shared(OwnedRwLockReadGuard<()>),
    Exclusive(OwnedRwLockWriteGuard<()>),
}

pub async fn shared_guard() -> Result<NotesGuard, Error> {
    let lock = notes_lock().read_owned();
    // A browser has no pushes to wait for, and no Tokio timer to wait with.
    if cfg!(target_family = "wasm") {
        return Ok(NotesGuard::Shared(lock.await));
    }
    tokio::time::timeout(WRITER_WAIT, lock)
        .await
        .map(NotesGuard::Shared)
        .map_err(|_| Error::NotesVcsBusy)
}

/// Waits for the exclusive guard, forever if `wait` is `None`.
pub async fn exclusive_guard(wait: Option<Duration>) -> Result<NotesGuard, Error> {
    let lock = notes_lock().write_owned();
    // As in `shared_guard`
    if cfg!(target_family = "wasm") {
        return Ok(NotesGuard::Exclusive(lock.await));
    }
    match wait {
        Some(wait) => tokio::time::timeout(wait, lock)
            .await
            .map(NotesGuard::Exclusive)
            .map_err(|_| Error::NotesVcsBusy),
        None => Ok(NotesGuard::Exclusive(lock.await)),
    }
}

/// Runs `future` to completion on its own task while holding `guard`. A client disconnecting drops
/// the caller's future, but not this task, so a change cannot stop between writing the database
/// and committing its files.
pub async fn run_holding<T, F>(guard: NotesGuard, future: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: Future<Output = Result<T, Error>> + Send + 'static,
{
    // A browser has no Tokio runtime to spawn on, and nothing there cancels a request midway.
    if cfg!(target_family = "wasm") {
        let _guard = guard;
        return future.await;
    }
    let handle = crate::collection::spawn(async move {
        let _guard = guard;
        future.await
    });
    handle
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))
}

/// [`run_holding`] with a shared guard: for ordinary requests that change notes.
pub async fn run_shared<T, F>(future: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: Future<Output = Result<T, Error>> + Send + 'static,
{
    run_holding(shared_guard().await?, future).await
}

/// [`run_holding`] with an exclusive guard, waiting at most [`WRITER_WAIT`]: for requests whose
/// database changes are followed by regenerating files of arbitrary notes (undo, tag and parser
/// changes), which must not interleave with other writers.
pub async fn run_exclusive<T, F>(future: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: Future<Output = Result<T, Error>> + Send + 'static,
{
    run_holding(exclusive_guard(Some(WRITER_WAIT)).await?, future).await
}
