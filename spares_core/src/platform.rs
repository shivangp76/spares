//! The files `spares_core` reads and writes outside the database: `config.toml` and the parser
//! templates, under [`get_config_dir`](crate::config::get_config_dir), and the images, audio and
//! image occlusions notes name.
//!
//! Natively they are the file system, and these functions are `std::fs`'s.
//!
//! In a browser there is no file system. There, the files are an in-memory directory:
//!
//! - The device fills it when it opens its copy of the collection, from what it saved in the
//!   browser's storage (OPFS).
//! - The device gets the server's `config.toml` and parser templates at each sync, and replaces
//!   them here (`load_dir`, wasm only). Settings are changed on the laptop, so on the device these
//!   files are read-only in practice.
//! - Images, audio and image occlusions are named by the path they have on the server, in its
//!   directories (`set_media_dirs`, wasm only), since notes name them by that path. The device
//!   keeps the image occlusions on the server, which the parser reads, and the files added on it
//!   until they reach the server (see [`crate::sync::media`]). What code writes here (`write`) the
//!   device saves after each request (`take_written`). Only the clozes SVGs, which the parser reads
//!   synchronously, stay in memory. The rest stay in the device's storage (`load_stored`), and
//!   [`read`] gets them from there (`set_loader`).
//!
//! A browser never writes note or card files, nor renders them: the server does that after the
//! device's changes reach it (see [`crate::sync`]). Its Typst cards have their image occlusions'
//! overlays in their sources, which need the size of the original image, and [`image_size`] has
//! it once the image has been in memory. Until then, and for other cards, it makes their images,
//! in memory, when the web app asks for them (see `parsers::read_device_card`, wasm only). The web app downloads them, and tells the device
//! which non-empty HTML files for web clients it has (`load_web_renders`, wasm only), which
//! [`has_web_render`] answers from. Apart from that, only the code that reads config and templates,
//! and that stores and parses those files, goes through this module. Everything else uses `std::fs`,
//! which fails in a browser.

#[cfg(not(target_family = "wasm"))]
pub use std::fs::create_dir_all;
#[cfg(not(target_family = "wasm"))]
pub use std::fs::read_to_string;
#[cfg(not(target_family = "wasm"))]
pub use std::fs::rename;
#[cfg(not(target_family = "wasm"))]
pub use std::fs::write;
#[cfg(not(target_family = "wasm"))]
use std::path::Path;

#[cfg(target_family = "wasm")]
pub use memory::Loader;
#[cfg(target_family = "wasm")]
pub use memory::MediaDirs;
#[cfg(target_family = "wasm")]
pub use memory::clear;
#[cfg(target_family = "wasm")]
pub use memory::create_dir_all;
#[cfg(target_family = "wasm")]
pub use memory::files;
#[cfg(target_family = "wasm")]
pub use memory::has_web_render;
#[cfg(target_family = "wasm")]
pub use memory::image_size;
#[cfg(target_family = "wasm")]
pub use memory::is_file;
#[cfg(target_family = "wasm")]
pub use memory::load_dir;
#[cfg(target_family = "wasm")]
pub use memory::load_files;
#[cfg(target_family = "wasm")]
pub use memory::load_stored;
#[cfg(target_family = "wasm")]
pub use memory::load_web_renders;
#[cfg(target_family = "wasm")]
pub use memory::media_dirs;
#[cfg(target_family = "wasm")]
pub use memory::read;
#[cfg(target_family = "wasm")]
pub use memory::read_to_string;
#[cfg(target_family = "wasm")]
pub use memory::rename;
#[cfg(target_family = "wasm")]
pub use memory::set_loader;
#[cfg(target_family = "wasm")]
pub use memory::set_media_dirs;
#[cfg(target_family = "wasm")]
pub use memory::take_written;
#[cfg(target_family = "wasm")]
pub use memory::version;
#[cfg(target_family = "wasm")]
pub use memory::write;

/// Whether `path` is a file.
#[cfg(not(target_family = "wasm"))]
pub fn is_file(path: &Path) -> bool {
    path.is_file()
}

/// The contents of `path`. Async since, in a browser, they may be in the device's storage.
#[cfg(not(target_family = "wasm"))]
#[allow(clippy::unused_async)] // As in a browser
pub async fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// The width and height of the image at `path`, from its header, if they can be had without
/// waiting. Natively they always can.
#[cfg(not(target_family = "wasm"))]
#[allow(clippy::unnecessary_wraps)] // As in a browser
pub fn image_size(path: &Path) -> Option<Result<(u32, u32), String>> {
    Some(image::image_dimensions(path).map_err(|e| e.to_string()))
}

/// Whether `path`, the HTML for web clients next to a rendered file, is there and not empty (see
/// [`crate::parsers::web_render_filepath`]).
#[cfg(not(target_family = "wasm"))]
pub fn has_web_render(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() > 0)
}

/// The in-memory directory of a browser. It's compiled natively under tests, to test it.
#[cfg(any(test, target_family = "wasm"))]
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
mod memory {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    use std::future::Future;
    use std::io;
    use std::path::Path;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::MutexGuard;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    /// Contents by absolute path
    static FILES: Mutex<BTreeMap<PathBuf, Vec<u8>>> = Mutex::new(BTreeMap::new());
    /// The files in the device's storage whose contents aren't in memory
    static STORED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    /// The files [`write`] changed since [`take_written`]
    static WRITTEN: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    /// The non-empty HTML files for web clients the web app has downloaded, by absolute path
    static WEB_RENDERS: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    static LOADER: Mutex<Option<Loader>> = Mutex::new(None);
    static MEDIA_DIRS: Mutex<Option<MediaDirs>> = Mutex::new(None);
    /// The width and height of each image whose contents have been in memory since they last
    /// changed, by absolute path, for [`image_size`]
    static SIZES: Mutex<BTreeMap<PathBuf, (u32, u32)>> = Mutex::new(BTreeMap::new());
    /// See [`version`]
    static VERSION: AtomicU64 = AtomicU64::new(0);

    /// A number that changes whenever the files in memory do, so that what is parsed from them,
    /// like `config.toml`, can be kept until then.
    pub fn version() -> u64 {
        VERSION.load(Ordering::Acquire)
    }

    fn changed() {
        VERSION.fetch_add(1, Ordering::AcqRel);
    }

    /// Reads a file in the device's storage, for [`read`].
    pub type Loader = fn(PathBuf) -> Pin<Box<dyn Future<Output = io::Result<Vec<u8>>> + Send>>;

    /// The server's directories for images, audio and image occlusions (see
    /// [`crate::sync::devices::DeviceConfig`]).
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct MediaDirs {
        pub image: PathBuf,
        pub audio: PathBuf,
        pub image_occlusion: PathBuf,
    }

    fn lock<T>(mutex: &'static Mutex<T>) -> MutexGuard<'static, T> {
        // A panic while holding a lock leaves its value whole: each operation is one call on it.
        // Locks are taken in the order of the statics above.
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn not_found(path: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} doesn't exist", path.display()),
        )
    }

    /// Records the size of the image at `path` from `contents`, its new contents, or forgets it if
    /// they aren't an image.
    fn remember_size(path: &Path, contents: &[u8]) {
        let size = image::ImageReader::new(io::Cursor::new(contents))
            .with_guessed_format()
            .ok()
            .and_then(|reader| reader.into_dimensions().ok());
        let mut sizes = lock(&SIZES);
        match size {
            Some(size) => sizes.insert(path.to_path_buf(), size),
            None => sizes.remove(path),
        };
    }

    /// Replaces the files under `dir` with `files`, which are under it too.
    pub fn load_dir(dir: &Path, files: BTreeMap<PathBuf, Vec<u8>>) {
        lock(&SIZES).retain(|path, _| !path.starts_with(dir));
        for (path, contents) in &files {
            remember_size(path, contents);
        }
        let mut all = lock(&FILES);
        all.retain(|path, _| !path.starts_with(dir));
        all.extend(files);
        changed();
    }

    /// Adds files the device keeps, with their contents.
    pub fn load_files(files: impl IntoIterator<Item = (PathBuf, Vec<u8>)>) {
        let mut all = lock(&FILES);
        let mut stored = lock(&STORED);
        for (path, contents) in files {
            remember_size(&path, &contents);
            stored.remove(&path);
            all.insert(path, contents);
        }
        changed();
    }

    /// Adds files the device keeps in its storage, and drops their contents from memory. [`read`]
    /// gets them from the device.
    pub fn load_stored(paths: impl IntoIterator<Item = PathBuf>) {
        let mut all = lock(&FILES);
        let mut stored = lock(&STORED);
        for path in paths {
            all.remove(&path);
            stored.insert(path);
        }
        changed();
    }

    /// Forgets every file, but not the downloaded HTML files for web clients.
    pub fn clear() {
        lock(&FILES).clear();
        lock(&STORED).clear();
        lock(&WRITTEN).clear();
        lock(&SIZES).clear();
        changed();
    }

    /// Every file in memory.
    pub fn files() -> BTreeMap<PathBuf, Vec<u8>> {
        lock(&FILES).clone()
    }

    pub fn is_file(path: &Path) -> bool {
        lock(&FILES).contains_key(path) || lock(&STORED).contains(path)
    }

    /// Replaces the downloaded HTML files for web clients with `paths`.
    pub fn load_web_renders(paths: BTreeSet<PathBuf>) {
        *lock(&WEB_RENDERS) = paths;
    }

    pub fn has_web_render(path: &Path) -> bool {
        lock(&WEB_RENDERS).contains(path)
    }

    /// Only files in memory: the parser reads clozes SVGs with it, which stay there.
    pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
        let path = path.as_ref();
        let contents = lock(&FILES)
            .get(path)
            .cloned()
            .ok_or_else(|| not_found(path))?;
        String::from_utf8(contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// From memory, or else from the device's storage.
    pub async fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
        let path = path.as_ref();
        if let Some(contents) = lock(&FILES).get(path) {
            return Ok(contents.clone());
        }
        let loader = *lock(&LOADER);
        match loader {
            Some(loader) if lock(&STORED).contains(path) => {
                let contents = loader(path.to_path_buf()).await?;
                remember_size(path, &contents);
                Ok(contents)
            }
            _ => Err(not_found(path)),
        }
    }

    /// The width and height of the image at `path`, if its contents have been in memory since
    /// they last changed: written, loaded or read. The parser needs them synchronously, but the
    /// device's storage is read asynchronously, and keeping every image in memory, or reading
    /// them all when the device opens its copy, would cost more than it saves.
    pub fn image_size(path: &Path) -> Option<Result<(u32, u32), String>> {
        lock(&SIZES).get(path).copied().map(Ok)
    }

    /// Sets how [`read`] gets files from the device's storage.
    pub fn set_loader(loader: Loader) {
        *lock(&LOADER) = Some(loader);
    }

    #[allow(clippy::unnecessary_wraps)] // As `std::fs::write`
    pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
        let path = path.as_ref().to_path_buf();
        remember_size(&path, contents.as_ref());
        lock(&FILES).insert(path.clone(), contents.as_ref().to_vec());
        lock(&STORED).remove(&path);
        lock(&WRITTEN).insert(path);
        changed();
        Ok(())
    }

    /// Only files in memory.
    pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
        let from = from.as_ref();
        let contents = lock(&FILES).remove(from).ok_or_else(|| not_found(from))?;
        lock(&WRITTEN).remove(from);
        lock(&SIZES).remove(from);
        write(to, contents)
    }

    /// The files written since the last call, for the device to save.
    pub fn take_written() -> Vec<(PathBuf, Vec<u8>)> {
        let all = lock(&FILES);
        let written = std::mem::take(&mut *lock(&WRITTEN));
        written
            .into_iter()
            .filter_map(|path| Some((path.clone(), all.get(&path)?.clone())))
            .collect()
    }

    /// Directories aren't kept: a file's path is all there is.
    #[allow(clippy::unnecessary_wraps)] // As `std::fs::create_dir_all`
    pub fn create_dir_all(_path: impl AsRef<Path>) -> io::Result<()> {
        Ok(())
    }

    /// Sets the server's directories, which [`crate::api::image::get_image_directory`] and the
    /// like return from then on, or clears them with `None`.
    pub fn set_media_dirs(dirs: Option<MediaDirs>) {
        *lock(&MEDIA_DIRS) = dirs;
    }

    pub fn media_dirs() -> Option<MediaDirs> {
        lock(&MEDIA_DIRS).clone()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn load_from_storage(
            path: PathBuf,
        ) -> Pin<Box<dyn Future<Output = io::Result<Vec<u8>>> + Send>> {
            Box::pin(async move { Ok(path.to_string_lossy().into_owned().into_bytes()) })
        }

        /// A tiny PNG of `width` by `height` pixels.
        fn png(width: u32, height: u32) -> Vec<u8> {
            let mut png = Vec::new();
            image::RgbaImage::new(width, height)
                .write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            png
        }

        fn load_image_from_storage(
            _path: PathBuf,
        ) -> Pin<Box<dyn Future<Output = io::Result<Vec<u8>>> + Send>> {
            Box::pin(async { Ok(png(7, 6)) })
        }

        /// The only test of `FILES`, `STORED`, `WRITTEN`, `LOADER` and `SIZES`, which are global.
        #[tokio::test]
        async fn writes_reads_loads_and_stores() {
            let config = Path::new("/spares/config");
            let path = config.join("typst/templates/card_template.typ");
            clear();
            assert!(!is_file(&path));
            assert_eq!(
                read_to_string(&path).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );

            create_dir_all(path.parent().unwrap()).unwrap();
            write(&path, "#let card = 1").unwrap();
            assert!(is_file(&path));
            assert!(!is_file(path.parent().unwrap()));
            assert_eq!(read_to_string(&path).unwrap(), "#let card = 1");
            assert_eq!(read(&path).await.unwrap(), b"#let card = 1");
            assert_eq!(take_written(), [(path.clone(), b"#let card = 1".to_vec())]);
            assert_eq!(take_written(), []);

            // Replacing a directory leaves the others
            let image = Path::new("/srv/spares/images/cell-0123456789ab.png");
            load_files([(image.to_path_buf(), vec![1])]);
            let other = config.join("config.toml");
            load_dir(config, BTreeMap::from([(other.clone(), b"x".to_vec())]));
            assert!(!is_file(&path));
            assert!(is_file(&other));
            assert!(is_file(image));
            assert_eq!(take_written(), []);

            // Stored files are read from the device
            load_stored([image.to_path_buf()]);
            assert!(is_file(image));
            assert_eq!(
                read(image).await.unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
            set_loader(load_from_storage);
            assert_eq!(
                read(image).await.unwrap(),
                image.to_str().unwrap().as_bytes()
            );
            assert!(read_to_string(image).is_err());
            assert!(
                read(Path::new("/srv/spares/images/other.png"))
                    .await
                    .is_err()
            );

            rename(&other, &path).unwrap();
            assert!(!is_file(&other));
            assert_eq!(read_to_string(&path).unwrap(), "x");
            assert_eq!(take_written(), [(path.clone(), b"x".to_vec())]);

            // An image's size is known once its contents have been in memory, and until they
            // change
            let original = Path::new("/srv/spares/image_occlusions/cell.png");
            assert_eq!(image_size(original), None);
            load_files([(original.to_path_buf(), png(4, 3))]);
            load_stored([original.to_path_buf()]);
            assert_eq!(image_size(original), Some(Ok((4, 3))));
            write(original, png(5, 2)).unwrap();
            assert_eq!(image_size(original), Some(Ok((5, 2))));
            write(original, "not an image").unwrap();
            assert_eq!(image_size(original), None);
            assert_eq!(image_size(&path), None);
            take_written();
            clear();
            set_loader(load_image_from_storage);
            load_stored([original.to_path_buf()]);
            assert_eq!(image_size(original), None);
            read(original).await.unwrap();
            assert_eq!(image_size(original), Some(Ok((7, 6))));
            write(&path, "x").unwrap();

            write(&path, [0xff]).unwrap();
            assert_eq!(
                read_to_string(&path).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            clear();
            assert!(!is_file(image));
            assert_eq!(image_size(original), None);
            assert_eq!(take_written(), []);
        }

        /// The only test of `WEB_RENDERS`, which is global.
        #[test]
        fn loads_web_renders() {
            let path = Path::new("/spares/cache/markdown/abc.html");
            assert!(!has_web_render(path));
            load_web_renders(BTreeSet::from([path.to_path_buf()]));
            assert!(has_web_render(path));
            assert!(!has_web_render(&path.with_extension("pdf")));
            load_web_renders(BTreeSet::new());
            assert!(!has_web_render(path));
        }
    }
}
