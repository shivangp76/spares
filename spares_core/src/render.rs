//! How a host renders notes: every render runs `latexmk`, `pandoc` or `typst` on a user's text, so
//! every one goes through [`Renderer`], in a sandbox, on every host.
//!
//! - **The sandbox** ([`Sandbox`]): `sandbox-exec` on macOS (deprecated, but still the only one
//!   built in), bubblewrap (`bwrap`) on Linux. A render has no network, reads only the system's
//!   directories, its collection's directory and cache and the host's
//!   [`RenderConfig::read_dirs`], and writes only its collection's files directory (where rendered
//!   files go) and its scratch directory (`render/` in the collection's cache: temporary files,
//!   TeX's font cache). It is the only limit on what a render reads: Typst's `--root` is `/`.
//!   A host without its sandbox doesn't render, unless `host.toml` says `sandbox = "none"`.
//! - **Limits**: a render is stopped after `timeout_seconds`, gets `cpu_seconds` of CPU and, on
//!   Linux, `memory_mb` of memory. A user's renders wait their turn beyond `renders_at_once`.
//! - **LaTeX** never gets shell escape (`shell_escape=f`), may only write files where it is told
//!   to (`openout_any=p`), and `latexmk` reads no `latexmkrc`. **Pandoc** gets no filters, Lua or
//!   other: a defaults file that names any is refused ([`check_pandoc_defaults`]).
//! - **Engines** can be turned off: a host with `engines = ["typst"]` renders only Typst, and
//!   browsers render the rest.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::LibraryError;

/// `[render]` in `host.toml`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderConfig {
    /// Directories outside users' collections that every render may read, such as the laptop's
    /// own preambles that its templates import (`/Users/me/learning/preambles`). Empty on a host
    /// with users who don't trust each other.
    pub read_dirs: Vec<PathBuf>,
    /// The sandbox renders run in.
    pub sandbox: Sandbox,
    /// The engines this host renders with: `latex`, `pandoc` and `typst`.
    pub engines: Vec<Engine>,
    /// A render is stopped after this long.
    pub timeout_seconds: u64,
    /// CPU time a render may use.
    pub cpu_seconds: u64,
    /// Memory a render may use, in MB. Only enforced on Linux. `0`: no limit.
    pub memory_mb: u64,
    /// A user's renders at once; the others wait.
    pub renders_at_once: usize,
    /// Directories that each note's rendered file is also in, by parser (`typst`, `latex-note`,
    /// `markdown`), with the same name: `<note id>.pdf`. This is where editors on this machine
    /// write the PDFs of the files they compile, so that one PDF viewer shows a note whether the
    /// editor or spares rendered it, as with tinymist's `outputPath = $TYPST_OUT_DIR/$name`:
    ///
    /// ```toml
    /// [render.copy_notes_to]
    /// typst = "/Users/me/.cache/typst"
    /// ```
    ///
    /// The file there is a hard link to the one in the collection's files directory, not a copy,
    /// so a render writes it there as it writes it, and an editor compiling the note into it may
    /// write the collection's until spares renders the note again. It is a copy when the
    /// directory is on another filesystem. Only cards' files stay in the collection alone. Notes
    /// of different users with the same id would replace each other's, so this is for a host
    /// with one user, such as a laptop's. The link is made outside the sandbox, by the host.
    pub copy_notes_to: BTreeMap<String, PathBuf>,
    /// Directories Typst finds fonts in (searched recursively), besides the fonts built into
    /// Typst and, unless [`typst_system_fonts`](Self::typst_system_fonts) is off, the system's.
    /// Renders may read them.
    ///
    /// ```toml
    /// typst_font_dirs = ["/Users/me/Library/Fonts"]
    /// ```
    pub typst_font_dirs: Vec<PathBuf>,
    /// Whether Typst also finds the system's fonts, the ones `typst fonts` lists. Default: on.
    ///
    /// Each run of the `typst` CLI looks through all of them before compiling: about 75 ms of
    /// the 155 ms a card takes on a Mac with 400 fonts. Turning this off, with the directories of
    /// the fonts notes use in [`typst_font_dirs`](Self::typst_font_dirs), saves that per file.
    /// Renders in a worker ([`typst_in_process`](Self::typst_in_process)) look through them once
    /// per worker, so it matters little there. A font that is in neither is replaced by one of
    /// Typst's, with a warning.
    pub typst_system_fonts: bool,
    /// Whether Typst notes render in worker processes with the Typst compiler built in, rather
    /// than with one run of the `typst` CLI per file. Default: on. A worker runs in the same
    /// sandbox, with the same limits, as a `typst` run, and keeps its fonts, packages and parsed
    /// files between renders, so that a card takes a few milliseconds rather than 155. Each of a
    /// user's [`renders_at_once`](Self::renders_at_once) has its own worker.
    ///
    /// Needs spares built with the `typst-render` feature (the default): without it, and when a
    /// worker can't start, renders use the `typst` CLI.
    pub typst_in_process: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            read_dirs: Vec::new(),
            sandbox: Sandbox::Auto,
            engines: vec![Engine::Latex, Engine::Pandoc, Engine::Typst],
            timeout_seconds: 60,
            cpu_seconds: 120,
            memory_mb: 2048,
            renders_at_once: 4,
            copy_notes_to: BTreeMap::new(),
            typst_font_dirs: Vec::new(),
            typst_system_fonts: true,
            typst_in_process: true,
        }
    }
}

/// What renders run in (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sandbox {
    /// The platform's: `sandbox-exec` on macOS, bubblewrap on Linux
    #[default]
    Auto,
    SandboxExec,
    Bubblewrap,
    /// None: renders run as the host's user, reading what it can. Only for a host whose users
    /// all trust each other, on a platform without a sandbox.
    None,
}

/// A program renders run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, strum::Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Engine {
    /// `latexmk` and the TeX it runs
    Latex,
    Pandoc,
    Typst,
}

impl Engine {
    /// The engine of a render command, by its program's name.
    pub fn of_program(program: &str) -> Option<Self> {
        match Path::new(program).file_name()?.to_str()? {
            "latexmk" | "pdflatex" | "xelatex" | "lualatex" => Some(Self::Latex),
            "pandoc" => Some(Self::Pandoc),
            "typst" => Some(Self::Typst),
            _ => None,
        }
    }
}

static CONFIG: RwLock<Option<RenderConfig>> = RwLock::new(None);

/// Sets the host's render settings, once at startup.
pub fn set_config(config: RenderConfig) {
    *CONFIG
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
}

/// The host's render settings. Without a host, the defaults, except under tests (see
/// [`crate::config::is_test_mode`]), which render files of their own outside of any collection
/// and so without a sandbox; `tests/kill_running_renders.rs` checks the sandbox.
pub fn config() -> RenderConfig {
    CONFIG
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| {
            if crate::config::is_test_mode() {
                RenderConfig {
                    sandbox: Sandbox::None,
                    ..RenderConfig::default()
                }
            } else {
                RenderConfig::default()
            }
        })
}

/// Call first in a binary's `main`. In a process started as a Typst worker (see the
/// `typst-render` feature and [`RenderConfig::typst_in_process`]), this serves renders and exits.
/// In any other, it lets renders start workers from this executable. Without the feature, it does
/// nothing. A binary that doesn't call it, such as a test's, renders Typst with the `typst` CLI.
pub fn init_typst_workers() {
    #[cfg(all(feature = "typst-render", not(target_family = "wasm")))]
    crate::parsers::impls::typst::worker::init();
}

/// What a render of the current collection may read: its directories and the host's
/// [`RenderConfig::read_dirs`].
pub fn readable_dirs() -> Vec<PathBuf> {
    let mut dirs = crate::collection::paths().dirs();
    dirs.extend(config().read_dirs);
    dirs
}

/// Whether `path` is in one of [`readable_dirs`].
pub fn is_readable(path: &Path) -> bool {
    readable_dirs().iter().any(|dir| path.starts_with(dir))
}

fn refused(message: String) -> Error {
    Error::Library(LibraryError::InvalidConfig(message))
}

/// A collection's turns at rendering (see [`RenderConfig::renders_at_once`]).
#[derive(Debug, Default)]
pub struct RenderSlots {
    running: Mutex<usize>,
    freed: Condvar,
}

/// A turn at rendering, given back when dropped.
pub struct RenderSlot<'a>(&'a RenderSlots);

impl RenderSlots {
    /// Waits for one of `at_once` turns.
    pub fn take(&self, at_once: usize) -> RenderSlot<'_> {
        let mut running = crate::collection::lock(&self.running);
        while *running >= at_once.max(1) {
            running = self
                .freed
                .wait(running)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *running += 1;
        RenderSlot(self)
    }
}

impl Drop for RenderSlot<'_> {
    fn drop(&mut self) {
        *crate::collection::lock(&self.0.running) -= 1;
        self.0.freed.notify_one();
    }
}

/// Runs a collection's render commands: in the host's sandbox, with its limits.
pub struct Renderer {
    config: RenderConfig,
    readable: Vec<PathBuf>,
    writable: Vec<PathBuf>,
    scratch: PathBuf,
}

impl Renderer {
    /// The current collection's renderer.
    pub fn current() -> Result<Self, Error> {
        let paths = crate::collection::paths();
        let config = config();
        let scratch = paths.render.clone();
        for dir in [&scratch, &paths.files] {
            std::fs::create_dir_all(dir).map_err(|e| Error::Io {
                description: format!("Failed to create {}", dir.display()),
                source: e,
            })?;
        }
        let mut readable = readable_dirs();
        readable.extend(tool_dirs());
        readable.extend(config.typst_font_dirs.iter().cloned());
        Ok(Self {
            writable: vec![paths.files.clone(), scratch.clone()],
            readable,
            scratch,
            config,
        })
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.config.timeout_seconds.max(1))
    }

    /// The current collection's renders at once: its user's, else the host's.
    pub fn renders_at_once(&self) -> usize {
        match crate::collection::state()
            .renders_at_once
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            0 => self.config.renders_at_once,
            user => user,
        }
    }

    /// `command`, to run in the sandbox: refused if its engine is off, with the environment that
    /// keeps TeX from running other programs, temporary files in the scratch directory, and the
    /// limits.
    pub fn prepare(&self, command: &Command) -> Result<Command, Error> {
        let program = command.get_program().to_string_lossy().into_owned();
        if let Some(engine) = Engine::of_program(&program)
            && !self.config.engines.contains(&engine)
        {
            return Err(refused(format!(
                "this host doesn't render with {engine}; browsers render these notes"
            )));
        }
        let mut sandboxed = match self.sandbox()? {
            Sandbox::SandboxExec => {
                let mut wrapped = Command::new("/usr/bin/sandbox-exec");
                wrapped
                    .arg("-p")
                    .arg(self.sandbox_exec_profile())
                    .arg(command.get_program())
                    .args(command.get_args());
                wrapped
            }
            Sandbox::Bubblewrap => {
                let mut wrapped = Command::new("bwrap");
                wrapped
                    .args(self.bubblewrap_args(command.get_current_dir()))
                    .arg("--")
                    .arg(command.get_program())
                    .args(command.get_args());
                wrapped
            }
            Sandbox::None | Sandbox::Auto => {
                let mut plain = Command::new(command.get_program());
                plain.args(command.get_args());
                plain
            }
        };
        if let Some(dir) = command.get_current_dir() {
            sandboxed.current_dir(dir);
        }
        for (key, value) in command.get_envs() {
            match value {
                Some(value) => sandboxed.env(key, value),
                None => sandboxed.env_remove(key),
            };
        }
        let scratch = &self.scratch;
        sandboxed
            // No `\write18`, nor writing files outside where TeX is told to
            .env("shell_escape", "f")
            .env("openout_any", "p")
            // TeX's generated fonts and the like, and temporary files
            .env("TEXMFVAR", scratch.join("texmf-var"))
            .env("TMPDIR", scratch.join("tmp"))
            .env("XDG_CACHE_HOME", scratch.join("cache"));
        let _ = std::fs::create_dir_all(scratch.join("tmp"));
        self.limit(&mut sandboxed);
        Ok(sandboxed)
    }

    /// [`Self::prepare`] for a program that isn't one of the [`Engine`]s, such as a Typst worker
    /// (see `parsers::impls::typst::worker`), which may also read `readable`, such as its own
    /// executable. The caller checks its engine is on.
    pub fn prepare_program(
        &self,
        command: &Command,
        readable: &[PathBuf],
    ) -> Result<Command, Error> {
        let mut renderer = Self {
            config: self.config.clone(),
            readable: self.readable.clone(),
            writable: self.writable.clone(),
            scratch: self.scratch.clone(),
        };
        renderer.readable.extend(readable.iter().cloned());
        renderer.prepare(command)
    }

    /// The host's render settings, as this renderer runs them
    pub fn config(&self) -> &RenderConfig {
        &self.config
    }

    fn sandbox(&self) -> Result<Sandbox, Error> {
        let available = |sandbox| match sandbox {
            Sandbox::SandboxExec => Path::new("/usr/bin/sandbox-exec").exists(),
            Sandbox::Bubblewrap => which("bwrap"),
            Sandbox::None | Sandbox::Auto => true,
        };
        let sandbox = match self.config.sandbox {
            Sandbox::Auto if cfg!(target_os = "macos") => Sandbox::SandboxExec,
            Sandbox::Auto if cfg!(target_os = "linux") => Sandbox::Bubblewrap,
            Sandbox::Auto => {
                return Err(refused(
                    "this platform has no sandbox for renders. Set `sandbox = \"none\"` in \
                     host.toml's [render] if every user of this host trusts the others."
                        .to_string(),
                ));
            }
            sandbox => sandbox,
        };
        if available(sandbox) {
            Ok(sandbox)
        } else {
            Err(refused(format!(
                "renders run in {sandbox:?}, which isn't installed (on Linux: `apt install \
                 bubblewrap`)"
            )))
        }
    }

    /// The `sandbox-exec` profile: everything but the network, reading files outside the
    /// readable directories, and writing outside the writable ones.
    pub fn sandbox_exec_profile(&self) -> String {
        let subpaths = |dirs: &[PathBuf]| {
            dirs.iter()
                .flat_map(|dir| {
                    // The profile matches real paths: /tmp is /private/tmp.
                    let real = dir.canonicalize().unwrap_or_else(|_| dir.clone());
                    [dir.clone(), real]
                })
                .fold(String::new(), |mut rules, dir| {
                    let _ = write!(rules, " (subpath {})", sbpl_string(&dir.to_string_lossy()));
                    rules
                })
        };
        let system: Vec<PathBuf> = MACOS_SYSTEM_DIRS.iter().map(PathBuf::from).collect();
        format!(
            "(version 1)\n\
             (allow default)\n\
             (deny network*)\n\
             (deny file-read-data (require-not (require-any (literal \"/\"){}{}{})))\n\
             (deny file-write* (require-not (require-any{} (literal \"/dev/null\") (literal \
             \"/dev/tty\") (subpath \"/dev/fd\"))))\n",
            subpaths(&system),
            subpaths(&self.readable),
            subpaths(&self.writable),
            subpaths(&self.writable),
        )
    }

    /// bubblewrap's arguments: a new mount namespace with the system's directories read-only,
    /// the readable directories read-only, the writable ones writable, and no network.
    pub fn bubblewrap_args(&self, current_dir: Option<&Path>) -> Vec<String> {
        let mut args: Vec<String> = [
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
        ]
        .map(String::from)
        .to_vec();
        for dir in LINUX_SYSTEM_DIRS {
            args.extend([
                "--ro-bind-try".to_string(),
                dir.to_string(),
                dir.to_string(),
            ]);
        }
        for dir in &self.readable {
            let dir = dir.to_string_lossy().into_owned();
            args.extend(["--ro-bind-try".to_string(), dir.clone(), dir]);
        }
        for dir in &self.writable {
            let dir = dir.to_string_lossy().into_owned();
            args.extend(["--bind".to_string(), dir.clone(), dir]);
        }
        if let Some(dir) = current_dir {
            args.extend(["--chdir".to_string(), dir.to_string_lossy().into_owned()]);
        }
        args
    }

    /// CPU time and memory, set in the child before it runs the command.
    #[cfg_attr(not(unix), allow(clippy::unused_self))]
    fn limit(&self, command: &mut Command) {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let cpu = self.config.cpu_seconds;
            let memory = self.config.memory_mb.saturating_mul(1024 * 1024);
            let set = move || {
                let limit = |resource, value: u64| {
                    let value = libc::rlim_t::try_from(value).unwrap_or(libc::RLIM_INFINITY);
                    let limit = libc::rlimit {
                        rlim_cur: value,
                        rlim_max: value,
                    };
                    // SAFETY: setrlimit only reads `limit`.
                    unsafe { libc::setrlimit(resource, &raw const limit) };
                };
                if cpu > 0 {
                    limit(libc::RLIMIT_CPU, cpu);
                }
                // macOS doesn't enforce RLIMIT_AS, and refuses to lower it.
                if memory > 0 && cfg!(target_os = "linux") {
                    limit(libc::RLIMIT_AS, memory);
                }
                Ok(())
            };
            // SAFETY: the closure only calls setrlimit, which is async-signal-safe.
            unsafe { command.pre_exec(set) };
        }
        #[cfg(not(unix))]
        let _ = command;
    }
}

/// A Typst source to render with [`preview_typst`]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewSource {
    /// Names the source in its result. A plain file stem (see [`is_preview_name`]), unique among
    /// the sources rendered together.
    pub name: String,
    pub text: String,
}

/// What [`preview_typst`] made of a [`PreviewSource`]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewResult {
    pub name: String,
    /// Whether it compiled
    pub ok: bool,
    /// Errors and warnings, as `typst compile` prints them
    pub diagnostics: String,
    /// Each page as SVG, in order. Empty if it didn't compile.
    pub pages: Vec<String>,
}

/// `POST /api/notes/vcs/render-preview`: sources to render with [`preview_typst`]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RenderPreviewRequest {
    pub sources: Vec<PreviewSource>,
}

/// The answer to a [`RenderPreviewRequest`]: one result per source, in order
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RenderPreviewResponse {
    pub results: Vec<PreviewResult>,
}

/// Whether `name` can name a [`PreviewSource`]: letters, digits, `-` and `_`, so that it is a
/// file name in the preview's directory and nothing else.
pub fn is_preview_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Renders Typst sources that aren't the collection's notes to SVG pages, such as the old and new
/// sides of edited note files for `spares vcs render-diff`.
///
/// Each source is written to a new directory under the current collection's render scratch
/// directory and rendered there as a note is: in the same sandbox and with the same limits, by
/// the collection's Typst workers if it has them, taking its turns at rendering (at most
/// `renders_at_once` at once). So a source can import what a note can, by absolute path, and no
/// more. The directory is removed afterwards. Nothing is cached, and the collection's notes, cards
/// and rendered files are left alone.
///
/// Returns one result per source, in order. A source with an invalid or repeated name, or that
/// doesn't compile, gets a result that isn't `ok`.
pub fn preview_typst(sources: &[PreviewSource]) -> Vec<PreviewResult> {
    static PREVIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = crate::collection::paths()
        .render
        .join("previews")
        .join(format!(
            "{}-{}",
            std::process::id(),
            PREVIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
    preview_typst_in(&dir, sources)
}

/// [`preview_typst`], in `dir`, which is created and then removed
fn preview_typst_in(dir: &Path, sources: &[PreviewSource]) -> Vec<PreviewResult> {
    let failed = |name: &str, diagnostics: String| PreviewResult {
        name: name.to_string(),
        diagnostics,
        ..PreviewResult::default()
    };
    // Also creates the scratch directory
    let prepared = Renderer::current().and_then(|renderer| {
        let dir = std::path::absolute(dir).map_err(|e| Error::Io {
            description: format!("Failed to make {} absolute", dir.display()),
            source: e,
        })?;
        std::fs::create_dir_all(&dir).map_err(|e| Error::Io {
            description: format!("Failed to create {}", dir.display()),
            source: e,
        })?;
        Ok((renderer.renders_at_once(), dir))
    });
    let (at_once, dir) = match prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            return sources
                .iter()
                .map(|source| failed(&source.name, format!("error: {e}\n")))
                .collect();
        }
    };
    let mut seen = std::collections::HashSet::new();
    let mut results: Vec<Option<PreviewResult>> = sources
        .iter()
        .map(|source| {
            if !is_preview_name(&source.name) {
                Some(failed(
                    &source.name,
                    "error: a preview's name has only letters, digits, `-` and `_`\n".to_string(),
                ))
            } else if !seen.insert(source.name.as_str()) {
                Some(failed(
                    &source.name,
                    "error: another source has this name\n".to_string(),
                ))
            } else {
                None
            }
        })
        .collect();
    let todo: Mutex<Vec<usize>> = Mutex::new(
        (0..sources.len())
            .rev()
            .filter(|&i| results[i].is_none())
            .collect(),
    );
    let done: Mutex<Vec<(usize, PreviewResult)>> = Mutex::new(Vec::new());
    let context = crate::collection::capture();
    std::thread::scope(|scope| {
        let threads = at_once.max(1).min(sources.len());
        for _ in 0..threads {
            scope.spawn(|| {
                context.run(|| {
                    // Each takes a turn at rendering, so at most `at_once` run at once
                    while let Some(i) = crate::collection::lock(&todo).pop() {
                        let result = preview_one(&dir, &sources[i]);
                        crate::collection::lock(&done).push((i, result));
                    }
                });
            });
        }
    });
    for (i, result) in done
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        results[i] = Some(result);
    }
    let _ = std::fs::remove_dir_all(&dir);
    results
        .into_iter()
        .zip(sources)
        .map(|(result, source)| {
            result.unwrap_or_else(|| failed(&source.name, "error: not rendered\n".to_string()))
        })
        .collect()
}

/// Renders one source of [`preview_typst`] in `dir`
fn preview_one(dir: &Path, source: &PreviewSource) -> PreviewResult {
    let mut result = PreviewResult {
        name: source.name.clone(),
        ..PreviewResult::default()
    };
    let input = dir.join(format!("{}.typ", source.name));
    let output = dir.join(format!("{}.svg", source.name));
    if let Err(e) = std::fs::write(&input, &source.text) {
        result.diagnostics = format!("error: failed to write {}: {e}\n", input.display());
        return result;
    }
    let rendered = crate::parsers::impls::typst::render_svg_pages(&input, &output);
    match rendered {
        Ok(rendered) => {
            let diagnostics = String::from_utf8_lossy(&rendered.stderr);
            result.diagnostics = without_dir(&diagnostics, dir);
            result.ok = rendered.status.success();
        }
        Err(e) => result.diagnostics = format!("error: {e}\n"),
    }
    for page in 1.. {
        let path = crate::parsers::impls::typst::svg_page_path(&output, page);
        match std::fs::read_to_string(&path) {
            Ok(svg) if result.ok => result.pages.push(svg),
            Ok(_) => {}
            Err(_) => break,
        }
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_file(&input);
    result
}

/// `diagnostics` naming files in `dir` by their names alone, not where they were rendered. The
/// `typst` CLI prints paths relative to its root, `/`, and workers print them from `/`; `/tmp` is
/// `/private/tmp` on macOS, as Typst names it.
fn without_dir(diagnostics: &str, dir: &Path) -> String {
    let mut diagnostics = diagnostics.to_string();
    // The longer paths first, as the others are in them
    for dir in [dir.canonicalize().ok(), Some(dir.to_path_buf())]
        .into_iter()
        .flatten()
    {
        let relative = format!("{}/", dir.to_string_lossy().trim_start_matches('/'));
        diagnostics = diagnostics
            .replace(&format!("/{relative}"), "")
            .replace(&relative, "");
    }
    diagnostics
}

/// Directories of the tools renders run, besides the system's, that they read: the TeX
/// distribution's (when it is in a home directory), the user's fonts, and Typst's package cache.
fn tool_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = &home {
        for dir in [
            "Library/Fonts",
            "Library/Caches/typst",
            ".cache/typst",
            ".local/share/typst",
            "Library/Application Support/typst",
            ".fonts",
            ".local/share/fonts",
            "Library/TinyTeX",
            ".TinyTeX",
            "texmf",
            "Library/texmf",
        ] {
            dirs.push(home.join(dir));
        }
    }
    dirs.extend(
        crate::parsers::impls::latex::tex_distribution_dirs()
            .iter()
            .cloned(),
    );
    dirs.retain(|dir| dir.exists());
    dirs
}

/// What every render may read on macOS: the system, and where tools are installed.
const MACOS_SYSTEM_DIRS: [&str; 14] = [
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/Library",
    "/opt",
    "/Applications",
    "/private/etc",
    "/private/var/db",
    "/private/var/select",
    "/dev",
    "/etc",
    "/var/db",
    "/nix",
];

/// What every render may read on Linux.
const LINUX_SYSTEM_DIRS: [&str; 11] = [
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/lib32",
    "/etc",
    "/opt",
    "/var/lib",
    "/nix",
    "/run/current-system",
];

fn sbpl_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// Refuses a pandoc defaults file that names filters (Lua or other programs), or lets the PDF
/// engine run other programs.
pub fn check_pandoc_defaults(contents: &str, path: &Path) -> Result<(), Error> {
    for line in contents.lines() {
        let line = line.trim_start();
        if line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("filters:")
            || lower.starts_with("lua-filter")
            || lower.contains("shell-escape")
            || lower.contains("write18")
        {
            return Err(refused(format!(
                "{} names filters or shell escape, which renders don't run: `{}`",
                path.display(),
                line.trim()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_by_program() {
        assert_eq!(Engine::of_program("latexmk"), Some(Engine::Latex));
        assert_eq!(Engine::of_program("/usr/bin/pandoc"), Some(Engine::Pandoc));
        assert_eq!(Engine::of_program("typst"), Some(Engine::Typst));
        assert_eq!(Engine::of_program("sh"), None);
    }

    #[test]
    fn pandoc_defaults_with_filters_are_refused() {
        let path = Path::new("defaults.yaml");
        assert!(check_pandoc_defaults("to: pdf\npdf-engine: xelatex\n", path).is_ok());
        assert!(check_pandoc_defaults("# filters: [a.lua]\nto: pdf\n", path).is_ok());
        for bad in [
            "filters:\n  - steal.lua\n",
            "lua-filter: x.lua",
            "pdf-engine-opts: [-shell-escape]",
            "pdf-engine-opt: --enable-write18",
        ] {
            assert!(check_pandoc_defaults(bad, path).is_err(), "{bad}");
        }
    }

    fn renderer() -> Renderer {
        Renderer {
            config: RenderConfig::default(),
            readable: vec![PathBuf::from("/srv/spares/users/a")],
            writable: vec![
                PathBuf::from("/srv/spares/users/a/files"),
                PathBuf::from("/srv/spares/users/a/render"),
            ],
            scratch: PathBuf::from("/srv/spares/users/a/render"),
        }
    }

    #[test]
    fn sandboxes_confine_reads_and_writes() {
        let renderer = renderer();
        let profile = renderer.sandbox_exec_profile();
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(subpath \"/srv/spares/users/a\")"));
        assert!(profile.contains(
            "(deny file-write* (require-not (require-any (subpath \"/srv/spares/users/a/files\")"
        ));
        let args = renderer.bubblewrap_args(Some(Path::new("/srv/spares/users/a/cards")));
        let joined = args.join(" ");
        assert!(joined.starts_with("--unshare-all"));
        assert!(joined.contains("--ro-bind-try /srv/spares/users/a /srv/spares/users/a"));
        assert!(joined.contains("--bind /srv/spares/users/a/files /srv/spares/users/a/files"));
        assert!(joined.ends_with("--chdir /srv/spares/users/a/cards"));
        assert!(!joined.contains("--share-net"));
    }

    fn preview_dir(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("spares-preview-{test}-{}", std::process::id()))
    }

    fn source(name: &str, text: &str) -> PreviewSource {
        PreviewSource {
            name: name.to_string(),
            text: text.to_string(),
        }
    }

    /// Each source gets its own result, in order, with its pages as SVG, and the directory is
    /// removed afterwards.
    #[test]
    fn previews_render_each_source_to_pages() {
        let dir = preview_dir("pages");
        let results = preview_typst_in(
            &dir,
            &[
                source("two-pages", "One\n#pagebreak()\nTwo"),
                source("broken", "#unknown-function()"),
                source("one_page", "Only"),
            ],
        );
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["two-pages", "broken", "one_page"]);
        assert!(results[0].ok, "{}", results[0].diagnostics);
        assert_eq!(results[0].pages.len(), 2);
        assert!(results[0].pages.iter().all(|page| page.starts_with("<svg")));
        assert!(!results[1].ok);
        assert_eq!(results[1].pages.len(), 0);
        assert!(
            results[1].diagnostics.contains("unknown variable"),
            "{}",
            results[1].diagnostics
        );
        assert!(
            results[1].diagnostics.contains("┌─ broken.typ:1:1"),
            "{}",
            results[1].diagnostics
        );
        assert!(results[2].ok, "{}", results[2].diagnostics);
        assert_eq!(results[2].pages.len(), 1);
        assert!(!dir.exists());
    }

    /// Diagnostics name the file alone, whether a worker (from `/`) or the CLI (relative to `/`)
    /// printed them.
    #[test]
    fn diagnostics_name_files_without_their_directory() {
        let dir = Path::new("/no/such/previews/1");
        for printed in [
            "/no/such/previews/1/a-new.typ:2:1",
            "no/such/previews/1/a-new.typ:2:1",
        ] {
            assert_eq!(
                without_dir(&format!("  ┌─ {printed}\n"), dir),
                "  ┌─ a-new.typ:2:1\n"
            );
        }
    }

    /// A name that isn't a plain file stem, or that another source has, isn't rendered.
    #[test]
    fn previews_refuse_bad_names() {
        let dir = preview_dir("names");
        let results = preview_typst_in(
            &dir,
            &[
                source("../escape", "x"),
                source("", "x"),
                source("a.typ", "x"),
                source("same", "x"),
                source("same", "y"),
            ],
        );
        let ok: Vec<bool> = results.iter().map(|r| r.ok).collect();
        assert_eq!(ok, [false, false, false, true, false]);
        assert!(results[0].diagnostics.contains("only letters"));
        assert!(results[4].diagnostics.contains("another source"));
        assert!(!dir.exists());
        assert!(is_preview_name("abc-old_2"));
        assert!(!is_preview_name("a/b"));
    }

    #[test]
    fn engines_can_be_off() {
        let mut renderer = renderer();
        renderer.config.engines = vec![Engine::Typst];
        renderer.config.sandbox = Sandbox::None;
        assert!(renderer.prepare(&Command::new("latexmk")).is_err());
        assert!(renderer.prepare(&Command::new("pandoc")).is_err());
        let typst = renderer.prepare(&Command::new("typst")).unwrap();
        assert_eq!(typst.get_program(), "typst");
        let shell_escape = typst
            .get_envs()
            .find(|(key, _)| *key == "shell_escape")
            .and_then(|(_, value)| value);
        assert_eq!(shell_escape, Some("f".as_ref()));
    }
}
