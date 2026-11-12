//! Development tasks for spares, run with `cargo xtask <task>`:
//!
//! - `build-wasm [out-dir]` builds `spares_wasm` for browsers into a package the web app imports:
//!   the wasm module and its JS bindings, in `spares_frontend/frontend/src/local/pkg` by default.
//!   `bun run build` runs it first. See CONTRIBUTING.md.
//!
//! The JS bindings come from `wasm-bindgen-cli-support`, the library the `wasm-bindgen` CLI is
//! made of. Its version is pinned to `spares_wasm`'s `wasm-bindgen`, so they always match without
//! installing the CLI.

use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use wasm_bindgen_cli_support::Bindgen;

/// Homebrew's LLVM, whose clang targets wasm. `spares deploy` checks for it too.
const MACOS_LLVM: &str = "/opt/homebrew/opt/llvm/bin";

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("build-wasm") => build_wasm(args.next().map(PathBuf::from)),
        _ => bail!("Usage: cargo xtask build-wasm [out-dir]"),
    }
}

fn build_wasm(out: Option<PathBuf>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let out = out.unwrap_or_else(|| root.join("spares_frontend/frontend/src/local/pkg"));
    let mut cargo = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo.current_dir(root).args([
        "build",
        "-p",
        "spares_wasm",
        "--target",
        "wasm32-unknown-unknown",
        "--profile",
        "wasm",
    ]);
    // SQLite's C needs a clang that targets wasm. Apple's doesn't, so on macOS use Homebrew's LLVM.
    if cfg!(target_os = "macos") && env::var_os("CC_wasm32_unknown_unknown").is_none() {
        let llvm = Path::new(MACOS_LLVM);
        if !llvm.join("clang").is_file() {
            bail!("Building spares_wasm needs Homebrew's LLVM: brew install llvm");
        }
        cargo
            .env("CC_wasm32_unknown_unknown", llvm.join("clang"))
            .env("AR_wasm32_unknown_unknown", llvm.join("llvm-ar"));
    }
    // Ignoring failure: without rustup, the target is installed some other way, or cargo says
    // it's missing
    let _ = Command::new("rustup")
        .args(["target", "add", "wasm32-unknown-unknown"])
        .output();
    if !cargo.status()?.success() {
        bail!("Building spares_wasm failed");
    }
    let target = env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    Bindgen::new()
        .input_path(target.join("wasm32-unknown-unknown/wasm/spares_wasm.wasm"))
        .web(true)?
        .typescript(true)
        // The bindings load the module from next to themselves by default
        .omit_default_module_path(false)
        // The name section is about a third of the module, and only helps debuggers.
        .remove_name_section(true)
        .remove_producers_section(true)
        .generate(&out)
        .context("Generating spares_wasm's JS bindings failed")?;
    let size = std::fs::metadata(out.join("spares_wasm_bg.wasm"))?.len();
    println!("{}: {size} bytes", out.display());
    Ok(())
}
