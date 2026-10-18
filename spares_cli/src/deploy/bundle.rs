//! The directory of spares built for the server, which `setup_vps.sh --bundle` installs:
//! `spares_server`, `frontend/`, `setup_vps.sh` itself, and unless the server skips LaTeX,
//! `pandoc.deb`. It is copied with rsync to [`REMOTE_DIR`] on the server and kept there, so the next
//! deploy only sends what changed: mostly the binary, as the frontend's files are named by their
//! contents.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;

use super::progress::Progress;
use super::shell::command_exists;
use super::shell::run;
use super::shell::run_quietly;

/// Where the bundle is kept on the server, relative to the home directory of the user deploying
pub(super) const REMOTE_DIR: &str = ".cache/spares-bundle";

/// What a server runs: its Rust target and Debian architecture
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Target {
    triple: &'static str,
    deb_arch: &'static str,
}

impl Target {
    /// The target of a server whose `uname -m` is `arch`
    pub(super) fn of_arch(arch: &str) -> Result<Self, Error> {
        let (triple, deb_arch) = match arch {
            "x86_64" => ("x86_64-unknown-linux-gnu", "amd64"),
            "aarch64" => ("aarch64-unknown-linux-gnu", "arm64"),
            _ => return Err(miette!("Unsupported server architecture: {arch}")),
        };
        Ok(Self { triple, deb_arch })
    }

    fn arch(self) -> &'static str {
        self.triple.split('-').next().expect("not empty")
    }

    /// The target `server` had the last time, kept in the cache
    fn last(server: &str) -> Option<Self> {
        let targets = read_targets();
        Self::of_arch(targets.get(server)?.as_str()?).ok()
    }

    fn remember(self, server: &str) {
        let Some(file) = targets_file() else {
            return;
        };
        let mut targets = read_targets();
        targets.insert(server.to_string(), self.arch().into());
        let _ = fs::create_dir_all(file.parent().expect("in the cache"));
        let _ = fs::write(file, serde_json::Value::Object(targets).to_string());
    }
}

fn targets_file() -> Option<PathBuf> {
    Some(super::cache_dir()?.join("targets.json"))
}

/// The `uname -m` of each server deployed to, by user@host
fn read_targets() -> serde_json::Map<String, serde_json::Value> {
    targets_file()
        .and_then(|file| fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// What building for a server needs to know about it
pub(super) struct Server {
    pub(super) target: Target,
    /// Whether it renders markdown notes, so needs pandoc
    pub(super) with_pandoc: bool,
}

/// A directory under the system's temporary one, removed when dropped.
pub(super) struct TempDir(PathBuf);

impl TempDir {
    pub(super) fn new() -> Result<Self, Error> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir =
            std::env::temp_dir().join(format!("spares-deploy-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).into_diagnostic()?;
        Ok(Self(dir))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The bundle, in a temporary directory removed when dropped.
pub(super) struct Bundle(TempDir);

impl Bundle {
    pub(super) fn path(&self) -> &Path {
        self.0.path()
    }

    /// Builds spares from the checkout at `repo` for `server` (as user@host), as the "Building"
    /// step. `connect` tells what the server is. It runs alongside building the frontend, and
    /// alongside compiling the server too when it has the target it had the last time.
    pub(super) async fn build(
        progress: &Progress,
        repo: &Path,
        server: &str,
        connect: impl FnOnce() -> Result<Server, Error> + Send,
    ) -> Result<Self, Error> {
        progress.step("Building", &format!("spares for {server}"));
        for command in ["zig", "cargo-zigbuild", "bun", "rustup"] {
            if !command_exists(command) {
                return Err(miette!(
                    "{command} is needed to build spares for the server. Install zig and \
                     cargo-zigbuild with: brew install zig && cargo install --locked cargo-zigbuild"
                ));
            }
        }
        let frontend = repo.join("spares_frontend/frontend");
        let last = Target::last(server);
        let Server {
            target,
            with_pandoc,
        } = std::thread::scope(|scope| {
            let built_frontend = scope.spawn(|| {
                let _part = progress.background("Building the frontend");
                // Quietly, as cargo shows its output meanwhile
                run_quietly(Command::new("bun").arg("install").current_dir(&frontend))?;
                run_quietly(
                    Command::new("bun")
                        .args(["run", "build"])
                        .current_dir(&frontend),
                )
            });
            let connected = scope.spawn(|| {
                let _part = progress.background(&format!("Connecting to {server}"));
                connect()
            });
            let compiled = last.map(|target| (target, compile(progress, repo, target)));
            let connected = join(connected)?;
            connected.target.remember(server);
            match compiled {
                Some((target, compiled)) if target == connected.target => compiled?,
                _ => compile(progress, repo, connected.target)?,
            }
            join(built_frontend)?;
            Ok::<_, Error>(connected)
        })?;

        let pandoc = if with_pandoc {
            Some(pandoc_deb(progress, repo, target.deb_arch).await?)
        } else {
            None
        };

        progress.part("Packing");
        let bundle = Self(TempDir::new()?);
        let dir = bundle.path();
        fs::copy(
            repo.join("target")
                .join(target.triple)
                .join("release/spares_server"),
            dir.join("spares_server"),
        )
        .into_diagnostic()?;
        run(Command::new("cp")
            .arg("-R")
            .arg(frontend.join("dist"))
            .arg(dir.join("frontend")))?;
        fs::copy(repo.join("scripts/setup_vps.sh"), dir.join("setup_vps.sh")).into_diagnostic()?;
        if let Some(pandoc) = pandoc {
            fs::copy(pandoc, dir.join("pandoc.deb")).into_diagnostic()?;
        }
        Ok(bundle)
    }
}

/// Compiles `spares_server` for `target`, into `target/<triple>/release` of `repo`.
fn compile(progress: &Progress, repo: &Path, target: Target) -> Result<(), Error> {
    let triple = target.triple;
    progress.part(&format!("Compiling spares_server for {triple}"));
    run(Command::new("rustup")
        .args(["target", "add", triple])
        .stdout(Stdio::null()))?;
    // glibc 2.31 (Debian 11, Ubuntu 20.04) or newer runs it. Without debuginfo, which is stripped
    // anyway, it compiles about a quarter faster. Fat LTO and one codegen unit make the server
    // faster at the cost of a longer compile, here rather than in the workspace's release
    // profile so local builds and ones on a small VPS stay quick.
    run(Command::new("cargo")
        .args(["zigbuild", "--release", "-p", "spares_server", "--target"])
        .arg(format!("{triple}.2.31"))
        .env("CARGO_PROFILE_RELEASE_DEBUG", "false")
        .env("CARGO_PROFILE_RELEASE_STRIP", "debuginfo")
        .env("CARGO_PROFILE_RELEASE_LTO", "fat")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1")
        .current_dir(repo))
}

/// The result of a scoped thread, passing on its panic
fn join<T>(thread: std::thread::ScopedJoinHandle<'_, Result<T, Error>>) -> Result<T, Error> {
    thread
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// The pandoc package that `setup_vps.sh` installs (its `PANDOC_VERSION`), downloaded once into
/// the cache.
async fn pandoc_deb(progress: &Progress, repo: &Path, deb_arch: &str) -> Result<PathBuf, Error> {
    let setup = fs::read_to_string(repo.join("scripts/setup_vps.sh")).into_diagnostic()?;
    let version = setup
        .lines()
        .find_map(|line| line.strip_prefix("PANDOC_VERSION=\"")?.strip_suffix('"'))
        .ok_or_else(|| miette!("No PANDOC_VERSION in setup_vps.sh."))?;
    let cache = super::cache_dir().ok_or_else(|| miette!("HOME is not set."))?;
    let cached = cache.join(format!("pandoc-{version}-{deb_arch}.deb"));
    if !cached.is_file() {
        let url = format!(
            "https://github.com/jgm/pandoc/releases/download/{version}/pandoc-{version}-1-{deb_arch}.deb"
        );
        progress.part(&format!("Downloading pandoc {version}"));
        let bytes = reqwest::get(&url)
            .await
            .and_then(reqwest::Response::error_for_status)
            .into_diagnostic()?
            .bytes()
            .await
            .into_diagnostic()?;
        fs::create_dir_all(&cache).into_diagnostic()?;
        let partial = cached.with_extension("deb.tmp");
        fs::write(&partial, bytes).into_diagnostic()?;
        fs::rename(&partial, &cached).into_diagnostic()?;
    }
    Ok(cached)
}
