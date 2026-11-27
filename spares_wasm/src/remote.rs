//! The server, as a browser's copy sees it: [`HttpRemote`] over the browser's `fetch`, with a full
//! sync's snapshot kept in the browser's storage rather than a file.

use std::path::Path;

use async_trait::async_trait;
use rsqlite_vfs::transfer::DbTransfer;
use spares_core::Error;
use spares_core::sync::http::HttpRemote;
use spares_core::sync::protocol::FullUploadResponse;
use spares_core::sync::protocol::MetaResponse;
use spares_core::sync::protocol::PullRequest;
use spares_core::sync::protocol::PullResponse;
use spares_core::sync::protocol::PushRequest;
use spares_core::sync::protocol::PushResponse;
use spares_core::sync::run::SyncRemote;

use crate::storage::opened;
use crate::storage::remove_file;

pub(crate) struct DeviceRemote(pub(crate) HttpRemote);

fn storage_error(e: impl std::fmt::Debug) -> Error {
    Error::Sync(format!("browser storage: {e:?}"))
}

/// The name of `path` in the VFS, which keeps files by name
fn file_name(path: &Path) -> Result<&str, Error> {
    path.to_str()
        .ok_or_else(|| storage_error(format!("{} is not UTF-8", path.display())))
}

#[async_trait(?Send)]
impl SyncRemote for DeviceRemote {
    async fn meta(&self, replica: u8) -> Result<MetaResponse, Error> {
        self.0.meta(replica).await
    }

    async fn pull(&self, request: PullRequest) -> Result<PullResponse, Error> {
        self.0.pull(request).await
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, Error> {
        self.0.push(request).await
    }

    /// Imports the server's database into the VFS under `path`, where `full::restore` attaches it.
    async fn download(&self, path: &Path) -> Result<(), Error> {
        import(path, &self.0.download_bytes().await?)
    }

    /// Uploads the snapshot `full::snapshot` wrote to the VFS under `path`, and removes it.
    async fn upload(&self, path: &Path, replica: u8) -> Result<FullUploadResponse, Error> {
        let vfs = opened().map_err(storage_error)?.vfs;
        let name = file_name(path)?;
        let bytes = vfs.export_db(name).map_err(storage_error)?;
        remove_file(&vfs, name).map_err(storage_error)?;
        self.0.upload_bytes(bytes, replica).await
    }
}

/// Imports `bytes`, a collection's database, into the VFS under `path`, where `full::restore`
/// attaches it.
fn import(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let vfs = opened().map_err(storage_error)?.vfs;
    let name = file_name(path)?;
    remove_file(&vfs, name).map_err(storage_error)?;
    vfs.import_db(name, bytes).map_err(storage_error)
}

/// A database the web app already has, for `spares_core::sync::run::load_snapshot`: it only
/// downloads, since there is no server behind it.
pub(crate) struct SnapshotRemote(pub(crate) Vec<u8>);

fn no_server() -> Error {
    Error::Sync("this copy was loaded from a snapshot and has no server to sync with".to_string())
}

#[async_trait(?Send)]
impl SyncRemote for SnapshotRemote {
    async fn meta(&self, _replica: u8) -> Result<MetaResponse, Error> {
        Err(no_server())
    }

    async fn pull(&self, _request: PullRequest) -> Result<PullResponse, Error> {
        Err(no_server())
    }

    async fn push(&self, _request: PushRequest) -> Result<PushResponse, Error> {
        Err(no_server())
    }

    async fn download(&self, path: &Path) -> Result<(), Error> {
        import(path, &self.0)
    }

    async fn upload(&self, _path: &Path, _replica: u8) -> Result<FullUploadResponse, Error> {
        Err(no_server())
    }
}
