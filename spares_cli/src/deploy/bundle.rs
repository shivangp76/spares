//! The archive of spares built for the server, which `setup_vps.sh --bundle` installs:
//! `spares_server`, `frontend/`, and unless the server skips LaTeX, `pandoc.deb`.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;

use super::progress::Progress;
use super::shell::Ssh;
use super::shell::command_exists;
use super::shell::run;

/// What a server runs: its Rust target and Debian architecture
#[derive(Clone, Copy)]
pub(super) struct Target {
    triple: &'static str,
    deb_arch: &'static str,
}

impl Target {
    /// The target of `server` (as user@host)
    pub(super) fn of(ssh: &Ssh, server: &str) -> Result<Self, Error> {
        let arch = ssh.output(server, "uname -m")?;
        let (triple, deb_arch) = match arch.as_str() {
            "x86_64" => ("x86_64-unknown-linux-gnu", "amd64"),
            "aarch64" => ("aarch64-unknown-linux-gnu", "arm64"),
            _ => return Err(miette!("Unsupported server architecture: {arch}")),
        };
        Ok(Self { triple, deb_arch })
    }
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
    pub(super) fn path(&self) -> PathBuf {
        self.0.path().join("spares-bundle.tar.gz")
    }

    /// Its size, e.g. "38.2 MB"
    pub(super) fn size(&self) -> String {
        #[allow(clippy::cast_precision_loss)]
        let megabytes = fs::metadata(self.path()).map_or(0, |m| m.len()) as f64 / 1_000_000.0;
        format!("{megabytes:.1} MB")
    }

    /// Builds spares for `target` from the checkout at `repo`, as the "Building" step.
    pub(super) async fn build(
        progress: &Progress,
        repo: &Path,
        target: Target,
        with_pandoc: bool,
    ) -> Result<Self, Error> {
        progress.step("Building", &format!("spares for {}", target.triple));
        for command in ["zig", "cargo-zigbuild", "bun", "rustup"] {
            if !command_exists(command) {
                return Err(miette!(
                    "{command} is needed to build spares for the server. Install zig and \
                     cargo-zigbuild with: brew install zig && cargo install --locked cargo-zigbuild"
                ));
            }
        }
        let Target { triple, deb_arch } = target;
        progress.part("Compiling spares_server");
        run(Command::new("rustup")
            .args(["target", "add", triple])
            .stdout(Stdio::null()))?;
        // glibc 2.31 (Debian 11, Ubuntu 20.04) or newer runs it. Without debuginfo, which is
        // stripped anyway, it compiles about a quarter faster.
        run(Command::new("cargo")
            .args(["zigbuild", "--release", "-p", "spares_server", "--target"])
            .arg(format!("{triple}.2.31"))
            .env("CARGO_PROFILE_RELEASE_DEBUG", "false")
            .env("CARGO_PROFILE_RELEASE_STRIP", "debuginfo")
            .current_dir(repo))?;
        progress.part("Building the frontend");
        let frontend = repo.join("spares_frontend/frontend");
        run(Command::new("bun").arg("install").current_dir(&frontend))?;
        run(Command::new("bun")
            .args(["run", "build"])
            .current_dir(&frontend))?;

        let pandoc = if with_pandoc {
            Some(pandoc_deb(progress, repo, deb_arch).await?)
        } else {
            None
        };

        progress.part("Packing");
        let contents = TempDir::new()?;
        let dir = contents.path();
        fs::copy(
            repo.join("target")
                .join(triple)
                .join("release/spares_server"),
            dir.join("spares_server"),
        )
        .into_diagnostic()?;
        run(Command::new("cp")
            .arg("-R")
            .arg(frontend.join("dist"))
            .arg(dir.join("frontend")))?;
        if let Some(pandoc) = pandoc {
            fs::copy(pandoc, dir.join("pandoc.deb")).into_diagnostic()?;
        }

        let bundle = Self(TempDir::new()?);
        // Without macOS metadata, which GNU tar on the server warns about
        run(Command::new("tar")
            .args(["--no-xattrs", "--no-mac-metadata", "-czf"])
            .arg(bundle.path())
            .arg("-C")
            .arg(dir)
            .arg(".")
            .env("COPYFILE_DISABLE", "1"))?;
        Ok(bundle)
    }
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
