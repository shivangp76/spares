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
    /// Where users' caches are: their rendered files, raw card files, render hashes and sync's
    /// snapshots, everything spares recreates, in `<cache_dir>/users/<user_id>`. Kept out of the
    /// root, so that backing up the root (`~/.local/share`, say) leaves them out. By default
    /// `$XDG_CACHE_HOME/spares` (`~/.cache/spares`) for a host in the default root
    /// (`$XDG_DATA_HOME/spares`), and `cache` in the root for a host elsewhere, so a scratch host
    /// never shares one with another host (see [`super::cache_root`]).
    pub cache_dir: Option<PathBuf>,
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
    /// How many collections sync with their upstreams at once (see [`crate::auto_sync`]).
    pub sync_at_once: usize,
    /// How the host sends email: verifying addresses and resetting passwords (see
    /// [`super::signup`]).
    pub email: super::signup::EmailConfig,
    /// What each user may use, unless their account says otherwise (see [`super::limits`]).
    pub limits: super::limits::Limits,
    /// How notes are rendered (see [`spares_core::render`]).
    pub render: spares_core::render::RenderConfig,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".to_string(),
            cache_dir: None,
            frontend_dir: None,
            public_url: None,
            signups: Signups::Closed,
            max_open_collections: 64,
            idle_minutes: 30,
            max_connections_per_collection: 8,
            session_days: 365,
            sync_at_once: 4,
            email: super::signup::EmailConfig::default(),
            limits: super::limits::Limits::default(),
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
        if config.signups == Signups::Open && config.email.command.is_none() {
            return Err(
                "open signups need email to verify addresses: set `[email] command`".to_string(),
            );
        }
        if let Some(url) = &config.public_url
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            return Err(format!(
                "`public_url` must start with http:// or https://, not {url}"
            ));
        }
        let parsers: Vec<&str> = spares_core::parsers::get_all_parsers()
            .iter()
            .map(|parser| parser().get_parser_name())
            .collect();
        for (parser, dir) in &config.render.copy_notes_to {
            if !parsers.contains(&parser.as_str()) {
                return Err(format!(
                    "`render.copy_notes_to` names {parser}, which is not one of {}",
                    parsers.join(", ")
                ));
            }
            if !dir.is_absolute() {
                return Err(format!(
                    "`render.copy_notes_to.{parser}` must be an absolute path, not {}",
                    dir.display()
                ));
            }
        }
        if !config.render.copy_notes_to.is_empty() && config.signups != Signups::Closed {
            return Err(
                "`render.copy_notes_to` is for a host with one user, so signups must be closed"
                    .to_string(),
            );
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
# Users' caches, everything spares recreates. Default: ~/.cache/spares for a host in
# ~/.local/share/spares, else `cache` in this directory.
# cache_dir = "/path/to/cache"
# frontend_dir = "/path/to/spares_frontend/frontend/dist"
# public_url = "https://spares.example.com"
# closed, invite or open
signups = "closed"
# max_open_collections = 64
# idle_minutes = 30

[email]
# A command that sends the message on its standard input, e.g. "sendmail -t". Open signups and
# password resets need it.
# command = "sendmail -t"
# from = "spares@example.com"

[limits]
# Sized for one trusted user, as before hosts. Lower them before opening signups.
# sync_body_mb = 1024
# media_body_mb = 128
# storage_mb = 0            # 0: no quota
# requests_per_minute = 0   # 0: no limit
# search_seconds = 30
# renders_at_once = 0       # 0: [render]'s

[render]
# Directories outside users' collections that every render may read, such as preambles that this
# machine's templates import. Leave it empty on a host whose users don't trust each other.
# read_dirs = ["/Users/me/learning/preambles"]
# sandbox = "auto"          # auto (sandbox-exec on macOS, bubblewrap on Linux), sandbox-exec,
#                           # bubblewrap, or none
# engines = ["latex", "pandoc", "typst"]
# timeout_seconds = 60
# cpu_seconds = 120
# memory_mb = 2048          # Linux only
# renders_at_once = 4       # per user
# typst_in_process = true   # Typst renders in workers with Typst built in, not one `typst` per file
# Fonts for Typst, besides its own and (unless turned off) the system's
# typst_font_dirs = ["/Users/me/Library/Fonts"]
# typst_system_fonts = true
# Put each rendered note here too (a hard link), by parser, e.g. where an editor writes the PDFs
# it compiles. For a host with one user.
# [render.copy_notes_to]
# typst = "/Users/me/.cache/typst"
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
        let config = HostConfig::parse(
            "public_url = \"https://spares.example.com/\"\nsignups = \"open\"\n[email]\ncommand = \"sendmail -t\"",
        )
        .unwrap();
        assert_eq!(config.web_origin(), "https://spares.example.com");
        assert!(config.secure_cookies());
        assert_eq!(config.signups, Signups::Open);
        assert!(HostConfig::parse("max_open_collections = 0").is_err());
        assert!(
            HostConfig::parse("signups = \"open\"").is_err(),
            "open signups need email"
        );
        assert!(HostConfig::parse("nope = 1").is_err());
        let copies = HostConfig::parse("[render.copy_notes_to]\ntypst = \"/home/a/.cache/typst\"")
            .unwrap()
            .render
            .copy_notes_to;
        assert_eq!(copies["typst"], Path::new("/home/a/.cache/typst"));
        assert!(HostConfig::parse("[render.copy_notes_to]\ntypist = \"/a\"").is_err());
        assert!(HostConfig::parse("[render.copy_notes_to]\ntypst = \"a\"").is_err());
        assert!(
            HostConfig::parse("signups = \"invite\"\n[render.copy_notes_to]\ntypst = \"/a\"")
                .is_err()
        );
        let template = HostConfig::parse(&HostConfig::template("0.0.0.0:9000")).unwrap();
        assert_eq!(template.bind, "0.0.0.0:9000");
    }
}
