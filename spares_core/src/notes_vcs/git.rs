use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;

use crate::Error;

/// Variables git sets for hooks, or that a shell may carry, which would make git operate on a
/// different repository, index or object store than the one it is run in.
pub const GIT_LOCATION_VARIABLES: [&str; 10] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_QUARANTINE_PATH",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_COMMON_DIR",
    "GIT_LITERAL_PATHSPECS",
];

/// A `git` command run in `repo`, isolated from the user's configuration and the caller's git
/// environment. Commits made with it are attributed to `spares`.
pub fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    for variable in GIT_LOCATION_VARIABLES {
        command.env_remove(variable);
    }
    command
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "user.name=spares",
            "-c",
            "user.email=spares@localhost",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.quotepath=off",
            "-c",
            "status.renames=false",
        ]);
    command
}

/// Runs git and returns its output, whatever its exit status.
pub fn git_output<S: AsRef<OsStr>>(
    repo: &Path,
    args: &[S],
    stdin: Option<&[u8]>,
) -> Result<Output, Error> {
    let mut command = git_command(repo);
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| Error::NotesVcs(format!("Failed to run git: {e}")))?;
    if let Some(input) = stdin {
        // Dropping stdin afterwards closes it, so git sees the end of the input.
        let mut pipe = child.stdin.take().expect("stdin is piped");
        pipe.write_all(input)
            .map_err(|e| Error::NotesVcs(format!("Failed to write to git: {e}")))?;
    }
    child
        .wait_with_output()
        .map_err(|e| Error::NotesVcs(format!("Failed to run git: {e}")))
}

/// Runs git and returns its stdout, or an error with its stderr if it failed.
pub fn git<S: AsRef<OsStr>>(
    repo: &Path,
    args: &[S],
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    let output = git_output(repo, args, stdin)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_error(args, &output))
    }
}

pub fn git_error<S: AsRef<OsStr>>(args: &[S], output: &Output) -> Error {
    let args = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    Error::NotesVcs(format!(
        "`git {args}` failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Paths separated by NUL bytes, for `--pathspec-from-file=- --pathspec-file-nul` and `-z` output.
pub fn nul_join<P: AsRef<Path>>(paths: &[P]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(path.as_ref().as_os_str().as_encoded_bytes());
        out.push(0);
    }
    out
}

pub fn nul_split(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split(|&b| b == 0).filter(|s| !s.is_empty())
}

pub fn rev_parse(repo: &Path, rev: &str) -> Result<String, Error> {
    let out = git(
        repo,
        &["rev-parse", "--verify", "--end-of-options", rev],
        None,
    )?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}
