//! In its own test binary, since it runs render commands in collections of its own.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use spares_core::Error;
use spares_core::collection::Collection;
use spares_core::collection::CollectionPaths;
use spares_core::parsers::kill_running_renders;
use spares_core::parsers::render_command_output;

async fn collection(name: &str) -> Arc<Collection> {
    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let root = Path::new("/nonexistent/spares/users").join(name);
    Arc::new(Collection::new(name, db, CollectionPaths::in_dir(root)))
}

/// Stopping a collection's renders stops its running and later ones, and no other collection's.
#[tokio::test]
async fn test_kill_running_renders_stops_running_and_later_renders() {
    let stopped = collection("stopped").await;
    let other = collection("other").await;
    let start = Instant::now();
    let renders = (0..3)
        .map(|_| {
            let stopped = Arc::clone(&stopped);
            thread::spawn(move || {
                let _collection = stopped.enter();
                render_command_output(Command::new("sleep").arg("30"), "sleep")
            })
        })
        .collect::<Vec<_>>();
    let other_render = {
        let other = Arc::clone(&other);
        thread::spawn(move || {
            let _collection = other.enter();
            render_command_output(Command::new("sleep").arg("2"), "sleep")
        })
    };
    thread::sleep(Duration::from_millis(500));
    {
        let _collection = stopped.enter();
        kill_running_renders();
    }
    for render in renders {
        let output = render.join().unwrap().unwrap();
        assert!(!output.status.success());
    }
    assert!(start.elapsed() < Duration::from_secs(10));

    // Renders started afterwards are refused
    let result = {
        let _collection = stopped.enter();
        render_command_output(&mut Command::new("true"), "true")
    };
    assert!(matches!(
        result,
        Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::Interrupted
    ));

    // The other collection's renders go on.
    assert!(other_render.join().unwrap().unwrap().status.success());
    let _collection = other.enter();
    assert!(
        render_command_output(&mut Command::new("true"), "true")
            .unwrap()
            .status
            .success()
    );
}
