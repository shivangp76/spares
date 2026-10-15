//! Running the programs `spares deploy` drives, over SSH or here, and asking the user.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use colored::Colorize;
use inquire::Confirm;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;

pub(super) fn bold(title: &str) {
    println!("\n{}", format!("==> {title}").bold());
}

pub(super) fn warn(message: &str) {
    eprintln!("{}", message.yellow());
}

/// Asks a yes or no question, yes by default. Escape answers no.
pub(super) fn confirm(question: &str) -> Result<bool, Error> {
    match Confirm::new(question).with_default(true).prompt() {
        Ok(answer) => Ok(answer),
        Err(inquire::InquireError::OperationCanceled) => Ok(false),
        Err(e) => Err(miette!("{e}")),
    }
}

/// Waits for Enter.
pub(super) fn pause(message: &str) -> Result<(), Error> {
    print!("{message}");
    std::io::stdout().flush().into_diagnostic()?;
    std::io::stdin()
        .read_line(&mut String::new())
        .into_diagnostic()?;
    Ok(())
}

pub(super) fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

/// `s` quoted for a POSIX shell.
pub(super) fn quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Runs `command`, failing if it does.
pub(super) fn run(command: &mut Command) -> Result<(), Error> {
    let status = command
        .status()
        .map_err(|e| miette!("Could not run {}: {e}", program(command)))?;
    if status.success() {
        Ok(())
    } else {
        Err(miette!("{} failed ({status}).", program(command)))
    }
}

/// The output of `command`, trimmed, failing if it does. Its errors go to the terminal.
pub(super) fn output(command: &mut Command) -> Result<String, Error> {
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| miette!("Could not run {}: {e}", program(command)))?;
    if !output.status.success() {
        return Err(miette!("{} failed ({}).", program(command), output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn program(command: &Command) -> String {
    let program = command.get_program().to_string_lossy();
    match command.get_args().next().and_then(OsStr::to_str) {
        Some(arg) if !arg.starts_with('-') && program != "ssh" => format!("`{program} {arg}`"),
        _ => format!("`{program}`"),
    }
}

/// ssh and scp sharing one connection per host for the whole run, so Tailscale SSH asks to log in
/// at most once.
pub(super) struct Ssh {
    options: Vec<String>,
}

impl Ssh {
    pub(super) fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        Self {
            options: [
                "-o",
                "ControlMaster=auto",
                "-o",
                &format!("ControlPath={home}/.ssh/spares-deploy-%C"),
                "-o",
                "ControlPersist=10m",
            ]
            .map(String::from)
            .to_vec(),
        }
    }

    /// `ssh` with the shared connection's options; add the host and the command.
    pub(super) fn command(&self) -> Command {
        let mut command = Command::new("ssh");
        command.args(&self.options);
        command
    }

    /// The output of `remote_command`, a shell command, run on `host`.
    pub(super) fn output(&self, host: &str, remote_command: &str) -> Result<String, Error> {
        output(self.command().args([host, remote_command]))
    }

    /// Runs `remote_command` on `host` in a terminal, so it can ask for input.
    pub(super) fn run_tty(&self, host: &str, remote_command: &str) -> Result<(), Error> {
        run(self.command().args(["-t", host, remote_command]))
    }

    /// Copies `files` to `destination` (`host:path`).
    pub(super) fn upload(&self, files: &[&Path], destination: &str) -> Result<(), Error> {
        run(Command::new("scp")
            .arg("-q")
            .args(&self.options)
            .args(files)
            .arg(destination))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_leaves_plain_words_alone() {
        assert_eq!(
            quote("/Users/me/.local/share/spares"),
            "/Users/me/.local/share/spares"
        );
        assert_eq!(quote("America/New_York"), "America/New_York");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("$HOME"), "'$HOME'");
    }
}
