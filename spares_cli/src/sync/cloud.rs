use std::path::Path;
use std::process::Command;

use spares_core::config::get_data_dir;
use spares_core::config::read_external_config;
use spares_core::parsers::get_all_parsers;
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

const DATABASE_FILENAME: &str = "spares-main.sqlite";

pub(crate) async fn sync_cloud() -> Result<(), String> {
    let config = read_external_config().map_err(|e| e.to_string())?;
    let remote_host = config
        .remote_host
        .ok_or("remote_host is not set in config.toml")?;

    let data_dir = get_data_dir();
    let local = data_dir.to_str().unwrap();

    // Pass 1: DB only
    println!("Syncing database → {}", remote_host);
    sync_database(&remote_host, &data_dir).await?;

    // Pass 2: Everything else (cards, image occlusions, render assets). The server reads notes from
    // the database, so raw note files are not needed. Neither are the raw cards of parsers that
    // render in the browser, since they are generated from the database on demand.
    println!("Syncing files → {}", remote_host);
    let mut excludes = vec![
        // The database, its WAL/SHM/journal files and local backups. The database is synced above.
        "--exclude=*.sqlite*".to_string(),
        "--exclude=/notes/".to_string(),
    ];
    excludes.extend(
        get_all_parsers()
            .into_iter()
            .map(|parser| parser())
            .filter(|parser| parser.renders_in_browser())
            .map(|parser| format!("--exclude=/cards/{}/", parser.get_parser_name())),
    );
    let source = format!("{}/", local);
    let destination = format!("{}:{}/", remote_host, local);
    let mut args = vec!["-avz"];
    args.extend(excludes.iter().map(String::as_str));
    args.extend([source.as_str(), destination.as_str()]);
    run_rsync(&args)?;

    println!("Done.");
    Ok(())
}

/// Replaces the remote database with a consistent snapshot of the local one.
///
/// Copying the database file directly is unsafe: changes still in the `-wal` file would be
/// missing, and the remote's stale `-wal`/`-shm` files could be applied to the new file, corrupting
/// it. Instead a snapshot is taken with `VACUUM INTO` (which includes the WAL and is safe while the
/// local server is running), uploaded next to the remote database, and loaded with `sqlite3`'s
/// `.restore`. That uses SQLite's backup API, so it is safe while the remote server has the
/// database open.
async fn sync_database(remote_host: &str, data_dir: &Path) -> Result<(), String> {
    // The remote keeps its data in the same directory as the local machine.
    let database = data_dir.join(DATABASE_FILENAME);
    let database_str = database
        .to_str()
        .ok_or("Database path is not valid UTF-8")?;
    let incoming = format!("{database_str}.incoming");
    let snapshot =
        std::env::temp_dir().join(format!("{DATABASE_FILENAME}.sync-{}", std::process::id()));
    let snapshot_str = snapshot
        .to_str()
        .ok_or("Snapshot path is not valid UTF-8")?;
    let result = async {
        snapshot_database(&database, &snapshot).await?;
        run_rsync(&["-avz", snapshot_str, &format!("{remote_host}:{incoming}")])?;
        run_ssh(remote_host, &restore_command(database_str, &incoming))
    }
    .await;
    let _ = std::fs::remove_file(&snapshot);
    result
}

/// Writes a consistent copy of `database`, including changes still in its WAL, to `snapshot`.
async fn snapshot_database(database: &Path, snapshot: &Path) -> Result<(), String> {
    // `VACUUM INTO` refuses to overwrite a file.
    let _ = std::fs::remove_file(snapshot);
    let mut connection = SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .connect()
        .await
        .map_err(|e| format!("Failed to open {}: {e}", database.display()))?;
    sqlx::query("VACUUM INTO ?")
        .bind(
            snapshot
                .to_str()
                .ok_or("Snapshot path is not valid UTF-8")?,
        )
        .execute(&mut connection)
        .await
        .map_err(|e| format!("Failed to snapshot {}: {e}", database.display()))?;
    Ok(())
}

/// Shell command that loads `incoming` into `database` and then deletes `incoming`. `sqlite3` runs
/// each argument after the database as a separate command. The timeout waits for the server to
/// release its locks instead of failing immediately.
fn restore_command(database: &str, incoming: &str) -> String {
    format!(
        "sqlite3 {} {} {} && rm -f {}",
        shell_quote(database),
        shell_quote(".timeout 30000"),
        shell_quote(&format!(".restore {}", sqlite_quote(incoming))),
        shell_quote(incoming),
    )
}

/// Quotes `s` as a single POSIX shell word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quotes `s` as an argument to a `sqlite3` dot-command.
fn sqlite_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

fn run_ssh(remote_host: &str, command: &str) -> Result<(), String> {
    let status = Command::new("ssh")
        .arg(remote_host)
        .arg(command)
        .status()
        .map_err(|e| format!("Failed to run ssh: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Restoring the database on {remote_host} failed ({status}). `sqlite3` must be installed there."
        ))
    }
}

fn run_rsync(args: &[&str]) -> Result<(), String> {
    let status = Command::new("rsync")
        .args(args)
        .status()
        .map_err(|e| format!("Failed to run rsync: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rsync exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use sqlx::ConnectOptions;
    use sqlx::Connection;
    use sqlx::SqliteConnection;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::sqlite::SqliteJournalMode;

    use super::restore_command;
    use super::snapshot_database;

    async fn open(path: &PathBuf) -> SqliteConnection {
        let mut connection = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .connect()
            .await
            .unwrap();
        // Keep writes in the WAL so the test covers uncheckpointed changes.
        sqlx::query("PRAGMA wal_autocheckpoint = 0")
            .execute(&mut connection)
            .await
            .unwrap();
        connection
    }

    async fn values(connection: &mut SqliteConnection) -> Vec<String> {
        sqlx::query_scalar("SELECT value FROM t ORDER BY value")
            .fetch_all(connection)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_sync_database_snapshot_and_restore() {
        let dir =
            std::env::temp_dir().join(format!("spares-cloud-sync-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("local's db.sqlite");
        let remote = dir.join("remote's db.sqlite");
        let incoming = dir.join("remote's db.sqlite.incoming");

        // Local database with a change that is only in its WAL.
        let mut local_connection = open(&local).await;
        for statement in [
            "CREATE TABLE t (value TEXT)",
            "INSERT INTO t VALUES ('a'), ('b')",
        ] {
            sqlx::query(statement)
                .execute(&mut local_connection)
                .await
                .unwrap();
        }
        assert!(
            std::fs::metadata(local.with_extension("sqlite-wal"))
                .unwrap()
                .len()
                > 0
        );

        // Remote database held open by the "server", with stale changes in its own WAL.
        let mut server_connection = open(&remote).await;
        for statement in [
            "CREATE TABLE t (value TEXT)",
            "INSERT INTO t VALUES ('stale')",
        ] {
            sqlx::query(statement)
                .execute(&mut server_connection)
                .await
                .unwrap();
        }

        snapshot_database(&local, &incoming).await.unwrap();
        let status = Command::new("sh")
            .arg("-c")
            .arg(restore_command(
                remote.to_str().unwrap(),
                incoming.to_str().unwrap(),
            ))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!incoming.exists());

        assert_eq!(values(&mut server_connection).await, ["a", "b"]);
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&mut server_connection)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");

        local_connection.close().await.unwrap();
        server_connection.close().await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
