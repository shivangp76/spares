//! The runtime on `wasm32-unknown-unknown`, which has no threads, and where `std::time::Instant`
//! panics. Tasks run on the browser's event loop, and timers on `setTimeout`.

use std::future::Future;
use std::pin::pin;
use std::time::Duration;

use futures_channel::oneshot;
use futures_util::future::Either;
use futures_util::future::select;

use super::TimeoutError;

/// As with Tokio, dropping the handle detaches the task rather than cancelling it. sqlx relies on
/// this: it drops the handle of the task that returns a connection to the pool.
pub fn spawn<F: Future + 'static>(fut: F) -> oneshot::Receiver<F::Output> {
    let (tx, rx) = oneshot::channel();
    wasm_bindgen_futures::spawn_local(async move {
        let _ = tx.send(fut.await);
    });
    rx
}

/// The timer runs as its own task, since sqlx's futures must be `Send` and a JS timer isn't.
pub async fn sleep(duration: Duration) {
    let (tx, rx) = oneshot::channel::<()>();
    wasm_bindgen_futures::spawn_local(async move {
        gloo_timers::future::sleep(duration).await;
        let _ = tx.send(());
    });
    let _ = rx.await;
}

pub async fn timeout<F: Future>(duration: Duration, f: F) -> Result<F::Output, TimeoutError> {
    match select(pin!(f), pin!(sleep(duration))).await {
        Either::Left((output, _)) => Ok(output),
        Either::Right(_) => Err(TimeoutError),
    }
}
