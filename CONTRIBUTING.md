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
# Format code
cargo fmt

# Check for common issues
cargo clippy
```

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

Sync between hosts is tested with two of them, one the other's upstream (`spares_server/tests/two_hosts.rs`).

An end-to-end test runs a device's copy in headless Chromium, WebKit and Firefox, against a host of its own on an empty collection: setting it up, using it offline, syncing, resolving a conflict, and removing it (see `e2e/local_copy.ts`):

```sh
cd spares_frontend/frontend
bunx playwright-core install chromium-headless-shell webkit firefox  # once
bun run build && bun run e2e
```

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
