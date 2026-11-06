//! Support for parsers whose cards web clients compile themselves (see
//! [`Parseable::renders_in_browser`]), so the server only needs the database and the files the
//! sources read, not a raw and rendered file per card.
//!
//! - The raw sources are generated from the database on demand ([`add_browser_render_sources`]).
//! - Files the sources read (preambles, figures, image occlusions' images, ...) are discovered
//!   when rendering locally. Rendering records them for each rendered file (see
//!   [`super::render_state`]), and [`sync_render_assets`] copies them into a content-addressed store in
//!   the data directory, which `spares sync cloud` copies to the server, but for the copies of
//!   media, which the server stores from its own media files (see [`get_render_asset`]). Files
//!   are stored by the absolute path sources read them by. No source reads a rendered image
//!   occlusion: a Typst card has its clozes in its source, over the original image (see
//!   [`Parseable::image_occlusion_overlays`]), and a device that keeps a copy of the collection
//!   makes the images of its Markdown cards itself (see `parsers::read_device_card`, wasm only).
//! - Audio clips (see [`crate::parsers::audio`]) are stored the same way for every parser, since
//!   web clients play them outside the rendered card. Renderers don't read them, so they are
//!   found in the notes themselves ([`audio_clips`]).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::create_dir_all;
use std::fs::read_dir;
use std::fs::remove_file;
use std::fs::rename;
use std::fs::write;
use std::hash::BuildHasher;
use std::hash::RandomState;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use log::warn;
use sqlx::FromRow;
use sqlx::sqlite::SqlitePool;

use crate::CardErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::api::note::render_note_data_to_generate_files_request;
use crate::api::render_state::read_render_dependencies;
use crate::api::review::relative_to_files_dir;
use crate::api::review::web_render_path;
use crate::config::get_data_dir;
use crate::model::NoteId;
use crate::parsers::CardData;
use crate::parsers::Parseable;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::TemplateType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::generate_files::construct_card_file_contents;
use crate::parsers::generate_files::construct_note_file_contents;
use crate::parsers::get_all_parsers;
use crate::parsers::get_cards;
use crate::schema::note::NoteRenderResponse;
use crate::schema::review::BrowserRenderSources;
use crate::schema::review::CardBackRenderedPath;
use crate::schema::review::GetReviewCardResponse;

/// Content-addressed copies of render assets, named by their blake3 hash.
pub fn get_render_assets_dir() -> PathBuf {
    let mut dir = get_data_dir();
    dir.push("render_assets");
    dir
}

/// The note of `response`'s card, as rendering it needs, and the card parsed from it.
pub(crate) async fn load_review_card(
    db: &SqlitePool,
    parser: &dyn Parseable,
    response: &GetReviewCardResponse,
) -> Result<(GenerateNoteFilesRequest, CardData), Error> {
    let note_id = response.note_id;
    let request = get_generate_files_request(db, note_id).await?;
    let card_order = response.card_order as usize;
    let card = card_order
        .checked_sub(1)
        .and_then(|i| {
            get_cards(parser, None, &request.note_data, false, false)
                .map(|mut cards| (i < cards.len()).then(|| cards.swap_remove(i)))
                .transpose()
        })
        .transpose()?
        .ok_or_else(|| {
            not_found(format!(
                "Note {} has no card {card_order}.",
                note_id.letters()
            ))
        })?;
    Ok((request, card))
}

/// Fills in [`GetReviewCardResponse::browser_sources`] for `card`, `response`'s card parsed from
/// `request` (see [`load_review_card`]), if `parser` renders in the browser.
pub(crate) fn add_browser_render_sources(
    parser: &dyn Parseable,
    response: &mut GetReviewCardResponse,
    request: &GenerateNoteFilesRequest,
    card: &CardData,
) -> Result<(), Error> {
    if !parser.renders_in_browser() {
        return Ok(());
    }
    response.browser_sources = Some(card_sources(parser, response, request, card)?);
    Ok(())
}

/// Fills in [`GetReviewCardResponse::device_sources`] for `card` as
/// [`add_browser_render_sources`] does, on a device and if `parser` is Markdown's: the web app
/// shows them with its own Markdown renderer, then with pandoc's WebAssembly build, while the
/// server hasn't rendered the card's note.
pub(crate) fn add_device_sources(
    parser: &dyn Parseable,
    response: &mut GetReviewCardResponse,
    request: &GenerateNoteFilesRequest,
    card: &CardData,
) -> Result<(), Error> {
    if !cfg!(target_family = "wasm") || parser.get_parser_name() != "markdown" {
        return Ok(());
    }
    response.device_sources = Some(card_sources(parser, response, request, card)?);
    Ok(())
}

/// The sources of both sides of `card`, `response`'s card parsed from `request`.
fn card_sources(
    parser: &dyn Parseable,
    response: &GetReviewCardResponse,
    request: &GenerateNoteFilesRequest,
    card: &CardData,
) -> Result<BrowserRenderSources, Error> {
    let card_order = response.card_order as usize;
    let template = |template_type| get_template(parser, template_type);
    let (card_template, body_placeholder) = template(TemplateType::Card)?;
    let card_side = |side| {
        construct_card_file_contents(
            parser,
            &card_template,
            &body_placeholder,
            request,
            card_order,
            card,
            side,
        )
    };
    let card_back = match response.card_back_raw_path {
        CardBackRenderedPath::CardBack(_) => card_side(CardSide::Back),
        CardBackRenderedPath::Note(_) => {
            let (note_template, body_placeholder) = template(TemplateType::Note)?;
            parser.note_render_source(construct_note_file_contents(
                parser,
                &note_template,
                &body_placeholder,
                request,
            ))
        }
    };
    Ok(BrowserRenderSources {
        card_front: card_side(CardSide::Front),
        card_back,
    })
}

/// How a client shows note `note_id` rendered: the rendered file, or the raw source to compile
/// itself if its parser renders in the browser.
pub async fn get_note_render(
    db: &SqlitePool,
    note_id: NoteId,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<NoteRenderResponse, Error> {
    let parser_name: String = sqlx::query_scalar(
        r"SELECT p.name FROM note n JOIN parser p ON n.parser_id = p.id WHERE n.id = ?",
    )
    .bind(note_id)
    .fetch_optional(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?
    .ok_or_else(|| not_found(format!("Note {} not found.", note_id.letters())))?;
    let parser = find_parser(&parser_name, all_parsers)?;

    let mut rendered_path = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    rendered_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));

    let browser_source = if parser.renders_in_browser() {
        let request = get_generate_files_request(db, note_id).await?;
        let (note_template, body_placeholder) = get_template(parser.as_ref(), TemplateType::Note)?;
        Some(construct_note_file_contents(
            parser.as_ref(),
            &note_template,
            &body_placeholder,
            &request,
        ))
    } else {
        None
    };
    let rendered_path = relative_to_files_dir(rendered_path);
    let web_path = if parser.has_web_render() {
        web_render_path(&rendered_path)
    } else {
        None
    };
    Ok(NoteRenderResponse {
        parser_name,
        rendered_path,
        web_path,
        browser_source,
    })
}

fn not_found(description: String) -> Error {
    Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(description)))
}

async fn get_generate_files_request(
    db: &SqlitePool,
    note_id: NoteId,
) -> Result<GenerateNoteFilesRequest, Error> {
    let note_data = get_render_note_data(db, Some(vec![note_id]))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| not_found(format!("Note {} not found.", note_id.letters())))?;
    let note_links = get_note_links_map(db, &[note_id]).await?;
    Ok(render_note_data_to_generate_files_request(
        &note_data,
        Some(&note_links),
    ))
}

fn get_template(
    parser: &dyn Parseable,
    template_type: TemplateType,
) -> Result<(String, String), Error> {
    parser
        .get_template_data(template_type)
        .map_err(|e| Error::Io {
            description: format!(
                "Failed to read template for parser {}",
                parser.get_parser_name()
            ),
            source: e,
        })
}

#[derive(Debug, FromRow)]
struct RenderAssetRow {
    path: String,
    hash: String,
    mtime: i64,
    size: i64,
}

/// Records the dependencies of `parser`'s rendered files for `note_ids` (all notes if `None`), and
/// the audio clips they link to, and refreshes every stored asset whose file changed since it was
/// last stored, then removes those no note reads any more (see [`remove_unread_render_assets`]).
/// For parsers that do not [render in the browser](Parseable::renders_in_browser),
/// only audio clips are recorded.
///
/// Files that do not exist locally keep their stored copy, so this is safe to run on a server that
/// only has the synced store.
pub async fn sync_render_assets<S: BuildHasher>(
    db: &SqlitePool,
    parser: &dyn Parseable,
    note_ids: Option<&HashSet<NoteId, S>>,
) -> Result<(), Error> {
    // Other parsers' cards are shown rendered, so web clients only need their audio clips
    let (mut files, packages) = if parser.renders_in_browser() {
        read_render_dependencies(db, parser.get_parser_name(), note_ids).await?
    } else {
        Default::default()
    };
    files.extend(audio_clips(db, parser, note_ids).await?);
    if files.is_empty() && packages.is_empty() && !parser.renders_in_browser() {
        return Ok(());
    }

    let existing: Vec<RenderAssetRow> =
        sqlx::query_as("SELECT path, hash, mtime, size FROM render_asset")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let mut changed = false;
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;
    let existing: HashMap<String, RenderAssetRow> = existing
        .into_iter()
        .map(|row| (row.path.clone(), row))
        .collect();
    let paths: BTreeMap<String, PathBuf> = files
        .iter()
        .filter_map(|path| Some((path.to_str()?.to_owned(), path.clone())))
        .chain(existing.keys().map(|key| (key.clone(), PathBuf::from(key))))
        .collect();

    let store_dir = get_render_assets_dir();
    create_dir_all(&store_dir).map_err(|e| Error::Io {
        description: format!("Failed to create {}", store_dir.display()),
        source: e,
    })?;
    for (key, path) in paths {
        // Missing locally, e.g. on a server. Keep the stored copy.
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_nanos()).ok())
            .unwrap_or_default();
        let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        if existing
            .get(&key)
            .is_some_and(|row| row.mtime == mtime && row.size == size)
        {
            continue;
        }
        let hash = store_render_asset(&path, &store_dir)?;
        if existing.get(&key).is_some_and(|row| row.hash == hash) {
            sqlx::query("UPDATE render_asset SET mtime = ?, size = ? WHERE path = ?")
                .bind(mtime)
                .bind(size)
                .bind(&key)
        } else {
            changed = true;
            sqlx::query(
                r"INSERT INTO render_asset (path, hash, mtime, size) VALUES (?, ?, ?, ?)
                ON CONFLICT(path) DO UPDATE SET hash = excluded.hash, mtime = excluded.mtime, size = excluded.size",
            )
            .bind(&key)
            .bind(&hash)
            .bind(mtime)
            .bind(size)
        }
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    }
    for spec in &packages {
        sqlx::query("INSERT OR IGNORE INTO render_package (spec) VALUES (?)")
            .bind(spec)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;

    // Even if no file changed, what the notes read may have, such as a figure a note no longer
    // includes
    let removed = remove_unread_rows(db, None).await?;
    if changed || removed > 0 {
        remove_unreferenced_render_assets(db, &store_dir, UNREFERENCED_GRACE).await?;
    }
    Ok(())
}

/// The audio clips that `parser`'s notes of `note_ids` (all notes if `None`) link to by absolute
/// path.
async fn audio_clips<S: BuildHasher>(
    db: &SqlitePool,
    parser: &dyn Parseable,
    note_ids: Option<&HashSet<NoteId, S>>,
) -> Result<BTreeSet<PathBuf>, Error> {
    let ids = note_ids.map(|ids| serde_json::to_string(&ids.iter().collect::<Vec<_>>()));
    let ids = ids.transpose().map_err(|e| {
        Error::Library(LibraryError::Card(CardErrorKind::InvalidInput(format!(
            "Failed to list note ids: {e}"
        ))))
    })?;
    let data: Vec<String> = sqlx::query_scalar(
        r"SELECT n.data FROM note n JOIN parser p ON p.id = n.parser_id
        WHERE p.name = ?1 AND (?2 IS NULL OR n.id IN (SELECT value FROM json_each(?2)))",
    )
    .bind(parser.get_parser_name())
    .bind(ids)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(data
        .iter()
        .flat_map(|data| parser.audio_refs(data))
        .map(|audio_ref| PathBuf::from(audio_ref.path))
        .filter(|path| path.is_absolute())
        .collect())
}

/// Copies `path` into `store_dir` and returns its hash.
fn store_render_asset(path: &Path, store_dir: &Path) -> Result<String, Error> {
    let contents = std::fs::read(path).map_err(|e| Error::Io {
        description: format!("Failed to read {}", path.display()),
        source: e,
    })?;
    let hash = blake3::hash(&contents).to_hex().to_string();
    let stored_path = store_dir.join(&hash);
    if !stored_path.exists() {
        // Write then rename so a partially written file is never served.
        let temp_path = store_dir.join(format!("{hash}.tmp"));
        write(&temp_path, &contents)
            .and_then(|()| rename(&temp_path, &stored_path))
            .map_err(|e| Error::Io {
                description: format!("Failed to write {}", stored_path.display()),
                source: e,
            })?;
    }
    Ok(hash)
}

/// How long a stored copy stays after no row names it any more. One a host just copied to the
/// server (see `spares sync cloud`) has no row until its push, and a render's new copy none until
/// its transaction commits.
const UNREFERENCED_GRACE: Duration = Duration::from_mins(10);

/// When `metadata`'s file was put where it is: its inode change time, which copies made with their
/// modification time kept (`rsync -a`) don't keep.
fn placed_at(metadata: &std::fs::Metadata) -> Option<SystemTime> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let seconds = u64::try_from(metadata.ctime()).ok()?;
        let nanos = u32::try_from(metadata.ctime_nsec()).ok()?;
        Some(UNIX_EPOCH + Duration::new(seconds, nanos))
    }
    #[cfg(not(unix))]
    {
        metadata.modified().ok()
    }
}

/// Removes the stored copies in `store_dir` that no row names, once they have been there for
/// `min_age`.
async fn remove_unreferenced_render_assets(
    db: &SqlitePool,
    store_dir: &Path,
    min_age: Duration,
) -> Result<(), Error> {
    let referenced: HashSet<String> = sqlx::query_scalar("SELECT DISTINCT hash FROM render_asset")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?
        .into_iter()
        .collect();
    let Ok(entries) = read_dir(store_dir) else {
        return Ok(());
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let unreferenced = entry
            .file_name()
            .to_str()
            // Not one being written (see `store_render_asset`)
            .is_some_and(|name| {
                !referenced.contains(name)
                    && Path::new(name).extension().is_none_or(|ext| ext != "tmp")
            });
        let old_enough = || {
            entry
                .metadata()
                .ok()
                .and_then(|metadata| placed_at(&metadata))
                .is_none_or(|at| now.duration_since(at).unwrap_or_default() >= min_age)
        };
        if unreferenced && old_enough() {
            let _ = remove_file(entry.path());
        }
    }
    Ok(())
}

/// Files and Typst packages read by notes, as `render_asset` names files (by path) and
/// `render_package` packages (by spec).
#[derive(Debug, Default)]
pub struct RenderReads {
    pub files: HashSet<String>,
    pub packages: HashSet<String>,
}

/// What the notes here read (see [`RenderReads`]): what rendering recorded for the notes of
/// parsers that [render in the browser](Parseable::renders_in_browser), and every note's audio
/// clips. The second value is false if a note of such a parser has no recorded render, so what it
/// reads isn't known: it failed to render, or hasn't rendered here since `render_output` came, or
/// this machine doesn't render them at all (see `render_browser_parsers`).
async fn read_by_notes(db: &SqlitePool) -> Result<(RenderReads, bool), Error> {
    let mut reads = RenderReads::default();
    let mut complete = true;
    for parser in get_all_parsers().into_iter().map(|parser| parser()) {
        let as_keys = |paths: BTreeSet<PathBuf>| {
            paths
                .into_iter()
                .filter_map(|path| path.to_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        if parser.renders_in_browser() {
            let (files, packages) =
                read_render_dependencies::<RandomState>(db, parser.get_parser_name(), None).await?;
            reads.files.extend(as_keys(files));
            reads.packages.extend(packages);
            let unrecorded: bool = sqlx::query_scalar(
                r"SELECT EXISTS (SELECT 1 FROM note n JOIN parser p ON p.id = n.parser_id
                WHERE p.name = ? AND NOT EXISTS (SELECT 1 FROM render_output r WHERE r.note_id = n.id))",
            )
            .bind(parser.get_parser_name())
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
            complete &= !unrecorded;
        }
        reads.files.extend(as_keys(
            audio_clips::<RandomState>(db, parser.as_ref(), None).await?,
        ));
    }
    Ok((reads, complete))
}

/// Removes the `render_asset` and `render_package` rows of files and packages no note reads any
/// more, then the stored copies no row names, such as the older copies of a file that changed
/// (with `orphaned`, which says a row's hash changed, even if no row is removed), once they have
/// been there a while (see [`UNREFERENCED_GRACE`]).
///
/// `pushed` is what a host sent with its push, which it found rendering its notes (see
/// `crate::sync::server::push`); those rows stay. Without it, rows are removed only if what every
/// note reads is known here, since rows this machine can't tell are read come from another's
/// renders. Returns how many rows were removed.
pub async fn remove_unread_render_assets(
    db: &SqlitePool,
    pushed: Option<&RenderReads>,
    orphaned: bool,
) -> Result<usize, Error> {
    let removed = remove_unread_rows(db, pushed).await?;
    if orphaned || removed > 0 {
        remove_unreferenced_render_assets(db, &get_render_assets_dir(), UNREFERENCED_GRACE).await?;
    }
    Ok(removed)
}

/// The rows half of [`remove_unread_render_assets`].
async fn remove_unread_rows(db: &SqlitePool, pushed: Option<&RenderReads>) -> Result<usize, Error> {
    let sqlx_error = |e| Error::Sqlx { source: e };
    let not_pushed = |values: Vec<String>, pushed_values: fn(&RenderReads) -> &HashSet<String>| {
        values
            .into_iter()
            .filter(|value| pushed.is_none_or(|reads| !pushed_values(reads).contains(value)))
            .collect::<Vec<_>>()
    };
    let files: Vec<String> = sqlx::query_scalar("SELECT path FROM render_asset")
        .fetch_all(db)
        .await
        .map_err(sqlx_error)?;
    let packages: Vec<String> = sqlx::query_scalar("SELECT spec FROM render_package")
        .fetch_all(db)
        .await
        .map_err(sqlx_error)?;
    let files = not_pushed(files, |reads| &reads.files);
    let packages = not_pushed(packages, |reads| &reads.packages);
    if files.is_empty() && packages.is_empty() {
        return Ok(0);
    }
    let (read, complete) = read_by_notes(db).await?;
    if !complete && pushed.is_none() {
        return Ok(0);
    }
    let mut removed = 0;
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    for path in files.iter().filter(|path| !read.files.contains(*path)) {
        sqlx::query("DELETE FROM render_asset WHERE path = ?")
            .bind(path)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
        removed += 1;
    }
    for spec in packages
        .iter()
        .filter(|spec| !read.packages.contains(*spec))
    {
        sqlx::query("DELETE FROM render_package WHERE spec = ?")
            .bind(spec)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_error)?;
    }
    tx.commit().await.map_err(sqlx_error)?;
    Ok(removed)
}

/// The stored copy of the file a source refers to as `path`, and its hash.
///
/// `spares sync cloud` doesn't copy the stored copies of media (images, audio and image
/// occlusions), since the sync gives the server the files themselves. The first time one is
/// asked for, it is stored from the media file at its path, if that has the recorded contents.
pub async fn get_render_asset(
    db: &SqlitePool,
    path: &str,
) -> Result<Option<(PathBuf, String)>, Error> {
    let hash: Option<String> = sqlx::query_scalar("SELECT hash FROM render_asset WHERE path = ?")
        .bind(path)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let Some(hash) = hash else {
        return Ok(None);
    };
    let store_dir = get_render_assets_dir();
    let stored = store_dir.join(&hash);
    if !stored.exists() {
        store_from_media(Path::new(path), &hash, &store_dir);
    }
    Ok(Some((stored, hash)))
}

/// Stores the file at `path` as the render asset `hash`, if it is in the media directory and
/// has that hash. Any other file stays unread, so a request can't learn about it.
fn store_from_media(path: &Path, hash: &str, store_dir: &Path) {
    let media = crate::config::get_media_dir();
    let in_media = path
        .canonicalize()
        .ok()
        .zip(media.canonicalize().ok())
        .is_some_and(|(path, media)| path.starts_with(media));
    if !in_media || create_dir_all(store_dir).is_err() {
        return;
    }
    match std::fs::read(path) {
        Ok(contents) if blake3::hash(&contents).to_hex().as_str() == hash => {
            if let Err(e) = store_render_asset(path, store_dir) {
                warn!("Failed to store {} as a render asset: {e}", path.display());
            }
        }
        Ok(_) => warn!(
            "{} differs from the render asset recorded for it",
            path.display()
        ),
        Err(_) => {}
    }
}

/// Stored Typst files, e.g. preambles. Most sources read the same few, so web clients load them
/// ahead of compiling rather than discover each by a failed compile.
pub async fn list_render_sources(db: &SqlitePool) -> Result<Vec<String>, Error> {
    sqlx::query_scalar("SELECT path FROM render_asset WHERE path LIKE '%.typ' ORDER BY path")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

/// Package specs (`namespace/name/version`) that rendered files have imported.
pub async fn list_render_packages(db: &SqlitePool) -> Result<Vec<String>, Error> {
    sqlx::query_scalar("SELECT spec FROM render_package ORDER BY spec")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

#[cfg(test)]
mod tests {

    use sqlx::sqlite::SqlitePool;

    use super::get_render_asset;
    use super::get_render_assets_dir;

    /// `spares sync cloud` doesn't copy render assets of media, which the server stores from its
    /// own media files when asked for them, but no other file.
    #[sqlx::test]
    async fn render_assets_of_media_are_stored_when_asked_for(pool: SqlitePool) {
        let unique = format!("{}-{}", std::process::id(), line!());
        let media = crate::config::get_media_dir().join("images");
        std::fs::create_dir_all(&media).unwrap();
        let outside = std::env::temp_dir().join(format!("spares-outside-{unique}"));
        let mut paths = Vec::new();
        for (path, contents) in [
            (media.join(format!("in-media-{unique}.png")), "in media"),
            (media.join(format!("changed-{unique}.png")), "changed"),
            (outside.join("figure.png"), "outside"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{contents} {unique}")).unwrap();
            let hash = blake3::hash(format!("{contents} {unique}").as_bytes())
                .to_hex()
                .to_string();
            sqlx::query("INSERT INTO render_asset (path, hash, mtime, size) VALUES (?, ?, 0, 0)")
                .bind(path.to_str().unwrap())
                .bind(&hash)
                .execute(&pool)
                .await
                .unwrap();
            paths.push((path, hash));
        }
        std::fs::write(&paths[1].0, "edited since").unwrap();

        let stored = |i: usize| get_render_assets_dir().join(&paths[i].1).exists();
        for (path, hash) in &paths {
            let (stored_path, found) = get_render_asset(&pool, path.to_str().unwrap())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&found, hash);
            assert_eq!(stored_path, get_render_assets_dir().join(hash));
        }
        assert!(stored(0));
        assert!(!stored(1), "not with other contents");
        assert!(!stored(2), "not from outside the media directory");
        assert_eq!(get_render_asset(&pool, "/nowhere.png").await.unwrap(), None);

        for (path, hash) in &paths {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(get_render_assets_dir().join(hash));
        }
        let _ = std::fs::remove_dir_all(outside);
    }

    /// Audio clips are read from the notes, of the requested notes only.
    #[sqlx::test]
    async fn audio_clips_are_read_from_the_notes(pool: SqlitePool) {
        use std::collections::HashSet;

        use chrono::Utc;

        use super::audio_clips;
        use crate::api::note::create_notes;
        use crate::api::parser::tests::create_parser_helper;
        use crate::model::NoteId;
        use crate::parsers::get_all_parsers;
        use crate::parsers::impls::markdown::MarkdownParser;
        use crate::schema::note::CreateNoteRequest;
        use crate::schema::note::CreateNotesRequest;

        let parser = create_parser_helper(&pool, "markdown").await;
        let note = |data: &str| CreateNoteRequest {
            data: data.to_string(),
            keywords: Vec::new(),
            tags: Vec::new(),
            is_suspended: false,
            custom_data: serde_json::Map::new(),
        };
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![
                note("{{Paris}} [🔊](/m/b.mp3) [🔊](rel.mp3) {{ [🔊](/m/a.ogg) }}"),
                note("{{Berlin}} [🔊](/m/c.mp3)"),
            ],
        };
        create_notes(&pool, request, Utc::now(), &get_all_parsers(), false)
            .await
            .unwrap();
        let ids: Vec<NoteId> = sqlx::query_scalar("SELECT id FROM note ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();

        let clips = async |ids: Option<&HashSet<NoteId>>| {
            audio_clips(&pool, &MarkdownParser::new(), ids)
                .await
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            clips(None).await,
            ["/m/a.ogg", "/m/b.mp3", "/m/c.mp3"].map(std::path::PathBuf::from)
        );
        assert_eq!(
            clips(Some(&HashSet::from([ids[1]]))).await,
            [std::path::PathBuf::from("/m/c.mp3")]
        );
        assert_eq!(
            clips(Some(&HashSet::new())).await,
            Vec::<std::path::PathBuf>::new()
        );
    }

    /// A Markdown card's sources hide the card's own cloze on the front. A server leaves them out
    /// of its answer, since it renders the card.
    #[sqlx::test]
    async fn test_markdown_card_sources(pool: SqlitePool) {
        use chrono::Utc;

        use super::card_sources;
        use super::load_review_card;
        use crate::api::note::create_notes;
        use crate::api::parser::tests::create_parser_helper;
        use crate::api::review::get_review_card_by_id;
        use crate::model::CardId;
        use crate::parsers::get_all_parsers;
        use crate::parsers::impls::markdown::MarkdownParser;
        use crate::schema::note::CreateNoteRequest;
        use crate::schema::note::CreateNotesRequest;

        let parser = create_parser_helper(&pool, "markdown").await;
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "{{Paris}} is in France and {{Berlin}} in Germany.".to_string(),
                keywords: Vec::new(),
                tags: Vec::new(),
                is_suspended: false,
                custom_data: serde_json::Map::new(),
            }],
        };
        let now = Utc::now();
        create_notes(&pool, request, now, &get_all_parsers(), false)
            .await
            .unwrap();
        let card_id: CardId = sqlx::query_scalar(r#"SELECT id FROM card WHERE "order" = 1"#)
            .fetch_one(&pool)
            .await
            .unwrap();
        let response = get_review_card_by_id(&pool, card_id, None, now, &get_all_parsers())
            .await
            .unwrap()
            .unwrap();
        assert!(response.device_sources.is_none());

        let parser = MarkdownParser::new();
        let (request, card) = load_review_card(&pool, &parser, &response).await.unwrap();
        let sources = card_sources(&parser, &response, &request, &card).unwrap();
        assert!(
            sources
                .card_front
                .contains("[card-1 _____]{.mark} is in France")
        );
        assert!(!sources.card_front.contains("Paris"));
        // The card's back is its note, by default, with its clozes as their text
        assert!(sources.card_back.contains(r"\[Paris\] is in France"));
    }

    /// Rows no note reads go, but only once what every note reads is known, or with what a host
    /// pushed, which stays.
    #[sqlx::test]
    async fn render_assets_no_note_reads_are_removed(pool: SqlitePool) {
        use std::collections::HashSet;

        use super::RenderReads;
        use super::remove_unread_rows;
        use crate::api::parser::tests::create_parser_helper;

        let markdown = create_parser_helper(&pool, "markdown").await;
        let typst = create_parser_helper(&pool, "typst").await;
        for (data, parser) in [
            ("{{Paris}} [🔊](/m/a.ogg)", markdown.id),
            ("= Typst", typst.id),
        ] {
            sqlx::query("INSERT INTO note (data, custom_data, parser_id) VALUES (?, '{}', ?)")
                .bind(data)
                .bind(parser)
                .execute(&pool)
                .await
                .unwrap();
        }
        let typst_note: i64 = sqlx::query_scalar("SELECT id FROM note WHERE parser_id = ?")
            .bind(typst.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let add = async |paths: &[&str], packages: &[&str]| {
            for path in paths {
                sqlx::query(
                    "INSERT INTO render_asset (path, hash, mtime, size) VALUES (?, 'h', 0, 0)",
                )
                .bind(path)
                .execute(&pool)
                .await
                .unwrap();
            }
            for spec in packages {
                sqlx::query("INSERT INTO render_package (spec) VALUES (?)")
                    .bind(spec)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };
        let rows = async || {
            let paths: Vec<String> =
                sqlx::query_scalar("SELECT path FROM render_asset ORDER BY path")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            let specs: Vec<String> =
                sqlx::query_scalar("SELECT spec FROM render_package ORDER BY spec")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            (paths, specs)
        };
        add(
            &["/fig/old.png", "/fig/used.png", "/m/a.ogg"],
            &["preview/old/0.1.0", "preview/used/0.1.0"],
        )
        .await;

        // The Typst note hasn't rendered here, so what it reads isn't known
        assert_eq!(remove_unread_rows(&pool, None).await.unwrap(), 0);
        assert_eq!(rows().await.0.len(), 3);

        sqlx::query(
            "INSERT INTO render_output (note_id, output, key, dependencies) VALUES (?, '', 'k', ?)",
        )
        .bind(typst_note)
        .bind(r#"{"files": ["/fig/used.png"], "packages": ["preview/used/0.1.0"]}"#)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(remove_unread_rows(&pool, None).await.unwrap(), 1);
        assert_eq!(
            rows().await,
            (
                vec!["/fig/used.png".to_string(), "/m/a.ogg".to_string()],
                vec!["preview/used/0.1.0".to_string()]
            )
        );

        // On a server, what a host pushed stays, though no note here is known to read it
        add(&["/fig/pushed.png"], &["preview/pushed/0.1.0"]).await;
        let pushed = RenderReads {
            files: HashSet::from(["/fig/pushed.png".to_string()]),
            packages: HashSet::from(["preview/pushed/0.1.0".to_string()]),
        };
        assert_eq!(remove_unread_rows(&pool, Some(&pushed)).await.unwrap(), 0);
        assert_eq!(rows().await.0.len(), 3);
        // Even if what the notes here read isn't known
        sqlx::query("DELETE FROM render_output")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            remove_unread_rows(&pool, Some(&RenderReads::default()))
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            rows().await,
            (vec!["/m/a.ogg".to_string()], Vec::new()),
            "the audio clip is read by its note"
        );
    }

    /// Stored copies no row names are removed once they have been there a while, but not one
    /// being written.
    #[sqlx::test]
    async fn stored_copies_no_row_names_are_removed(pool: SqlitePool) {
        let store = std::env::temp_dir().join(format!(
            "spares-render-assets-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&store);
        std::fs::create_dir_all(&store).unwrap();
        for name in ["named", "unnamed", "writing.tmp"] {
            std::fs::write(store.join(name), name).unwrap();
        }
        sqlx::query(
            "INSERT INTO render_asset (path, hash, mtime, size) VALUES ('/a', 'named', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let left = async |min_age| {
            super::remove_unreferenced_render_assets(&pool, &store, min_age)
                .await
                .unwrap();
            let mut left: Vec<String> = std::fs::read_dir(&store)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            left.sort();
            left
        };
        // Not yet: it may be a copy whose row is on its way
        assert_eq!(
            left(super::UNREFERENCED_GRACE).await,
            ["named", "unnamed", "writing.tmp"]
        );
        assert_eq!(
            left(std::time::Duration::ZERO).await,
            ["named", "writing.tmp"]
        );
        std::fs::remove_dir_all(&store).unwrap();
    }
}
