//! Two hosts, one the other's upstream: the laptop's copy of a collection syncing with the root, as
//! one of its user's devices (see `spares_core::sync::devices` and `spares_server::handlers::relay`).

use std::path::PathBuf;
use std::sync::Arc;

use spares_core::collection::Collection;
use spares_core::config::Replica;
use spares_core::model::NoteId;
use spares_core::parsers::get_all_parsers;
use spares_core::schema::note::CreateNoteRequest;
use spares_core::schema::note::CreateNotesRequest;
use spares_core::schema::parser::CreateParserRequest;
use spares_core::sync::devices::EnrollRequest;
use spares_core::sync::http::HttpRemote;
use spares_core::sync::http::RemoteAuth;
use spares_core::sync::run::SyncMode;
use spares_server::AppState;
use spares_server::host::Host;
use spares_server::host::accounts::NewUser;
use spares_server::host::auth::Scope;
use spares_server::route::create_router;

struct TestHost {
    host: Arc<Host>,
    url: String,
    root: PathBuf,
}

impl Drop for TestHost {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A host on a port of its own, with the account `me`.
async fn host(name: &str) -> TestHost {
    let root = std::env::temp_dir().join(format!(
        "spares-two-hosts-{name}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let host = Host::create(&root, "127.0.0.1:0").await.unwrap();
    host.create_user(NewUser {
        id: Some("me"),
        name: "me",
        email: None,
        password: "password1",
        admin: true,
        email_verified: true,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = create_router(
        Arc::new(AppState {
            host: Arc::clone(&host),
        }),
        None,
    );
    tokio::spawn(async move { axum::serve(listener, router).await });
    TestHost { host, url, root }
}

async fn note_texts(collection: &Collection) -> Vec<String> {
    sqlx::query_scalar("SELECT data FROM note ORDER BY id")
        .fetch_all(&collection.db)
        .await
        .unwrap()
}

async fn create(collection: &Arc<Collection>, parser_id: i64, data: &str) -> NoteId {
    let created = collection
        .scope(spares_core::api::note::create_notes(
            &collection.db,
            CreateNotesRequest {
                parser_id,
                requests: vec![CreateNoteRequest {
                    data: data.to_string(),
                    keywords: Vec::new(),
                    tags: Vec::new(),
                    is_suspended: false,
                    custom_data: serde_json::Map::new(),
                }],
            },
            chrono::Utc::now(),
            &get_all_parsers(),
            false,
        ))
        .await
        .unwrap();
    created.notes[0].id
}

async fn sync(collection: &Arc<Collection>, mode: SyncMode) {
    let temp = collection.paths.cache.join("sync");
    std::fs::create_dir_all(&temp).unwrap();
    collection
        .scope(spares_core::sync::files::sync_now(
            &collection.db,
            mode,
            &temp,
        ))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn a_laptop_syncs_with_its_upstream_as_a_device() {
    let root = host("root").await;
    let laptop = host("laptop").await;

    // The laptop's collection, with a note, before it has an upstream: the root of its own chain
    let collection = laptop.host.collection("me").await.unwrap();
    assert_eq!(collection.replica(), Some(Replica::ROOT));
    let parser = collection
        .scope(spares_core::api::parser::create_parser(
            &collection.db,
            CreateParserRequest {
                name: "markdown".to_string(),
            },
            false,
        ))
        .await
        .unwrap();
    create(&collection, parser.id, "On the laptop: {{Paris}}").await;

    // `spares sync login`: a sync token at the root, and a replica from it
    let (_, token) = spares_server::host::auth::create_token(
        &root.host.accounts,
        "me",
        "laptop (host)",
        Scope::Sync,
    )
    .await
    .unwrap();
    let remote = HttpRemote::new(&root.url, RemoteAuth::Token(token.clone())).unwrap();
    let enrolled = remote
        .enroll_with(&EnrollRequest {
            name: "laptop (host)".to_string(),
            host: true,
            replica: None,
        })
        .await
        .unwrap();
    assert_eq!(enrolled.replica, 0, "a host gets the laptop's number");
    let devices = spares_server::host::auth::devices(&root.host.accounts, "me")
        .await
        .unwrap();
    assert_eq!(devices[0].replica, Some(0), "recorded on its token");
    let config = collection.paths.config_file();
    let contents = std::fs::read_to_string(&config).unwrap_or_default();
    let contents = spares_core::config::set_sync_values(
        &contents,
        [
            ("url", root.url.clone().into()),
            ("token", token.clone().into()),
            ("replica", 0_i64.into()),
        ],
    )
    .unwrap();
    std::fs::write(&config, contents).unwrap();
    collection.set_replica(Some(Replica::LAPTOP));

    // The first sync is a full one; then changes go both ways.
    sync(&collection, SyncMode::FullUpload).await;
    let upstream = root.host.collection("me").await.unwrap();
    assert_eq!(
        note_texts(&upstream).await,
        ["On the laptop: {{[o:1]Paris}}"]
    );
    create(&upstream, parser.id, "On the root: {{Rome}}").await;
    create(&collection, parser.id, "Laptop again: {{Lyon}}").await;
    sync(&collection, SyncMode::Normal).await;
    let mut laptop_notes = note_texts(&collection).await;
    let mut root_notes = note_texts(&upstream).await;
    laptop_notes.sort();
    root_notes.sort();
    assert_eq!(laptop_notes.len(), 3, "{laptop_notes:?}");
    assert_eq!(laptop_notes, root_notes);

    // A browser on the laptop enrolls with the root through it.
    let (_, cli_token) =
        spares_server::host::auth::create_token(&laptop.host.accounts, "me", "CLI", Scope::All)
            .await
            .unwrap();
    let through_laptop = HttpRemote::new(&laptop.url, RemoteAuth::Token(cli_token)).unwrap();
    let browser = through_laptop.enroll("phone").await.unwrap();
    assert_eq!(browser.replica, 2);
    let at_root: Vec<i64> = sqlx::query_scalar("SELECT replica FROM sync_device ORDER BY replica")
        .fetch_all(&upstream.db)
        .await
        .unwrap();
    assert_eq!(at_root, [0, 2]);
    let at_laptop: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_device")
        .fetch_one(&collection.db)
        .await
        .unwrap();
    assert_eq!(at_laptop, 0, "the laptop gives out no numbers");
    let devices = through_laptop.devices().await.unwrap();
    assert_eq!(devices.len(), 2);
    through_laptop.unenroll(2).await.unwrap();

    // Another user on the root can't reach this collection with their own token.
    root.host
        .create_user(NewUser {
            id: Some("other"),
            name: "other",
            email: None,
            password: "password2",
            admin: false,
            email_verified: false,
        })
        .await
        .unwrap();
    let (_, other_token) =
        spares_server::host::auth::create_token(&root.host.accounts, "other", "x", Scope::Sync)
            .await
            .unwrap();
    let other = HttpRemote::new(&root.url, RemoteAuth::Token(other_token)).unwrap();
    let err = spares_core::sync::run::SyncRemote::meta(&other, 0)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not enrolled"), "{err}");

    drop((collection, upstream));
    laptop.host.close_all().await;
    root.host.close_all().await;
}

/// The laptop's collection, synced with `root` as its upstream once, and the root's copy.
async fn laptop_with_upstream(
    root: &TestHost,
    laptop: &TestHost,
) -> (Arc<Collection>, Arc<Collection>) {
    let collection = laptop.host.collection("me").await.unwrap();
    let (_, token) = spares_server::host::auth::create_token(
        &root.host.accounts,
        "me",
        "laptop (host)",
        Scope::Sync,
    )
    .await
    .unwrap();
    HttpRemote::new(&root.url, RemoteAuth::Token(token.clone()))
        .unwrap()
        .enroll_with(&EnrollRequest {
            name: "laptop (host)".to_string(),
            host: true,
            replica: None,
        })
        .await
        .unwrap();
    let config = collection.paths.config_file();
    let contents = std::fs::read_to_string(&config).unwrap_or_default();
    let contents = spares_core::config::set_sync_values(
        &contents,
        [
            ("url", root.url.clone().into()),
            ("token", token.into()),
            ("replica", 0_i64.into()),
        ],
    )
    .unwrap();
    std::fs::write(&config, contents).unwrap();
    collection.set_replica(Some(Replica::LAPTOP));
    sync(&collection, SyncMode::FullUpload).await;
    let upstream = root.host.collection("me").await.unwrap();
    (collection, upstream)
}

/// The directory of `collection`'s image occlusion files
async fn image_occlusion_dir(collection: &Arc<Collection>) -> PathBuf {
    collection
        .scope(async { spares_core::sync::media::MediaKind::ImageOcclusion.directory() })
        .await
}

/// Writes `contents` to `path`, changed `seconds_ago` seconds ago (negative for later)
fn write_at(path: &std::path::Path, contents: &[u8], seconds_ago: i64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    let now = std::time::SystemTime::now();
    let delta = std::time::Duration::from_secs(seconds_ago.unsigned_abs());
    let at = if seconds_ago >= 0 {
        now - delta
    } else {
        now + delta
    };
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

/// An image occlusion's files change under one name, so a sync compares them by hash: a file
/// replaced on either side replaces the other side's, the newer one winning.
#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_image_occlusion_file_reaches_the_other_side() {
    let root = host("root").await;
    let laptop = host("laptop").await;
    let (collection, upstream) = laptop_with_upstream(&root, &laptop).await;
    let (laptop_dir, root_dir) = (
        image_occlusion_dir(&collection).await,
        image_occlusion_dir(&upstream).await,
    );
    let svg = |width: u32| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><g class="layer" id="clozes-group"><rect id="a" x="1" y="1" width="{width}" height="2"/></g></svg>"#
        )
    };
    // Named as before names ended in a hash, as most of the user's are
    let (image, clozes) = ("roster.png", "roster_clozes.svg");
    write_at(&laptop_dir.join(image), b"first image", 60);
    write_at(&laptop_dir.join(clozes), svg(2).as_bytes(), 60);
    sync(&collection, SyncMode::Normal).await;
    assert_eq!(std::fs::read(root_dir.join(image)).unwrap(), b"first image");

    // Replaced on the laptop, at the same path: before, the root kept the first image for good.
    write_at(&laptop_dir.join(image), b"second image", -5);
    sync(&collection, SyncMode::Normal).await;
    assert_eq!(
        std::fs::read(root_dir.join(image)).unwrap(),
        b"second image"
    );

    // Rewritten on the root, as parsing a note edited there writes card orders into it
    write_at(&root_dir.join(clozes), svg(3).as_bytes(), -30);
    sync(&collection, SyncMode::Normal).await;
    assert_eq!(
        std::fs::read_to_string(laptop_dir.join(clozes)).unwrap(),
        svg(3)
    );
    assert_eq!(
        std::fs::read(laptop_dir.join(image)).unwrap(),
        b"second image",
        "the laptop's newer image is kept"
    );

    // Once both sides have the same files, a sync sends and receives none.
    let listed = |dir: PathBuf| {
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().into_string().unwrap(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .filter(|(name, _)| !name.starts_with('.'))
            .collect();
        files.sort();
        files
    };
    assert_eq!(listed(laptop_dir.clone()), listed(root_dir.clone()));
    let remote = collection
        .scope(async {
            let upstream = spares_core::config::read_external_config().unwrap().sync;
            HttpRemote::new(&upstream.url.unwrap(), RemoteAuth::token(upstream.token)).unwrap()
        })
        .await;
    let sent = collection
        .scope(spares_core::sync::files::send_media(&remote))
        .await
        .unwrap();
    let received = collection
        .scope(spares_core::sync::files::receive_media(&remote))
        .await
        .unwrap();
    assert_eq!((sent, received.received), (0, 0));

    drop((collection, upstream));
    laptop.host.close_all().await;
    root.host.close_all().await;
}

/// A root whose `[sync] url` is its own address, as a copied config.toml can make it, fails its own
/// sync and its devices' at once, instead of relaying to itself until it runs out of open files.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_whose_upstream_is_itself_refuses_the_loop() {
    let root = host("loop").await;
    let collection = root.host.collection("me").await.unwrap();
    let (_, token) =
        spares_server::host::auth::create_token(&root.host.accounts, "me", "itself", Scope::Sync)
            .await
            .unwrap();
    let config = collection.paths.config_file();
    let contents = std::fs::read_to_string(&config).unwrap_or_default();
    let contents = spares_core::config::set_sync_values(
        &contents,
        [("url", root.url.clone().into()), ("token", token.into())],
    )
    .unwrap();
    std::fs::write(&config, contents).unwrap();

    let within = std::time::Duration::from_secs(30);
    // Its own sync
    let temp = collection.paths.cache.join("sync");
    std::fs::create_dir_all(&temp).unwrap();
    let err = tokio::time::timeout(
        within,
        collection.scope(spares_core::sync::files::sync_now(
            &collection.db,
            SyncMode::Normal,
            &temp,
        )),
    )
    .await
    .expect("refused at once")
    .unwrap_err();
    assert!(err.to_string().contains("leads back to this host"), "{err}");

    // A device's request, which the host relays to its upstream: itself
    let (_, cli_token) =
        spares_server::host::auth::create_token(&root.host.accounts, "me", "CLI", Scope::All)
            .await
            .unwrap();
    let device = HttpRemote::new(&root.url, RemoteAuth::Token(cli_token)).unwrap();
    let err = tokio::time::timeout(within, device.enroll("phone"))
        .await
        .expect("refused at once")
        .unwrap_err();
    assert!(err.to_string().contains("leads back to this host"), "{err}");

    drop(collection);
    root.host.close_all().await;
}
