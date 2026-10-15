//! A progress bar pinned to the terminal's last line, below the output of a command, that follows
//! the "==> " lines `setup_vps.sh` prints for its steps.

use std::io::Read as _;
use std::io::Write as _;
use std::process::Command;
use std::process::Stdio;
use std::sync::Once;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use miette::Error;
use miette::miette;
use terminal_size::Height;
use terminal_size::Width;
use terminal_size::terminal_size;

const WIDTH: usize = 20;

/// The rows of the terminal while a bar is shown, else 0, for clearing it on Ctrl-C.
static SHOWN_ROWS: AtomicU16 = AtomicU16::new(0);

pub(super) struct StatusBar {
    steps: &'static [&'static str],
    step: usize,
    start: Instant,
    rows: u16,
    cols: u16,
    /// The line being received, up to its last 1000 bytes
    line: Vec<u8>,
}

impl StatusBar {
    /// Shows the bar at the first step, or nothing if the output is not a terminal.
    pub(super) fn start(steps: &'static [&'static str]) -> Option<Self> {
        let (Width(cols), Height(rows)) = terminal_size()?;
        let bar = Self {
            steps,
            step: 0,
            start: Instant::now(),
            rows,
            cols,
            line: Vec::new(),
        };
        clear_on_ctrl_c();
        // Makes room for the bar if the cursor is on the last line
        out(b"\n\x1b[1A");
        bar.set_region();
        bar.draw();
        Some(bar)
    }

    /// Scrolls the output above the last line, keeping the cursor where it is.
    fn set_region(&self) {
        SHOWN_ROWS.store(self.rows, Ordering::Relaxed);
        out(format!("\x1b7\x1b[1;{}r\x1b8", self.rows.saturating_sub(1)).as_bytes());
    }

    fn draw(&self) {
        let elapsed = self.start.elapsed().as_secs();
        let filled = WIDTH * self.step / self.steps.len();
        let label = format!(
            " {}/{} {}  {}:{:02}",
            self.step + 1,
            self.steps.len(),
            self.steps[self.step],
            elapsed / 60,
            elapsed % 60
        );
        let label: String = label
            .chars()
            .take(usize::from(self.cols).saturating_sub(WIDTH + 1))
            .collect();
        out(format!(
            "\x1b7\x1b[{};1H\x1b[2K\x1b[1m{}\x1b[2m{}\x1b[0m{label}\x1b8",
            self.rows,
            "█".repeat(filled),
            "░".repeat(WIDTH - filled),
        )
        .as_bytes());
    }

    /// Redraws the bar, at the new size if the terminal was resized.
    fn tick(&mut self) {
        if let Some((Width(cols), Height(rows))) = terminal_size()
            && (cols, rows) != (self.cols, self.rows)
        {
            (self.cols, self.rows) = (cols, rows);
            self.set_region();
        }
        self.draw();
    }

    /// Moves the bar to the step whose title starts the latest "==> " line of `data`, so steps
    /// that never come are skipped.
    fn read(&mut self, data: &[u8]) {
        self.line.extend_from_slice(data);
        let Some(end) = self.line.iter().rposition(|&b| b == b'\n') else {
            self.keep_tail();
            return;
        };
        let complete: Vec<u8> = self.line.drain(..=end).collect();
        self.keep_tail();
        let mut moved = false;
        for line in complete.split(|&b| b == b'\n') {
            let line: Vec<u8> = line.iter().copied().filter(|&b| b != b'\r').collect();
            let Some(title) = line.strip_prefix(b"==> ") else {
                continue;
            };
            if let Some(i) =
                (self.step..self.steps.len()).find(|&i| title.starts_with(self.steps[i].as_bytes()))
            {
                self.step = i;
                moved = true;
            }
        }
        if moved {
            self.draw();
        }
    }

    fn keep_tail(&mut self) {
        if self.line.len() > 1000 {
            self.line.drain(..self.line.len() - 1000);
        }
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        clear(self.rows);
        SHOWN_ROWS.store(0, Ordering::Relaxed);
    }
}

/// Runs `command`, passing its output through to the terminal with `bar` below it. It reads and
/// writes as the data comes, not by lines, so prompts without a newline (e.g. sudo's) still show.
pub(super) fn run(bar: Option<StatusBar>, mut command: Command) -> Result<(), Error> {
    let Some(mut bar) = bar else {
        return super::shell::run(&mut command);
    };
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| miette!("Could not run ssh: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buffer = vec![0; 65536];
        while let Ok(n) = stdout.read(&mut buffer)
            && n > 0
        {
            if sender.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(data) => {
                out(&data);
                bar.read(&data);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => bar.tick(),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = reader.join();
    let status = child.wait().map_err(|e| miette!("{e}"))?;
    drop(bar);
    if status.success() {
        Ok(())
    } else {
        Err(miette!("Updating the server failed ({status})."))
    }
}

fn out(bytes: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

/// Gives the whole terminal back to the output, and blanks the bar's line.
fn clear(rows: u16) {
    out(format!("\x1b7\x1b[r\x1b8\x1b7\x1b[{rows};1H\x1b[2K\x1b8").as_bytes());
}

/// Exits on Ctrl-C as before, clearing the bar first if one is shown.
fn clear_on_ctrl_c() {
    static INSTALLED: Once = Once::new();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    INSTALLED.call_once(|| {
        runtime.spawn(async {
            if tokio::signal::ctrl_c().await.is_ok() {
                let rows = SHOWN_ROWS.load(Ordering::Relaxed);
                if rows > 0 {
                    clear(rows);
                }
                std::process::exit(130);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEPS: &[&str] = &[
        "Uploading",
        "Installing packages",
        "Installing pandoc",
        "Configuring",
    ];

    /// The step `bar` is at after reading `chunks`
    fn step_after(chunks: &[&[u8]]) -> usize {
        let mut bar = std::mem::ManuallyDrop::new(StatusBar {
            steps: STEPS,
            step: 0,
            start: Instant::now(),
            rows: 24,
            cols: 80,
            line: Vec::new(),
        });
        for chunk in chunks {
            bar.read(chunk);
        }
        bar.step
    }

    #[test]
    fn follows_step_lines_split_anywhere() {
        assert_eq!(step_after(&[b"==> Installing packages\r\n"]), 1);
        assert_eq!(step_after(&[b"==> Install", b"ing pandoc 3.6\n"]), 2);
        // Skips steps that never come, and never goes back
        assert_eq!(
            step_after(&[b"==> Configuring\n==> Installing packages\n"]),
            3
        );
        // Only whole "==> " lines count
        assert_eq!(step_after(&[b"==> Configuring"]), 0);
        assert_eq!(step_after(&[b"x ==> Configuring\n", b"Configuring\n"]), 0);
        assert_eq!(step_after(&[b"==> Unknown\n"]), 0);
    }
}
