# Plan: a host for many users

What it would take for spares to be hosted for the public, one database per user. There is one way to run `spares_server`: as a host. The laptop is a host with one account and signups closed, so it only ever holds its owner's data, and it runs the same code paths a public host does. Self-hosted installs change in breaking ways where that keeps one code path: there is no single-user mode.

Today one process serves one collection for one person:

- `AppState` (`spares_server/src/lib.rs`) holds one `SqlitePool`, and the schema has no owner.
- Paths, the database and config come from environment variables (`SPARES_DATA_DIR`, `SPARES_FILES_DIR`, `SPARES_CACHE_DIR`, `SPARES_CONFIG_DIR`, `DATABASE_URL`, `*_OUT_DIR`, `TYPST_ROOT`) and one config file (`read_external_config`).
- Statics hold collection state: `DEVICE_REPLICA` (`spares_core::config`), `RUNNING_RENDERS` (`spares_core::parsers::helpers`), the LaTeX and pandoc directory caches, and the notes repository (`notes_vcs::server_repo_dir`) with its one writer lock (`run_exclusive`).
- One key (`SPARES_API_KEY`, `require_api_key`; `SYNC_API_KEY` for sync) lets a request do anything, and CORS allows any origin.
- Sync has two roles (`SPARES_SYNC_ROLE`): the laptop is a client, the server is the server. The replica comes from `SPARES_REPLICA` (`0` the laptop, `1` the server); browsers enroll for `2` to `4` (`spares_core::sync::devices`). A collection has at most 8 replicas, one per residue modulo `ids::ID_STEP`.
- Notes are rendered by running `latexmk`, `pandoc` and `typst` on their text, unsandboxed. On a public host that runs anyone's input: LaTeX reads files (`\input{/etc/passwd}`) and, with shell escape, runs commands; pandoc includes files and fetches URLs.
- `SYNC_BODY_LIMIT` (1 GiB) and `MEDIA_BODY_LIMIT` (128 MiB) are sized for one trusted user.

## Model

- **Host:** one `spares_server` process, one root directory and one host config. The laptop, a personal VPS and a public host are all hosts; they differ only in `host.toml`.
- **User:** an account with a directory of its own, holding its collection.
- **Device:** the CLI, a browser, or another host's copy of the collection, with a revocable token tied to a user.
- **Upstream:** a collection can name another host and a device token there. It then syncs with that host as one of its user's devices. The laptop's collection names the public host (or a personal VPS) as its upstream.

```
<SPARES_ROOT>/
  host.toml              # bind address, limits, signup policy, frontend dir
  accounts.sqlite        # users, password hashes, sessions, device tokens, quotas, usage
  users/<user_id>/
    collection.sqlite    # today's schema, unchanged: no user_id columns
    config.toml          # what read_external_config reads today, per user, plus its upstream
    files/  media/  render/  cache/
    notes.git/           # the notes repository
```

A user's directory has the same shape on every host. Giving the laptop's user the same id as the account on the public host makes the directory portable: moving a collection between them is copying it.

A database per user keeps the schema, sync, the id scheme (`spares_core::ids`) and the browser's copy (`spares_wasm`) as they are: each is already one collection in one database. A shared database with a `user_id` on every table would change every query, and a missed `WHERE` would show one user's notes to another.

## What breaks

| Today | After |
|---|---|
| `SPARES_DATA_DIR`, `SPARES_FILES_DIR`, `SPARES_CACHE_DIR`, `SPARES_CONFIG_DIR`, `DATABASE_URL`, `*_OUT_DIR`, `TYPST_ROOT` | Derived from `SPARES_ROOT` and the user's id |
| `SPARES_API_KEY`, `SYNC_API_KEY` | Accounts, cookie sessions and device tokens; the CLI runs `spares login` once per host |
| One config file | `config.toml` per user; the host's settings in `host.toml` |
| `SPARES_SYNC_ROLE` | Every host serves sync for its users; a collection with an upstream also syncs with it |
| `SPARES_REPLICA` | Given out by the root of the collection's sync chain when a device enrolls |
| `latexmk`, `pandoc` and `typst` run directly | Through a sandbox (`Renderer`), on every host |
| `allow_origin(Any)` | Same-origin cookies for the web app, bearer tokens for the rest |

Moving over: `spares host init` creates the root and the first account (an admin), and prints a device token for the CLI. `spares host import <data dir> <user>` moves today's data directory, files directory, config and notes repository into `users/<id>/`. Devices then each run `spares login` once. This runs once on the laptop and once on the VPS.

## 1. A collection, not globals

`Collection` holds what is process-global today:

```rust
pub struct Collection {
    pub db: SqlitePool,
    pub paths: CollectionPaths,   // data, files, cache, rendered files, notes repository
    pub config: Arc<Config>,      // what read_external_config returns today, plus the upstream
    pub replica: Replica,         // this host's replica for the collection
    pub renders: RunningRenders,  // was the static RUNNING_RENDERS
    pub vcs_lock: VcsLock,        // was the global writer lock of notes_vcs
}
```

- Replace the functions that read the environment or statics (`get_files_dir`, `get_cache_dir`, `get_replica`, `notes_vcs::server_repo_dir`, a parser's `get_output_rendered_dir`, the LaTeX and pandoc directory caches) with methods on `Collection`, or pass one to them.
- Handlers get an `Arc<Collection>` from an extractor instead of `AppState::db`.
- The CLI and `spares_wasm` use the same type; the browser builds one over its own storage (`spares_core::platform`).
- `kill_running_renders` stops one collection's renders, except at shutdown.

## 2. The host

- Reads `host.toml`.
- Keeps the open collections in an LRU, with a cap on open pools and closing idle ones. The laptop just never has more than one open.
- Migrates a collection when it is first opened, under a lock of its own, and all of them with `spares host migrate --all`.
- Creates a user's directory and initializes `notes.git` when the account is created.
- Whoever can read `SPARES_ROOT` is the admin: `spares host reset-password`, `spares host token` and the other admin commands work on the files directly, without logging in.

## 3. Accounts and auth

- `accounts.sqlite`: users (argon2 hashes), sessions, device tokens (scopes, last used), invites, quotas and usage. Collections never refer to it, so a user's directory can move between hosts.
- One middleware gives each request its `(User, Option<Device>, Arc<Collection>)`, replacing `require_api_key`. The collection always comes from the session or token, never from the request's path or body.
- The web app uses cookie sessions with CSRF protection, and CORS allows only its own origin. Sessions are long-lived, so the web app on the laptop asks for the password about as often as a new browser appears. Devices use bearer tokens.
- Signups, in `host.toml`: `closed`, `invite`, or `open` with email verification. The laptop and a personal VPS are `closed`, which is what keeps them to one user.
- The CLI stores a token per host, from `spares login <host>`, in the keychain or under the config directory.
- Deleting an account deletes its directory. Exporting one is a tarball of the directory, which another host imports.

## 4. Sync

- There are no roles. Every host is the server for its users' collections: a device syncs with its own user's collection and nothing else.
- A collection with an upstream is also a client of it: `auto_sync` pulls from and pushes to the upstream with that collection's device token, as the laptop does to the server today. The laptop's browsers sync with the laptop, and their changes reach the upstream through it.
- **Replicas.** A collection has 8 replicas, too few to hand blocks of them to each host. So the **root** of the chain, the host whose copy of the collection has no upstream, gives out every number:
  - A host enrolls with its upstream like any device and gets its replica then. The root keeps `1`, as the server does today; the laptop gets `0` when it first enrolls.
  - A host with an upstream enrolls its own devices by asking the upstream for a number on the device's behalf, so enrolling a browser on the laptop needs the upstream to be reachable. A collection with no upstream gives out numbers itself, from `sync_device` as today.
  - Revoking a device's token frees its number at the root.
- The protocol is today's with device tokens instead of `SYNC_API_KEY`, and with enrollment forwarded upstream. It gets a new version, since this breaks the old one.
- A `full` upload is checked before it replaces the collection: `PRAGMA integrity_check`, the schema's version, and its size against the quota.

## 5. Rendering

- Every render, on every host, goes through `Renderer`, in a sandbox: bubblewrap or nsjail on Linux, `sandbox-exec` on macOS (deprecated, but still the only built-in one).
- The sandbox has no network, reads system paths only, writes only a scratch directory of its own, and has limits on CPU, memory and time.
- LaTeX never gets shell escape, and pandoc gets no Lua filters.
- A queue per user, with a limit on renders at once.
- A host can turn engines off, e.g. only Typst and rendering in the browser.

## 6. Limits

- `Limits`: body sizes, storage quota, requests per minute, a search's time and renders at once. Defaults in `host.toml`, overridden per user in `accounts.sqlite`. The laptop's `host.toml` sets them as high as today's.
- Usage is counted in `accounts.sqlite`.

## 7. Files

- A route that serves files (`render_asset`, `image`, `audio`, sync's media) finds them under `Collection::paths` only, and refuses `..` and absolute paths.
- Rendered files and media are named within a user's directory, so two users' files never collide.

## 8. Background work

- One scheduler for the host runs `auto_sync` (for collections with an upstream) and render cleanup for the collections that are open or have work due, with a limit on how many at once.

## 9. Running a host

- Backups per user directory (Litestream or snapshots), and of `accounts.sqlite`.
- Structured logs with the user's id and never a note's text.
- Health and metrics endpoints.
- Past one machine: users are pinned to a machine by id, and moving one is copying a directory.
- A public host also needs email (verification, password reset), a privacy policy, data export and deletion, and a way to handle reports of abuse.

## 10. Testing

- A scratch server is a temporary `SPARES_ROOT`: `spares host init --print-token` creates it with one account and prints a token for the test to use. Nothing outside the root is read or written, so the `*_OUT_DIR` and `SYNC_ROLE` care that isolated test servers need today goes away.
- Sync tests run two scratch hosts, one the other's upstream.

## Order

1. `Collection` and the `SPARES_ROOT` layout, with `spares host init` and `spares host import`: one user, no auth yet. Most of what breaks breaks here.
2. Accounts, sessions, device tokens and `spares login`; the API keys go.
3. Sync between hosts: upstreams, device tokens, replicas from the root, and the new protocol version. `SPARES_SYNC_ROLE` and `SPARES_REPLICA` go.
4. `Renderer` and its sandbox for every render.
5. Limits, quotas and checking uploaded databases.
6. Signup policies, export, backups and the admin commands.

After 3 the laptop and the VPS run as hosts with one account each, and use every code path a public host does except signups; the rest makes opening signups safe. The laptop gains a login and a sandbox, which is more work on macOS than on Linux.

Delete this file once the items above are done.
