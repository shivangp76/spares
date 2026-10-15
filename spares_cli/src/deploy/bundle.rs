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

use super::shell::Ssh;
use super::shell::bold;
use super::shell::command_exists;
use super::shell::run;

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

    /// Builds spares for `server` (as user@host) from the checkout at `repo`.
    pub(super) async fn build(
        ssh: &Ssh,
        repo: &Path,
        server: &str,
        with_pandoc: bool,
    ) -> Result<Self, Error> {
        for command in ["zig", "cargo-zigbuild", "bun", "rustup"] {
            if !command_exists(command) {
                return Err(miette!(
                    "{command} is needed to build spares for the server. Install zig and \
                     cargo-zigbuild with: brew install zig && cargo install --locked cargo-zigbuild"
                ));
            }
        }
        let arch = ssh.output(server, "uname -m")?;
        let (target, deb_arch) = match arch.as_str() {
            "x86_64" => ("x86_64-unknown-linux-gnu", "amd64"),
            "aarch64" => ("aarch64-unknown-linux-gnu", "arm64"),
            _ => return Err(miette!("Unsupported server architecture: {arch}")),
        };
        bold(&format!("Building spares for {target}"));
        run(Command::new("rustup")
            .args(["target", "add", target])
            .stdout(Stdio::null()))?;
        // glibc 2.31 (Debian 11, Ubuntu 20.04) or newer runs it
        run(Command::new("cargo")
            .args(["zigbuild", "--release", "-p", "spares_server", "--target"])
            .arg(format!("{target}.2.31"))
            .env("CARGO_PROFILE_RELEASE_STRIP", "debuginfo")
            .current_dir(repo))?;
        let frontend = repo.join("spares_frontend/frontend");
        run(Command::new("bun").arg("install").current_dir(&frontend))?;
        run(Command::new("bun")
            .args(["run", "build"])
            .current_dir(&frontend))?;

        let contents = TempDir::new()?;
        let dir = contents.path();
        fs::copy(
            repo.join("target")
                .join(target)
                .join("release/spares_server"),
            dir.join("spares_server"),
        )
        .into_diagnostic()?;
        run(Command::new("cp")
            .arg("-R")
            .arg(frontend.join("dist"))
            .arg(dir.join("frontend")))?;
        if with_pandoc {
            let pandoc = pandoc_deb(repo, deb_arch).await?;
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
        let size = fs::metadata(bundle.path()).into_diagnostic()?.len();
        #[allow(clippy::cast_precision_loss)]
        let megabytes = size as f64 / 1_000_000.0;
        println!("Built {megabytes:.1} MB to upload.");
        Ok(bundle)
    }
}

/// The pandoc package that `setup_vps.sh` installs (its `PANDOC_VERSION`), downloaded once into
/// the cache.
async fn pandoc_deb(repo: &Path, deb_arch: &str) -> Result<PathBuf, Error> {
    let setup = fs::read_to_string(repo.join("scripts/setup_vps.sh")).into_diagnostic()?;
    let version = setup
        .lines()
        .find_map(|line| line.strip_prefix("PANDOC_VERSION=\"")?.strip_suffix('"'))
        .ok_or_else(|| miette!("No PANDOC_VERSION in setup_vps.sh."))?;
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))
        .ok_or_else(|| miette!("HOME is not set."))?
        .join("spares-deploy");
    let cached = cache.join(format!("pandoc-{version}-{deb_arch}.deb"));
    if !cached.is_file() {
        let url = format!(
            "https://github.com/jgm/pandoc/releases/download/{version}/pandoc-{version}-1-{deb_arch}.deb"
        );
        println!("Downloading {url}");
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
