//! Files after a sync. The database is the source of truth, so the files of every note a sync
//! changed are regenerated from it, those of deleted notes removed, and card files past a note's
//! last card deleted, on whichever side the change landed.
//!
//! With the notes repository ([`notes_vcs`]), regenerated note files are committed to the server
//! copy, and edits not pushed yet live in the user's clone, so a sync never overwrites them: they
//! meet the sync's commit when the clone is rebased.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use log::Level;
use log::error;
use log::log;
use sqlx::SqlitePool;

use super::bump_notes_version;
use super::http::HttpRemote;
use super::http::RemoteAuth;
use super::media;
use super::media::MediaFile;
use super::media::MediaKind;
use super::media::Replaced;
use super::run::SyncMode;
use super::run::SyncReport;
use super::run::sync;
use crate::Error;
use crate::api::note::delete_card_files;
use crate::api::note::render_notes;
use crate::api::note::stale_files::card_orders_on_disk;
use crate::api::note::stale_files::delete_files_of_deleted_notes;
use crate::config::read_external_config;
use crate::model::NoteId;
use crate::notes_vcs;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;
use crate::schema::note::NotesSelector;
use crate::schema::note::RenderNotesRequest;

/// Syncs with `sync.url` from the config, then updates files. For the client.
pub async fn sync_now(
    db: &SqlitePool,
    mode: SyncMode,
    temp_dir: &Path,
) -> Result<SyncReport, Error> {
    let upstream = read_external_config()?.sync;
    let url = upstream
        .url
        .ok_or_else(|| Error::Sync("`sync.url` is not set in config.toml".to_string()))?;
    let remote = HttpRemote::new(&url, RemoteAuth::token(upstream.token))?;
    // Before the push, so the upstream has them when it renders the notes that name them, as a
    // browser sends its own (see `super::media`)
    match send_media(&remote).await {
        Ok(0) => {}
        Ok(sent) => log::info!("Sent {sent} images, clips and image occlusions"),
        // The push would fail the same way. The caller says so, e.g. automatic sync only once the
        // upstream has stayed unreachable a while, not in each of a closed lid's dark wakes.
        Err(e @ Error::SyncUnreachable(_)) => return Err(e),
        Err(e) => error!("Failed to send images and audio to {url}: {e}"),
    }
    let report = sync(db, &remote, mode, temp_dir).await?;
    // Before the files of changed notes are regenerated, which may name them
    let replaced = match receive_media(&remote).await {
        Ok(ReceivedMedia { received: 0, .. }) => Vec::new(),
        Ok(ReceivedMedia { received, replaced }) => {
            log::info!("Received {received} images, clips and image occlusions");
            replaced
        }
        Err(e) => {
            error!("Failed to receive images and audio from {url}: {e}");
            Vec::new()
        }
    };
    after_sync(db, &report.notes_changed, &report.notes_deleted).await;
    render_notes_naming(db, &replaced).await;
    // Automatic sync sends local changes within a minute, so a sync that only sent is routine.
    let level = if report.pulled > 0 || !report.conflicts.is_empty() {
        Level::Info
    } else {
        Level::Debug
    };
    log!(
        level,
        "Synced with {url}: {} received, {} sent, {} conflicts",
        report.pulled,
        report.pushed,
        report.conflicts.len()
    );
    Ok(report)
}

/// The kinds of media the sync exchanges with the upstream.
const MEDIA_KINDS: [MediaKind; 3] = [
    MediaKind::Image,
    MediaKind::Audio,
    MediaKind::ImageOcclusion,
];

/// The upstream's files of `kind`, and ours, by name.
async fn media_files(
    remote: &HttpRemote,
    kind: MediaKind,
) -> Result<(BTreeMap<String, MediaFile>, BTreeMap<String, MediaFile>), Error> {
    let by_name = |files: Vec<MediaFile>| {
        files
            .into_iter()
            .map(|file| (file.name.clone(), file))
            .collect::<BTreeMap<_, _>>()
    };
    let theirs = by_name(remote.list_media(kind).await?);
    let ours = by_name(media::list_media(kind)?);
    Ok((theirs, ours))
}

/// Whether `newer`, a file of one side, replaces `older`, the other side's file of that name: their
/// contents differ, and `newer` changed last. Only image occlusions have [hashes](MediaFile::hash),
/// since the other files are named by theirs. Files changed in the same second are left as they
/// are, until one changes again.
fn replaces(newer: &MediaFile, older: &MediaFile) -> bool {
    matches!((&newer.hash, &older.hash), (Some(a), Some(b)) if a != b) && newer.mtime > older.mtime
}

/// Sends the upstream the images, audio and image occlusions it lacks, and the image occlusion
/// files changed here since it got them, before a sync. An image or clip's name ends in its
/// contents' hash (see [`super::media`]), so a name never stands for two of them, and the
/// upstream never replaces one. Returns how many were sent.
pub async fn send_media(remote: &HttpRemote) -> Result<usize, Error> {
    let mut sent = 0;
    for kind in MEDIA_KINDS {
        let (theirs, ours) = media_files(remote, kind).await?;
        for (name, file) in &ours {
            let replaced = match theirs.get(name) {
                None => None,
                Some(their_file) if replaces(file, their_file) => their_file.hash.as_deref(),
                Some(_) => continue,
            };
            let path = kind.directory().join(name);
            let contents = std::fs::read(&path).map_err(|e| Error::Io {
                description: format!("Failed to read {}", path.display()),
                source: e,
            })?;
            match remote.upload_media(kind, name, contents, replaced).await {
                Ok(()) => sent += 1,
                Err(e) => log::warn!("Failed to send {name}: {e}"),
            }
        }
    }
    Ok(sent)
}

/// What [`receive_media`] received
#[derive(Debug, Default)]
pub struct ReceivedMedia {
    /// How many files were received
    pub received: usize,
    /// The names of the image occlusion files that replaced this host's, whose notes are rendered
    /// again (see [`render_notes_naming`])
    pub replaced: Vec<String>,
}

/// Receives from the upstream the images, audio and image occlusions this host lacks, and the
/// image occlusion files changed there since this host got them, after a sync.
pub async fn receive_media(remote: &HttpRemote) -> Result<ReceivedMedia, Error> {
    let mut received = ReceivedMedia::default();
    for kind in MEDIA_KINDS {
        let (theirs, ours) = media_files(remote, kind).await?;
        for (name, file) in &theirs {
            let replaced = match ours.get(name) {
                None => None,
                Some(our_file) if replaces(file, our_file) => our_file.hash.as_deref(),
                Some(_) => continue,
            };
            let stored = async {
                let contents = remote.download_media(kind, name).await?;
                match replaced {
                    None => media::store_media(kind, name, &contents).map(|_| false),
                    Some(replaced) => media::replace_media(kind, name, &contents, replaced)
                        .map(|(_, replaced)| replaced == Replaced::Replaced),
                }
            };
            match stored.await {
                Ok(replaced) => {
                    received.received += 1;
                    if replaced {
                        received.replaced.push(name.clone());
                    }
                }
                Err(e) => log::warn!("Failed to receive {name}: {e}"),
            }
        }
    }
    Ok(received)
}

/// Renders again the notes that name any of `names`, image occlusion files a sync replaced. Their
/// text is unchanged, so they would be rendered again only once something else changed them. A
/// note naming a file of the same name elsewhere is rendered too, which its render cache skips.
/// Failures are logged.
pub async fn render_notes_naming(db: &SqlitePool, names: &[String]) {
    let note_ids = match notes_naming(db, names).await {
        Ok(note_ids) if note_ids.is_empty() => return,
        Ok(note_ids) => note_ids,
        Err(e) => {
            error!("Failed to find the notes naming {}: {e}", names.join(", "));
            return;
        }
    };
    let rendered = render_notes(
        db,
        RenderNotesRequest {
            selector: NotesSelector::Ids(note_ids.into_iter().collect()),
            immutable_note_ids: None,
            overridden_output_raw_dir: None,
            include_linked_notes: false,
            include_cards: true,
            generate_rendered: true,
            force_generate_rendered: false,
        },
        &get_all_parsers(),
    )
    .await;
    if let Err(e) = rendered {
        error!("Failed to render the notes naming replaced image occlusion files: {e}");
    }
}

/// The notes whose text has any of `names`
async fn notes_naming(db: &SqlitePool, names: &[String]) -> Result<BTreeSet<NoteId>, Error> {
    let mut note_ids = BTreeSet::new();
    for name in names {
        let found: Vec<NoteId> = sqlx::query_scalar("SELECT id FROM note WHERE instr(data, ?) > 0")
            .bind(name)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        note_ids.extend(found);
    }
    Ok(note_ids)
}

/// Every note id, for telling afterwards which notes a full sync removed.
pub async fn note_ids(db: &SqlitePool) -> Result<BTreeSet<NoteId>, Error> {
    let ids: Vec<NoteId> = sqlx::query_scalar("SELECT id FROM note")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ids.into_iter().collect())
}

/// Regenerates the files of `changed` notes and removes those of `deleted` ones, and card files
/// left over from cards a sync removed, and lets readers of the note change number know. The
/// database change has happened, so failures are logged rather than returned.
pub async fn after_sync(db: &SqlitePool, changed: &BTreeSet<NoteId>, deleted: &BTreeSet<NoteId>) {
    if changed.is_empty() && deleted.is_empty() {
        return;
    }
    let bumped = match db.acquire().await {
        Ok(mut conn) => bump_notes_version(&mut conn).await,
        Err(e) => Err(Error::Sqlx { source: e }),
    };
    if let Err(e) = bumped {
        error!("Failed to record that a sync changed notes: {e}");
    }
    if let Err(e) = update_files(db, changed, deleted).await {
        error!("Failed to update note files after a sync: {e}");
    }
}

async fn update_files(
    db: &SqlitePool,
    changed: &BTreeSet<NoteId>,
    deleted: &BTreeSet<NoteId>,
) -> Result<(), Error> {
    let parsers = get_all_parsers();
    let all: BTreeSet<NoteId> = changed.union(deleted).copied().collect();
    // The notes still there, by parser, with their number of cards
    let mut remaining: BTreeMap<String, BTreeMap<NoteId, usize>> = BTreeMap::new();
    for &note_id in &all {
        let current: Option<(String, i64)> = sqlx::query_as(
            "SELECT p.name, (SELECT COUNT(*) FROM card WHERE note_id = n.id)
             FROM note n JOIN parser p ON p.id = n.parser_id WHERE n.id = ?",
        )
        .bind(note_id)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        if let Some((parser_name, card_count)) = current {
            let card_count = usize::try_from(card_count).unwrap_or(0);
            remaining
                .entry(parser_name)
                .or_default()
                .insert(note_id, card_count);
        }
    }
    for (parser_name, notes) in remaining {
        let parser = find_parser(&parser_name, &parsers)?;
        let ids: BTreeSet<NoteId> = notes.keys().copied().collect();
        let on_disk = card_orders_on_disk(parser.as_ref(), &ids);
        for (note_id, orders) in on_disk {
            let stale: Vec<usize> = orders
                .into_iter()
                .filter(|order| *order > notes[&note_id])
                .collect();
            delete_card_files(parser.as_ref(), note_id, &stale)?;
        }
    }
    delete_files_of_deleted_notes(db, &all).await?;
    if !deleted.is_empty() {
        notes_vcs::reconcile_after(db, deleted.iter().copied().collect(), "sync").await;
    }
    if !changed.is_empty() {
        render_notes(
            db,
            RenderNotesRequest {
                selector: NotesSelector::Ids(changed.iter().copied().collect()),
                immutable_note_ids: None,
                overridden_output_raw_dir: None,
                include_linked_notes: false,
                include_cards: true,
                generate_rendered: true,
                force_generate_rendered: false,
            },
            &parsers,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn test_notes_naming(pool: SqlitePool) {
        let notes = crate::api::note::tests::tests::create_note_helper(&pool).await;
        let names = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            notes_naming(&pool, &names(&["Second", "Third"]))
                .await
                .unwrap(),
            BTreeSet::from([notes[1].id, notes[2].id])
        );
        assert!(
            notes_naming(&pool, &names(&["roster.png"]))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_replaces() {
        let file = |hash: Option<&str>, mtime| MediaFile {
            name: "roster.png".to_string(),
            size: 1,
            mtime,
            hash: hash.map(ToString::to_string),
        };
        assert!(replaces(&file(Some("b"), 2), &file(Some("a"), 1)));
        // Older, the same, changed in the same second, or without a hash
        assert!(!replaces(&file(Some("b"), 1), &file(Some("a"), 2)));
        assert!(!replaces(&file(Some("a"), 2), &file(Some("a"), 1)));
        assert!(!replaces(&file(Some("b"), 1), &file(Some("a"), 1)));
        assert!(!replaces(&file(Some("b"), 2), &file(None, 1)));
        assert!(!replaces(&file(None, 2), &file(None, 1)));
    }
}
