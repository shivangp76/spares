# Vendored sqlx

`sqlx-core` and `sqlx-sqlite` 0.9.0, patched so that sqlx runs on `wasm32-unknown-unknown`: in a
browser, on SQLite compiled to wasm. The root `Cargo.toml` uses them through `[patch.crates-io]`.
The rest of sqlx (`sqlx`, `sqlx-macros`, …) comes from crates.io as usual.

Why: the offline web app runs `spares_core` in the browser, and every query in core goes through
sqlx. Upstream sqlx doesn't support wasm. Patching it keeps all of core's queries as they are.

All changes are under `cfg(all(target_family = "wasm", target_os = "unknown"))`, so native builds
run the same code as before. The exceptions are mechanical: `web_time::Instant`, which is
`std::time::Instant` natively, and the SQLite connection's command loop, which moved into an
`async fn` that the native thread `block_on`s.

`./diff.sh` prints the whole patch, as a diff against the crates.io releases.

## What changed

- **sqlx-sqlite:**
  - `crate::ffi` (`lib.rs`) is `libsqlite3_sys` natively and `sqlite_wasm_rs` on wasm. Both export
    the same `sqlite3_*` functions, so every `libsqlite3_sys::` became `crate::ffi::`.
    `libsqlite3-sys` is a native-only dependency.
  - `connection/worker.rs`: the command loop is `async fn run`. Natively a thread runs it with
    `futures_executor::block_on`, as before. On wasm it is a `wasm_bindgen_futures::spawn_local`
    task. In the loop, blocking sends became `send_async().await` or `send().await`, and
    `block_on(lock())` became `lock().await`. The now-unused `blocking_send` is gone.
  - `migrate.rs`: `web_time::Instant`.
- **sqlx-core:**
  - `rt/rt_wasm.rs`: `spawn` (`spawn_local`), `sleep` and `timeout` (gloo-timers). `rt/mod.rs`
    uses them as the last branch before `missing_rt`, so a running Tokio runtime still wins.
    `spawn_blocking` runs the closure on the same thread, since there are no others.
  - `sync.rs`: the pool's semaphore is futures-intrusive's on wasm, as with async-std.
  - `pool/*`, `logger.rs`: `web_time::Instant`.
  - `pool/inner.rs`: `#[allow(deprecated)]` on a `fetch_update`, since a path dependency's warnings
    aren't capped like a crates.io dependency's.

On wasm, depend on sqlx without a runtime feature: tokio's `net` and `fs` don't build there.

### Pitfalls

- **Dropping a wasm `JoinHandle` must detach the task**, as Tokio's does. sqlx drops the handle of
  the task that returns a connection to the pool. A handle that cancels on drop
  (`futures_util::future::RemoteHandle`) made every query open a new SQLite connection: 700 µs per
  query instead of 15 µs.
- A JS timer isn't `Send`, and sqlx's futures must be, so `sleep` runs the timer in its own task
  and awaits a oneshot.

## Re-applying it to a new sqlx version

1. `vendor/diff.sh > /tmp/sqlx-wasm.patch`, before changing anything.
2. Download the new crates (`https://static.crates.io/crates/<name>/<name>-<version>.crate`), and
   extract them to `vendor/<name>-<version>`. Delete `.cargo_vcs_info.json`, `.cargo-ok`,
   `Cargo.toml.orig` and `Cargo.lock`. Commit them unchanged, so the patch stays its own diff.
3. Point `[patch.crates-io]` at the new directories, bump `sqlx` in the crates that use it, and
   delete the old directories.
4. Apply the old patch with the new version in its paths:
   `sed 's/-0\.9\.0/-<version>/g' /tmp/sqlx-wasm.patch | (cd vendor && patch -p1 -F3)`.
   - The import hunks (`crate::ffi`, `web_time::Instant`) usually apply.
   - `rt/mod.rs`, `sync.rs` and the command loop in `worker.rs` may need porting by hand.
   - For the command loop, take the new closure body and redo the edits listed above, rather than
     keeping the old `run`.
5. Look for new uses: `rg 'libsqlite3_sys|std::time::Instant|Instant::now|thread::' vendor/`.
6. Check:
   - `cargo test --workspace` and `cargo clippy --workspace`;
   - `cargo build -p spares_core --target wasm32-unknown-unknown`, with no warnings from sqlx;
   - the browser checks (review, search, a rating) on OPFS.
