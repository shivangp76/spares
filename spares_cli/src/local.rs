//! The host this CLI talks to, and the collection on this machine that it works on directly.
//!
//! Most commands only send requests to the host (see [`spares_core::config::host_client`]). Some
//! work on the collection's files on this machine too: opening rendered files, live notes, the
//! notes repository, `spares migrate ids`, `spares sync cloud` and `spares deploy`. They use the
//! collection in the host's root on this machine (`SPARES_ROOT`, else
//! `~/.local/share/spares-host`): the account `SPARES_USER` names, else the one the CLI is logged in
//! as there, else its only account.

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use spares_core::collection::Collection;
use spares_core::config::HostClient;
use spares_server::host::Host;
use spares_server::host::HostConfig;
use spares_server::host::accounts;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

/// The host to talk to: `--host`, else the host on this machine, at its `host.toml`'s address.
pub(crate) fn host_client(host: Option<String>, root: &Path) -> HostClient {
    let base_url = host.unwrap_or_else(|| {
        let bind = HostConfig::read(root).map_or_else(|_| HostConfig::default().bind, |c| c.bind);
        format!("http://{}", bind.replace("0.0.0.0", "127.0.0.1"))
    });
    HostClient {
        base_url: base_url.trim_end_matches('/').to_string(),
        token: std::env::var("SPARES_API_KEY")
            .ok()
            .filter(|key| !key.is_empty()),
    }
}

/// The collection on this machine (see the [module docs](self)), without migrating it or touching
/// its files: the host may be serving it.
pub(crate) async fn collection(root: &Path) -> Result<Arc<Collection>, String> {
    let host = Host::open(root).await.map_err(|e| e.to_string())?;
    let users = accounts::users(&host.accounts)
        .await
        .map_err(|e| e.to_string())?;
    let wanted = std::env::var("SPARES_USER").ok().filter(|u| !u.is_empty());
    let user = match (&wanted, users.as_slice()) {
        (Some(who), _) => accounts::find_user(&host.accounts, who)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("the host in {} has no account {who}", root.display()))?,
        (None, [user]) => user.clone(),
        (None, _) => {
            return Err(format!(
                "the host in {} has {} accounts; say which with SPARES_USER",
                root.display(),
                users.len()
            ));
        }
    };
    host.accounts.close().await;
    let paths = host.paths(&user.id);
    let options = SqliteConnectOptions::from_str(&paths.database_url())
        .map_err(|e| e.to_string())?
        .with_regexp();
    let db = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_lazy_with(options);
    let collection = Arc::new(Collection::new(user.id, db, paths));
    let replica = {
        let _entered = collection.enter();
        spares_server::host::collection_replica().ok()
    };
    collection.set_replica(replica);
    Ok(collection)
}

/// Where the collection on this machine is, and the address of the host serving it, for commands
/// that use both.
#[derive(Clone, Debug)]
pub(crate) struct LocalEnv {
    /// `sqlite://` URL of the collection's database
    pub(crate) database_url: String,
    /// The host's address, such as `127.0.0.1:8080`
    pub(crate) socket_address: String,
}

impl LocalEnv {
    pub(crate) fn new(collection: &Collection, host: &HostClient) -> Self {
        Self {
            database_url: collection.paths.database_url(),
            socket_address: host
                .base_url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .to_string(),
        }
    }
}
