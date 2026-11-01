//! `spares sync login` and `spares sync logout`: giving the collection's copy on the host the CLI
//! uses an upstream, which it then syncs with as one of its user's devices (see
//! `spares_core::sync::devices`).

use std::io::BufRead;

use reqwest::Client;
use serde_json::Value;
use serde_json::json;
use spares_core::sync::devices::EnrollRequest;
use spares_core::sync::http::HttpRemote;
use spares_core::sync::http::RemoteAuth;

use crate::login::normalize_host;

#[derive(clap::Args, Debug, Clone)]
pub(crate) struct UpstreamLoginArgs {
    /// The upstream, such as `https://spares.example.com`
    pub(crate) url: String,
    /// The account's name there (asked for if not given)
    #[arg(long)]
    pub(crate) user: Option<String>,
    /// Read the password from stdin instead of asking for it
    #[arg(long, conflicts_with = "token")]
    pub(crate) password_stdin: bool,
    /// The password, given by `spares deploy`
    #[arg(skip)]
    pub(crate) password: Option<String>,
    /// A `sync` device token for the account there (`spares host token <user> --scope sync`)
    /// instead of a password
    #[arg(long)]
    pub(crate) token: Option<String>,
}

async fn answer(response: reqwest::Result<reqwest::Response>, what: &str) -> Result<Value, String> {
    let response = response.map_err(|e| format!("Cannot reach {what}: {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!(
            "{what} refused: {}",
            body["message"]
                .as_str()
                .map_or_else(|| status.to_string(), str::to_string)
        ))
    }
}

fn host_name() -> String {
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "a host".to_string())
}

/// Gets a sync token at the upstream, enrolls this host's copy there (keeping its replica number
/// if it isn't the root's), and tells the host.
pub(crate) async fn login(
    base_url: &str,
    client: &Client,
    args: UpstreamLoginArgs,
) -> Result<(), String> {
    let url = normalize_host(&args.url);
    let status = answer(
        client
            .get(format!("{base_url}/api/sync/status"))
            .send()
            .await,
        "this host",
    )
    .await?;
    // Logging in again to the same upstream, or to the one a collection from before hosts names
    // without a token, replaces the token.
    if let Some(current) = status["sync_url"].as_str()
        && normalize_host(current) != url
        && status["sync_token_set"].as_bool() == Some(true)
    {
        return Err(format!(
            "This collection syncs with {current} already. `spares sync logout` first."
        ));
    }
    let name = format!("{} (host)", host_name());
    let token = if let Some(token) = args.token {
        token
    } else {
        {
            let user = match args.user {
                Some(user) => user,
                None => inquire::Text::new(&format!("Name on {url}:"))
                    .prompt()
                    .map_err(|e| e.to_string())?,
            };
            let password = if let Some(password) = args.password {
                password
            } else if args.password_stdin {
                let mut line = String::new();
                std::io::stdin()
                    .lock()
                    .read_line(&mut line)
                    .map_err(|e| e.to_string())?;
                line.trim_end_matches(['\n', '\r']).to_string()
            } else {
                inquire::Password::new("Password:")
                    .without_confirmation()
                    .prompt()
                    .map_err(|e| e.to_string())?
            };
            let body = answer(
                Client::new()
                    .post(format!("{url}/api/auth/token"))
                    .json(&json!({ "user": user, "password": password, "name": name,
                        "scope": "sync" }))
                    .send()
                    .await,
                &url,
            )
            .await?;
            body["token"]
                .as_str()
                .ok_or("no token in the answer")?
                .to_string()
        }
    };
    // A copy with a number of its own (the laptop's `0`, imported) keeps it; the root's `1` is
    // the upstream's.
    let current = status["replica"]
        .as_u64()
        .and_then(|r| u8::try_from(r).ok());
    let claim = current.filter(|r| *r != spares_core::config::Replica::ROOT.number());
    let remote =
        HttpRemote::new(&url, RemoteAuth::Token(token.clone())).map_err(|e| e.to_string())?;
    let enrolled = remote
        .enroll_with(&EnrollRequest {
            name,
            host: true,
            replica: claim,
        })
        .await
        .map_err(|e| e.to_string())?;
    answer(
        client
            .put(format!("{base_url}/api/sync/upstream"))
            .json(&json!({ "url": url, "token": token, "replica": enrolled.replica }))
            .send()
            .await,
        "this host",
    )
    .await?;
    println!(
        "This host's copy of the collection is now replica {} of {url}. Sync with \
         `spares sync cloud`; the first sync asks which copy to keep.",
        enrolled.replica
    );
    Ok(())
}

/// Frees this host's number at the upstream and stops syncing with it. The copy here becomes the
/// root of its own chain.
pub(crate) async fn logout(base_url: &str, client: &Client) -> Result<(), String> {
    let config = spares_core::config::read_external_config().map_err(|e| e.to_string())?;
    let (Some(url), Some(replica)) = (config.sync.url, config.sync.replica) else {
        return Err("This collection has no upstream.".to_string());
    };
    let remote =
        HttpRemote::new(&url, RemoteAuth::token(config.sync.token)).map_err(|e| e.to_string())?;
    if let Err(e) = remote.unenroll(replica).await {
        eprintln!("Couldn't free replica {replica} at {url} ({e}); remove it there.");
    }
    answer(
        client
            .delete(format!("{base_url}/api/sync/upstream"))
            .send()
            .await,
        "this host",
    )
    .await?;
    println!("This host's copy no longer syncs with {url}.");
    Ok(())
}

/// [`login`] as `spares deploy` does: with `password` if it has it, else a prompt.
pub(crate) async fn login_with(
    base_url: &str,
    client: &Client,
    url: &str,
    user: &str,
    password: Option<String>,
) -> Result<(), String> {
    login(
        base_url,
        client,
        UpstreamLoginArgs {
            url: url.to_string(),
            user: Some(user.to_string()),
            password_stdin: false,
            password,
            token: None,
        },
    )
    .await
}
