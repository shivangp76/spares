//! In its own test binary, since [`kill_running_renders`] stops renders for the rest of the process.
#![cfg(unix)]

use std::process::Command;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use spares_core::Error;
use spares_core::parsers::kill_running_renders;
use spares_core::parsers::render_command_output;

#[test]
fn test_kill_running_renders_stops_running_and_later_renders() {
    let start = Instant::now();
    let renders = (0..3)
        .map(|_| thread::spawn(|| render_command_output(Command::new("sleep").arg("30"), "sleep")))
        .collect::<Vec<_>>();
    thread::sleep(Duration::from_millis(500));
    kill_running_renders();
    for render in renders {
        let output = render.join().unwrap().unwrap();
        assert!(!output.status.success());
    }
    assert!(start.elapsed() < Duration::from_secs(10));

    // Renders started afterwards are refused
    let result = render_command_output(&mut Command::new("true"), "true");
    assert!(matches!(
        result,
        Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::Interrupted
    ));
}
