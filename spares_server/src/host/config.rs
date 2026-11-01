//! `host.toml`: the settings of a host, in its root directory.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use spares_core::Error;
use spares_core::LibraryError;

/// The file name of a host's settings, in its root.
pub const HOST_CONFIG_FILE_NAME: &str = "host.toml";

/// `host.toml`. Every setting has a default, so an empty file is a host on `127.0.0.1:8080` with
/// signups closed: the laptop's.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    /// The address the host listens on.
    pub bind: String,
    /// The web app's built files (`spares_frontend/frontend/dist`), served at `/`. Without it, only
    /// the API is served.
    pub frontend_dir: Option<PathBuf>,
    /// The address browsers reach the host at, such as `https://spares.example.com`, if not
    /// `bind`'s. The web app is only allowed from this origin (CORS), and session cookies are
    /// `Secure` when it is `https`.
    pub public_url: Option<String>,
    /// Who can create an account (see [`Signups`]).
    pub signups: Signups,
    /// How many collections are kept open at once. The least recently used one is closed to open
    /// another, once nothing is using it.
    pub max_open_collections: usize,
    /// A collection nothing used for this many minutes is closed.
    pub idle_minutes: u64,
    /// Connections of each open collection's database pool.
    pub max_connections_per_collection: u32,
    /// How long a web app's session lasts without being used, in days. Each use extends it.
    pub session_days: u64,
    /// How notes are rendered (see [`spares_core::render`]).
    pub render: spares_core::render::RenderConfig,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".to_string(),
            frontend_dir: None,
            public_url: None,
            signups: Signups::Closed,
            max_open_collections: 64,
            idle_minutes: 30,
            max_connections_per_collection: 8,
            session_days: 365,
            render: spares_core::render::RenderConfig::default(),
        }
    }
}

/// Who can create an account on a host. The admin can always add one (`spares host add-user`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Signups {
    /// No one: the laptop and a personal VPS, which is what keeps them to one user.
    #[default]
    Closed,
    /// Anyone with an invite from an admin (`spares host invite`).
    Invite,
    /// Anyone, once they verify their email address.
    Open,
}

impl HostConfig {
    /// Reads `host.toml` in `root`, or the defaults if there is none.
    pub fn read(root: &Path) -> Result<Self, Error> {
        let path = root.join(HOST_CONFIG_FILE_NAME);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(Error::Io {
                    description: format!("Failed to read {}", path.display()),
                    source: e,
                });
            }
        };
        Self::parse(&contents).map_err(|e| {
            Error::Library(LibraryError::InvalidConfig(format!(
                "{}: {e}",
                path.display()
            )))
        })
    }

    pub fn parse(contents: &str) -> Result<Self, String> {
        let config: Self = toml_edit::de::from_str(contents).map_err(|e| e.to_string())?;
        if config.max_open_collections == 0 {
            return Err("`max_open_collections` must be at least 1".to_string());
        }
        if let Some(url) = &config.public_url
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            return Err(format!(
                "`public_url` must start with http:// or https://, not {url}"
            ));
        }
        Ok(config)
    }

    /// The origin the web app is served from, which alone may call the API from a browser.
    pub fn web_origin(&self) -> String {
        match &self.public_url {
            Some(url) => url.trim_end_matches('/').to_string(),
            None => format!("http://{}", self.bind),
        }
    }

    /// Whether session cookies are `Secure`.
    pub fn secure_cookies(&self) -> bool {
        self.web_origin().starts_with("https://")
    }

    /// The commented `host.toml` that `spares host init` writes.
    pub fn template(bind: &str) -> String {
        format!(
            r#"# The settings of this spares host. See `HostConfig` in spares_server's docs.
bind = "{bind}"
# frontend_dir = "/path/to/spares_frontend/frontend/dist"
# public_url = "https://spares.example.com"
# closed, invite or open
signups = "closed"
# max_open_collections = 64
# idle_minutes = 30

[render]
# Directories outside users' collections that every render may read, such as preambles that this
# machine's templates import. Leave it empty on a host whose users don't trust each other.
# read_dirs = ["/Users/me/learning/preambles"]
"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_config_parses_with_defaults() {
        let config = HostConfig::parse("").unwrap();
        assert_eq!(config.bind, "127.0.0.1:8080");
        assert_eq!(config.signups, Signups::Closed);
        assert_eq!(config.web_origin(), "http://127.0.0.1:8080");
        assert!(!config.secure_cookies());
        let config =
            HostConfig::parse("public_url = \"https://spares.example.com/\"\nsignups = \"open\"")
                .unwrap();
        assert_eq!(config.web_origin(), "https://spares.example.com");
        assert!(config.secure_cookies());
        assert_eq!(config.signups, Signups::Open);
        assert!(HostConfig::parse("max_open_collections = 0").is_err());
        assert!(HostConfig::parse("nope = 1").is_err());
        let template = HostConfig::parse(&HostConfig::template("0.0.0.0:9000")).unwrap();
        assert_eq!(template.bind, "0.0.0.0:9000");
    }
}
