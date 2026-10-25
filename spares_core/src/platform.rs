//! The files `spares_core` reads and writes outside the database: `config.toml` and the parser
//! templates, under [`get_config_dir`](crate::config::get_config_dir).
//!
//! Natively they are the file system, and these functions are `std::fs`'s.
//!
//! In a browser there is no file system. There, the files are an in-memory directory:
//!
//! - The device fills it when it opens its copy of the collection ([`load`]), from what it saved in
//!   the browser's storage (OPFS).
//! - The device gets the server's `config.toml` and parser templates at each sync, writes them here
//!   ([`write`]), and saves them again ([`files`]). Settings are changed on the laptop, so on the
//!   device these files are read-only in practice.
//!
//! A browser never writes note or card files, nor renders them: the server does that after the
//! device's changes reach it (see [`crate::sync`]). So only the code that reads config and
//! templates goes through this module. Everything else uses `std::fs`, which fails in a browser.

#[cfg(not(target_family = "wasm"))]
pub use std::fs::create_dir_all;
#[cfg(not(target_family = "wasm"))]
pub use std::fs::read_to_string;
#[cfg(not(target_family = "wasm"))]
pub use std::fs::write;
#[cfg(not(target_family = "wasm"))]
use std::path::Path;

#[cfg(target_family = "wasm")]
pub use memory::create_dir_all;
#[cfg(target_family = "wasm")]
pub use memory::files;
#[cfg(target_family = "wasm")]
pub use memory::is_file;
#[cfg(target_family = "wasm")]
pub use memory::load;
#[cfg(target_family = "wasm")]
pub use memory::read_to_string;
#[cfg(target_family = "wasm")]
pub use memory::write;

/// Whether `path` is a file.
#[cfg(not(target_family = "wasm"))]
pub fn is_file(path: &Path) -> bool {
    path.is_file()
}

/// The in-memory directory of a browser. It's compiled natively under tests, to test it.
#[cfg(any(test, target_family = "wasm"))]
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
mod memory {
    use std::collections::BTreeMap;
    use std::io;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::MutexGuard;

    /// Contents by absolute path
    static FILES: Mutex<BTreeMap<PathBuf, Vec<u8>>> = Mutex::new(BTreeMap::new());

    fn files_lock() -> MutexGuard<'static, BTreeMap<PathBuf, Vec<u8>>> {
        // A panic while holding the lock leaves the map whole: each operation is one map call.
        FILES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replaces every file with `files`.
    pub fn load(files: BTreeMap<PathBuf, Vec<u8>>) {
        *files_lock() = files;
    }

    /// Every file, to save.
    pub fn files() -> BTreeMap<PathBuf, Vec<u8>> {
        files_lock().clone()
    }

    pub fn is_file(path: &Path) -> bool {
        files_lock().contains_key(path)
    }

    pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
        let path = path.as_ref();
        let contents = files_lock().get(path).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} doesn't exist", path.display()),
            )
        })?;
        String::from_utf8(contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    #[allow(clippy::unnecessary_wraps)] // As `std::fs::write`
    pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
        files_lock().insert(path.as_ref().to_path_buf(), contents.as_ref().to_vec());
        Ok(())
    }

    /// Directories aren't kept: a file's path is all there is.
    #[allow(clippy::unnecessary_wraps)] // As `std::fs::create_dir_all`
    pub fn create_dir_all(_path: impl AsRef<Path>) -> io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The only test of `FILES`, which is global.
        #[test]
        fn writes_reads_saves_and_loads() {
            let path = Path::new("/spares/config/typst/templates/card_template.typ");
            load(BTreeMap::new());
            assert!(!is_file(path));
            assert_eq!(
                read_to_string(path).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );

            create_dir_all(path.parent().unwrap()).unwrap();
            write(path, "#let card = 1").unwrap();
            assert!(is_file(path));
            assert!(!is_file(path.parent().unwrap()));
            assert_eq!(read_to_string(path).unwrap(), "#let card = 1");

            let saved = files();
            load(BTreeMap::new());
            assert!(!is_file(path));
            load(saved);
            assert_eq!(read_to_string(path).unwrap(), "#let card = 1");

            write(path, [0xff]).unwrap();
            assert_eq!(
                read_to_string(path).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            load(BTreeMap::new());
        }
    }
}
