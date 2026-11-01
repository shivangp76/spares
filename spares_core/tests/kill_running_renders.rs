//! In its own test binary, since it runs render commands in collections of its own.
#![cfg(unix)]

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
    let root = std::env::temp_dir()
        .join(format!("spares-kill-renders-{}", std::process::id()))
        .join(name);
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
    // Killed while running, or refused if it hadn't started yet
    for render in renders {
        match render.join().unwrap() {
            Ok(output) => assert!(!output.status.success()),
            Err(Error::Io { source, .. }) => {
                assert_eq!(source.kind(), std::io::ErrorKind::Interrupted);
            }
            Err(e) => panic!("{e}"),
        }
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

/// A render reads its collection and the system, writes its files and scratch directories, and
/// nothing else: not another collection's files, not the network.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn renders_run_in_a_sandbox() {
    let base = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("spares-sandbox-{}", std::process::id()));
    let paths = |user: &str| {
        CollectionPaths::new(
            base.join("data/users").join(user),
            base.join("cache/users").join(user),
        )
    };
    let (mine, theirs) = (paths("mine"), paths("theirs"));
    for paths in [&mine, &theirs] {
        paths.create().unwrap();
        std::fs::write(paths.root.join("secret.txt"), "secret").unwrap();
        std::fs::write(paths.cache.join("card.typ"), "card").unwrap();
    }
    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let collection = Arc::new(Collection::new("mine", db, mine.clone()));
    let _entered = collection.enter();
    let run = |script: String| {
        render_command_output(Command::new("/bin/sh").args(["-c", &script]), "sh").unwrap()
    };
    for own in [mine.root.join("secret.txt"), mine.cache.join("card.typ")] {
        let read = run(format!("cat {}", own.display()));
        assert!(read.status.success(), "{}: {read:?}", own.display());
    }
    for other in [
        theirs.root.join("secret.txt"),
        theirs.cache.join("card.typ"),
    ] {
        let read = run(format!("cat {}", other.display()));
        assert!(!read.status.success(), "{}: {read:?}", other.display());
    }
    let home = std::env::var("HOME").unwrap();
    assert!(
        !run(format!("ls {home}/.ssh {home}/Documents"))
            .status
            .success()
    );
    let write_files = run(format!("touch {}", mine.files.join("out.pdf").display()));
    assert!(write_files.status.success(), "{write_files:?}");
    let write_scratch = run(format!("touch {}", mine.render.join("x").display()));
    assert!(write_scratch.status.success(), "{write_scratch:?}");
    for outside in [mine.database(), theirs.files.join("x.pdf")] {
        let write = run(format!("touch {}", outside.display()));
        assert!(!write.status.success(), "{}: {write:?}", outside.display());
    }
    let network = run("/usr/bin/nc -z -G 3 1.1.1.1 443".to_string());
    assert!(!network.status.success(), "{network:?}");
    std::fs::remove_dir_all(&base).unwrap();
}
