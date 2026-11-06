//! What the server runs: its version and build time, and the web app build it serves. The web app
//! compares that build with its own to offer a reload once the server has a newer one, since an
//! installed web app, as on iOS, is often a page resumed from days ago rather than loaded again.

use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

use axum::Json;
use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;

/// The web app's build has its id here (`build-id.ts` in `spares_frontend` writes it).
const BUILD_FILE: &str = "build.json";

#[derive(Debug, Serialize)]
pub(crate) struct Version {
    server: ServerVersion,
    /// The contents of the web app's `build.json` (`{"build": "<id>"}`), or null when the server
    /// serves no web app, or one built without an id, as by vite's dev server.
    frontend: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ServerVersion {
    /// The crate's version, which isn't bumped as it changes, so `built` tells builds apart.
    version: &'static str,
    /// When the running binary was built, by its file's modification time. Null if that can't be
    /// read.
    built: Option<DateTime<Utc>>,
    /// When the server started, so a deploy that didn't restart it shows.
    started: DateTime<Utc>,
}

/// Read when the server starts ([`init`]): the binary may be replaced while it runs, and
/// on Linux its path then names the new file, or one that is gone.
static SERVER: LazyLock<ServerVersion> = LazyLock::new(|| ServerVersion {
    version: env!("CARGO_PKG_VERSION"),
    built: std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from),
    started: Utc::now(),
});

/// Records the server's build and start time, before its binary can be replaced.
pub(crate) fn init() {
    LazyLock::force(&SERVER);
}

/// `GET /api/version`: the server's version, and the web app build in `frontend_dir`. The build is
/// read on each request, since a deploy can replace the web app without restarting the server.
pub(crate) async fn version_handler(frontend_dir: Option<PathBuf>) -> Json<Version> {
    let frontend = match frontend_dir {
        Some(dir) => tokio::task::spawn_blocking(move || read_build(&dir))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    Json(Version {
        server: ServerVersion {
            version: SERVER.version,
            built: SERVER.built,
            started: SERVER.started,
        },
        frontend,
    })
}

fn read_build(dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join(BUILD_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_the_frontend_build_on_each_request() {
        let dir = std::env::temp_dir().join(format!("spares-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let version = |dir: Option<PathBuf>| async { version_handler(dir).await.0 };

        assert!(version(None).await.frontend.is_none());
        // A build without an id
        assert!(version(Some(dir.clone())).await.frontend.is_none());

        std::fs::write(dir.join(BUILD_FILE), r#"{"build":"abc"}"#).unwrap();
        let first = version(Some(dir.clone())).await;
        assert_eq!(first.frontend.unwrap()["build"], "abc");
        assert_eq!(first.server.version, env!("CARGO_PKG_VERSION"));
        assert!(first.server.built.is_some());

        std::fs::write(dir.join(BUILD_FILE), r#"{"build":"def"}"#).unwrap();
        let second = version(Some(dir.clone())).await;
        assert_eq!(second.frontend.unwrap()["build"], "def");
        assert_eq!(second.server.started, first.server.started);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
