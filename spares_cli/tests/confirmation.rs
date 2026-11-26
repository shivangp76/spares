//! Commands that ask before changing data fail when they can't ask, rather than exit 0 having done
//! nothing.

use std::process::Output;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::Router;
use axum::routing::get;
use spares_core::schema::review::ReschedulePreviewResponse;
use spares_core::schema::review::RescheduleResponse;

/// A fake host answering the reschedule endpoints, counting the reschedules it was asked for.
async fn fake_host() -> (String, Arc<AtomicUsize>) {
    let reschedules = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reschedules);
    let router = Router::new().route(
        "/api/scheduler/{name}/reschedule",
        get(|| async {
            Json(ReschedulePreviewResponse {
                moved_count: 0,
                moved_cards_kept: true,
            })
        })
        .post(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            async { Json(RescheduleResponse { event_id: None }) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    (format!("http://{address}"), reschedules)
}

/// Runs `spares` against `host` with stdin from /dev/null, as a script or `ssh host cmd` would,
/// and nothing of this machine's config.
async fn spares(host: &str, args: &[&str]) -> Output {
    let home = std::env::temp_dir().join(format!("spares-confirmation-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_spares"));
    command
        .args(args)
        .env_remove("SPARES_SCHEDULER")
        .env("SPARES_HOST", host)
        .env("SPARES_TOKEN", "token")
        .env("SPARES_ROOT", home.join("root"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.output().await.unwrap()
}

#[tokio::test]
async fn a_reschedule_without_a_terminal_fails() {
    let (host, reschedules) = fake_host().await;
    let output = spares(&host, &["card", "reschedule", "-s", "fsrs"]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("can't ask for confirmation without a terminal; pass --yes to go ahead"),
        "{stderr}"
    );
    assert_eq!(reschedules.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_reschedule_with_yes_goes_ahead_without_a_terminal() {
    let (host, reschedules) = fake_host().await;
    let output = spares(&host, &["card", "reschedule", "-s", "fsrs", "--yes"]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(reschedules.load(Ordering::SeqCst), 1);
}
