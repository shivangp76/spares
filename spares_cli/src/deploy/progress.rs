//! What `spares deploy` shows as it goes: a heading for each step and the time it took, a line
//! pinned to the bottom of the terminal with the step it is on and how far along the whole run is,
//! and the times of all steps at the end. How far along is told from how long each step took the
//! last time, kept in the cache.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::io::Write as _;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::Once;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use colored::ColoredString;
use colored::Colorize;
use miette::Error;
use miette::miette;
use serde_json::Value;
use terminal_size::Height;
use terminal_size::Width;
use terminal_size::terminal_size;

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const BAR_WIDTH: usize = 20;
const FRAME: Duration = Duration::from_millis(100);

/// The progress of the one run of `spares deploy` in this process, shared with the thread that
/// redraws the footer and with the Ctrl-C handler.
static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<State>> {
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct State {
    steps: Vec<Step>,
    /// The index in `steps` of the step running, if one is
    current: Option<usize>,
    start: Instant,
    /// How long each step took the last time, by name
    last_times: BTreeMap<String, Duration>,
    /// The terminal's size, while the footer is shown
    footer: Option<(u16, u16)>,
    frame: usize,
}

struct Step {
    name: &'static str,
    started: Option<Instant>,
    took: Option<Duration>,
    parts: Vec<Part>,
}

/// A part of a step, e.g. one of the "==> " steps of `setup_vps.sh`
struct Part {
    name: String,
    started: Instant,
    took: Option<Duration>,
    /// Whether it runs alongside the other parts (see [`Progress::background`]) instead of until
    /// the next one starts
    background: bool,
}

/// A part running alongside others, which ends when this is dropped
pub(super) struct BackgroundPart {
    step: usize,
    part: usize,
}

/// The handle to the progress of a run. Dropping it before [`Progress::finish`] reports the
/// step it stopped at as failed.
pub(super) struct Progress {
    command: &'static str,
    stop: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
    finished: bool,
}

// The state is shared, but taking the handle makes sure a run was started
#[allow(clippy::unused_self)]
impl Progress {
    /// Starts a run of `spares deploy <command>` that goes through `steps`, which are named as
    /// [`Progress::step`] is called with.
    pub(super) fn start(command: &'static str, steps: &[&'static str]) -> Self {
        let initial = State {
            steps: steps
                .iter()
                .map(|&name| Step {
                    name,
                    started: None,
                    took: None,
                    parts: Vec::new(),
                })
                .collect(),
            current: None,
            start: Instant::now(),
            last_times: load_times(command),
            footer: None,
            frame: 0,
        };
        *state() = Some(initial);
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = terminal_size().is_some().then(|| {
            clear_on_ctrl_c();
            show_footer();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(FRAME);
                    if let Some(state) = state().as_mut() {
                        state.frame += 1;
                        state.redraw();
                    }
                }
            })
        });
        Self {
            command,
            stop,
            ticker,
            finished: false,
        }
    }

    /// Ends the step running, and starts the one named `name`, printing its heading: `name`
    /// followed by `detail`.
    pub(super) fn step(&self, name: &'static str, detail: &str) {
        let mut guard = state();
        let state = guard.as_mut().expect("started");
        state.end_step();
        let i = if let Some(i) = state.steps.iter().position(|step| step.name == name) {
            i
        } else {
            state.steps.push(Step {
                name,
                started: None,
                took: None,
                parts: Vec::new(),
            });
            state.steps.len() - 1
        };
        state.steps[i].started = Some(Instant::now());
        state.current = Some(i);
        let count = format!("[{}/{}]", i + 1, state.steps.len());
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(" {detail}")
        };
        println!("\n{} {}{detail}", count.blue().bold(), name.bold());
        state.redraw();
    }

    /// Starts a part of the step running, ending the one before.
    pub(super) fn part(&self, name: &str) {
        let mut guard = state();
        let state = guard.as_mut().expect("started");
        let Some(step) = state.current.map(|i| &mut state.steps[i]) else {
            return;
        };
        end_parts(step, false);
        step.parts.push(Part {
            name: name.to_string(),
            started: Instant::now(),
            took: None,
            background: false,
        });
        println!("  {} {name}", "›".cyan().bold());
        state.redraw();
    }

    /// Starts a part of the step running alongside the others, e.g. on another thread, which ends
    /// when the returned handle is dropped, or with the step. Unlike [`Progress::part`], it does
    /// not end the part before, and it prints the time it took when it ends.
    pub(super) fn background(&self, name: &str) -> BackgroundPart {
        let mut guard = state();
        let state = guard.as_mut().expect("started");
        let step = state.current.expect("in a step");
        let parts = &mut state.steps[step].parts;
        parts.push(Part {
            name: name.to_string(),
            started: Instant::now(),
            took: None,
            background: true,
        });
        let part = parts.len() - 1;
        println!("  {} {name}", "›".cyan().bold());
        state.redraw();
        BackgroundPart { step, part }
    }

    /// Ends the step running, printing the time it took.
    pub(super) fn end_step(&self) {
        if let Some(state) = state().as_mut() {
            state.end_step();
        }
    }

    /// Ends the run: prints the time of each step and in all, and keeps them for the next run.
    pub(super) fn finish(mut self) {
        self.finished = true;
        self.stop_ticker();
        let Some(mut state) = state().take() else {
            return;
        };
        state.end_step();
        hide_footer(&mut state);
        let total = state.start.elapsed();
        let rows: Vec<(usize, &str, Duration)> = state
            .steps
            .iter()
            .filter_map(|step| Some((step, step.took?)))
            .flat_map(|(step, took)| {
                let parts = step
                    .parts
                    .iter()
                    .filter_map(|part| Some((4, part.name.as_str(), part.took?)));
                std::iter::once((2, step.name, took)).chain(parts)
            })
            .collect();
        let width = rows
            .iter()
            .map(|(indent, name, _)| indent + name.chars().count())
            .max()
            .unwrap_or(0)
            .max(10);
        println!("\n{}", "Times".bold());
        for (indent, name, took) in rows {
            let label = format!("{:indent$}{name}", "");
            let time = format!("{:>8}", human(took));
            if indent == 2 {
                println!("{label:width$}{time}");
            } else {
                println!("{}", format!("{label:width$}{time}").dimmed());
            }
        }
        println!(
            "\n{} {}",
            "✓".green().bold(),
            format!("Done in {}", human(total)).green().bold()
        );
        save_times(
            self.command,
            state
                .steps
                .iter()
                .filter_map(|step| Some((step.name, step.took?))),
        );
    }

    /// Runs `command`, passing its output through to the terminal, and makes each "==> " line of
    /// its standard output a part of the step running. It passes the output on as it comes, not by
    /// lines, so prompts without a newline (e.g. sudo's) still show.
    pub(super) fn run(&self, mut command: Command) -> Result<(), Error> {
        let program = command.get_program().to_string_lossy().into_owned();
        let mut child = command
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| miette!("Could not run {program}: {e}"))?;
        let mut stdout = child.stdout.take().expect("piped");
        let mut lines = StepLines::default();
        let mut buffer = vec![0; 65536];
        loop {
            let n = match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            for piece in lines.read(&buffer[..n]) {
                match piece {
                    Piece::Output(bytes) => out(&bytes),
                    Piece::Step(title) => self.part(&title),
                }
            }
        }
        out(&lines.held);
        let status = child.wait().map_err(|e| miette!("{e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(miette!("{program} failed ({status})."))
        }
    }

    fn stop_ticker(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
    }
}

impl Drop for BackgroundPart {
    fn drop(&mut self) {
        let mut guard = state();
        let Some(state) = guard.as_mut() else {
            return;
        };
        let part = &mut state.steps[self.step].parts[self.part];
        if part.took.is_none() {
            let took = part.started.elapsed();
            part.took = Some(took);
            println!(
                "  {} {}",
                "✓".green(),
                format!("{} took {}", part.name, human(took)).dimmed()
            );
            state.redraw();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.stop_ticker();
        let Some(mut state) = state().take() else {
            return;
        };
        hide_footer(&mut state);
        if let Some(step) = state.current.map(|i| &state.steps[i]) {
            let took = step
                .started
                .map(|started| started.elapsed())
                .unwrap_or_default();
            eprintln!(
                "\n{} {}",
                "✗".red().bold(),
                format!(
                    "{} failed after {} ({} in all)",
                    step.name,
                    human(took),
                    human(state.start.elapsed())
                )
                .red()
            );
        }
    }
}

/// Runs `f`, e.g. a prompt, with the footer hidden.
pub(super) fn without_footer<T>(f: impl FnOnce() -> T) -> T {
    let shown = state().as_mut().is_some_and(hide_footer);
    let result = f();
    if shown {
        show_footer();
    }
    result
}

impl State {
    fn end_step(&mut self) {
        let Some(i) = self.current.take() else {
            return;
        };
        let step = &mut self.steps[i];
        end_parts(step, true);
        let took = step
            .started
            .map(|started| started.elapsed())
            .unwrap_or_default();
        step.took = Some(took);
        println!(
            "{} {}",
            "✓".green().bold(),
            format!("{} took {}", step.name, human(took)).dimmed()
        );
    }

    /// The time the whole run is expected to take, from the times of the last run, if every step
    /// has one.
    fn expected_total(&self) -> Option<Duration> {
        let mut total = Duration::ZERO;
        for step in &self.steps {
            let last = self.last_times.get(step.name).copied();
            total += match (step.took, step.started) {
                (Some(took), _) => took,
                (None, Some(started)) => last?.max(started.elapsed()),
                (None, None) => last?,
            };
        }
        // Time outside the steps, e.g. before the first
        let in_steps: Duration = self
            .steps
            .iter()
            .filter_map(|step| step.took)
            .sum::<Duration>()
            + self
                .current
                .and_then(|i| self.steps[i].started)
                .map(|started| started.elapsed())
                .unwrap_or_default();
        Some(total + self.start.elapsed().saturating_sub(in_steps))
    }

    /// Draws the footer, at the new size if the terminal was resized.
    fn redraw(&mut self) {
        let Some(shown) = self.footer else {
            return;
        };
        if let Some((Width(cols), Height(rows))) = terminal_size()
            && (rows, cols) != shown
        {
            self.footer = Some((rows, cols));
            set_region(rows);
        }
        let (rows, cols) = self.footer.expect("shown");
        let line = render(&self.footer_segments(), usize::from(cols).saturating_sub(1));
        out(format!("\x1b7\x1b[{rows};1H\x1b[2K{line}\x1b8").as_bytes());
    }

    /// The footer as styled segments: on the left the step and its part, on the right how far
    /// along the run is.
    fn footer_segments(&self) -> (Vec<Segment>, Vec<Segment>) {
        let spinner = SPINNER[self.frame % SPINNER.len()];
        let mut left = vec![segment(format!("{spinner} "), |s| s.cyan())];
        if let Some(i) = self.current {
            let step = &self.steps[i];
            left.push(segment(format!("{}/{} ", i + 1, self.steps.len()), |s| {
                s.dimmed()
            }));
            left.push(segment(step.name.to_string(), |s| s.bold()));
            let running: Vec<&str> = step
                .parts
                .iter()
                .filter(|part| part.took.is_none())
                .map(|part| part.name.as_str())
                .collect();
            if !running.is_empty() {
                left.push(segment(format!(" › {}", running.join(" · ")), |s| {
                    s.normal()
                }));
            }
            if let Some(started) = step.started {
                left.push(segment(format!("  {}", clock(started.elapsed())), |s| {
                    s.dimmed()
                }));
            }
        }
        let elapsed = self.start.elapsed();
        let right = match self.expected_total() {
            Some(expected) if !expected.is_zero() => {
                let filled = (BAR_WIDTH as u128 * elapsed.as_millis() / expected.as_millis())
                    .min(BAR_WIDTH as u128) as usize;
                vec![
                    segment("█".repeat(filled), |s| s.cyan()),
                    segment("░".repeat(BAR_WIDTH - filled), |s| s.dimmed()),
                    segment(format!(" {} / ~{}", clock(elapsed), clock(expected)), |s| {
                        s.normal()
                    }),
                ]
            }
            _ => vec![segment(clock(elapsed), |s| s.normal())],
        };
        (left, right)
    }
}

/// Text and how it looks
type Segment = (String, fn(&str) -> ColoredString);

fn segment(text: String, style: fn(&str) -> ColoredString) -> Segment {
    (text, style)
}

/// `segments` laid out in `width` columns, the right ones at the right edge, dropped if they do
/// not fit, and the left ones cut short if they do not.
fn render((left, right): &(Vec<Segment>, Vec<Segment>), width: usize) -> String {
    let len = |segments: &[Segment]| -> usize {
        segments.iter().map(|(text, _)| text.chars().count()).sum()
    };
    let (left_len, right_len) = (len(left), len(right));
    let mut line = String::new();
    let mut room = width;
    for (text, style) in left {
        let text: String = text.chars().take(room).collect();
        room -= text.chars().count();
        line.push_str(&style(&text).to_string());
    }
    if left_len + 2 + right_len <= width {
        line.push_str(&" ".repeat(width - left_len - right_len));
        for (text, style) in right {
            line.push_str(&style(text).to_string());
        }
    }
    line
}

/// Ends the parts of `step` running, but those in the background only if `background`.
fn end_parts(step: &mut Step, background: bool) {
    for part in &mut step.parts {
        if part.took.is_none() && (background || !part.background) {
            part.took = Some(part.started.elapsed());
        }
    }
}

/// Keeps the last line of the terminal for the footer and draws it.
fn show_footer() {
    let Some((Width(cols), Height(rows))) = terminal_size() else {
        return;
    };
    let mut guard = state();
    let Some(state) = guard.as_mut() else {
        return;
    };
    // Makes room for the footer if the cursor is on the last line
    out(b"\n\x1b[1A");
    set_region(rows);
    state.footer = Some((rows, cols));
    state.redraw();
}

/// Gives the whole terminal back to the output and blanks the footer. Returns whether it was
/// shown.
fn hide_footer(state: &mut State) -> bool {
    let Some((rows, _)) = state.footer.take() else {
        return false;
    };
    clear(rows);
    true
}

/// Scrolls the output above the last line, keeping the cursor where it is.
fn set_region(rows: u16) {
    out(format!("\x1b7\x1b[1;{}r\x1b8", rows.saturating_sub(1)).as_bytes());
}

fn clear(rows: u16) {
    out(format!("\x1b7\x1b[r\x1b8\x1b7\x1b[{rows};1H\x1b[2K\x1b8").as_bytes());
}

fn out(bytes: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

/// Exits on Ctrl-C as before, hiding the footer first if it is shown.
fn clear_on_ctrl_c() {
    static INSTALLED: Once = Once::new();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    INSTALLED.call_once(|| {
        runtime.spawn(async {
            if tokio::signal::ctrl_c().await.is_ok() {
                if let Some(state) = state().as_mut() {
                    hide_footer(state);
                }
                std::process::exit(130);
            }
        });
    });
}

/// e.g. 4.2s, 42s, 3m 07s, 1h 02m
fn human(duration: Duration) -> String {
    let secs = duration.as_secs();
    match secs {
        0..10 => format!("{:.1}s", duration.as_secs_f64()),
        10..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// e.g. 3:07
fn clock(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn times_file() -> Option<std::path::PathBuf> {
    Some(super::cache_dir()?.join("times.json"))
}

/// How long each step of `command` took the last time it succeeded
fn load_times(command: &str) -> BTreeMap<String, Duration> {
    let Some(times) = times_file()
        .and_then(|file| fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return BTreeMap::new();
    };
    times
        .get(command)
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, secs)| {
            let secs = Duration::try_from_secs_f64(secs.as_f64()?).ok()?;
            Some((name.clone(), secs))
        })
        .collect()
}

fn save_times<'a>(command: &str, steps: impl Iterator<Item = (&'a str, Duration)>) {
    let Some(file) = times_file() else {
        return;
    };
    let mut times = fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let steps: serde_json::Map<String, Value> = steps
        .map(|(name, took)| (name.to_string(), took.as_secs_f64().into()))
        .collect();
    times[command] = Value::Object(steps);
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(file, times.to_string());
}

/// What the output of a command is made of
#[derive(Debug, PartialEq)]
enum Piece {
    Output(Vec<u8>),
    /// The title of a "==> " line
    Step(String),
}

/// Picks the "==> " lines out of a command's output. Lines that may be one are held back until
/// they end or turn out not to be; the rest passes on as it comes.
#[derive(Default)]
struct StepLines {
    held: Vec<u8>,
    /// Whether the current line is known not to be a "==> " line
    passing: bool,
}

impl StepLines {
    const PREFIX: &[u8] = b"==> ";

    fn read(&mut self, data: &[u8]) -> Vec<Piece> {
        let mut pieces = Vec::new();
        let mut output = Vec::new();
        for &byte in data {
            if self.passing {
                output.push(byte);
                self.passing = byte != b'\n';
                continue;
            }
            self.held.push(byte);
            if byte == b'\n' {
                let line = self.held.trim_ascii_end();
                if let Some(title) = line.strip_prefix(Self::PREFIX) {
                    if !output.is_empty() {
                        pieces.push(Piece::Output(std::mem::take(&mut output)));
                    }
                    pieces.push(Piece::Step(String::from_utf8_lossy(title).into_owned()));
                    self.held.clear();
                } else {
                    output.append(&mut self.held);
                }
            } else if !(Self::PREFIX.starts_with(&self.held) || self.held.starts_with(Self::PREFIX))
            {
                output.append(&mut self.held);
                self.passing = true;
            }
        }
        if !output.is_empty() {
            pieces.push(Piece::Output(output));
        }
        pieces
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(chunks: &[&[u8]]) -> Vec<Piece> {
        let mut lines = StepLines::default();
        chunks.iter().flat_map(|chunk| lines.read(chunk)).collect()
    }

    fn output(bytes: &[u8]) -> Piece {
        Piece::Output(bytes.to_vec())
    }

    fn step(title: &str) -> Piece {
        Piece::Step(title.to_string())
    }

    #[test]
    fn picks_out_step_lines_split_anywhere() {
        assert_eq!(
            read(&[b"==> Installing packages\r\n"]),
            [step("Installing packages")]
        );
        assert_eq!(
            read(&[b"a\n=", b"=> Install", b"ing pandoc 3.6\nb"]),
            [output(b"a\n"), step("Installing pandoc 3.6"), output(b"b")]
        );
        // Only whole "==> " lines count, and the rest passes as it comes
        assert_eq!(
            read(&[b"x ==> Configuring\n", b"Password: "]),
            [output(b"x ==> Configuring\n"), output(b"Password: ")]
        );
        assert_eq!(read(&[b"==>\n"]), [output(b"==>\n")]);
        assert_eq!(read(&[b"==> Configuring"]), []);
    }

    #[test]
    fn human_durations() {
        assert_eq!(human(Duration::from_millis(4200)), "4.2s");
        assert_eq!(human(Duration::from_secs(42)), "42s");
        assert_eq!(human(Duration::from_secs(187)), "3m 07s");
        assert_eq!(human(Duration::from_secs(3720)), "1h 02m");
    }
}
