use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::config::get_data_dir;
use crate::model::NoteId;
use crate::parsers::RenderOutputType;

/// This cannot be overridden since [`get_note_info_from_filepath`] needs to be deterministic.
pub fn get_output_raw_dir(
    parser_name: &str,
    output_type: RenderOutputType,
    overridden_base_dir: Option<&Path>,
) -> PathBuf {
    let mut output_raw_dir = match overridden_base_dir {
        Some(base_dir) => base_dir.to_path_buf(),
        _ => get_data_dir(),
    };
    // Note/card is split before parser since everything in the `cards` subdirectory will never have to be edited. Only its rendered file will be viewed. This is not true for notes. Notes will have both their raw file and rendered file viewed. Thus, this will make it easier to just search through notes, instead of notes and cards.
    let sub_dir = match output_type {
        RenderOutputType::Note => "notes",
        RenderOutputType::Card(..) => "cards",
    };
    output_raw_dir.push(sub_dir);
    // SAFETY: The parser is validated so that its name is a valid directory name.
    output_raw_dir.push(parser_name);
    output_raw_dir
}

/// What compiling a rendered file read, besides the raw file itself and files the renderer ships
/// with (e.g. a TeX distribution). Written next to each rendered file (see
/// [`render_dependencies_filepath`]). For parsers that
/// [render in the browser](crate::parsers::Parseable::renders_in_browser), the files are shipped to
/// web clients. For the others, `spares sync cloud` copies them to the server so it can render too.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RenderDependencies {
    /// Absolute paths of local files.
    pub files: Vec<PathBuf>,
    /// Package specs as `namespace/name/version`, e.g. `preview/cetz/0.5.2`.
    pub packages: Vec<String>,
}

impl RenderDependencies {
    /// Sorts and deduplicates the dependencies, then writes them to `dependencies_filepath`.
    pub fn write(mut self, dependencies_filepath: &Path) -> Result<(), String> {
        self.files.sort();
        self.files.dedup();
        self.packages.sort();
        self.packages.dedup();
        let contents = serde_json::to_string(&self).map_err(|e| e.to_string())?;
        std::fs::write(dependencies_filepath, contents).map_err(|e| e.to_string())
    }
}

pub fn render_dependencies_filepath(output_rendered_filepath: &Path) -> PathBuf {
    output_rendered_filepath.with_extension("deps.json")
}

impl RenderDependencies {
    /// Adds `files` to the dependencies already written to `dependencies_filepath`, if any.
    pub fn extend_file(dependencies_filepath: &Path, files: Vec<PathBuf>) -> Result<(), String> {
        let mut dependencies = match std::fs::read_to_string(dependencies_filepath) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|e| e.to_string())?,
            Err(_) => Self::default(),
        };
        dependencies.files.extend(files);
        dependencies.write(dependencies_filepath)
    }
}

/// The HTML written for web clients next to a rendered file by parsers with a
/// [web render](crate::parsers::Parseable::has_web_render). An empty file means web clients show
/// the rendered file instead. It replaces a stale HTML file on the server when synced, since sync
/// does not delete files.
pub fn web_render_filepath(output_rendered_filepath: &Path) -> PathBuf {
    output_rendered_filepath.with_extension("html")
}

/// Whether `output_rendered_filepath` has HTML for web clients.
pub fn has_web_render_file(output_rendered_filepath: &Path) -> bool {
    std::fs::metadata(web_render_filepath(output_rendered_filepath)).is_ok_and(|m| m.len() > 0)
}

/// Whether a note's web clients are shown HTML, read from the `render` key of its custom data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebRenderMode {
    /// HTML unless rendering it loses content.
    #[default]
    Auto,
    /// Always the rendered file.
    Pdf,
    /// HTML whenever it renders, even if content is lost.
    Html,
}

impl WebRenderMode {
    pub const CUSTOM_DATA_KEY: &str = "render";

    /// Unrecognized values are [`Self::Auto`].
    pub fn from_custom_data(custom_data: &crate::model::CustomData) -> Self {
        match custom_data
            .get(Self::CUSTOM_DATA_KEY)
            .and_then(|v| v.as_str())
        {
            Some("pdf") => Self::Pdf,
            Some("html") => Self::Html,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug)]
pub struct NoteFilepathData {
    pub parser_name: String,
    pub note_id: NoteId,
}

/// Text to pass to the first answerable cloze when viewing the card front so that the pdf viewer
/// can scroll to it.
pub fn cloze_tag_str(note_id: NoteId, card_order: impl std::fmt::Display) -> String {
    format!("card-{}-{card_order}", note_id.letters())
}

pub fn get_note_info_from_filepath(note_filepath: &Path) -> Result<NoteFilepathData, String> {
    let parser_name = note_filepath
        .parent()
        .ok_or("Failed to get parent".to_string())?
        .file_name()
        .ok_or("Failed to get file name".to_string())?
        .to_str()
        .ok_or("Failed to convert to string".to_string())?
        .to_string();
    let note_id = note_filepath
        .file_stem()
        .ok_or("Failed to get file stem".to_string())?
        .to_str()
        .ok_or("Failed to convert to string".to_string())?
        .parse::<NoteId>()
        .map_err(|e| format!("Failed to parse note id: {e}"))?;
    Ok(NoteFilepathData {
        parser_name,
        note_id,
    })
}

/// How long a command rendering a note or card may run before it is stopped.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct RunningRenders {
    /// Process groups of render commands that have been spawned but not yet reaped. An id is
    /// removed while holding the lock before or as its process is reaped, so a group in here
    /// cannot have had its id reused.
    process_groups: HashSet<u32>,
    /// Set by [`kill_running_renders`], after which no more render commands are started.
    stopped: bool,
}

static RUNNING_RENDERS: LazyLock<Mutex<RunningRenders>> = LazyLock::new(Default::default);

fn running_renders() -> MutexGuard<'static, RunningRenders> {
    RUNNING_RENDERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stops every running render command, along with any processes it started, and makes any later
/// render command fail without being started. Killing does nothing on platforms other than Unix.
pub fn kill_running_renders() {
    let mut running_renders = running_renders();
    running_renders.stopped = true;
    for &process_group in &running_renders.process_groups {
        kill_process_group(process_group);
    }
}

/// Like [`Command::output`], but stops the command, and any processes it started (e.g. `pdflatex`
/// under `latexmk`), if it runs longer than [`RENDER_TIMEOUT`].
pub fn render_command_output(command: &mut Command, name: &str) -> Result<Output, Error> {
    command_output_with_timeout(command, name, RENDER_TIMEOUT)
}

fn command_output_with_timeout(
    command: &mut Command,
    name: &str,
    timeout: Duration,
) -> Result<Output, Error> {
    // Its own process group, so the whole tree can be killed at once
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    // The lock is held while spawning, so a command cannot start after `kill_running_renders`
    // without being refused or killed.
    let mut child = {
        let mut running_renders = running_renders();
        if running_renders.stopped {
            return Err(Error::Io {
                description: format!("{name} command was not run since renders were stopped"),
                source: std::io::ErrorKind::Interrupted.into(),
            });
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Io {
                description: format!("Failed to run {name} command"),
                source: e,
            })?;
        running_renders.process_groups.insert(child.id());
        child
    };
    // Pipes are drained while waiting, so a command filling one is not blocked writing to it.
    let read_pipe = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buffer);
            }
            buffer
        })
    };
    let stdout = read_pipe(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = read_pipe(child.stderr.take().map(|p| Box::new(p) as _));

    let deadline = Instant::now() + timeout;
    let wait_error = |e| Error::Io {
        description: format!("Failed to wait for {name} command"),
        source: e,
    };
    let status = loop {
        {
            let mut running_renders = running_renders();
            let result = child.try_wait();
            if !matches!(result, Ok(None)) {
                running_renders.process_groups.remove(&child.id());
            }
            if let Some(status) = result.map_err(wait_error)? {
                break status;
            }
        }
        if Instant::now() >= deadline {
            running_renders().process_groups.remove(&child.id());
            kill_process_tree(&mut child);
            let _ = child.wait();
            return Err(Error::Io {
                description: format!(
                    "{name} command was stopped after running for {} seconds",
                    timeout.as_secs()
                ),
                source: std::io::ErrorKind::TimedOut.into(),
            });
        }
        thread::sleep(Duration::from_millis(10));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn kill_process_tree(child: &mut Child) {
    // The group is the child's, which has not been reaped yet, so its id cannot have been reused.
    if !kill_process_group(child.id()) {
        let _ = child.kill();
    }
}

/// Sends `SIGKILL` to a process group whose leader has not been reaped yet. Returns whether the
/// signal could be sent.
fn kill_process_group(process_group: u32) -> bool {
    #[cfg(unix)]
    if let Ok(process_group) = libc::pid_t::try_from(process_group) {
        // SAFETY: `killpg` only sends a signal.
        return unsafe { libc::killpg(process_group, libc::SIGKILL) } == 0;
    }
    let _ = process_group;
    false
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Duration;
    use std::time::Instant;

    use super::command_output_with_timeout;
    use crate::Error;

    #[test]
    fn test_command_output_with_timeout_returns_output() {
        let output = command_output_with_timeout(
            Command::new("sh").args(["-c", "echo out; echo err >&2"]),
            "sh",
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_command_output_with_timeout_kills_process_tree() {
        let pid_file = std::env::temp_dir().join(format!("spares-timeout-{}", std::process::id()));
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let start = Instant::now();
        let result = command_output_with_timeout(
            Command::new("sh").args(["-c", &script]),
            "sh",
            Duration::from_millis(500),
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(matches!(
            result,
            Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::TimedOut
        ));

        // The `sleep` started by the command was killed as well
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let _ = std::fs::remove_file(&pid_file);
        std::thread::sleep(Duration::from_millis(200));
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive);
    }
}
