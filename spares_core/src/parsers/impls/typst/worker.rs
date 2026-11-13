//! Rendering Typst in worker processes with the Typst compiler built in (the `typst-render`
//! feature), rather than with one run of the `typst` CLI per file.
//!
//! A run of `typst compile` spends most of its time before it compiles anything: on a Mac with 400
//! fonts it takes 155 ms for a card, of which looking through the system's fonts is about 75 ms
//! and evaluating the note's preamble and packages about 55 ms. A worker does that once, and
//! keeps its fonts, the files it read and Typst's memoized work between renders, so a card takes a
//! few milliseconds: 321 cards took 1.1 s with 4 workers, against 14.3 s with 4 `typst` runs at
//! once.
//!
//! - **Isolation**: a worker is this executable, started with [`WORKER_ENV`] set, so it serves
//!   renders instead of doing what the executable does. It runs in the same sandbox, with the same
//!   limits, as a `typst` run ([`crate::render::Renderer`]), and may also read its executable. As
//!   Typst's `--root` is `/`, the sandbox is all that limits what it reads. It never fetches
//!   packages: they come from Typst's package cache, as for a `typst` run in the sandbox.
//! - **Limits**: a render that takes longer than `timeout_seconds` gets its worker killed. The CPU
//!   limit is for the whole process, so a worker that has used half of `cpu_seconds` finishes its
//!   render and exits, and the next render starts a new one.
//! - **Turns**: a worker takes one of the collection's turns at rendering for each render, so
//!   there are at most `renders_at_once` of them, each kept (idle) for the next render until the
//!   collection closes or [`kill_running_renders`](crate::parsers::kill_running_renders) stops
//!   them.
//! - **Protocol**: one JSON [`Request`] per line on the worker's standard input, one JSON
//!   [`Response`] per line on its standard output. Its standard error is the host's.
//! - **Fallback**: a binary that doesn't call [`init`] first (tests, for instance), a host with
//!   [`typst_in_process`](crate::render::RenderConfig::typst_in_process) off, and a worker that
//!   can't start all render with the `typst` CLI instead.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::ChildStdin;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use log::warn;
use serde::Deserialize;
use serde::Serialize;
use typst::Library;
use typst::LibraryExt;
use typst::World;
use typst::diag::FileResult;
use typst::diag::SourceDiagnostic;
use typst::diag::Warned;
use typst::foundations::Bytes;
use typst::foundations::Datetime;
use typst::syntax::FileId;
use typst::syntax::RootedPath;
use typst::syntax::Source;
use typst::syntax::VirtualPath;
use typst::syntax::VirtualRoot;
use typst::text::Font;
use typst::text::FontBook;
use typst::utils::LazyHash;
use typst_kit::datetime::Time;
use typst_kit::diagnostics::DiagnosticFormat;
use typst_kit::diagnostics::DiagnosticWorld;
use typst_kit::diagnostics::termcolor::NoColor;
use typst_kit::downloader::Downloader;
use typst_kit::files::FileStore;
use typst_kit::files::FsRoot;
use typst_kit::files::SystemFiles;
use typst_kit::fonts::FontStore;
use typst_kit::packages::SystemPackages;
use typst_layout::PagedDocument;

use super::svg_page_path;
use crate::Error;
use crate::LibraryError;

/// Set in a worker's environment, to the JSON of its [`WorkerConfig`]
pub const WORKER_ENV: &str = "SPARES_TYPST_WORKER";

/// What a worker is started with
#[derive(Debug, Deserialize, Serialize)]
struct WorkerConfig {
    /// See [`crate::render::RenderConfig::typst_font_dirs`]
    font_dirs: Vec<PathBuf>,
    /// See [`crate::render::RenderConfig::typst_system_fonts`]
    system_fonts: bool,
    /// The process's CPU limit, `0` for none. It exits once it has used half.
    cpu_seconds: u64,
}

/// Compile `input`, a Typst file, to `output` in `format`
#[derive(Debug, Deserialize, Serialize)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    #[serde(default)]
    format: Format,
}

/// What a render writes
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    /// One PDF, `output`
    #[default]
    Pdf,
    /// One SVG per page, named after `output` by [`svg_page_path`]: `<stem>-<n>.svg`
    Svg,
}

#[derive(Debug, Deserialize, Serialize)]
struct Response {
    /// Whether `output` was written
    ok: bool,
    /// Errors and warnings, as `typst compile` prints them
    diagnostics: String,
    /// The files the compile read besides `input`, as absolute paths
    dependencies: Vec<PathBuf>,
    /// Whether the worker exits after this response (see [`WorkerConfig::cpu_seconds`])
    retire: bool,
}

/// This executable, if renders may start workers from it (see [`init`])
static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();

/// See [`crate::render::init_typst_workers`]
pub(crate) fn init() {
    if let Some(config) = std::env::var_os(WORKER_ENV) {
        std::process::exit(run_worker(&config.to_string_lossy()));
    }
    if let Ok(executable) = std::env::current_exe() {
        let _ = EXECUTABLE.set(executable);
    }
}

// ---- The host's side ----

/// A running worker
pub(crate) struct Worker {
    /// Its collection's, without keeping it open: the collection keeps its workers
    state: Weak<crate::collection::CollectionState>,
    child: Child,
    stdin: ChildStdin,
    /// Its standard output's lines, read on a thread of their own so a render can time out
    responses: mpsc::Receiver<String>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let state = self.state.upgrade();
        crate::parsers::kill_render_process(state.as_deref(), &mut self.child);
    }
}

/// A collection's idle workers (see [`crate::collection::CollectionState`])
#[derive(Default)]
pub(crate) struct TypstWorkers {
    idle: Mutex<Vec<Worker>>,
}

/// Compiles `input` to `output` in `format` in one of the current collection's workers, returning
/// what `typst compile` would have printed and the files it read. `None` if renders don't use
/// workers (see the [module docs](self)), for the caller to run the `typst` CLI.
pub(crate) fn render(
    input: &Path,
    output: &Path,
    format: Format,
) -> Option<Result<(Output, Vec<PathBuf>), Error>> {
    let executable = EXECUTABLE.get()?;
    let renderer = match crate::render::Renderer::current() {
        Ok(renderer) => renderer,
        Err(e) => return Some(Err(e)),
    };
    if !renderer.config().typst_in_process {
        return None;
    }
    // Refused like a `typst` run: Typst turned off on this host, or no sandbox
    if let Err(e) = renderer.prepare(&Command::new("typst")) {
        return Some(Err(e));
    }
    let state = crate::collection::state();
    let _slot = state.render_slots.take(renderer.renders_at_once());
    let idle = crate::collection::lock(&state.typst_workers.idle).pop();
    let mut worker = match idle {
        Some(worker) => worker,
        None => match spawn(&state, &renderer, executable) {
            Ok(worker) => worker,
            Err(e) => {
                static WARNED: AtomicBool = AtomicBool::new(false);
                if !WARNED.swap(true, Ordering::Relaxed) {
                    warn!("Typst renders with the `typst` CLI: a Typst worker didn't start: {e}");
                }
                return None;
            }
        },
    };
    let request = Request {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        format,
    };
    let result = exchange(&mut worker, &request, renderer.timeout());
    match result {
        Ok(response) => {
            if !response.retire {
                crate::collection::lock(&state.typst_workers.idle).push(worker);
            }
            let output = Output {
                status: exit_status(response.ok),
                stdout: Vec::new(),
                stderr: response.diagnostics.into_bytes(),
            };
            Some(Ok((output, response.dependencies)))
        }
        // The worker is dropped, which kills it
        Err(e) => Some(Err(e)),
    }
}

fn worker_error(description: String) -> Error {
    Error::Library(LibraryError::InvalidConfig(description))
}

fn spawn(
    state: &Arc<crate::collection::CollectionState>,
    renderer: &crate::render::Renderer,
    executable: &Path,
) -> Result<Worker, Error> {
    let config = renderer.config();
    let worker_config = WorkerConfig {
        font_dirs: config.typst_font_dirs.clone(),
        system_fonts: config.typst_system_fonts,
        cpu_seconds: config.cpu_seconds,
    };
    let mut command = Command::new(executable);
    command
        .env(
            WORKER_ENV,
            serde_json::to_string(&worker_config).unwrap_or_default(),
        )
        .current_dir("/");
    let mut sandboxed = renderer.prepare_program(&command, &[executable.to_path_buf()])?;
    sandboxed
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = crate::parsers::spawn_render_process(state, &mut sandboxed, "Typst worker")?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        crate::parsers::kill_render_process(Some(state), &mut child);
        return Err(worker_error("Typst worker has no pipes".to_string()));
    };
    let (sender, responses) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    Ok(Worker {
        state: Arc::downgrade(state),
        child,
        stdin,
        responses,
    })
}

fn exchange(worker: &mut Worker, request: &Request, timeout: Duration) -> Result<Response, Error> {
    let line = serde_json::to_string(request).unwrap_or_default();
    let io = |e: std::io::Error| Error::Io {
        description: "Failed to send a render to a Typst worker".to_string(),
        source: e,
    };
    writeln!(worker.stdin, "{line}").map_err(io)?;
    worker.stdin.flush().map_err(io)?;
    match worker.responses.recv_timeout(timeout) {
        Ok(line) => serde_json::from_str(&line)
            .map_err(|e| worker_error(format!("Typst worker answered `{line}`: {e}"))),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Io {
            description: format!(
                "typst command was stopped after running for {} seconds",
                timeout.as_secs()
            ),
            source: std::io::ErrorKind::TimedOut.into(),
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Io {
            description: "Typst worker stopped while rendering".to_string(),
            source: std::io::ErrorKind::Interrupted.into(),
        }),
    }
}

/// What a `typst` run that succeeded or failed exits with
fn exit_status(ok: bool) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(if ok { 0 } else { 1 << 8 })
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(u32::from(!ok))
    }
}

// ---- The worker's side ----

/// Serves renders until its standard input closes. Returns the exit code.
fn run_worker(config: &str) -> i32 {
    let config: WorkerConfig = match serde_json::from_str(config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Typst worker started with `{config}`: {e}");
            return 2;
        }
    };
    // Typst recurses deeply, on this thread and on Rayon's.
    let _ = rayon::ThreadPoolBuilder::new()
        .stack_size(64 << 20)
        .build_global();
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || serve(&config))
        .map_or(2, |thread| thread.join().unwrap_or(2))
}

fn serve(config: &WorkerConfig) -> i32 {
    let mut world = WorkerWorld::new(config);
    let mut stdout = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(e) => {
                eprintln!("Typst worker got `{line}`: {e}");
                return 2;
            }
        };
        let mut response =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| world.compile(&request)))
                .unwrap_or_else(|_| Response {
                    ok: false,
                    diagnostics: "error: Typst panicked\n".to_string(),
                    dependencies: Vec::new(),
                    retire: true,
                });
        response.retire |= config.cpu_seconds > 0 && cpu_used() * 2 >= config.cpu_seconds;
        let written = serde_json::to_string(&response)
            .map_err(std::io::Error::other)
            .and_then(|line| writeln!(stdout, "{line}"))
            .and_then(|()| stdout.flush());
        if written.is_err() || response.retire {
            break;
        }
    }
    0
}

/// Seconds of CPU this process has used
fn cpu_used() -> u64 {
    #[cfg(unix)]
    {
        // SAFETY: getrusage only writes `usage`.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `usage` is a valid rusage.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) } != 0 {
            return 0;
        }
        u64::try_from(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec).unwrap_or(0)
    }
    #[cfg(not(unix))]
    0
}

/// Packages come from Typst's package cache only: a render has no network.
struct NoDownloads;

impl Downloader for NoDownloads {
    fn stream(
        &self,
        _key: &dyn std::any::Any,
        url: &str,
    ) -> std::io::Result<(Option<usize>, Box<dyn std::io::Read>)> {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "renders don't download packages ({url}): compile a file that uses it with \
                 `typst` once to put it in Typst's package cache"
            ),
        ))
    }
}

/// What a worker keeps between renders
struct WorkerWorld {
    library: LazyHash<Library>,
    fonts: FontStore,
    files: FileStore<SystemFiles>,
    time: Time,
}

/// [`WorkerWorld`] compiling one file
struct MainFile<'a> {
    world: &'a WorkerWorld,
    main: FileId,
}

impl WorkerWorld {
    fn new(config: &WorkerConfig) -> Self {
        // In the order `typst` looks for fonts: given directories, the system's, its own
        let mut fonts = FontStore::new();
        for dir in &config.font_dirs {
            fonts.extend(typst_kit::fonts::scan(dir));
        }
        if config.system_fonts {
            fonts.extend(typst_kit::fonts::system());
        }
        fonts.extend(typst_kit::fonts::embedded());
        Self {
            library: LazyHash::new(Library::default()),
            fonts,
            files: FileStore::new(SystemFiles::new(
                FsRoot::new(PathBuf::from("/")),
                SystemPackages::new(NoDownloads),
            )),
            time: Time::system(),
        }
    }

    fn compile(&mut self, request: &Request) -> Response {
        // Files are read again if they changed, keeping what Typst made of the ones that didn't
        self.files.reset();
        self.time.reset();
        let failed = |diagnostics: String| Response {
            ok: false,
            diagnostics,
            dependencies: Vec::new(),
            retire: false,
        };
        let Ok(vpath) = VirtualPath::virtualize(Path::new("/"), &request.input) else {
            return failed(format!(
                "error: {} is not an absolute path\n",
                request.input.display()
            ));
        };
        let main = FileId::new(RootedPath::new(VirtualRoot::Project, vpath));
        let file = MainFile { world: self, main };
        let Warned { output, warnings } = typst::compile::<PagedDocument>(&file);
        let written = output.and_then(|document| match request.format {
            Format::Pdf => {
                let options = typst_pdf::PdfOptions {
                    // As `--no-pdf-tags`, to keep files small
                    tagged: false,
                    ..Default::default()
                };
                typst_pdf::pdf(&document, &options)
                    .map(|pdf| std::fs::write(&request.output, pdf).map_err(|e| (e, None)))
            }
            Format::Svg => Ok(write_svg_pages(&document, &request.output)),
        });
        let (ok, mut diagnostics) = match written {
            Ok(Ok(())) => (true, String::new()),
            Ok(Err((e, page))) => {
                let path = page.map_or_else(
                    || request.output.clone(),
                    |page| svg_page_path(&request.output, page),
                );
                (
                    false,
                    format!("error: failed to write {}: {e}\n", path.display()),
                )
            }
            Err(errors) => (false, emit(&file, &errors)),
        };
        diagnostics.push_str(&emit(&file, &warnings));

        let (loader, ids) = self.files.dependencies();
        let mut dependencies: Vec<PathBuf> = ids
            .filter_map(|id| loader.resolve(id).ok())
            .filter(|path| *path != request.input)
            .collect();
        dependencies.sort();
        // Forget what the last renders didn't use, so a worker doesn't grow without end
        typst::comemo::evict(10);
        Response {
            ok,
            diagnostics,
            dependencies,
            retire: false,
        }
    }
}

/// Writes each page of `document` as an SVG (see [`svg_page_path`]). On failure, the error and the
/// page it was writing.
fn write_svg_pages(
    document: &PagedDocument,
    output: &Path,
) -> Result<(), (std::io::Error, Option<usize>)> {
    let options = typst_svg::SvgOptions::default();
    for (i, page) in document.pages().iter().enumerate() {
        let path = svg_page_path(output, i + 1);
        std::fs::write(&path, typst_svg::svg(page, &options)).map_err(|e| (e, Some(i + 1)))?;
    }
    Ok(())
}

/// `diagnostics` as `typst compile` prints them, without colour
fn emit(file: &MainFile<'_>, diagnostics: &[SourceDiagnostic]) -> String {
    if diagnostics.is_empty() {
        return String::new();
    }
    let mut out = NoColor::new(Vec::new());
    if let Err(e) =
        typst_kit::diagnostics::emit(&mut out, file, diagnostics.iter(), DiagnosticFormat::Human)
    {
        return format!("error: failed to print Typst's diagnostics: {e}\n");
    }
    String::from_utf8_lossy(&out.into_inner()).into_owned()
}

impl World for MainFile<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.world.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.world.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.world.files.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.world.files.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.world.fonts.font(index)
    }

    fn today(&self, offset: Option<typst::foundations::Duration>) -> Option<Datetime> {
        self.world.time.today(offset)
    }
}

impl DiagnosticWorld for MainFile<'_> {
    fn name(&self, id: FileId) -> String {
        match id.root() {
            VirtualRoot::Package(package) => format!("{package}{}", id.vpath().get_with_slash()),
            VirtualRoot::Project => id.vpath().get_with_slash().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> WorkerWorld {
        WorkerWorld::new(&WorkerConfig {
            font_dirs: Vec::new(),
            system_fonts: false,
            cpu_seconds: 0,
        })
    }

    /// A file that imports another compiles to a PDF, naming the import as its dependency, and
    /// compiles again after the import changed.
    #[test]
    fn compiles_and_reads_changed_files_again() {
        let dir = std::env::temp_dir().join(format!("spares-typst-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let (main, imported, output) = (
            dir.join("main.typ"),
            dir.join("lib.typ"),
            dir.join("main.pdf"),
        );
        std::fs::write(&imported, "#let word = [one]").unwrap();
        std::fs::write(&main, "#import \"lib.typ\": word\n#word").unwrap();
        let mut world = world();
        let request = Request {
            input: main.clone(),
            output: output.clone(),
            format: Format::Pdf,
        };

        let response = world.compile(&request);
        assert!(response.ok, "{}", response.diagnostics);
        assert_eq!(response.dependencies, std::slice::from_ref(&imported));
        let first = std::fs::read(&output).unwrap();
        assert!(first.starts_with(b"%PDF"));

        std::fs::write(&imported, "#let word = [two, and longer]").unwrap();
        let response = world.compile(&request);
        assert!(response.ok, "{}", response.diagnostics);
        assert_ne!(std::fs::read(&output).unwrap(), first);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An SVG render writes one file per page, numbered from 1.
    #[test]
    fn compiles_each_page_to_an_svg() {
        let dir =
            std::env::temp_dir().join(format!("spares-typst-worker-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let (main, output) = (dir.join("main.typ"), dir.join("main.svg"));
        std::fs::write(&main, "One\n#pagebreak()\nTwo").unwrap();
        let response = world().compile(&Request {
            input: main,
            output: output.clone(),
            format: Format::Svg,
        });
        assert!(response.ok, "{}", response.diagnostics);
        for page in [1, 2] {
            let svg = std::fs::read_to_string(svg_page_path(&output, page)).unwrap();
            assert!(svg.starts_with("<svg"), "{svg}");
        }
        assert_eq!(svg_page_path(&output, 1), dir.join("main-1.svg"));
        assert!(!svg_page_path(&output, 3).exists());
        assert!(!output.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A request without a format is for a PDF, as before there were SVG renders.
    #[test]
    fn requests_default_to_pdf() {
        let request: Request =
            serde_json::from_str(r#"{"input": "/a.typ", "output": "/a.pdf"}"#).unwrap();
        assert_eq!(request.format, Format::Pdf);
    }

    /// An error is reported as `typst compile` prints it, and nothing is written.
    #[test]
    fn reports_errors() {
        let dir =
            std::env::temp_dir().join(format!("spares-typst-worker-err-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let (main, output) = (dir.join("main.typ"), dir.join("main.pdf"));
        std::fs::write(&main, "#unknown-function()").unwrap();
        let response = world().compile(&Request {
            input: main,
            output: output.clone(),
            format: Format::Pdf,
        });
        assert!(!response.ok);
        assert!(
            response
                .diagnostics
                .contains("error: unknown variable: unknown-function"),
            "{}",
            response.diagnostics
        );
        assert!(
            response.diagnostics.contains("main.typ"),
            "{}",
            response.diagnostics
        );
        assert!(!output.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
