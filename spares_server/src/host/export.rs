//! Moving a collection between hosts, and backing a host up.
//!
//! - **Export** ([`export`]): a `.tar.gz` of the user's directory, with a consistent copy of the
//!   database. What spares recreates is in the user's cache, outside it (see
//!   [`super::cache_root`]), so it isn't exported: the importing host renders again. A user downloads their own
//!   (`GET /api/account/export`); the admin runs `spares host export`.
//! - **Import** ([`import_archive`]): another host's export, into an account whose collection is
//!   empty (`spares host import <archive> <user>`). A user's directory has the same shape on every
//!   host, so this is unpacking it.
//! - **Backups** ([`backup`]): an export of every user's directory, and a copy of
//!   `accounts.sqlite`, into a directory (`spares host backup`), which works while the host runs.
//!   For continuous backups, point Litestream at each `collection.sqlite` and `accounts.sqlite`
//!   instead, and snapshot the rest of the directories.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use spares_core::Error;

use super::Host;
use super::accounts::invalid;
use super::accounts::random_string;

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        description: path.display().to_string(),
        source,
    }
}

fn run_tar(command: &mut Command) -> Result<(), Error> {
    let output = command.output().map_err(|e| Error::Io {
        description: "Failed to run tar".to_string(),
        source: e,
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(invalid(format!(
            "tar failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// A consistent copy of a SQLite database that may be in use.
pub async fn snapshot(database: &Path, to: &Path) -> Result<(), Error> {
    use std::str::FromStr;
    let _ = std::fs::remove_file(to);
    let options =
        sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite://{}", database.display()))
            .map_err(super::accounts::sqlx_error)?
            .read_only(true);
    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(super::accounts::sqlx_error)?;
    sqlx::query("VACUUM INTO ?")
        .bind(to.to_string_lossy())
        .execute(&db)
        .await
        .map_err(super::accounts::sqlx_error)?;
    db.close().await;
    Ok(())
}

/// Writes user `id`'s directory to `out`, a `.tar.gz`.
pub async fn export(host: &Host, id: &str, out: &Path) -> Result<(), Error> {
    let paths = host.paths(id);
    if !paths.database().is_file() {
        return Err(invalid(format!("{id} has no collection")));
    }
    let staging = host.root.join(format!(".export-{}", random_string(10)));
    std::fs::create_dir_all(&staging).map_err(|e| io(&staging, e))?;
    let result = async {
        snapshot(
            &paths.database(),
            &staging.join(spares_core::collection::DATABASE_FILE_NAME),
        )
        .await?;
        // The directory without its database, then the copy of the database, then compressed:
        // `--exclude` would match the copy too.
        let tar = staging.join("export.tar");
        run_tar(
            Command::new("tar")
                .arg("-cf")
                .arg(&tar)
                .args([
                    "--exclude",
                    "./collection.sqlite*",
                    "--exclude",
                    "./render",
                    "--exclude",
                    "./cache",
                ])
                .arg("-C")
                .arg(&paths.root)
                .arg("."),
        )?;
        run_tar(
            Command::new("tar")
                .arg("-rf")
                .arg(&tar)
                .arg("-C")
                .arg(&staging)
                .arg(spares_core::collection::DATABASE_FILE_NAME),
        )?;
        let file = std::fs::File::create(out).map_err(|e| io(out, e))?;
        let status = Command::new("gzip")
            .arg("-c")
            .arg(&tar)
            .stdout(file)
            .status()
            .map_err(|e| io(out, e))?;
        if status.success() {
            Ok(())
        } else {
            Err(invalid(format!("gzip failed ({status})")))
        }
    }
    .await;
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Unpacks an export into user `id`'s directory, whose collection must be empty, and opens the
/// collection, which checks and migrates it.
pub async fn import_archive(host: &Host, id: &str, archive: &Path) -> Result<(), Error> {
    let paths = host.paths(id);
    // Refused if a member would land outside the directory, before anything changes
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .map_err(|e| io(archive, e))?;
    let names = String::from_utf8_lossy(&listing.stdout);
    if !listing.status.success()
        || names
            .lines()
            .any(|name| name.starts_with('/') || name.split('/').any(|part| part == ".."))
    {
        return Err(invalid(format!(
            "{} isn't an export of a collection",
            archive.display()
        )));
    }
    if !names
        .lines()
        .any(|name| name.trim_start_matches("./") == spares_core::collection::DATABASE_FILE_NAME)
    {
        return Err(invalid(format!(
            "{} has no {}",
            archive.display(),
            spares_core::collection::DATABASE_FILE_NAME
        )));
    }
    super::import::ensure_empty_collection(host, id, &paths).await?;
    run_tar(
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(&paths.root),
    )?;
    if !paths.database().is_file() {
        return Err(invalid(format!(
            "{} has no {}",
            archive.display(),
            spares_core::collection::DATABASE_FILE_NAME
        )));
    }
    host.close(id).await;
    host.collection(id).await?;
    host.close(id).await;
    Ok(())
}

/// Exports every user's directory into `dir`, and copies `accounts.sqlite` there. Returns what it
/// wrote.
pub async fn backup(host: &Host, dir: &Path) -> Result<Vec<PathBuf>, Error> {
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let mut written = Vec::new();
    let accounts = dir.join(super::accounts::ACCOUNTS_FILE_NAME);
    snapshot(
        &host.root.join(super::accounts::ACCOUNTS_FILE_NAME),
        &accounts,
    )
    .await?;
    written.push(accounts);
    for user in super::accounts::users(&host.accounts).await? {
        if !host.paths(&user.id).database().is_file() {
            continue;
        }
        let out = dir.join(format!("{}.tar.gz", user.id));
        export(host, &user.id, &out).await?;
        written.push(out);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::accounts::NewUser;
    use crate::host::tests::test_host;

    /// An export moves a collection to another host's account.
    #[tokio::test]
    async fn an_export_moves_a_collection() {
        let (from, _from_root) = test_host().await;
        let collection = from.collection("me").await.unwrap();
        sqlx::query("INSERT INTO app_state (key, value) VALUES ('moved', 'yes')")
            .execute(&collection.db)
            .await
            .unwrap();
        std::fs::create_dir_all(collection.paths.media.join("images")).unwrap();
        std::fs::write(collection.paths.media.join("images/a.png"), "png").unwrap();
        std::fs::write(collection.paths.files.join("a.pdf"), "pdf").unwrap();
        std::fs::write(collection.paths.render.join("scratch"), "x").unwrap();
        drop(collection);
        let out = std::env::temp_dir().join(format!("spares-export-{}.tar.gz", random_string(6)));
        export(&from, "me", &out).await.unwrap();

        let (to, to_root) = test_host().await;
        to.create_user(NewUser {
            id: Some("newcomer"),
            name: "newcomer",
            email: None,
            password: "password1",
            admin: false,
            email_verified: false,
        })
        .await
        .unwrap();
        import_archive(&to, "newcomer", &out).await.unwrap();
        let moved = to.collection("newcomer").await.unwrap();
        let value: String = sqlx::query_scalar("SELECT value FROM app_state WHERE key = 'moved'")
            .fetch_one(&moved.db)
            .await
            .unwrap();
        assert_eq!(value, "yes");
        assert!(moved.paths.media.join("images/a.png").is_file());
        // Its cache isn't moved.
        assert!(!moved.paths.files.join("a.pdf").exists());
        assert!(!moved.paths.render.join("scratch").exists());
        // Not into a collection with notes, nor from an archive that isn't one
        assert!(
            import_archive(&to, "me", Path::new("/nonexistent.tar.gz"))
                .await
                .is_err()
        );

        let backups = to_root.0.join("backups");
        let written = backup(&to, &backups).await.unwrap();
        assert_eq!(written.len(), 3, "{written:?}");
        drop(moved);
        to.close_all().await;
        from.close_all().await;
        std::fs::remove_file(out).unwrap();
    }
}
