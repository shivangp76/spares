//! The admin's commands: `spares host …`, also `spares_server host …` on a machine with only the
//! server. Whoever can read the host's root is its admin, so these work on its files directly,
//! without logging in. The ones that change collections refuse to run while the host does (see
//! [`super::lock`]).

use std::io::BufRead;
use std::path::PathBuf;

use clap::Args;
use clap::Subcommand;
use spares_core::Error;
use spares_core::config::Replica;

use super::Host;
use super::accounts;
use super::accounts::NewUser;
use super::accounts::invalid;
use super::import::ImportOptions;
use super::lock::HostLock;

#[derive(Args, Debug)]
pub struct HostArgs {
    /// The host's root directory (default: `SPARES_ROOT`, else `~/.local/share/spares-host`)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
    #[command(subcommand)]
    pub command: HostCommand,
}

/// How a command gets a password: `--password-stdin`, else a prompt.
#[derive(Args, Debug, Clone)]
pub struct PasswordArgs {
    /// Read the password from the first line of stdin instead of prompting for it
    #[arg(long)]
    pub password_stdin: bool,
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    /// Create a host: its root, `host.toml`, and its first account, an admin
    Init {
        /// The account's name, which logs in
        #[arg(long)]
        user: String,
        /// The account's id, which names its directory. Give it the id of the same user's
        /// account on another host, so the directory can move between them.
        #[arg(long)]
        user_id: Option<String>,
        #[arg(long)]
        email: Option<String>,
        /// The address the host listens on
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: String,
        #[command(flatten)]
        password: PasswordArgs,
        /// Print only a device token for the account, for a test or a script to use
        #[arg(long)]
        print_token: bool,
    },
    /// Create a device token for an account, as `spares login` does, without its password
    Token {
        /// Its id, name or email
        user: String,
        /// What the device is called in the account's list of devices
        #[arg(long, default_value = "spares CLI")]
        name: String,
        /// `all`, or `sync` for another host that only syncs
        #[arg(long, default_value = "all")]
        scope: super::auth::Scope,
    },
    /// Revoke a device token
    Revoke {
        /// The account's id, name or email
        user: String,
        /// The device's id, from `spares host devices`
        device: i64,
    },
    /// List an account's devices
    Devices {
        /// Its id, name or email
        user: String,
    },
    /// Add an account, whatever the signup policy
    AddUser {
        name: String,
        #[arg(long)]
        user_id: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        admin: bool,
        #[command(flatten)]
        password: PasswordArgs,
    },
    /// List the accounts
    Users,
    /// Set an account's password
    ResetPassword {
        /// Its id, name or email
        user: String,
        #[command(flatten)]
        password: PasswordArgs,
    },
    /// Delete an account and its directory, collection and all
    DeleteUser {
        /// Its id, name or email
        user: String,
        /// Don't ask first
        #[arg(long)]
        yes: bool,
    },
    /// Move a collection into an account: another host's export, or one kept the way spares kept
    /// one before hosts
    ///
    /// An export (`spares host export`, or a user's download) is unpacked into `users/<id>/`.
    /// From before hosts, the data directory's database, notes repository, images, audio and
    /// image occlusions, the config directory's config.toml and templates, and the rendered files
    /// move into `users/<id>/`. See `spares_server::host::import` and `spares_server::host::export`.
    Import {
        /// An export (`.tar.gz`), or the old data directory (what `SPARES_DATA_DIR` was, by
        /// default ~/.local/share/spares)
        data_dir: PathBuf,
        /// The account to import into (its id, name or email). Its collection must be empty.
        user: String,
        /// The database, if not `spares-main.sqlite` in the data directory
        #[arg(long)]
        database: Option<PathBuf>,
        /// The old config directory (`SPARES_CONFIG_DIR`; default ~/.config/spares)
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// The old cache directory (`SPARES_CACHE_DIR`; default ~/.cache/spares)
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// A directory of rendered files to move into the files directory: `SPARES_FILES_DIR`,
        /// `MARKDOWN_OUT_DIR`, `LATEX_OUT_DIR`, `TYPST_OUT_DIR` (the default: those that are set)
        #[arg(long = "rendered-dir")]
        rendered_dirs: Vec<PathBuf>,
        /// Pandoc's user data directory, copied for rendering (default: ~/.local/share/pandoc)
        #[arg(long)]
        pandoc_dir: Option<PathBuf>,
        /// This host's replica number for the collection: what `SPARES_REPLICA` was (default:
        /// `SPARES_REPLICA`, else 1)
        #[arg(long)]
        replica: Option<Replica>,
        /// Copy rather than move, leaving the old directories as they were
        #[arg(long)]
        copy: bool,
    },
    /// Make an invite for `signups = "invite"`: a code that creates one account
    Invite {
        /// How many days it can be used for
        #[arg(long, default_value_t = 7)]
        days: i64,
    },
    /// Write an account's directory, collection and all, to a `.tar.gz` that another host
    /// imports (`spares host import <archive> <user>`)
    Export {
        /// Its id, name or email
        user: String,
        /// Where to write it [default: `<id>.tar.gz` here]
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Export every account into a directory, with a copy of accounts.sqlite. It works while the
    /// host runs.
    Backup { dir: PathBuf },
    /// Show an account's limits and usage, or change its limits (`-1` for the host's)
    Limits {
        /// Its id, name or email
        user: String,
        #[arg(long, allow_negative_numbers = true)]
        sync_body_mb: Option<i64>,
        #[arg(long, allow_negative_numbers = true)]
        media_body_mb: Option<i64>,
        /// `0` for no quota
        #[arg(long, allow_negative_numbers = true)]
        storage_mb: Option<i64>,
        /// `0` for no limit
        #[arg(long, allow_negative_numbers = true)]
        requests_per_minute: Option<i64>,
        #[arg(long, allow_negative_numbers = true)]
        search_seconds: Option<i64>,
        #[arg(long, allow_negative_numbers = true)]
        renders_at_once: Option<i64>,
    },
    /// Migrate collections to this version of spares, which opening one does too
    Migrate {
        /// Every account's collection
        #[arg(long, required = true)]
        all: bool,
    },
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn read_password(args: &PasswordArgs, prompt: &str) -> Result<String, Error> {
    if args.password_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| Error::Io {
                description: "Failed to read the password from stdin".to_string(),
                source: e,
            })?;
        return Ok(line.trim_end_matches(['\n', '\r']).to_string());
    }
    inquire::Password::new(prompt)
        .with_display_mode(inquire::PasswordDisplayMode::Masked)
        .prompt()
        .map_err(|e| invalid(e.to_string()))
}

async fn find(host: &Host, who: &str) -> Result<accounts::User, Error> {
    accounts::find_user(&host.accounts, who)
        .await?
        .ok_or_else(|| invalid(format!("no account is named {who}")))
}

/// Runs an admin command, printing what it did.
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub async fn run(args: HostArgs) -> Result<(), Error> {
    let root = args.root.unwrap_or_else(super::default_root);
    match args.command {
        HostCommand::Init {
            user,
            user_id,
            email,
            bind,
            password,
            print_token,
        } => {
            if root.join(accounts::ACCOUNTS_FILE_NAME).exists() {
                return Err(invalid(format!("{} is a host already", root.display())));
            }
            let password = read_password(&password, "Password for the new account:")?;
            let host = Host::create(&root, &bind).await?;
            let user = host
                .create_user(NewUser {
                    id: user_id.as_deref(),
                    name: &user,
                    email: email.as_deref(),
                    password: &password,
                    admin: true,
                    email_verified: email.is_some(),
                })
                .await?;
            let (_, token) = super::auth::create_token(
                &host.accounts,
                &user.id,
                "spares CLI",
                super::auth::Scope::All,
            )
            .await?;
            host.close_all().await;
            if print_token {
                println!("{token}");
                return Ok(());
            }
            println!(
                "Created the host {} with the account {} ({}).",
                root.display(),
                user.name,
                user.id
            );
            println!("Its settings are in {}.", root.join("host.toml").display());
            println!(
                "\nA device token for the CLI on this machine:\n\n    {token}\n\nOnce the host runs \
                 (`spares_server`), save it with `spares login http://{bind} --token <token>`, or \
                 log in with the password: `spares login http://{bind}`."
            );
        }
        HostCommand::AddUser {
            name,
            user_id,
            email,
            admin,
            password,
        } => {
            let _lock = HostLock::take(&root)?;
            let host = Host::open(&root).await?;
            let password = read_password(&password, "Password for the new account:")?;
            let user = host
                .create_user(NewUser {
                    id: user_id.as_deref(),
                    name: &name,
                    email: email.as_deref(),
                    password: &password,
                    admin,
                    email_verified: email.is_some(),
                })
                .await?;
            host.close_all().await;
            println!("Added {} ({}).", user.name, user.id);
        }
        HostCommand::Users => {
            let host = Host::open(&root).await?;
            for user in accounts::users(&host.accounts).await? {
                println!(
                    "{}\t{}\t{}{}{}",
                    user.id,
                    user.name,
                    user.email.as_deref().unwrap_or("-"),
                    if user.admin { "\tadmin" } else { "" },
                    if user.disabled_at.is_some() {
                        "\tdisabled"
                    } else {
                        ""
                    },
                );
            }
        }
        HostCommand::ResetPassword { user, password } => {
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            let password = read_password(&password, &format!("New password for {}:", user.name))?;
            accounts::set_password(&host.accounts, &user.id, &password).await?;
            println!("Set the password of {} ({}).", user.name, user.id);
        }
        HostCommand::DeleteUser { user, yes } => {
            let _lock = HostLock::take(&root)?;
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            let dir = host.user_dir(&user.id);
            if !yes
                && !inquire::Confirm::new(&format!(
                    "Delete {} and {}, with its collection?",
                    user.name,
                    dir.display()
                ))
                .with_default(false)
                .prompt()
                .map_err(|e| invalid(e.to_string()))?
            {
                return Ok(());
            }
            host.delete_user(&user.id).await?;
            println!("Deleted {} ({}).", user.name, user.id);
        }
        HostCommand::Import {
            data_dir,
            user,
            database,
            config_dir,
            cache_dir,
            rendered_dirs,
            pandoc_dir,
            replica,
            copy,
        } => {
            let _lock = HostLock::take(&root)?;
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            if data_dir.is_file() {
                super::export::import_archive(&host, &user.id, &data_dir).await?;
                host.close_all().await;
                println!(
                    "Unpacked {} into {}.",
                    data_dir.display(),
                    host.user_dir(&user.id).display()
                );
                return Ok(());
            }
            let env_dir = |name: &str| std::env::var_os(name).map(PathBuf::from);
            let rendered_dirs = if rendered_dirs.is_empty() {
                [
                    "SPARES_FILES_DIR",
                    "MARKDOWN_OUT_DIR",
                    "LATEX_OUT_DIR",
                    "TYPST_OUT_DIR",
                ]
                .into_iter()
                .filter_map(env_dir)
                .collect()
            } else {
                rendered_dirs
            };
            let replica = match replica {
                Some(replica) => Some(replica),
                None => std::env::var("SPARES_REPLICA")
                    .ok()
                    .map(|r| r.parse().map_err(invalid))
                    .transpose()?,
            };
            let pandoc_dir = pandoc_dir.unwrap_or_else(|| home().join(".local/share/pandoc"));
            let options = ImportOptions {
                data_dir,
                database,
                config_dir: Some(config_dir.unwrap_or_else(|| home().join(".config/spares"))),
                cache_dir: Some(cache_dir.unwrap_or_else(|| home().join(".cache/spares"))),
                rendered_dirs,
                pandoc_dir: pandoc_dir.is_dir().then_some(pandoc_dir),
                replica,
                copy,
            };
            let report = super::import::import(&host, &user.id, &options).await?;
            host.close_all().await;
            let verb = if copy { "Copied" } else { "Moved" };
            for (from, to) in &report.moved {
                println!("{verb} {} to {}", from.display(), to.display());
            }
            for (from, to, count) in &report.merged {
                println!(
                    "{verb} {count} files from {} into {}",
                    from.display(),
                    to.display()
                );
            }
            for path in &report.skipped {
                println!("Kept what was already there instead of {}", path.display());
            }
            if report.notes_rewritten > 0 {
                println!(
                    "Rewrote the paths of images, audio and image occlusions in {} notes.",
                    report.notes_rewritten
                );
            }
            if !report.left.is_empty() {
                println!("Left where they were:");
                for path in &report.left {
                    println!("  {}", path.display());
                }
            }
            println!(
                "\nThe collection is replica {} on this host. Rendered files are in {}; point \
                 tools that read them (vimtex's LATEX_OUT_DIR, say) there. If its notes \
                 repository has a clone, run `spares vcs init --repair-hooks`.",
                report.replica.unwrap_or(1),
                host.paths(&user.id).files.display()
            );
        }
        HostCommand::Token { user, name, scope } => {
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            let (device, token) =
                super::auth::create_token(&host.accounts, &user.id, &name, scope).await?;
            eprintln!("Device {} of {} ({}):", device.id, user.name, user.id);
            println!("{token}");
        }
        HostCommand::Revoke { user, device } => {
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            let Some(revoked) = super::auth::revoke_token(&host.accounts, &user.id, device).await?
            else {
                return Err(invalid(format!("{} has no device {device}", user.name)));
            };
            if let Some(replica) = revoked.replica {
                let collection = host.collection(&user.id).await?;
                let db = collection.db.clone();
                collection
                    .scope(spares_core::sync::devices::unenroll(
                        &db,
                        spares_core::sync::devices::UnenrollRequest { replica },
                    ))
                    .await?;
                host.close_all().await;
            }
            println!("Revoked {} ({}).", revoked.name, revoked.id);
        }
        HostCommand::Devices { user } => {
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            for device in super::auth::devices(&host.accounts, &user.id).await? {
                let last_used = device.last_used_at.map_or("never".to_string(), |at| {
                    chrono::DateTime::from_timestamp(at, 0)
                        .map_or_else(|| at.to_string(), |at| at.to_rfc3339())
                });
                println!(
                    "{}\t{}\t{}\treplica {}\tlast used {last_used}",
                    device.id,
                    device.name,
                    device.scope,
                    device.replica.map_or("-".to_string(), |r| r.to_string()),
                );
            }
        }
        HostCommand::Limits {
            user,
            sync_body_mb,
            media_body_mb,
            storage_mb,
            requests_per_minute,
            search_seconds,
            renders_at_once,
        } => {
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            let mut overrides = super::limits::overrides(&host.accounts, &user.id).await?;
            let changes = [
                (&mut overrides.sync_body_mb, sync_body_mb),
                (&mut overrides.media_body_mb, media_body_mb),
                (&mut overrides.storage_mb, storage_mb),
                (&mut overrides.requests_per_minute, requests_per_minute),
                (&mut overrides.search_seconds, search_seconds),
                (&mut overrides.renders_at_once, renders_at_once),
            ];
            let mut any = false;
            for (field, value) in changes {
                if let Some(value) = value {
                    *field = (value >= 0).then_some(value);
                    any = true;
                }
            }
            if any {
                super::limits::set_overrides(&host.accounts, &user.id, &overrides).await?;
            }
            let limits = host.config.limits.with(&overrides);
            let usage = super::limits::usage(&host.accounts, &user.id).await?;
            let megabytes = |bytes: i64| bytes / 1024 / 1024;
            println!("{} ({})", user.name, user.id);
            println!(
                "  limits: sync {} MB, media {} MB, storage {}, {} requests a minute, searches {} s, \
                 renders at once {}",
                limits.sync_body_mb,
                limits.media_body_mb,
                if limits.storage_mb == 0 {
                    "unlimited".to_string()
                } else {
                    format!("{} MB", limits.storage_mb)
                },
                if limits.requests_per_minute == 0 {
                    "unlimited".to_string()
                } else {
                    limits.requests_per_minute.to_string()
                },
                limits.search_seconds,
                if limits.renders_at_once == 0 {
                    host.config.render.renders_at_once
                } else {
                    usize::try_from(limits.renders_at_once).unwrap_or(usize::MAX)
                },
            );
            println!(
                "  usage: {} MB stored, {} requests",
                megabytes(usage.storage_bytes),
                usage.requests
            );
        }
        HostCommand::Invite { days } => {
            let host = Host::open(&root).await?;
            let code = super::signup::create_invite(&host.accounts, days).await?;
            if host.config.signups != super::Signups::Invite {
                eprintln!("This host's signups are not `invite`, so the code does nothing yet.");
            }
            println!("{code}");
        }
        HostCommand::Export { user, out } => {
            let host = Host::open(&root).await?;
            let user = find(&host, &user).await?;
            let out = out.unwrap_or_else(|| PathBuf::from(format!("{}.tar.gz", user.id)));
            super::export::export(&host, &user.id, &out).await?;
            println!("Wrote {}.", out.display());
        }
        HostCommand::Backup { dir } => {
            let host = Host::open(&root).await?;
            for path in super::export::backup(&host, &dir).await? {
                println!("Wrote {}", path.display());
            }
        }
        HostCommand::Migrate { all: _ } => {
            let _lock = HostLock::take(&root)?;
            let host = Host::open(&root).await?;
            for id in host.migrate_all().await? {
                println!("Migrated {id}");
            }
        }
    }
    Ok(())
}
