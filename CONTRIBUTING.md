# Contributing to Spares

## Development Environment

### Code Coverage
```sh
# Generate and open coverage report
cargo llvm-cov --open
```

### Profiling
```sh
# Generate flamegraph
RUST_LOG="info" sudo -E cargo flamegraph --bin spares_server
open flamegraph.svg
```

## Code Style

We follow the Rust community's coding standards. Please run the following before submitting a PR:

```sh
# Format the files you changed
rustfmt +nightly --edition 2024 --config skip_children=true <files>

# Check for common issues
cargo clippy
```

`rustfmt.toml` uses options only nightly rustfmt has (`imports_granularity`, `group_imports`): stable `cargo fmt` ignores them and reformats whole crates. `skip_children` keeps rustfmt from following `mod` into other files. It still formats all of each file you give it, so check what it changed besides your edits (with jj: `jj diff --from 'at_operation(<op before>, @)' --to @`).

### Compatibility

What an older version of spares wrote keeps working in newer ones, so updating never needs a conversion by hand. The oldest version supported is the collection baseline of 2026-11-26 (see `spares_core::collection::migrator`):

- **Databases.** A change to the schema or to stored data is a new migration in `spares_core/migrations` (or `spares_server/accounts_migrations`) that converts existing rows. Never edit, squash or delete a migration that is on `main`: sqlx checksums applied migrations, and every database since the baseline has to migrate up from wherever it is.
- **Note files and syntax.** A note that parsed keeps parsing to the same cards, with the same card ids. To change a syntax, accept the old spelling alongside the new one.
- **Config files.** `config.toml` and `host.toml` reject unknown keys, so a renamed key keeps its old name as a `#[serde(alias)]`, and a removed key stays in the struct, accepted and ignored, with a doc comment saying so.
- **Sync.** A host and its devices check each other's sync protocol and schema versions (`spares_core::sync::protocol`), and refuse a mismatch with an error that says which side to update, rather than misreading each other. Bump the protocol version only for a change that can't be made compatibly. The web app is served by its host, so the two always match.
- **Code for old data stays.** A fallback that reads what older versions wrote is kept, with a comment naming the version that wrote it. Don't remove it once your own data has been converted.

`spares_core` and `spares_server`'s router also build for the browser, where the web app can keep a copy of the collection. Check that they still do:

```sh
rustup target add wasm32-unknown-unknown
# SQLite's C needs a clang that targets wasm. On macOS, Apple's doesn't: use Homebrew's LLVM.
export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
cargo clippy -p spares_wasm --target wasm32-unknown-unknown  # spares_core and spares_server's library too
```

There, sqlx is the patched copy in `vendor/` (see `vendor/README.md`).

The web app runs `spares_wasm` (spares in a browser, see its crate docs) in a worker, for a device that keeps a copy of the collection (`src/local`). `bun run build` builds it first, with the command below. On macOS, it needs Homebrew's LLVM (`brew install llvm`).

```sh
cargo xtask build-wasm
```

It writes the wasm module and its JS bindings to `spares_frontend/frontend/src/local/pkg`. The `wasm` profile builds it small (LTO, `opt-level = "z"`): about 7.6 MB, 1.75 MB with brotli. Its JS bindings come from `wasm-bindgen-cli-support`, at the version of the `wasm-bindgen` crate, so there is no `wasm-bindgen` CLI to install. When upgrading `wasm-bindgen`, upgrade both (in `spares_wasm/Cargo.toml` and `xtask/Cargo.toml`).

### A scratch host

A host keeps everything in its root directory and reads nothing else of yours, so a scratch host for testing is a temporary root:

```sh
export SPARES_ROOT=$(mktemp -d)
TOKEN=$(echo password1 | spares host init --user test --bind 127.0.0.1:8090 --password-stdin --print-token)
spares_server &                     # serves $SPARES_ROOT on 127.0.0.1:8090
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8090/api/parsers
```

- Give every CLI call both `SPARES_ROOT` and `SPARES_HOST`. Without them, the CLI uses the host you last logged in to, which may be your real one. A wrapper that clears `SPARES_*` variables needs them set again after it.
- `spares host token <user> --root <root>` prints a Bearer token without a password. It **creates a device** in that root's accounts, so never run it with a real root.
- Commands that ask for confirmation (`card optimize --apply`, rescheduling) fail when there is no terminal to ask in: pass `--yes`.
- Set `render_browser_parsers = false`, a top-level key of `users/<id>/config/config.toml` (not `host.toml`), as on a server. Otherwise the host renders Typst notes for the browser, and pushes take minutes.

A scratch host from a copy of a real one:

- `host.toml` holds just `bind = "127.0.0.1:<port>"` (and `frontend_dir`, see below). Drop `[render.copy_notes_to]`: it writes outside the root.
- Copy the collection with `sqlite3 -readonly <db> ".backup <copy>"`. Copy `accounts.sqlite` with `cp`: `.backup` fails on it in WAL mode.
- Remove `[sync]` from the copied `users/<id>/config/config.toml` before starting the server. It holds the real upstream's URL and token, and the server syncs on startup. Without `[sync]`, the scratch host is replica 1.
- Copy `render_assets/` too, or Typst cards fail to compile. For the web app alone, copy `media/`, the user's config, and the card sources from its cache into `<root>/cache/users/<id>/cards`, but not `notes/` or `notes.git`.
- Copied notes repositories keep their `origin` and `core.hooksPath` pointing at the real ones. Repoint both and run `spares vcs init --repair-hooks` against the scratch host before any push. jj keeps per-repository config in `~/.config/jj/repos`, so delete the scratch entries afterwards.
- Copy the binaries you run (`cp -c`), so a rebuild can't swap them mid-run.

Sync between hosts is tested with two of them, one the other's upstream (`spares_server/tests/two_hosts.rs`).

An end-to-end test runs a device's copy in headless Chromium, WebKit and Firefox, against a host of its own on an empty collection: setting it up, using it offline, syncing, resolving a conflict, and removing it (see `e2e/local_copy.ts`):

```sh
cd spares_frontend/frontend
bunx playwright-core install chromium-headless-shell webkit firefox  # once
bun run build && bun run e2e
```

Pull requests run the native tests, frontend unit tests, and WASM compile check. The browser suite runs weekly and on manual dispatch. To time a broad keyword search on 10,000 notes through the host and through an offline Chromium copy, run this after building the frontend:

```sh
cd spares_frontend/frontend
SPARES_SEARCH_BENCH=1 bun e2e/local_copy.ts chromium
```

Dictionary lookup on the cards of the `markdown-language-tools` parser has an end-to-end test of its own, outside `bun run e2e`, which CI runs weekly. With `SPARES_E2E_SHOTS=<dir>` it saves a screenshot of each step:

```sh
bun e2e/language_tools.ts chromium   # or webkit, firefox
```

Cards shown as HTML are in a sandboxed iframe without `allow-scripts`. WebKit runs no event listeners in it, even ones the app adds from outside ("Blocked script execution in 'about:srcdoc'"), while Chromium and Firefox do. DOM calls from the app, such as `caretRangeFromPoint` or reading the selection, work in all three. So a feature that needs the pointer or clicks on a card gets them from the app's window, with the iframe's `pointer-events: none` while it does (see `src/languageTools/LookupLayer.tsx`).

The parser sits behind spares_core's `language-tools` feature, which the server, the CLI and the browser's build turn on. Check spares without it with `cargo test -p spares_core --lib` (the workspace's tests turn it on) and `cargo check -p spares_server -p spares_cli --no-default-features --all-targets`.

Before blaming a change for an e2e failure, run the suite on `main` too: WebKit and Firefox have failed there on their own.

#### In a jj workspace

A new jj workspace has no `svgedit` submodule checkout (jj leaves an empty directory there), `node_modules` or `src/local/pkg`. `scripts/setup_workspace.sh` clones them from your main checkout, and `target/debug` too, so the first build is incremental:

```sh
scripts/setup_workspace.sh <your main checkout>
cargo build -p spares_server
cd spares_frontend/frontend && bun run build && SPARES_SERVER_BIN=$PWD/../../target/debug/spares_server bun run e2e
```

`svgedit` has to be a real directory: vite rejects a symlinked one. `bun run e2e` uses a temporary root of its own and port 8796, and takes about five minutes.

### The web app in a headless browser

To check a change in the web app, or take screenshots of it, build it into a directory of its own and let a scratch host serve it, instead of starting a second `vite` dev server (that one shares `node_modules/.vite` with yours, and opens a tab):

```sh
cd spares_frontend/frontend
npx tsc -b && npx vite build --outDir "$SCRATCH/dist" --emptyOutDir
# in the scratch host.toml: frontend_dir = "<scratch>/dist"
```

The server has no environment variable for `frontend_dir`. For before/after screenshots, build both versions and point `frontend_dir` at a symlink that you switch between them. A lazily loaded page's loading fallback shows only when it is loaded directly: navigating inside the app keeps the old page until the new one is ready.

With Playwright (`playwright-core`, pointing `executablePath` at the headless shell from `bunx playwright-core install chromium-headless-shell`):

- Log in with `page.request.post("/api/auth/login", {data: {user, password}})`, then set localStorage `spares_credentials` to `{serverUrl, csrfToken, user, schedulerName}`. `schedulerName` must be a real one (e.g. `fsrs`, from `/api/schedulers`), or Statistics fails. Set `spares_vim_keybindings` to `"true"` to test Vim mode.
- With a Bearer token instead of a password: `extraHTTPHeaders: {authorization: "Bearer …"}` on the context works when the scratch host serves the app itself. Otherwise `page.route("http://app.test/**")` can serve the built files and forward `/api` with the header; then `csrfToken` only needs to be non-empty.
- `page.route` can also fake an API response, to show a state the data can't.
- WebKit keeps OPFS (a device's copy) only in `launchPersistentContext`.
- On macOS, CodeMirror's `Mod` is Cmd: press `ControlOrMeta+Enter`.
- In Vim mode, press Escape twice: the first may only close autocompletion. The editor applies an outside change to its value about a second after typing stops.

The frontend's lint (react-hooks 7) rejects calling setState synchronously in an effect, even through `useEffectEvent`, and reading refs while computing a memoized value. Defer the call with `queueMicrotask`, or dispatch a DOM event (as `useSubmitKeys` does).

### The demo and the README's screenshots

The demo is the web app built with `VITE_SPARES_DEMO=1` (`src/demo.ts`): it has no host, and loads a collection of example notes into the browser's copy (`src/local/demo.ts`, with `load_snapshot` in `spares_wasm`). `demo/build.ts` makes that collection on a scratch host of its own, from `demo/notes.ts`, with three months of made-up reviews dated from the build. `.github/workflows/demo.yml` publishes it to GitHub Pages on every push to `main`, and weekly so that cards stay due.

To build it and try it, from anywhere in the repository:

```sh
cargo xtask demo         # builds dist-demo/ and serves it at http://127.0.0.1:8798/spares/
cargo xtask demo --dev   # vite's dev server instead, which reloads as the app changes
```

It makes the collection again (about two minutes, most of it rendering) only when there is none, when it is over a week old, or when `demo/notes.ts`, `demo/build.ts` or the migrations changed since. Otherwise it rebuilds only the app, keeping the collection. `--fresh` makes it again anyway, `--app` keeps it, `--open` opens a browser tab and `--port` changes the port; `demo/run.ts` has the details. The demo's dev server keeps its prebundled dependencies in `node_modules/.vite-demo`, so it doesn't disturb a dev server of the app. Then, in `spares_frontend/frontend`:

```sh
bun e2e/demo.ts                              # Chromium, WebKit and Firefox, about a minute each
bun demo/screenshots.ts                      # docs/screenshots/*.webp (needs cwebp)
```

CI runs `bun run build-demo` instead, which always builds both the app and the collection into `dist-demo/` (it needs `cargo build -p spares_cli -p spares_server` first). `bun demo/serve.ts dist-demo` serves a build as GitHub Pages does. Markdown notes in the demo avoid characters pdflatex can't typeset, such as `α` or `²`: their PDFs are rendered too, though the demo leaves them out.

### Clock-dependent tests

A study day ends at `day_starts_at` (4 AM by default), not at midnight. Tests take the hour from `queue::tests::day_starts_at()` and dates from `helpers::study_date`, and a test that needs a time later the same day uses `queue::tests::local_midday()`.

To run the tests at another local time, set `TZ` to a POSIX offset that makes it that time now. The sign is inverted: `TZ=XXX-1:47:30` is UTC+1:47:30. Both chrono's `Local` and SQLite follow it. The offset is the target time minus the current UTC time, wrapped to ±12 hours; check it with `TZ=… date`. Times worth a run are 23:05, 23:52, 23:59:20, 00:00:05, 00:30, 03:52, 03:59:20, 04:00:05 and 04:30. Compute each offset right before its run of `cargo test -p spares_core --lib`.

### On an iPhone in the iOS Simulator

Without the Simulator app, `xcrun simctl` (with `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` if the active developer directory is the command-line tools) boots a device, opens URLs (`simctl openurl`; a distinct URL such as `?tab=2` opens a second tab) and takes screenshots (`simctl io booted screenshot`). `safaridriver` can't reach simulators. To drive Safari, talk to the Web Inspector socket (`lsof -aU -c launchd_sim | grep webinspectord_sim`) with its binary-plist RPC:

- A page takes one inspector session, so keep one process holding it, logging the console and running evaluations.
- `Runtime.evaluate` ignores `awaitPromise`: store a result on `window` and poll for it. It does accept `emulateUserGesture`.
- Stub `window.confirm` and `alert` first: native dialogs block evaluation. Errors thrown in evaluated code have the stack `@`, so wrap the code in try/catch.
- Edit a CodeMirror editor through its view (`.cm-content`'s `cmTile.view.dispatch`), never through `innerText`.
- To go offline, stop the proxy the device reaches the host through: Safari never fires `online` afterwards. `simctl privacy grant` kills Safari.
- The device's copy is under the simulator's `…/FileSystem/spares/.opaque/`, as SQLite after a 4096-byte header.

## Documentation

Document things where they are implemented, so the docs change with the code:

- Settings: on their fields in `config.toml`'s structs (`spares_core::config`), and note and cloze settings on `NoteSettings`, `ClozeSettings` and `ClozeGroupingSettings`.
- Syntax and behavior of a feature, and the reasons for a design: in the module docs of the code that implements it, e.g. `spares_core::sync` and `spares_core::ids`.
- Command-line options: in the doc comments of the clap structs, which become `spares <command> --help`.
- Work that is planned but not done: in a plan file in `docs/`, such as `docs/queue_plan.md`.

The manual, in `spares_frontend/frontend/src/manual`, is for guides that walk through several parts at once, and for self-contained pages like the comparison with Anki. The web app shows it under Manual, and its help popups link to its sections, which `src/help/pages.test.ts` checks exist. A new page is added to `MANUAL_PAGES` in `src/help/pages.ts`. It points to the API docs for details, which are built with:

```sh
cargo doc --workspace --no-deps --document-private-items --open
```

Many documented modules are private to their crate, so `--document-private-items` is needed to see them. Tag non-Rust code blocks in doc comments (` ```md `, ` ```toml `, ` ```sh `, ` ```text `), or `cargo test` runs them as Rust doctests.

## Submitting Changes

1. Ensure all tests pass
2. Format your code
3. Update documentation if needed
4. Commit your changes with a descriptive message
5. Push to your fork
6. Create a Pull Request

### Pull Request Guidelines

- Provide a clear description of the changes
- Reference any related issues
- Include tests for new features or bug fixes
- Update documentation as needed
- Keep PRs focused and manageable in size

## Issue Reporting

When reporting issues:
- Provide detailed steps to reproduce
- Include relevant logs or error messages
- Specify your environment (OS, Rust version, etc.)

## Feature Requests

For feature requests:
- Explain the problem you're trying to solve
- Describe your proposed solution
- Provide use cases or examples
- Consider potential impacts on existing features

## Questions?

Feel free to:
- Open an issue for questions
- Join our community discussions
- Contact the maintainers directly

Thank you for contributing to Spares!

## Design Inspiration

These are some websites that were used for inspiration, listed in no particular order.

- <https://github.com/arctic-hen7/forne>
- <https://github.com/alfredholmes/TeXNotes>
- <https://zettelkasten.de/overview/>
- <https://natemeyvis.com/on-ankis-database/>
- <https://github.com/open-spaced-repetition/fsrs4anki/wiki/ABC-of-FSRS>
- <https://github.com/open-spaced-repetition/fsrs4anki/wiki/Spaced-Repetition-Algorithm:-A-Three%E2%80%90Day-Journey-from-Novice-to-Expert>
- <https://github.com/open-spaced-repetition/fsrs4anki-helper/wiki>
- [Anki SRS Algorithm Explained with code](https://juliensobczak.com/inspect/2022/05/30/anki-srs/)
- [Nielsen srs mathematics](https://cognitivemedium.com/srs-mathematics)
- [Anki CLI](https://github.com/lervag/apy) (not too relevant)
- [Rust CRUD API tutorial](https://medium.com/@raditzlawliet/build-crud-rest-api-with-rust-and-mysql-using-axum-sqlx-d7e50b3cd130)
- <https://github.com/ankidroid/Anki-Android/wiki/Database-Structure>
