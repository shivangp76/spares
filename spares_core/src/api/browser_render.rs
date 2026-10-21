//! Support for parsers whose cards web clients compile themselves (see
//! [`Parseable::renders_in_browser`]), so the server only needs the database and the files the
//! sources read, not a raw and rendered file per card.
//!
//! - The raw sources are generated from the database on demand ([`add_browser_render_sources`]).
//! - Files the sources read (preambles, figures, rendered image occlusions, ...) are discovered
//!   when rendering locally. Rendering writes a [`RenderDependencies`] file next to each rendered
//!   file, and [`sync_render_assets`] copies the files it lists into a content-addressed store in
//!   the data directory, which is synced to the server along with the database. Files are stored
//!   by the absolute path sources read them by, except rendered image occlusions (see
//!   [`IMAGE_OCCLUSION_ASSET_PREFIX`]).
//! - Audio clips (see [`crate::parsers::audio`]) are stored the same way for every parser, since
//!   web clients play them outside the rendered card.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::create_dir_all;
use std::fs::read_dir;
use std::fs::read_to_string;
use std::fs::remove_file;
use std::fs::rename;
use std::fs::write;
use std::hash::BuildHasher;
use std::path::Path;
use std::path::PathBuf;
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
use crate::api::review::relative_to_files_dir;
use crate::api::review::web_render_path;
use crate::config::get_data_dir;
use crate::model::NoteId;
use crate::parsers::CardData;
use crate::parsers::Parseable;
use crate::parsers::RenderDependencies;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::TemplateType;
use crate::parsers::audio::is_audio_path;
use crate::parsers::find_parser;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::generate_files::construct_card_file_contents;
use crate::parsers::generate_files::construct_note_file_contents;
use crate::parsers::get_cards;
use crate::parsers::get_image_occlusion_rendered_directory;
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

/// Prefix of the `render_asset` keys of rendered image occlusions, followed by their path in
/// [`get_image_occlusion_rendered_directory`]. Card sources read them in that directory, which is
/// in the cache directory of whichever machine generated the source: the client's when it rendered
/// the card, the server's when a web client compiles it. So they are stored by a path that is the
/// same on both.
pub const IMAGE_OCCLUSION_ASSET_PREFIX: &str = "@image-occlusions/";

/// The `render_asset` key of the file at `path`, or `None` if `path` is not UTF-8.
fn render_asset_key(path: &Path) -> Option<String> {
    match path.strip_prefix(get_image_occlusion_rendered_directory()) {
        Ok(relative) => Some(format!(
            "{IMAGE_OCCLUSION_ASSET_PREFIX}{}",
            relative.to_str()?
        )),
        Err(_) => path.to_str().map(str::to_owned),
    }
}

/// Where the file stored as `key` is on this machine (see [`render_asset_key`]).
fn render_asset_local_path(key: &str) -> PathBuf {
    match key.strip_prefix(IMAGE_OCCLUSION_ASSET_PREFIX) {
        Some(relative) => get_image_occlusion_rendered_directory().join(relative),
        None => PathBuf::from(key),
    }
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
            construct_note_file_contents(parser, &note_template, &body_placeholder, request)
        }
    };
    response.browser_sources = Some(BrowserRenderSources {
        card_front: card_side(CardSide::Front),
        card_back,
    });
    Ok(())
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

/// Records the dependencies of `parser`'s rendered files for `note_ids` (all notes if `None`) and
/// refreshes every stored asset whose file changed since it was last stored. For parsers that do
/// not [render in the browser](Parseable::renders_in_browser), only audio clips are recorded.
///
/// Files that do not exist locally keep their stored copy, so this is safe to run on a server that
/// only has the synced store.
pub async fn sync_render_assets<S: BuildHasher>(
    db: &SqlitePool,
    parser: &dyn Parseable,
    note_ids: Option<&HashSet<NoteId, S>>,
) -> Result<(), Error> {
    let (mut files, mut packages) = read_render_dependencies(parser, note_ids);
    // Other parsers' cards are shown rendered, so web clients only need their audio clips
    if !parser.renders_in_browser() {
        files.retain(|path| is_audio_path(path));
        packages.clear();
        if files.is_empty() {
            return Ok(());
        }
    }

    let existing: Vec<RenderAssetRow> =
        sqlx::query_as("SELECT path, hash, mtime, size FROM render_asset")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let mut changed = false;
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;
    let mut by_key: HashMap<String, RenderAssetRow> = HashMap::with_capacity(existing.len());
    for mut row in existing {
        // Rendered image occlusions were stored by their absolute path before
        // `IMAGE_OCCLUSION_ASSET_PREFIX`, which web clients asking the server cannot find.
        if let Some(key) = render_asset_key(Path::new(&row.path))
            && key != row.path
        {
            sqlx::query("UPDATE OR REPLACE render_asset SET path = ? WHERE path = ?")
                .bind(&key)
                .bind(&row.path)
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            row.path = key;
        }
        by_key.insert(row.path.clone(), row);
    }
    let existing = by_key;
    let paths: BTreeMap<String, PathBuf> = files
        .iter()
        .filter_map(|path| Some((render_asset_key(path)?, path.clone())))
        .chain(
            existing
                .keys()
                .map(|key| (key.clone(), render_asset_local_path(key))),
        )
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

    if changed {
        remove_unreferenced_render_assets(db, &store_dir).await?;
    }
    Ok(())
}

/// Collects the dependency files written next to `parser`'s rendered files of `note_ids` (all
/// notes if `None`).
pub fn read_render_dependencies<S: BuildHasher>(
    parser: &dyn Parseable,
    note_ids: Option<&HashSet<NoteId, S>>,
) -> (BTreeSet<PathBuf>, BTreeSet<String>) {
    let mut files = BTreeSet::new();
    let mut packages = BTreeSet::new();
    let rendered_dirs: BTreeSet<PathBuf> = [
        RenderOutputDirectoryType::Note,
        RenderOutputDirectoryType::Card,
    ]
    .into_iter()
    .map(|t| parser.get_output_rendered_dir(t))
    .collect();
    for dir in rendered_dirs {
        let Ok(entries) = read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !file_name.ends_with(".deps.json") {
                continue;
            }
            // Rendered files are named `<note id>.<ext>` or `<note id>-<order>-<side>.<ext>`.
            if let Some(note_ids) = note_ids {
                let note_id = file_name
                    .split(['-', '.'])
                    .next()
                    .and_then(|id| id.parse::<NoteId>().ok());
                if !note_id.is_some_and(|id| note_ids.contains(&id)) {
                    continue;
                }
            }
            let path = entry.path();
            match read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|s| {
                    serde_json::from_str::<RenderDependencies>(&s).map_err(|e| e.to_string())
                }) {
                Ok(dependencies) => {
                    files.extend(dependencies.files);
                    packages.extend(dependencies.packages);
                }
                Err(e) => warn!("Failed to read {}: {e}", path.display()),
            }
        }
    }
    (files, packages)
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

async fn remove_unreferenced_render_assets(db: &SqlitePool, store_dir: &Path) -> Result<(), Error> {
    let referenced: HashSet<String> = sqlx::query_scalar("SELECT DISTINCT hash FROM render_asset")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?
        .into_iter()
        .collect();
    let Ok(entries) = read_dir(store_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| !referenced.contains(name))
        {
            let _ = remove_file(entry.path());
        }
    }
    Ok(())
}

/// The stored copy of the file a source refers to as `path`, and its hash.
pub async fn get_render_asset(
    db: &SqlitePool,
    path: &str,
) -> Result<Option<(PathBuf, String)>, Error> {
    let key = render_asset_key(Path::new(path));
    let hash: Option<String> = sqlx::query_scalar("SELECT hash FROM render_asset WHERE path = ?")
        .bind(key.as_deref().unwrap_or(path))
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(hash.map(|hash| (get_render_assets_dir().join(&hash), hash)))
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
    use std::collections::HashSet;
    use std::path::Path;

    use sqlx::sqlite::SqlitePool;

    use super::IMAGE_OCCLUSION_ASSET_PREFIX;
    use super::get_render_asset;
    use super::render_asset_key;
    use super::render_asset_local_path;
    use super::sync_render_assets;
    use crate::model::NoteId;
    use crate::parsers::get_image_occlusion_rendered_directory;
    use crate::parsers::impls::typst::TypstParser;

    #[test]
    fn test_render_asset_key() {
        let image = get_image_occlusion_rendered_directory().join("abc-1-front-io-1.png");
        let key = render_asset_key(&image).unwrap();
        assert_eq!(
            key,
            format!("{IMAGE_OCCLUSION_ASSET_PREFIX}abc-1-front-io-1.png")
        );
        assert_eq!(render_asset_local_path(&key), image);

        let preamble = Path::new("/home/me/preamble.typ");
        assert_eq!(render_asset_key(preamble).unwrap(), "/home/me/preamble.typ");
        assert_eq!(render_asset_local_path("/home/me/preamble.typ"), preamble);
    }

    /// Rendered image occlusions stored by their absolute path are keyed again, so a server whose
    /// image occlusion directory differs finds them.
    #[sqlx::test]
    async fn test_sync_rekeys_image_occlusions(pool: SqlitePool) {
        let image = get_image_occlusion_rendered_directory().join("abc-1-front-io-1.png");
        for (path, hash) in [
            (image.to_str().unwrap(), "h1"),
            ("/home/me/preamble.typ", "h2"),
        ] {
            sqlx::query("INSERT INTO render_asset (path, hash, mtime, size) VALUES (?, ?, 0, 0)")
                .bind(path)
                .bind(hash)
                .execute(&pool)
                .await
                .unwrap();
        }

        // No notes, so only the stored rows are looked at. Neither file exists here.
        sync_render_assets(&pool, &TypstParser::new(), Some(&HashSet::<NoteId>::new()))
            .await
            .unwrap();

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT path, hash FROM render_asset ORDER BY path")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            [
                ("/home/me/preamble.typ".to_string(), "h2".to_string()),
                (
                    format!("{IMAGE_OCCLUSION_ASSET_PREFIX}abc-1-front-io-1.png"),
                    "h1".to_string()
                ),
            ]
        );
        let hash = |path: String| {
            let pool = pool.clone();
            async move {
                get_render_asset(&pool, &path)
                    .await
                    .unwrap()
                    .map(|(_, h)| h)
            }
        };
        assert_eq!(
            hash(image.to_str().unwrap().to_string()).await.as_deref(),
            Some("h1")
        );
        assert_eq!(
            hash("/home/me/preamble.typ".to_string()).await.as_deref(),
            Some("h2")
        );
        assert_eq!(
            hash("/elsewhere/abc-1-front-io-1.png".to_string()).await,
            None
        );
    }
}
