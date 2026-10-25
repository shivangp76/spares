use std::future::Future;
use std::pin::pin;
use std::time::Duration;

use futures_util::future::Either;
use futures_util::future::select;

use crate::rt::TimeoutError;

pub async fn sleep(duration: Duration) {
    timeout_future(duration).await;
}

pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, TimeoutError> {
    match select(pin!(future), timeout_future(duration)).await {
        Either::Left((result, _)) => Ok(result),
        Either::Right(_) => Err(TimeoutError),
    }
}

fn timeout_future(duration: Duration) -> impl Future {
    async_io::Timer::after(duration)
}
