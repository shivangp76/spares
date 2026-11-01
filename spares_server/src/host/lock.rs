//! Keeps the CLI's admin commands that change collections from running while the host serves
//! them: a running host holds `host.lock` in its root.

use std::fs::File;
use std::path::Path;

use spares_core::Error;

use super::accounts::invalid;

pub const LOCK_FILE_NAME: &str = "host.lock";

/// Held while it lives.
#[derive(Debug)]
pub struct HostLock(#[allow(dead_code, reason = "held for its lock")] File);

impl HostLock {
    /// Takes the host's lock, or says that the host is running.
    pub fn take(root: &Path) -> Result<Self, Error> {
        let path = root.join(LOCK_FILE_NAME);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| Error::Io {
                description: format!("Failed to open {}", path.display()),
                source: e,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self(file)),
            Err(std::fs::TryLockError::WouldBlock) => Err(invalid(format!(
                "the host in {} is running. Stop it first.",
                root.display()
            ))),
            Err(std::fs::TryLockError::Error(e)) => Err(Error::Io {
                description: format!("Failed to lock {}", path.display()),
                source: e,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let root = std::env::temp_dir().join(format!(
            "spares-lock-{}",
            super::super::accounts::random_string(8)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let lock = HostLock::take(&root).unwrap();
        assert!(HostLock::take(&root).is_err());
        drop(lock);
        HostLock::take(&root).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
