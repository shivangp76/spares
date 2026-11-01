//! `spares login` and `spares logout`: a device token per host, kept in `credentials.toml` in the
//! CLI's config directory (`$XDG_CONFIG_HOME/spares`, by default `~/.config/spares`), readable
//! only by its owner.
//!
//! ```toml
//! default = "http://127.0.0.1:8080"
//!
//! [hosts."http://127.0.0.1:8080"]
//! user = "shivang"          # the account's id
//! device = 3                # the token's id, which `spares logout` revokes
//! token = "spt_…"
//! ```
//!
//! The host the CLI talks to is `--host` (or `SPARES_HOST`), else `default`, which the last login
//! set, else the host on this machine. `SPARES_TOKEN` overrides the token, for scripts and tests.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::PathBuf;

use clap::Args;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Credentials {
    pub(crate) default: Option<String>,
    #[serde(default)]
    pub(crate) hosts: BTreeMap<String, HostCredentials>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct HostCredentials {
    pub(crate) user: String,
    pub(crate) device: Option<i64>,
    pub(crate) token: String,
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(
            || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
            PathBuf::from,
        )
        .join("spares")
}

pub(crate) fn credentials_file() -> PathBuf {
    config_dir().join("credentials.toml")
}

impl Credentials {
    pub(crate) fn read() -> Self {
        std::fs::read_to_string(credentials_file())
            .ok()
            .and_then(|contents| toml_edit::de::from_str(&contents).ok())
            .unwrap_or_default()
    }

    fn write(&self) -> Result<(), Error> {
        let path = credentials_file();
        std::fs::create_dir_all(config_dir()).into_diagnostic()?;
        let contents = toml_edit::ser::to_string_pretty(self).into_diagnostic()?;
        // Readable only by its owner, from the start
        let tmp = path.with_extension("toml.tmp");
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&tmp).into_diagnostic()?;
            std::io::Write::write_all(&mut file, contents.as_bytes()).into_diagnostic()?;
        }
        std::fs::rename(&tmp, &path).into_diagnostic()
    }
}

/// `https://example.com/` and `example.com` as `https://example.com`; `127.0.0.1:8080` as
/// `http://127.0.0.1:8080`.
pub(crate) fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else if host.starts_with("127.") || host.starts_with("localhost") || host.starts_with("[::1]")
    {
        format!("http://{host}")
    } else {
        format!("https://{host}")
    }
}

#[derive(Args, Debug)]
pub(crate) struct LoginArgs {
    /// The host, such as `https://spares.example.com` (default: the host on this machine)
    pub(crate) host: Option<String>,
    /// The account's name (asked for if not given)
    #[arg(long)]
    pub(crate) user: Option<String>,
    /// Read the password from stdin instead of asking for it
    #[arg(long, conflicts_with = "token")]
    pub(crate) password_stdin: bool,
    /// Save this device token (from `spares host init` or `spares host token`) instead of logging
    /// in with a password
    #[arg(long)]
    pub(crate) token: Option<String>,
    /// What this device is called in the account's list of devices
    #[arg(long)]
    pub(crate) name: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct LogoutArgs {
    /// The host (default: the one the CLI talks to)
    pub(crate) host: Option<String>,
}

#[derive(Deserialize)]
struct User {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct Device {
    id: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
    device: Device,
    user: User,
}

#[derive(Deserialize)]
struct MeResponse {
    user: User,
    device: Option<Device>,
}

async fn error_message(response: reqwest::Response) -> String {
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    body["message"]
        .as_str()
        .map_or_else(|| status.to_string(), str::to_string)
}

fn device_name() -> String {
    let machine = std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this machine".to_string());
    format!("spares CLI on {machine}")
}

pub(crate) async fn login(args: LoginArgs, local_host: &str) -> Result<(), Error> {
    let host = normalize_host(args.host.as_deref().unwrap_or(local_host));
    let client = reqwest::Client::new();
    let (token, user, device) = if let Some(token) = args.token {
        let token = if token == "-" {
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .into_diagnostic()?;
            line.trim().to_string()
        } else {
            token
        };
        let response = client
            .get(format!("{host}/api/me"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| miette!("Cannot reach {host}: {e}"))?;
        if !response.status().is_success() {
            return Err(miette!(
                "{host} refused the token: {}",
                error_message(response).await
            ));
        }
        let me: MeResponse = response.json().await.into_diagnostic()?;
        (token, me.user, me.device.map(|d| d.id))
    } else {
        let user = match args.user {
            Some(user) => user,
            None => inquire::Text::new("Name:").prompt().into_diagnostic()?,
        };
        let password = if args.password_stdin {
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .into_diagnostic()?;
            line.trim_end_matches(['\n', '\r']).to_string()
        } else {
            inquire::Password::new("Password:")
                .without_confirmation()
                .with_display_mode(inquire::PasswordDisplayMode::Masked)
                .prompt()
                .into_diagnostic()?
        };
        let response = client
            .post(format!("{host}/api/auth/token"))
            .json(&serde_json::json!({
                "user": user,
                "password": password,
                "name": args.name.unwrap_or_else(device_name),
            }))
            .send()
            .await
            .map_err(|e| miette!("Cannot reach {host}: {e}"))?;
        if !response.status().is_success() {
            return Err(miette!("{host} refused: {}", error_message(response).await));
        }
        let response: TokenResponse = response.json().await.into_diagnostic()?;
        (response.token, response.user, Some(response.device.id))
    };
    let mut credentials = Credentials::read();
    credentials.hosts.insert(
        host.clone(),
        HostCredentials {
            user: user.id.clone(),
            device,
            token,
        },
    );
    credentials.default = Some(host.clone());
    credentials.write()?;
    println!(
        "Logged in to {host} as {} ({}). The token is in {}.",
        user.name,
        user.id,
        credentials_file().display()
    );
    Ok(())
}

pub(crate) async fn logout(args: LogoutArgs, current_host: &str) -> Result<(), Error> {
    let host = normalize_host(args.host.as_deref().unwrap_or(current_host));
    let mut credentials = Credentials::read();
    let Some(saved) = credentials.hosts.remove(&host) else {
        return Err(miette!("Not logged in to {host}"));
    };
    if let Some(device) = saved.device {
        let revoked = reqwest::Client::new()
            .delete(format!("{host}/api/auth/devices/{device}"))
            .bearer_auth(&saved.token)
            .send()
            .await;
        if !revoked.is_ok_and(|r| r.status().is_success()) {
            eprintln!("Couldn't revoke the token on {host}; it is forgotten here only.");
        }
    }
    if credentials.default.as_deref() == Some(host.as_str()) {
        credentials.default = credentials.hosts.keys().next().cloned();
    }
    credentials.write()?;
    println!("Logged out of {host}.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_normalized() {
        assert_eq!(normalize_host("127.0.0.1:8080/"), "http://127.0.0.1:8080");
        assert_eq!(
            normalize_host("spares.example.com"),
            "https://spares.example.com"
        );
        assert_eq!(normalize_host("http://vps:8080"), "http://vps:8080");
    }
}
