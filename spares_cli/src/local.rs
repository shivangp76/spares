//! The host this CLI talks to, and the collection on this machine that it works on directly.
//!
//! Most commands only send requests to the host (see [`spares_core::config::host_client`]). Some
//! work on the collection's files on this machine too: opening rendered files, live notes, the
//! notes repository, `spares sync cloud` and `spares deploy`. They use the
//! collection in the host's root on this machine (`SPARES_ROOT`, else
//! `~/.local/share/spares`): the account `SPARES_USER` names, else the one the CLI is logged in
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

use crate::login::Credentials;
use crate::login::normalize_host;

/// The address of the host on this machine, from its `host.toml`: the default address when there
/// is no host here, unless `SPARES_ROOT` names one. Never another host's when this one's settings
/// can't be read.
pub(crate) fn local_host(root: &Path) -> Result<String, String> {
    let has_settings = root
        .join(spares_server::host::config::HOST_CONFIG_FILE_NAME)
        .is_file();
    if !has_settings && std::env::var_os(spares_server::host::ROOT_ENV).is_some() {
        return Err(format!(
            "SPARES_ROOT is {}, which is no spares host (it has no host.toml). Create one with \
             `spares host init`.",
            root.display()
        ));
    }
    let config = HostConfig::read(root).map_err(|e| e.to_string())?;
    Ok(format!(
        "http://{}",
        config.bind.replace("0.0.0.0", "127.0.0.1")
    ))
}

/// The host to talk to and its token (see [`crate::login`]): `--host`, else the one last logged in
/// to, else the host on this machine.
pub(crate) fn host_client(
    host: Option<String>,
    root: &Path,
    credentials: &Credentials,
) -> Result<HostClient, String> {
    let base_url = match host
        .map(|host| normalize_host(&host))
        .or_else(|| credentials.default.clone())
    {
        Some(url) => url,
        None => local_host(root)?,
    };
    let token = std::env::var("SPARES_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .or_else(|| credentials.hosts.get(&base_url).map(|c| c.token.clone()));
    Ok(HostClient { base_url, token })
}

/// The collection on this machine (see the [module docs](self)), without migrating it or touching
/// its files: the host may be serving it.
pub(crate) async fn collection(
    root: &Path,
    credentials: &Credentials,
) -> Result<Arc<Collection>, String> {
    let host = Host::open(root).await.map_err(|e| e.to_string())?;
    let users = accounts::users(&host.accounts)
        .await
        .map_err(|e| e.to_string())?;
    // The account the CLI is logged in as on this machine's host
    let logged_in = local_host(root)
        .ok()
        .and_then(|host| credentials.hosts.get(&host))
        .map(|c| c.user.clone());
    let wanted = std::env::var("SPARES_USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or(logged_in);
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

/// Where the collection on this machine is, for commands that work on it directly
#[derive(Clone, Debug)]
pub(crate) struct LocalEnv {
    /// `sqlite://` URL of the collection's database
    pub(crate) database_url: String,
}

impl LocalEnv {
    pub(crate) fn new(collection: &Collection) -> Self {
        Self {
            database_url: collection.paths.database_url(),
        }
    }
}

/// The collection on this machine's host: the host's root, and the id and name of its account.
pub(crate) async fn account() -> Result<(std::path::PathBuf, String, String), miette::Error> {
    let collection = spares_core::collection::current().ok_or_else(|| {
        miette::miette!("No spares host on this machine. Create one with `spares host init`.")
    })?;
    let root = spares_server::host::default_root();
    let host = Host::open(&root)
        .await
        .map_err(|e| miette::miette!("{e}"))?;
    let user = accounts::user(&host.accounts, &collection.id)
        .await
        .map_err(|e| miette::miette!("{e}"))?
        .ok_or_else(|| miette::miette!("The host has no account {}", collection.id))?;
    host.accounts.close().await;
    Ok((root, user.id, user.name))
}

/// Turns the panic of code that needs the collection on this machine, when it couldn't be opened,
/// into an error that says why.
pub(crate) fn explain_missing_collection(reason: String) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or_default();
        if message.starts_with("no collection is current") {
            eprintln!(
                "Error: this command works on the collection of the host on this machine, which \
                 couldn't be opened: {reason}"
            );
            std::process::exit(1);
        }
        default(info);
    }));
}
