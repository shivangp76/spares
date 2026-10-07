use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;
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

/// What compiling a rendered file read, besides the raw file itself. Written next to the rendered
/// file by parsers that [render in the browser](crate::parsers::Parseable::renders_in_browser).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RenderDependencies {
    /// Absolute paths of local files.
    pub files: Vec<PathBuf>,
    /// Package specs as `namespace/name/version`, e.g. `preview/cetz/0.5.2`.
    pub packages: Vec<String>,
}

pub fn render_dependencies_filepath(output_rendered_filepath: &Path) -> PathBuf {
    output_rendered_filepath.with_extension("deps.json")
}

#[derive(Debug)]
pub struct NoteFilepathData {
    pub parser_name: String,
    pub note_id: NoteId,
}

/// Text to pass to the first answerable cloze when viewing the card front so that the pdf viewer
/// can scroll to it.
pub fn cloze_tag_str(note_id: NoteId, card_order: impl std::fmt::Display) -> String {
    format!("card-{note_id}-{card_order}")
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
        .map_err(|_| "Failed to parse note id".to_string())?;
    Ok(NoteFilepathData {
        parser_name,
        note_id,
    })
}

/// How long a command rendering a note or card may run before it is stopped.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(60);

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
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Io {
            description: format!("Failed to run {name} command"),
            source: e,
        })?;
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
        if let Some(status) = child.try_wait().map_err(wait_error)? {
            break status;
        }
        if Instant::now() >= deadline {
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
    #[cfg(unix)]
    if let Ok(process_group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `killpg` only sends a signal. The group is the child's, which has not been
        // reaped yet, so its id cannot have been reused.
        unsafe { libc::killpg(process_group, libc::SIGKILL) };
        return;
    }
    let _ = child.kill();
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
