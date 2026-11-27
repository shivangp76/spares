//! Files in the cache that rendering made for notes and cards that are gone.
//!
//! Deleting a note deletes its files, and rendering a note deletes those of the cards it lost (see
//! [`delete_output_files`]). Files a process stopped before deleting, or that predate either, are
//! found by their names and removed in the background once per process when a collection opens
//! ([`prune_in_background`]).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(not(target_family = "wasm"))]
use std::time::Duration;
use std::time::SystemTime;

use sqlx::SqlitePool;

use super::delete_note_files;
use crate::Error;
use crate::model::NoteId;
use crate::parsers::Parseable;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::get_all_parsers;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::image_occlusion_rendered_path;

/// The note and, for a card's file, the card order that a file of the cache was made for, from
/// its name: `<note>[-<order>-<front|back>[-io-<n>]].<extension>` (see
/// [`Parseable::get_output_filename`]). `None` for other files, including the temporary ones a
/// render leaves beside a file (`<name>.<extension>.<suffix>`).
fn parse_output_file_name(name: &str) -> Option<(NoteId, Option<usize>)> {
    let (stem, extension) = name.split_once('.')?;
    if extension.is_empty() || !extension.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let mut parts = stem.split('-');
    let letters = parts.next()?;
    if letters.is_empty() || !letters.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let note_id = NoteId::from_letters(letters).ok()?;
    let Some(order) = parts.next() else {
        return Some((note_id, None));
    };
    if !order.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let order = order.parse().ok()?;
    if !matches!(parts.next(), Some("front" | "back")) {
        return None;
    }
    match (parts.next(), parts.next(), parts.next()) {
        (None, ..) => {}
        (Some("io"), Some(n), None) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {}
        _ => return None,
    }
    Some((note_id, Some(order)))
}

/// The directories with `parser`'s card files: their raw files, and the rendered ones.
fn card_dirs(parser: &dyn Parseable) -> [PathBuf; 2] {
    [
        get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(1, CardSide::Front),
            None,
        ),
        parser.get_output_rendered_dir(RenderOutputDirectoryType::Card),
    ]
}

/// The orders of the cards of `note_ids` that have files of `parser` on disk.
pub(crate) fn card_orders_on_disk(
    parser: &dyn Parseable,
    note_ids: &BTreeSet<NoteId>,
) -> BTreeMap<NoteId, BTreeSet<usize>> {
    let mut orders: BTreeMap<NoteId, BTreeSet<usize>> = BTreeMap::new();
    if note_ids.is_empty() {
        return orders;
    }
    for dir in card_dirs(parser) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some((note_id, Some(order))) =
                entry.file_name().to_str().and_then(parse_output_file_name)
                && note_ids.contains(&note_id)
            {
                orders.entry(note_id).or_default().insert(order);
            }
        }
    }
    orders
}

/// Deletes the files of those of `note_ids` that are no longer in the database, which a sync or
/// undo removed without them. Their parser may be gone with them, so every parser's are deleted.
pub async fn delete_files_of_deleted_notes(
    db: &SqlitePool,
    note_ids: &BTreeSet<NoteId>,
) -> Result<(), Error> {
    if note_ids.is_empty() {
        return Ok(());
    }
    let ids = serde_json::to_string(note_ids).unwrap_or_default();
    let existing: Vec<NoteId> =
        sqlx::query_scalar("SELECT id FROM note WHERE id IN (SELECT value FROM json_each(?))")
            .bind(ids)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let mut deleted = note_ids.clone();
    for id in existing {
        deleted.remove(&id);
    }
    for parser in get_all_parsers().iter().map(|p| p()) {
        let orders = card_orders_on_disk(parser.as_ref(), &deleted);
        for &note_id in &deleted {
            let orders: Vec<usize> = orders
                .get(&note_id)
                .map(|orders| orders.iter().copied().collect())
                .unwrap_or_default();
            delete_note_files(parser.as_ref(), note_id, &orders)?;
        }
    }
    Ok(())
}

/// How long a file is left alone after it was written, since a render may have written it for a
/// note it hasn't stored yet.
#[cfg(not(target_family = "wasm"))]
const GRACE: Duration = Duration::from_mins(10);

/// Runs [`prune_stale_files`] in the background, the first time a process opens the current
/// collection.
#[cfg(not(target_family = "wasm"))]
pub fn prune_in_background(db: SqlitePool) {
    static PRUNED: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());
    if !crate::collection::lock(&PRUNED).insert(crate::collection::paths().cache.clone()) {
        return;
    }
    crate::collection::spawn(async move {
        let older_than = SystemTime::now() - GRACE;
        match prune_stale_files(&db, older_than).await {
            Ok(0) => {}
            Ok(count) => log::info!("Removed {count} files of notes and cards that are gone"),
            Err(e) => log::warn!("Failed to remove files of notes and cards that are gone: {e}"),
        }
    });
}

/// Removes the files in the cache that rendering made for notes that are gone, or for cards past
/// their note's last, of those modified before `older_than`. Returns how many it removed.
pub async fn prune_stale_files(db: &SqlitePool, older_than: SystemTime) -> Result<usize, Error> {
    let rows: Vec<(NoteId, i64)> = sqlx::query_as(
        r#"SELECT n.id, COALESCE(MAX(c."order"), 0) FROM note n
           LEFT JOIN card c ON c.note_id = n.id GROUP BY n.id"#,
    )
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let last_card: HashMap<NoteId, usize> = rows
        .into_iter()
        .map(|(id, last)| (id, usize::try_from(last).unwrap_or(0)))
        .collect();
    crate::collection::run_blocking(move || remove_stale_files(&last_card, older_than)).await
}

/// See [`prune_stale_files`]; `last_card` has every note, with the order of its last card.
fn remove_stale_files(
    last_card: &HashMap<NoteId, usize>,
    older_than: SystemTime,
) -> Result<usize, Error> {
    let mut dirs = BTreeSet::new();
    // What rendering writes: raw files, rendered ones with their HTML, and image occlusions. Only
    // those are removed, in case the directories hold anything else.
    let mut extensions = BTreeSet::from(["pdf", "html", "png", "svg"]);
    for parser in get_all_parsers().iter().map(|p| p()) {
        dirs.extend(card_dirs(parser.as_ref()));
        dirs.insert(parser.get_output_rendered_dir(RenderOutputDirectoryType::Note));
        extensions.insert(parser.file_extension());
    }
    // Only looked in, so not made: a collection with no image occlusions has no such directory
    dirs.insert(image_occlusion_rendered_path());
    let mut removed = 0;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some((note_id, card)) = parse_output_file_name(name) else {
                continue;
            };
            if !name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extensions.contains(extension))
            {
                continue;
            }
            let stale = match last_card.get(&note_id) {
                None => true,
                Some(&last) => card.is_some_and(|order| order > last),
            };
            let old = || {
                entry.metadata().is_ok_and(|metadata| {
                    metadata.is_file() && metadata.modified().is_ok_and(|time| time < older_than)
                })
            };
            if !stale || !old() {
                continue;
            }
            let path = entry.path();
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    crate::sync::media::rendered_path_changed(&path);
                    removed += 1;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(Error::Io {
                        source: e,
                        description: format!("Failed to remove {}", path.display()),
                    });
                }
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use chrono::Utc;

    use super::*;
    use crate::api::note::create_notes;
    use crate::collection::Collection;
    use crate::collection::CollectionPaths;
    use crate::parsers::image_occlusion::get_image_occlusion_rendered_directory;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;

    #[test]
    fn output_file_names() {
        let id = |letters| Some(NoteId::from_letters(letters).unwrap());
        let cases = [
            ("abc.pdf", id("abc").map(|id| (id, None))),
            ("abc.html", id("abc").map(|id| (id, None))),
            ("abc-1-front.typ", id("abc").map(|id| (id, Some(1)))),
            ("abc-12-back.pdf", id("abc").map(|id| (id, Some(12)))),
            ("abc-2-back-io-3.png", id("abc").map(|id| (id, Some(2)))),
            ("abc-1-front.pdf.before", None),
            ("abc-1-front.pdf.reuse.tmp", None),
            ("abc-1-side.pdf", None),
            ("abc-x-front.pdf", None),
            ("abc-1-front-io-.png", None),
            ("abc-1-front-extra.pdf", None),
            ("Abc.pdf", None),
            ("card-abc-1.pdf", None),
            ("aux", None),
            (".pdf", None),
        ];
        for (name, expected) in cases {
            assert_eq!(parse_output_file_name(name), expected, "{name}");
        }
    }

    #[test]
    fn test_card_orders_on_disk() {
        let parser = crate::parsers::find_parser("markdown", &get_all_parsers()).unwrap();
        let [note_id, other] =
            ["zzq", "zzqb"].map(|letters| NoteId::from_letters(letters).unwrap());
        let dir = get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(1, CardSide::Front),
            None,
        );
        std::fs::create_dir_all(&dir).unwrap();
        let files = [
            "zzq-1-front.md",
            "zzq-1-back.md",
            "zzq-12-front.md",
            "zzqa-3-front.md",
            "zzqb-2-back.md",
        ];
        for file in files {
            std::fs::write(dir.join(file), "").unwrap();
        }
        assert_eq!(
            card_orders_on_disk(parser.as_ref(), &BTreeSet::from([note_id, other])),
            BTreeMap::from([
                (note_id, BTreeSet::from([1, 12])),
                (other, BTreeSet::from([2]))
            ])
        );
        for file in files {
            std::fs::remove_file(dir.join(file)).unwrap();
        }
    }

    fn write(path: &Path, modified: SystemTime) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    /// The sweep removes the files of deleted notes and of cards past a note's last, and leaves
    /// live files, files newer than its cutoff, and files it doesn't know.
    #[sqlx::test]
    async fn pruning_removes_only_old_files_of_what_is_gone(pool: SqlitePool) {
        let parser_row = crate::api::parser::tests::create_parser_helper(&pool, "markdown").await;
        let root = tempfile::tempdir().unwrap();
        let collection = Arc::new(Collection::new(
            "prune",
            pool.clone(),
            CollectionPaths::in_dir(root.path()),
        ));
        collection.set_replica(Some(crate::config::Replica::ROOT));
        collection
            .scope(async {
                let request = CreateNotesRequest {
                    parser_id: parser_row.id,
                    requests: vec![CreateNoteRequest {
                        data: "{{Paris}} is in France and {{Berlin}} in Germany.".to_string(),
                        keywords: Vec::new(),
                        tags: Vec::new(),
                        is_suspended: false,
                        custom_data: serde_json::Map::new(),
                    }],
                };
                create_notes(&pool, request, Utc::now(), &get_all_parsers(), false)
                    .await
                    .unwrap();
                let live: NoteId = sqlx::query_scalar("SELECT id FROM note")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                let (live, gone) = (live.letters(), "zzzz");
                let paths = crate::collection::paths();
                let raw = paths.cards().join("markdown");
                let rendered = paths.files.clone();
                let occlusions = get_image_occlusion_rendered_directory();
                let old = SystemTime::now() - Duration::from_hours(1);
                let cutoff = SystemTime::now() - Duration::from_mins(10);
                let kept = [
                    raw.join(format!("{live}-1-front.md")),
                    raw.join(format!("{live}-2-back.md")),
                    rendered.join(format!("{live}.pdf")),
                    rendered.join(format!("{live}-2-front.html")),
                    rendered.join("notes.txt"),
                    rendered.join(format!("{gone}.log")),
                    rendered.join(format!("{gone}-1-front.pdf.before")),
                ];
                let stale = [
                    raw.join(format!("{live}-3-front.md")),
                    raw.join(format!("{gone}-1-front.md")),
                    rendered.join(format!("{gone}.pdf")),
                    rendered.join(format!("{gone}-1-back.pdf")),
                    rendered.join(format!("{live}-3-back.html")),
                    occlusions.join(format!("{gone}-1-front-io-1.png")),
                ];
                for path in kept.iter().chain(&stale) {
                    write(path, old);
                }
                let recent = rendered.join("new.pdf");
                write(&recent, SystemTime::now());

                assert_eq!(prune_stale_files(&pool, cutoff).await.unwrap(), stale.len());
                for path in kept.iter().chain([&recent]) {
                    assert!(path.exists(), "{}", path.display());
                }
                for path in &stale {
                    assert!(!path.exists(), "{}", path.display());
                }
            })
            .await;
    }
}
