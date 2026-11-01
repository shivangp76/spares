# Plan: a host for many users (what is left)

spares runs as a host (`spares_server::host`): one root with `host.toml`, `accounts.sqlite` and a directory per user, collections that are not process-wide state (`spares_core::collection`), accounts with cookie sessions and device tokens, sync between hosts with replicas from the root (`spares_core::sync::devices`), renders in a sandbox (`spares_core::render`), limits and usage (`spares_server::host::limits`), signups, export and backups (`spares_server::host::signup`, `spares_server::host::export`). What remains before signups can open on a public host:

## Not checked on a real machine

- **bubblewrap**: renders on Linux run in `bwrap` with the arguments of `Renderer::bubblewrap_args`, which are unit-tested but have not run on Linux. Render a LaTeX, Markdown and Typst note on the VPS, and try the attacks `spares_core/tests/kill_running_renders.rs` tries with `sandbox-exec` (reading another user's file, writing outside `files/` and `render/`, the network).
- **`spares deploy` and `setup_vps.sh`** with hosts: a fresh deploy, and moving a server set up before hosts into a host (`init` once; `setup_vps.sh` runs `host import --replica 1`), have not been run against a VPS. After it, the laptop logs in with `spares sync login`, keeping replica `0`, and syncs normally (its `sync_client` row survives the import).

## Still to build

- **nsjail** as another sandbox on Linux. Only bubblewrap is there.
- **Media paths that don't depend on the root.** Notes name images, audio and image occlusions by absolute path, so two hosts share a collection only if their roots are at the same path (as `spares deploy` arranges). A public host's root differs from a laptop's: notes would name `media/...` relative to the collection's directory, resolved when rendering, with existing notes rewritten.
- **Typst packages** are read from the host's package cache: a render can't download one, since it has no network. A host fetches them outside renders, e.g. when a note names one it lacks.
- **The CLI's tokens** are in `credentials.toml` (mode 600), not the keychain.
- **A host relaying for its devices** relays the database's sync, but a device's rendered files and config come from that host, which renders the device's changes only after its own next sync with the upstream.

## Running a public host

- Backups: `spares host backup` exports every user and copies `accounts.sqlite`; continuous backups (Litestream on each `collection.sqlite` and on `accounts.sqlite`) are not set up by anything.
- Past one machine: pin users to machines by id; moving one is `spares host export` and `spares host import`.
- Email beyond `[email] command`: deliverability, bounces.
- A privacy policy, and a way to handle reports of abuse.

Delete this file once these are done.
