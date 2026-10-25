//! `spares migrate ids`: renumbers a database's notes from the old numeric ids to letter ids, once,
//! before a second replica exists (see [`super`] for the id scheme).
//!
//! Notes keep their order: the oldest gets `aaa`, the next `aab`, and so on through consecutive
//! laptop slots, so slot `z` stays free in every block for the server. The existing notes took
//! `aaa` to `giq`.
//!
//! It rewrites every reference (cards, tags, keywords, links, `inh:` references, tag queries),
//! empties the undo history, regenerates note and card files (with `git mv` in the notes
//! repository, so `git log --follow` works), clears render caches and writes
//! [`super::ID_MAP_FILE_NAME`]. The server refuses to start on an unmigrated database. The Anki
//! collection keeps the old numeric ids, so re-importing from Anki would add those notes as new
//! ones.
//!
//! The database is rewritten in one transaction ([`rewrite_database`]). Files are regenerated from
//! it afterwards ([`regenerate_files`]); `app_state` records that this is still to do, so rerunning
//! the command after a crash finishes it.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use fancy_regex::Captures;
use fancy_regex::Regex;
use sqlx::Connection;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use super::ID_MAP_FILE_NAME;
use super::MAX_PACKED;
use super::MIN_PACKED;
use super::NOTE_IDS_MIGRATED_KEY;
use super::NoteId;
use super::ids_after;
use crate::Error;
use crate::LibraryError;
use crate::api::browser_render::IMAGE_OCCLUSION_ASSET_PREFIX;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::api::note::render_notes;
use crate::config::Replica;
use crate::config::get_cache_dir;
use crate::config::get_data_dir;
use crate::notes_vcs;
use crate::notes_vcs::construct_db_note_file;
use crate::parsers::Parseable;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::get_all_parsers;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::get_image_occlusion_rendered_directory;
use crate::schema::note::NotesSelector;
use crate::schema::note::RenderNotesRequest;

/// `app_state` key present while files still have to be regenerated after the database was
/// migrated.
pub const FILES_PENDING_KEY: &str = "note_ids_migration_files_pending";

/// Tables whose row counts the migration must not change.
const COUNTED_TABLES: [&str; 9] = [
    "note",
    "card",
    "note_tag",
    "note_keyword",
    "note_link",
    "tag",
    "parser",
    "review_log",
    "card_tag",
];

/// A note's old and new id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdMapping {
    pub old: i64,
    pub new: NoteId,
    pub parser_name: String,
}

/// Text in the database that names notes by their old ids, and what it becomes.
#[derive(Clone, Debug)]
pub struct TextRewrite {
    /// e.g. `note aab` or `tag 3`
    pub owner: String,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug)]
pub struct MigrationPlan {
    /// In old id order, which is creation order
    pub mapping: Vec<IdMapping>,
    /// `inh:` references in note data and note ids in filtered tag queries
    pub rewrites: Vec<TextRewrite>,
    /// Rows of `render_asset` for image occlusion images, which are named by note id
    pub image_occlusion_assets: i64,
    pub events: i64,
    /// Rows that name notes which no longer exist, and are deleted (see [`ORPHANS`])
    pub orphans: Vec<(&'static str, i64)>,
}

/// Rows naming notes that no longer exist, which cannot be renumbered. They are left over from
/// deletes made while foreign keys were not enforced, and are deleted with a description of what
/// they were.
const ORPHANS: [(&str, &str); 4] = [
    (
        "tags of deleted notes",
        "FROM note_tag WHERE note_id NOT IN (SELECT id FROM note)",
    ),
    (
        "keywords of deleted notes",
        "FROM note_keyword WHERE note_id NOT IN (SELECT id FROM note)",
    ),
    (
        "links from deleted notes",
        "FROM note_link WHERE parent_note_id NOT IN (SELECT id FROM note)",
    ),
    (
        "links to deleted notes (kept, as unmatched)",
        "FROM note_link WHERE linked_note_id IS NOT NULL AND linked_note_id NOT IN (SELECT id FROM note)",
    ),
];

impl MigrationPlan {
    pub fn last(&self) -> Option<NoteId> {
        self.mapping.last().map(|m| m.new)
    }
}

/// Whether the database's note ids are already letter ids.
pub async fn is_migrated(db: &SqlitePool) -> Result<bool, Error> {
    let flag: Option<String> = sqlx::query_scalar("SELECT value FROM app_state WHERE key = ?")
        .bind(NOTE_IDS_MIGRATED_KEY)
        .fetch_optional(db)
        .await
        .map_err(sqlx_error)?;
    Ok(flag.is_some())
}

/// Whether the database was migrated but its files were not regenerated yet.
pub async fn files_pending(db: &SqlitePool) -> Result<bool, Error> {
    let flag: Option<String> = sqlx::query_scalar("SELECT value FROM app_state WHERE key = ?")
        .bind(FILES_PENDING_KEY)
        .fetch_optional(db)
        .await
        .map_err(sqlx_error)?;
    Ok(flag.is_some())
}

/// What migrating `db` would change. Fails if something names a note in a way that cannot be
/// rewritten, so nothing is changed in that case.
pub async fn plan(db: &SqlitePool) -> Result<MigrationPlan, Error> {
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    plan_on(&mut conn).await
}

#[expect(clippy::too_many_lines)]
async fn plan_on(conn: &mut SqliteConnection) -> Result<MigrationPlan, Error> {
    let notes: Vec<(i64, String)> = sqlx::query_as(
        "SELECT n.id, p.name FROM note n JOIN parser p ON p.id = n.parser_id ORDER BY n.id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    if let Some((id, _)) = notes.iter().find(|(id, _)| *id >= MIN_PACKED) {
        return Err(migrate_error(format!(
            "Note {id} already looks like a letter id ({}), but the database is not marked as \
             migrated. Restore the backup taken before the migration.",
            NoteId::from_packed(*id)
        )));
    }
    if notes.iter().any(|(id, _)| *id <= 0) {
        return Err(migrate_error(
            "The database has a note with an id of 0 or less.".to_string(),
        ));
    }
    let new_ids = ids_after(Replica::new_unchecked(0), None, notes.len())?;
    let mapping: Vec<IdMapping> = notes
        .into_iter()
        .zip(new_ids)
        .map(|((old, parser_name), new)| IdMapping {
            old,
            new,
            parser_name,
        })
        .collect();
    let by_old: HashMap<i64, NoteId> = mapping.iter().map(|m| (m.old, m.new)).collect();

    let mut rewrites = Vec::new();
    let datas: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, data FROM note WHERE data LIKE '%inh:%' ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    for (id, data) in datas {
        let after = rewrite_inh(&data, &by_old).map_err(|old| {
            migrate_error(format!(
                "Note {id} inherits from note {old} with `inh:{old}/…`, which does not exist. \
                 Fix the reference before migrating."
            ))
        })?;
        if after != data {
            rewrites.push(TextRewrite {
                owner: format!("note {}", by_old[&id]),
                before: data,
                after,
            });
        }
    }
    let queries: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, name, query FROM tag WHERE query IS NOT NULL ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    for (id, name, query) in queries {
        let after = rewrite_query(&query, &by_old).map_err(|reason| {
            migrate_error(format!(
                "The query of filtered tag `{name}` ({query}) {reason}. Edit it before migrating."
            ))
        })?;
        if after != query {
            rewrites.push(TextRewrite {
                owner: format!("tag `{name}` ({id})"),
                before: query,
                after,
            });
        }
    }

    let image_occlusion_assets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM render_asset WHERE path LIKE ? || '%' OR path LIKE ? || '%'",
    )
    .bind(image_occlusion_prefix())
    .bind(IMAGE_OCCLUSION_ASSET_PREFIX)
    .fetch_one(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event")
        .fetch_one(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    let orphan_cards: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE note_id NOT IN (SELECT id FROM note)")
            .fetch_one(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    if orphan_cards > 0 {
        return Err(migrate_error(format!(
            "{orphan_cards} cards belong to notes that no longer exist. They hold review history, \
             so they are not deleted automatically."
        )));
    }
    let mut orphans = Vec::new();
    for (what, from) in ORPHANS {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) {from}"))
            .fetch_one(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        if count > 0 {
            orphans.push((what, count));
        }
    }
    Ok(MigrationPlan {
        mapping,
        rewrites,
        image_occlusion_assets,
        events,
        orphans,
    })
}

/// `data` with every `inh:<old id>/` naming the new id, or the first old id that has no new one.
fn rewrite_inh(data: &str, by_old: &HashMap<i64, NoteId>) -> Result<String, i64> {
    let regex = Regex::new(r"(inh:\s*)(\d+)(?=\s*/)").expect("valid regex");
    let mut missing = None;
    let after = regex.replace_all(data, |caps: &Captures<'_, str>| {
        let old: i64 = caps[2].parse().unwrap_or(-1);
        if let Some(new) = by_old.get(&old) {
            format!("{}{}", &caps[1], new.letters())
        } else {
            missing.get_or_insert(old);
            caps[0].to_string()
        }
    });
    missing.map_or_else(|| Ok(after.into_owned()), Err)
}

/// `query` with every `id=<old id>` (and `linked_to_note`, `linked_from_note`) naming the new id.
/// Comparisons other than `=` cannot be translated reliably, so they are refused.
fn rewrite_query(query: &str, by_old: &HashMap<i64, NoteId>) -> Result<String, String> {
    const FIELDS: &str = r"(?<![\w.])(id|linked_to_note|linked_from_note)";
    let ordered = Regex::new(&format!(r"{FIELDS}\s*(<=|>=|<|>|~)\s*\d")).expect("valid regex");
    if let Ok(Some(found)) = ordered.find(query) {
        return Err(format!(
            "compares note ids with numbers (`{}`), which cannot be translated",
            found.as_str()
        ));
    }
    let equal = Regex::new(&format!(r"{FIELDS}(\s*=\s*)(\d+)\b")).expect("valid regex");
    let mut missing = None;
    let after = equal.replace_all(query, |caps: &Captures<'_, str>| {
        let old: i64 = caps[3].parse().unwrap_or(-1);
        if let Some(new) = by_old.get(&old) {
            format!("{}{}{}", &caps[1], &caps[2], new.letters())
        } else {
            missing.get_or_insert(old);
            caps[0].to_string()
        }
    });
    missing.map_or_else(
        || Ok(after.into_owned()),
        |old| Err(format!("names note {old}, which does not exist")),
    )
}

/// The prefix of image occlusion images stored before [`IMAGE_OCCLUSION_ASSET_PREFIX`].
fn image_occlusion_prefix() -> String {
    let mut prefix = get_image_occlusion_rendered_directory()
        .to_string_lossy()
        .into_owned();
    if !prefix.ends_with('/') {
        prefix.push('/');
    }
    prefix
}

/// Where the old → new id map is written.
pub fn id_map_path() -> PathBuf {
    get_data_dir().join(ID_MAP_FILE_NAME)
}

/// Writes the map from old numeric ids to letter ids, for reading old commit messages and anything
/// else that mentions old ids.
pub fn write_id_map(path: &Path, mapping: &[IdMapping]) -> Result<(), Error> {
    let mut contents = String::from("# old_id\tnew_id\tparser\n");
    for m in mapping {
        let _ = writeln!(
            contents,
            "{}\t{}\t{}",
            m.old,
            m.new.letters(),
            m.parser_name
        );
    }
    std::fs::write(path, contents).map_err(|e| io_error(path, e))
}

/// Reads a map written by [`write_id_map`].
pub fn read_id_map(path: &Path) -> Result<Vec<IdMapping>, Error> {
    let contents = std::fs::read_to_string(path).map_err(|e| io_error(path, e))?;
    contents
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let (Some(old), Some(new), Some(parser_name)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Err(migrate_error(format!(
                    "Malformed line in {}: {line}",
                    path.display()
                )));
            };
            Ok(IdMapping {
                old: old.parse().map_err(|_| {
                    migrate_error(format!("Malformed old id in {}: {line}", path.display()))
                })?,
                new: new.parse().map_err(|e| migrate_error(format!("{e}")))?,
                parser_name: parser_name.to_string(),
            })
        })
        .collect()
}

/// Renumbers every note in one transaction, then checks the result before committing: row counts
/// are unchanged, foreign keys hold and every note id decodes. Also empties the undo history,
/// drops `render_asset` rows of image occlusion images, seeds the laptop's note counter and marks
/// the database as migrated, with its files still to regenerate.
///
/// Holds an exclusive lock for the whole transaction, so it fails rather than waits if another
/// process (such as a running server) is writing.
pub async fn rewrite_database(db: &SqlitePool, plan: &MigrationPlan) -> Result<(), Error> {
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    // Foreign keys cannot be switched off inside a transaction.
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    let result = async {
        sqlx::query("BEGIN EXCLUSIVE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                migrate_error(format!(
                    "Could not lock the database ({e}). Stop the spares server and try again."
                ))
            })?;
        let result = rewrite_in_transaction(&mut conn, plan).await;
        let end = if result.is_ok() { "COMMIT" } else { "ROLLBACK" };
        sqlx::query(end)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        result
    }
    .await;
    // The connection goes back to the pool, so restore the default.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    result
}

#[expect(clippy::too_many_lines)]
async fn rewrite_in_transaction(
    conn: &mut SqliteConnection,
    plan: &MigrationPlan,
) -> Result<(), Error> {
    // The plan was made before the lock was taken, so check it still describes the database.
    let current = plan_on(conn).await?;
    if current.mapping != plan.mapping || current.orphans != plan.orphans {
        return Err(migrate_error(
            "Notes changed since the migration was planned. Stop the spares server and try again."
                .to_string(),
        ));
    }
    for (what, from) in ORPHANS {
        let statement = if what.contains("(kept") {
            format!(
                "UPDATE note_link SET linked_note_id = NULL, matched_keyword = NULL, score = NULL WHERE rowid IN (SELECT rowid {from})"
            )
        } else {
            format!("DELETE {from}")
        };
        sqlx::query(&statement)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    let counts_before = row_counts(conn).await?;

    sqlx::query(
        "CREATE TEMP TABLE note_id_map (old INTEGER PRIMARY KEY, new INTEGER NOT NULL UNIQUE)",
    )
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    for chunk in plan.mapping.chunks(400) {
        let mut builder = sqlx::QueryBuilder::new("INSERT INTO note_id_map (old, new) ");
        builder.push_values(chunk, |mut row, m| {
            row.push_bind(m.old).push_bind(m.new);
        });
        builder
            .build()
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }
    // Every new id is at least 27⁷, above every old id, so renumbering in one pass cannot collide.
    for statement in [
        "UPDATE note SET id = (SELECT new FROM note_id_map WHERE old = note.id)",
        "UPDATE card SET note_id = (SELECT new FROM note_id_map WHERE old = card.note_id)",
        "UPDATE note_tag SET note_id = (SELECT new FROM note_id_map WHERE old = note_tag.note_id)",
        "UPDATE note_keyword SET note_id = (SELECT new FROM note_id_map WHERE old = note_keyword.note_id)",
        "UPDATE note_link SET parent_note_id = (SELECT new FROM note_id_map WHERE old = note_link.parent_note_id)",
        "UPDATE note_link SET linked_note_id = (SELECT new FROM note_id_map WHERE old = note_link.linked_note_id) WHERE linked_note_id IS NOT NULL",
        // Undo history names notes by their old ids.
        "DELETE FROM event",
        "DROP TABLE note_id_map",
    ] {
        sqlx::query(statement)
            .execute(&mut *conn)
            .await
            .map_err(|e| migrate_error(format!("`{statement}` failed: {e}")))?;
    }
    for rewrite in &plan.rewrites {
        let (table, column) = if rewrite.owner.starts_with("note ") {
            ("note", "data")
        } else {
            ("tag", "query")
        };
        let changed = sqlx::query(&format!(
            "UPDATE {table} SET {column} = ? WHERE {column} = ?"
        ))
        .bind(&rewrite.after)
        .bind(&rewrite.before)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?
        .rows_affected();
        if changed == 0 {
            return Err(migrate_error(format!(
                "Failed to rewrite the note ids in {}",
                rewrite.owner
            )));
        }
    }
    sqlx::query("DELETE FROM render_asset WHERE path LIKE ? || '%' OR path LIKE ? || '%'")
        .bind(image_occlusion_prefix())
        .bind(IMAGE_OCCLUSION_ASSET_PREFIX)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if let Some(last) = plan.last() {
        sqlx::query(
            "INSERT INTO id_counter (replica, tbl, last) VALUES (0, 'note', ?)
             ON CONFLICT (replica, tbl) DO UPDATE SET last = MAX(last, excluded.last)",
        )
        .bind(last)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    for key in [NOTE_IDS_MIGRATED_KEY, FILES_PENDING_KEY] {
        sqlx::query("INSERT OR REPLACE INTO app_state (key, value) VALUES (?, 'true')")
            .bind(key)
            .execute(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    }

    let counts_after = row_counts(conn).await?;
    if counts_after != counts_before {
        return Err(migrate_error(format!(
            "Row counts changed (before: {counts_before:?}, after: {counts_after:?})"
        )));
    }
    let violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if !violations.is_empty() {
        return Err(migrate_error(format!(
            "Foreign keys do not hold after renumbering: {violations:?}"
        )));
    }
    let ids: Vec<NoteId> = sqlx::query_scalar("SELECT id FROM note ORDER BY id")
        .fetch_all(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if let Some(bad) = ids
        .iter()
        .find(|id| !(MIN_PACKED..=MAX_PACKED).contains(&id.packed()) || id.try_letters().is_none())
    {
        return Err(migrate_error(format!(
            "Note id {} does not decode after renumbering",
            bad.packed()
        )));
    }
    let expected: Vec<NoteId> = plan.mapping.iter().map(|m| m.new).collect();
    if ids != expected {
        return Err(migrate_error(
            "The renumbered note ids are not the planned ones".to_string(),
        ));
    }
    Ok(())
}

async fn row_counts(conn: &mut SqliteConnection) -> Result<Vec<(&'static str, i64)>, Error> {
    let mut counts = Vec::new();
    for table in COUNTED_TABLES {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&mut *conn)
            .await
            .map_err(sqlx_error)?;
        counts.push((table, count));
    }
    Ok(counts)
}

/// Copies the database to `path` (which must not exist) with `VACUUM INTO`.
pub async fn backup_database(db: &SqlitePool, path: &Path) -> Result<(), Error> {
    sqlx::query("VACUUM INTO ?")
        .bind(path.to_string_lossy().into_owned())
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(())
}

/// Opens a scratch copy of `db` at `path`, for a dry run.
pub async fn open_copy(db: &SqlitePool, path: &Path) -> Result<SqlitePool, Error> {
    backup_database(db, path).await?;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .with_regexp();
    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .map_err(sqlx_error)?;
    sqlx::migrate!("./migrations")
        .run(&mut conn)
        .await
        .map_err(|e| migrate_error(format!("Failed to migrate the copy: {e}")))?;
    conn.close().await.map_err(sqlx_error)?;
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(sqlx_error)
}

/// Note files that do not describe the database: after renumbering `migrated` (a copy), each
/// note's file is generated from it and compared with its current file under the old id, with
/// only the `note-id:` value swapped. Also lists files named like notes that are not notes.
///
/// Empty means renaming the files and rewriting their `note-id:` line is all the migration
/// changes in them.
pub async fn mismatched_note_files(
    migrated: &SqlitePool,
    mapping: &[IdMapping],
) -> Result<Vec<String>, Error> {
    let parsers = get_all_parsers();
    let by_new: HashMap<NoteId, &IdMapping> = mapping.iter().map(|m| (m.new, m)).collect();
    let by_old: HashMap<i64, NoteId> = mapping.iter().map(|m| (m.old, m.new)).collect();
    let rows = get_render_note_data(migrated, None).await?;
    let ids: Vec<NoteId> = rows.iter().map(|row| row.note_id).collect();
    let links = if ids.is_empty() {
        HashMap::default()
    } else {
        get_note_links_map(migrated, &ids).await?
    };
    let mut mismatches = Vec::new();
    let mut expected_names: HashMap<String, HashSet<String>> = HashMap::new();
    for row in &rows {
        let Some(m) = by_new.get(&row.note_id) else {
            mismatches.push(format!("note {} is not in the id map", row.note_id));
            continue;
        };
        let parser = find_parser(&row.parser_name, &parsers)?;
        let old_path = old_note_path(parser.as_ref(), m.old);
        expected_names
            .entry(parser.get_parser_name().to_string())
            .or_default()
            .insert(
                old_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
        let Ok(current) = std::fs::read_to_string(&old_path) else {
            mismatches.push(format!("{} is missing", old_path.display()));
            continue;
        };
        let expected = construct_db_note_file(parser.as_ref(), row, &links)?;
        let renumbered = swap_note_id_value(&current, m.old, m.new)
            .map(|contents| swap_linked_file_names(&contents, &by_old));
        if renumbered.as_deref() != Some(expected.as_str()) {
            mismatches.push(format!("{} differs from the database", old_path.display()));
        }
    }
    for (parser_name, names) in expected_names {
        let dir = notes_vcs::server_notes_dir().join(&parser_name);
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.split('.').next().unwrap_or_default();
            if !stem.is_empty()
                && stem.bytes().all(|b| b.is_ascii_digit())
                && !names.contains(&name)
            {
                mismatches.push(format!("{} is not a note", entry.path().display()));
            }
        }
    }
    Ok(mismatches)
}

/// `contents` with its single `note-id: <old>` setting naming `new`, or `None` if it does not have
/// exactly one.
fn swap_note_id_value(contents: &str, old: i64, new: NoteId) -> Option<String> {
    let regex = Regex::new(&format!(r"(note-id:\s*)0*{old}(?!\d)")).expect("valid regex");
    let found: Vec<_> = regex.find_iter(contents).collect();
    if found.len() != 1 {
        return None;
    }
    Some(
        regex
            .replace(contents, |caps: &Captures<'_, str>| {
                format!("{}{}", &caps[1], new.letters())
            })
            .into_owned(),
    )
}

/// `contents` with links to other notes' files (`…/typst/0042.typ`) naming their new files.
fn swap_linked_file_names(contents: &str, by_old: &HashMap<i64, NoteId>) -> String {
    let regex = Regex::new(r"(?<=/)(\d+)(?=\.(typ|md|tex)\b)").expect("valid regex");
    regex
        .replace_all(contents, |caps: &Captures<'_, str>| {
            caps[1]
                .parse::<i64>()
                .ok()
                .and_then(|old| by_old.get(&old))
                .map_or_else(|| caps[1].to_string(), |new| new.letters())
        })
        .into_owned()
}

/// What [`regenerate_files`] would touch.
#[derive(Clone, Debug, Default)]
pub struct FileCounts {
    /// Raw note files to rename
    pub note_files: usize,
    /// Raw card files to delete and write again
    pub card_files: usize,
    /// Rendered files, dependency lists and LaTeX aux directories to delete
    pub rendered_files: usize,
    /// Rendered files to compile again: each note and each side of each card
    pub to_render: usize,
}

/// Counts what [`regenerate_files`] would touch, from the old ids.
pub fn count_files(mapping: &[IdMapping], card_count: usize) -> FileCounts {
    let parsers = get_all_parsers();
    let mut counts = FileCounts::default();
    let mut parser_names: HashSet<&str> = HashSet::new();
    for m in mapping {
        parser_names.insert(&m.parser_name);
        if let Ok(parser) = find_parser(&m.parser_name, &parsers)
            && old_note_path(parser.as_ref(), m.old).exists()
        {
            counts.note_files += 1;
        }
    }
    for name in parser_names {
        let Ok(parser) = find_parser(name, &parsers) else {
            continue;
        };
        counts.card_files += numeric_entries(
            &get_output_raw_dir(name, RenderOutputType::Card(0, CardSide::Front), None),
            is_old_raw_name,
        )
        .len();
        for dir in rendered_dirs(parser.as_ref()) {
            counts.rendered_files += old_rendered_entries(&dir).len();
        }
    }
    counts.to_render = mapping.len() + 2 * card_count;
    counts
}

/// Where a note's raw file was under its old id.
fn old_note_path(parser: &dyn Parseable, old: i64) -> PathBuf {
    notes_vcs::server_notes_dir()
        .join(parser.get_parser_name())
        .join(format!("{old:0>4}.{}", parser.file_extension()))
}

fn rendered_dirs(parser: &dyn Parseable) -> Vec<PathBuf> {
    let mut dirs = vec![
        parser.get_output_rendered_dir(RenderOutputDirectoryType::Note),
        parser.get_output_rendered_dir(RenderOutputDirectoryType::Card),
    ];
    dirs.dedup();
    dirs
}

/// Raw card files: `0042-1.typ`, `0042-1-front.typ`.
fn is_old_raw_name(name: &str) -> bool {
    let regex = Regex::new(r"^\d+-\d+(-(front|back))?\.[A-Za-z]+$").expect("valid regex");
    regex.is_match(name).unwrap_or(false)
}

/// Files spares renders: `0042.pdf`, `0042-1-back.deps.json`, `0042.synctex.gz`, …, and LaTeX's
/// `aux/0042` directories. Other files there, such as `hw2-539.pdf`, are left alone.
fn is_old_rendered_name(name: &str) -> bool {
    let regex = Regex::new(
        r"^\d+(-\d+-(front|back))?\.(pdf|html|deps\.json|raw\.json|synctex\.gz|svg|png)$",
    )
    .expect("valid regex");
    regex.is_match(name).unwrap_or(false)
}

/// Entries of `dir` whose names `matches`.
fn numeric_entries(dir: &Path, matches: fn(&str) -> bool) -> Vec<PathBuf> {
    let mut entries = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return entries;
    };
    for entry in read.flatten() {
        if let Some(name) = entry.file_name().to_str()
            && matches(name)
        {
            entries.push(entry.path());
        }
    }
    entries
}

/// Files spares rendered under old ids in `dir`, and LaTeX's `aux/<old id>` directories.
fn old_rendered_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries = numeric_entries(dir, is_old_rendered_name);
    entries.extend(numeric_entries(&dir.join("aux"), |name| {
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
    }));
    entries
}

/// Compiles a few existing card files to a scratch directory to estimate how long rendering
/// `to_render` files takes, using as many threads as rendering does.
pub fn estimate_render_time(scratch: &Path, to_render: usize) -> Option<Duration> {
    const SAMPLE: usize = 8;
    let parsers = get_all_parsers();
    let mut timings = Vec::new();
    for parser in parsers.iter().map(|p| p()) {
        let dir = get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(0, CardSide::Front),
            None,
        );
        let mut files: Vec<PathBuf> = numeric_entries(&dir, is_old_raw_name)
            .into_iter()
            .filter(|p| p.to_string_lossy().contains("-front."))
            .collect();
        files.sort();
        let step = (files.len() / SAMPLE).max(1);
        for file in files.iter().step_by(step).take(SAMPLE) {
            let output = scratch.join(file.file_name()?).with_extension("pdf");
            let start = Instant::now();
            let result = parser.render_file(scratch, file, scratch, &output);
            if result.is_ok_and(|o| o.status.success()) {
                timings.push(start.elapsed());
            }
        }
    }
    if timings.is_empty() {
        return None;
    }
    let average = timings.iter().sum::<Duration>() / u32::try_from(timings.len()).ok()?;
    let threads = u32::try_from(rayon::current_num_threads()).ok()?.max(1);
    Some(average * u32::try_from(to_render).ok()? / threads)
}

/// Regenerates every file named by a note id from the migrated database, using the old → new map
/// from `id-map.tsv`:
/// - the notes repository: each raw note file is moved to its new name and rewritten (its
///   `note-id:` line changed), in one commit, so git records renames;
/// - without it, the raw note files under the data directory;
/// - raw card files, which embed the cloze tag;
/// - caches: rendered files, render hashes and rendered image occlusions are deleted and rendered
///   again.
///
/// Everything here can run again after a crash.
pub async fn regenerate_files(
    db: &SqlitePool,
    mapping: &[IdMapping],
    mut progress: impl FnMut(&str),
) -> Result<RegenerateReport, Error> {
    let parsers = get_all_parsers();
    let mut report = RegenerateReport::default();

    progress("Renaming note files");
    if notes_vcs::enabled() {
        report.notes_commit =
            notes_vcs::renumber_note_files(db, mapping, &id_map_commit_note()).await?;
    } else {
        for m in mapping {
            if let Ok(parser) = find_parser(&m.parser_name, &parsers) {
                let path = old_note_path(parser.as_ref(), m.old);
                if path.exists() {
                    std::fs::remove_file(&path).map_err(|e| io_error(&path, e))?;
                }
            }
        }
    }

    progress("Deleting old card files and rendered files");
    let parser_names: HashSet<&str> = mapping.iter().map(|m| m.parser_name.as_str()).collect();
    for name in &parser_names {
        let Ok(parser) = find_parser(name, &parsers) else {
            continue;
        };
        let cards = get_output_raw_dir(name, RenderOutputType::Card(0, CardSide::Front), None);
        for path in numeric_entries(&cards, is_old_raw_name) {
            remove_path(&path)?;
            report.deleted += 1;
        }
        for dir in rendered_dirs(parser.as_ref()) {
            for path in old_rendered_entries(&dir) {
                remove_path(&path)?;
                report.deleted += 1;
            }
        }
    }
    for dir in [
        get_cache_dir().join("note-hashes"),
        get_image_occlusion_rendered_directory(),
    ] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
        }
    }

    progress("Rendering every note and card (this takes a while)");
    let start = Instant::now();
    let render = render_notes(
        db,
        RenderNotesRequest {
            selector: NotesSelector::All,
            immutable_note_ids: None,
            overridden_output_raw_dir: None,
            include_linked_notes: false,
            include_cards: true,
            generate_rendered: true,
            force_generate_rendered: true,
        },
        &parsers,
    )
    .await;
    report.render_duration = start.elapsed();
    report.render_error = render.err().map(|e| e.to_string());

    sqlx::query("DELETE FROM app_state WHERE key = ?")
        .bind(FILES_PENDING_KEY)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(report)
}

#[derive(Clone, Debug, Default)]
pub struct RegenerateReport {
    /// Whether the notes repository got a commit
    pub notes_commit: bool,
    pub deleted: usize,
    pub render_duration: Duration,
    /// Notes can fail to render after the migration as they could before it. They are rendered
    /// again whenever they change, or with `spares note generate --render`.
    pub render_error: Option<String>,
}

fn id_map_commit_note() -> String {
    format!(
        "spares: renumber notes to letter ids\n\nThe old numeric ids are listed in {}.",
        id_map_path().display()
    )
}

fn remove_path(path: &Path) -> Result<(), Error> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|e| io_error(path, e))
}

fn sqlx_error(source: sqlx::Error) -> Error {
    Error::Sqlx { source }
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        description: format!("Failed to access {}", path.display()),
        source,
    }
}

fn migrate_error(message: String) -> Error {
    Error::Library(LibraryError::InvalidConfig(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_old(pairs: &[(i64, &str)]) -> HashMap<i64, NoteId> {
        pairs
            .iter()
            .map(|(old, new)| (*old, new.parse().unwrap()))
            .collect()
    }

    #[test]
    fn test_rewrite_inh() {
        let map = by_old(&[(12, "aal"), (4206, "giq")]);
        assert_eq!(
            rewrite_inh("{{[o:1;inh:12/2] a }} {{[inh: 4206/1] b}}", &map).unwrap(),
            "{{[o:1;inh:aal/2] a }} {{[inh: giq/1] b}}"
        );
        assert_eq!(
            rewrite_inh("no inheritance", &map).unwrap(),
            "no inheritance"
        );
        assert_eq!(rewrite_inh("{{[inh:7/1] a}}", &map), Err(7));
    }

    #[test]
    fn test_rewrite_query() {
        let map = by_old(&[(12, "aal"), (5, "aae")]);
        assert_eq!(
            rewrite_query(
                "tag=math and (id=12 or linked_to_note = 5) and c.id=12",
                &map
            )
            .unwrap(),
            "tag=math and (id=aal or linked_to_note = aae) and c.id=12"
        );
        assert!(rewrite_query("id>100", &map).is_err());
        assert!(rewrite_query("id=7", &map).is_err());
        assert_eq!(rewrite_query("c.id>3", &map).unwrap(), "c.id>3");
    }

    #[test]
    fn test_old_names() {
        for name in [
            "0042.pdf",
            "0042-1-back.pdf",
            "4206.deps.json",
            "0001-12-front.deps.json",
            "0042.synctex.gz",
            "0042.html",
        ] {
            assert!(is_old_rendered_name(name), "{name}");
        }
        for name in [
            "hw2-539.pdf",
            "539_HW1.pdf",
            "2026-chat.html",
            "debt.pdf",
            "gir.pdf",
            "0042-cameron.html",
        ] {
            assert!(!is_old_rendered_name(name), "{name}");
        }
        for name in ["0042-1.typ", "0042-1-front.typ", "0042-12-back.md"] {
            assert!(is_old_raw_name(name), "{name}");
        }
        for name in ["0042.typ", "gir-1.typ", "notes.typ"] {
            assert!(!is_old_raw_name(name), "{name}");
        }
    }

    #[test]
    fn test_swap_note_id_value() {
        let gir: NoteId = "gir".parse().unwrap();
        assert_eq!(
            swap_note_id_value("#se[note-id: 42]\nbody", 42, gir).as_deref(),
            Some("#se[note-id: gir]\nbody")
        );
        assert_eq!(swap_note_id_value("#se[note-id: 420]", 42, gir), None);
        assert_eq!(swap_note_id_value("no id", 42, gir), None);
    }

    #[test]
    fn test_swap_linked_file_names() {
        let map = by_old(&[(883, "bhy")]);
        assert_eq!(
            swap_linked_file_names(
                r#"#link("/data/notes/typst/0883.typ")[a] #link("/x/0999.typ") 0883.typ"#,
                &map
            ),
            r#"#link("/data/notes/typst/bhy.typ")[a] #link("/x/0999.typ") 0883.typ"#
        );
    }

    #[test]
    fn test_id_map_round_trip() {
        let dir = std::env::temp_dir().join(format!("spares-id-map-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ID_MAP_FILE_NAME);
        let mapping = vec![
            IdMapping {
                old: 1,
                new: "aaa".parse().unwrap(),
                parser_name: "typst".to_string(),
            },
            IdMapping {
                old: 4206,
                new: "giq".parse().unwrap(),
                parser_name: "markdown".to_string(),
            },
        ];
        write_id_map(&path, &mapping).unwrap();
        assert_eq!(read_id_map(&path).unwrap(), mapping);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    async fn exec(pool: &SqlitePool, sql: &str) {
        sqlx::query(sql).execute(pool).await.unwrap();
    }

    /// A database as it was before the migration: numeric note ids, with gaps from deletes.
    async fn old_database(pool: &SqlitePool) {
        exec(pool, "INSERT INTO parser (id, name) VALUES (1, 'markdown')").await;
        for (id, data) in [
            (1, "One {{ a }}"),
            (2, "Two {{[inh:1/1] b }}"),
            (5, "Five {{ c }}"),
        ] {
            sqlx::query(
                "INSERT INTO note (id, data, custom_data, parser_id) VALUES (?, ?, '{}', 1)",
            )
            .bind(id)
            .bind(data)
            .execute(pool)
            .await
            .unwrap();
            sqlx::query(
                r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
                   desired_retention, state, custom_data) VALUES (?, 1, 1, 0, 0, 0, 0.9, 0, '{}')"#,
            )
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        }
        exec(
            pool,
            "INSERT INTO tag (id, name, description, auto_delete) VALUES (1, 'math', '', 0)",
        )
        .await;
        exec(pool, "INSERT INTO tag (id, name, description, query, auto_delete) VALUES (2, 'five', '', 'id=5 or c.id=1', 0)").await;
        exec(
            pool,
            "INSERT INTO note_tag (note_id, tag_id) VALUES (2, 1), (5, 1)",
        )
        .await;
        exec(pool, "INSERT INTO note_keyword (note_id, keyword, embedded) VALUES (1, 'one', 0), (5, 'five', 0)").await;
        exec(pool, r#"INSERT INTO note_link (parent_note_id, linked_note_id, "order", searched_keyword) VALUES (5, 1, 0, 'one'), (2, NULL, 0, 'none')"#).await;
        exec(
            pool,
            "INSERT INTO event (kind, payload) VALUES ('CreateNotes', '{}')",
        )
        .await;
        // Left over from deleting note 4 while foreign keys were off
        let mut conn = pool.acquire().await.unwrap();
        for sql in [
            "PRAGMA foreign_keys = OFF",
            "INSERT INTO note_tag (note_id, tag_id) VALUES (4, 1)",
            "PRAGMA foreign_keys = ON",
        ] {
            sqlx::query(sql).execute(&mut *conn).await.unwrap();
        }
    }

    #[sqlx::test]
    async fn test_rewrite_database(pool: SqlitePool) {
        old_database(&pool).await;
        assert!(!is_migrated(&pool).await.unwrap());
        assert!(super::super::ensure_note_ids_migrated(&pool).await.is_err());

        let plan = plan(&pool).await.unwrap();
        let olds: Vec<i64> = plan.mapping.iter().map(|m| m.old).collect();
        let news: Vec<String> = plan.mapping.iter().map(|m| m.new.letters()).collect();
        assert_eq!(olds, [1, 2, 5]);
        assert_eq!(news, ["aaa", "aab", "aac"]);
        assert_eq!(plan.rewrites.len(), 2);
        assert_eq!(plan.events, 1);
        assert_eq!(plan.orphans, [("tags of deleted notes", 1)]);

        rewrite_database(&pool, &plan).await.unwrap();

        let id = |s: &str| s.parse::<NoteId>().unwrap();
        let notes: Vec<(NoteId, String)> = sqlx::query_as("SELECT id, data FROM note ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(notes[0].0, id("aaa"));
        assert_eq!(notes[1], (id("aab"), "Two {{[inh:aaa/1] b }}".to_string()));
        assert_eq!(notes[2].0, id("aac"));
        let card_notes: Vec<NoteId> = sqlx::query_scalar("SELECT note_id FROM card ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(card_notes, [id("aaa"), id("aab"), id("aac")]);
        let tagged: Vec<NoteId> =
            sqlx::query_scalar("SELECT note_id FROM note_tag ORDER BY note_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(tagged, [id("aab"), id("aac")]);
        let keyword_notes: Vec<NoteId> =
            sqlx::query_scalar("SELECT note_id FROM note_keyword ORDER BY note_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(keyword_notes, [id("aaa"), id("aac")]);
        let links: Vec<(NoteId, Option<NoteId>)> = sqlx::query_as(
            "SELECT parent_note_id, linked_note_id FROM note_link ORDER BY parent_note_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(links, [(id("aab"), None), (id("aac"), Some(id("aaa")))]);
        let query: String = sqlx::query_scalar("SELECT query FROM tag WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(query, "id=aac or c.id=1");
        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(events, 0);

        assert!(is_migrated(&pool).await.unwrap());
        assert!(files_pending(&pool).await.unwrap());
        super::super::ensure_note_ids_migrated(&pool).await.unwrap();
        // New notes continue after the last migrated one.
        let next = super::super::allocate_note_ids(&pool, 1).await.unwrap();
        assert_eq!(next, [id("aad")]);
        // Foreign keys are on again for the connection's next user.
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[sqlx::test]
    async fn test_rewrite_database_rolls_back_on_failure(pool: SqlitePool) {
        old_database(&pool).await;
        let mut plan = plan(&pool).await.unwrap();
        // A plan that no longer matches the notes is refused, and nothing changes.
        plan.mapping.pop();
        assert!(rewrite_database(&pool, &plan).await.is_err());
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM note ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, [1, 2, 5]);
        assert!(!is_migrated(&pool).await.unwrap());
    }

    #[sqlx::test]
    async fn test_plan_refuses_dangling_inh(pool: SqlitePool) {
        old_database(&pool).await;
        exec(
            &pool,
            "UPDATE note SET data = 'x {{[inh:9/1] y }}' WHERE id = 5",
        )
        .await;
        let err = plan(&pool).await.unwrap_err().to_string();
        assert!(err.contains("inh:9"), "{err}");
    }
}
