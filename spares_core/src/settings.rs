//! The collection's settings, kept in its database rather than in `config.toml`, so they can be
//! changed from any replica (the web app, a device, the CLI) and sync like the rest of the
//! collection.
//!
//! # Two kinds of settings
//!
//! [`SparesExternalConfig`](crate::config::SparesExternalConfig) is one struct, read from two
//! places:
//!
//! - **This machine's**, the keys in [`LOCAL_KEYS`]: how this copy syncs (`[sync]`), and the
//!   programs and rendering this machine has (`audio_player`, `tts_command`,
//!   `render_browser_parsers`). They stay in the collection's `config.toml`, and never leave it.
//! - **The collection's**, every other key: limits, steps, FSRS, the queue and its presets,
//!   leeches, goals, easy days, statistics, the parsers' settings, and so on. They are TOML text
//!   in the `collection_settings` table's one row, kept as written, comments included.
//!
//! [`crate::config::read_external_config`] reads the collection's text with this machine's
//! `config.toml` over it ([`overlay`]).
//!
//! # From `config.toml` into the collection
//!
//! A collection's settings were all in `config.toml` before. [`load`], when a host, the CLI or
//! a device's copy opens the collection, moves the collection's keys it finds there into the
//! database, over what is there, keeps the old file as `config.toml.collection-settings.bak`, and leaves only this
//! machine's keys in `config.toml`. So the first open after an update moves them all, and a
//! collection key written into `config.toml` by hand later (or by an older spares) is moved in the
//! same way the next time. Until then it applies over the collection's value, so what the file
//! says is never silently ignored. Every change through [`edit`] moves them in first.
//!
//! # Changes, undo and sync
//!
//! A change through the web app, `PUT`/`PATCH /api/settings` or `spares config` is an
//! [`UpdateSettings`](crate::model::EventType::UpdateSettings) event holding the text before and
//! after, so it can be undone like any other change. Undo applies the change in reverse key by key
//! ([`merge3`]), and refuses if a key it would change has changed again since.
//!
//! The row syncs like the other tables (`usn`, see [`crate::sync`]). When both sides changed it
//! since the last sync, the two texts are merged key by key against the last synced one
//! ([`merge3`]): a key changed on one side takes that side's value, and a key changed on both
//! takes the newer side's, as a tag's fields do. The text kept is this side's, comments included,
//! with the other side's changes written into it.
//!
//! # Reading without the database
//!
//! Most of spares reads settings through [`crate::config::read_external_config`], which isn't
//! async and has no database. It reads the text [`load`] or a change last cached in the
//! collection's state. A collection whose settings were never loaded (as in many tests) reads
//! `config.toml` alone, as before.

use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;
use toml_edit::DocumentMut;
use toml_edit::Item;

use crate::Error;
use crate::LibraryError;
use crate::model::EventType;

/// The top-level keys of `config.toml` that are about this machine rather than the collection,
/// which stay in `config.toml` (see the [module docs](self)).
pub const LOCAL_KEYS: [&str; 4] = [
    "sync",
    "audio_player",
    "tts_command",
    "render_browser_parsers",
];

#[cfg(not(target_family = "wasm"))]
/// The comment [`load`] puts at the top of a `config.toml` whose collection settings it moved.
const MOVED_HEADER: &str = "# This machine's settings. The collection's settings (limits, steps, FSRS, the queue, goals, \
     ...)\n# are kept in the collection: change them on the web app's Settings page or with \
     `spares config edit`.\n";

pub fn is_local_key(key: &str) -> bool {
    LOCAL_KEYS.contains(&key)
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Library(LibraryError::InvalidConfig(message.into()))
}

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

/// Parses `text` as TOML, naming `what` in the error.
pub fn parse_document(text: &str, what: &str) -> Result<DocumentMut, Error> {
    text.parse::<DocumentMut>()
        .map_err(|e| invalid(format!("{what}: {e}")))
}

/// `text` without its top-level keys for which `keep` is false, comments of the others kept.
fn filtered(text: &str, keep: impl Fn(&str) -> bool) -> Result<DocumentMut, Error> {
    let mut doc = parse_document(text, "config.toml")?;
    let drop: Vec<String> = doc
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| !keep(key))
        .collect();
    for key in drop {
        doc.remove(&key);
    }
    Ok(doc)
}

/// The collection's keys of a `config.toml` text (see [`LOCAL_KEYS`]).
pub fn collection_part(text: &str) -> Result<DocumentMut, Error> {
    filtered(text, |key| !is_local_key(key))
}

/// This machine's keys of a `config.toml` text (see [`LOCAL_KEYS`]).
pub fn local_part(text: &str) -> Result<DocumentMut, Error> {
    filtered(text, is_local_key)
}

/// Whether a document sets anything.
#[cfg(not(target_family = "wasm"))]
fn has_keys(doc: &DocumentMut) -> bool {
    doc.iter().next().is_some()
}

/// The item at `path` of a document, through tables and inline tables.
pub fn item_at<'a>(doc: &'a DocumentMut, path: &[String]) -> Option<&'a Item> {
    let (last, parents) = path.split_last()?;
    let mut table: &dyn toml_edit::TableLike = doc.as_table();
    for key in parents {
        table = table.get(key)?.as_table_like()?;
    }
    table.get(last)
}

/// Sets the item at `path`, making the tables on the way (replacing a value in the way), or
/// removes it with `None`, and then the tables that leaves empty.
pub fn set_item(doc: &mut DocumentMut, path: &[String], item: Option<Item>) {
    set_formatted(doc, path, item.map(|item| (None, item)));
}

/// The item at `path` of `source` with its key, comments included.
fn item_with_key(source: &DocumentMut, path: &[String]) -> Option<(toml_edit::Key, Item)> {
    let (last, parents) = path.split_last()?;
    let mut table: &dyn toml_edit::TableLike = source.as_table();
    for key in parents {
        table = table.get(key)?.as_table_like()?;
    }
    let (key, item) = table.get_key_value(last)?;
    Some((key.clone(), item.clone()))
}

/// Sets the item at `path` to `source`'s, the comments before its key included where there is no
/// such key yet, or removes it if `source` has none.
fn copy_item(doc: &mut DocumentMut, path: &[String], source: &DocumentMut) {
    set_formatted(
        doc,
        path,
        item_with_key(source, path).map(|(key, item)| (Some(key), item)),
    );
}

/// [`set_item`], with the key written as `key` (with its comments) where the table has none yet.
fn set_formatted(
    doc: &mut DocumentMut,
    path: &[String],
    item: Option<(Option<toml_edit::Key>, Item)>,
) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let Some((key, mut item)) = item else {
        remove_item(doc.as_item_mut(), parents, last);
        return;
    };
    let mut current: &mut Item = doc.as_item_mut();
    for parent in parents {
        let inline = current.is_inline_table();
        let table = current.as_table_like_mut().expect("a table on the way");
        if !table.get(parent).is_some_and(Item::is_table_like) {
            let new = if inline {
                Item::Value(toml_edit::Value::InlineTable(toml_edit::InlineTable::new()))
            } else {
                let mut new = toml_edit::Table::new();
                new.set_implicit(true);
                Item::Table(new)
            };
            table.insert(parent, new);
        }
        current = table.get_mut(parent).expect("made a table");
    }
    // A value replacing one keeps its comments
    let existing = current.as_table_like().and_then(|table| table.get(last));
    if let (Some(old), Item::Value(new)) = (existing.and_then(Item::as_value), &mut item) {
        *new.decor_mut() = old.decor().clone();
    }
    // A new key keeps the comments it had where it came from
    let key = key.filter(|_| existing.is_none());
    if let Item::Table(table) = &mut item {
        // Shown where it goes, not where it was in the other document
        table.set_position(None);
    }
    match current {
        Item::Value(toml_edit::Value::InlineTable(inline)) => {
            // An inline table holds values only
            let Ok(value) = item.into_value() else {
                return;
            };
            match (inline.get_mut(last), key) {
                // In place, which keeps the key and the comments before it
                (Some(old), _) => *old = value,
                (None, Some(key)) => {
                    inline.insert_formatted(&key, value);
                }
                (None, None) => {
                    inline.insert(last, value);
                }
            }
        }
        Item::Table(table) => match (table.get_mut(last), key) {
            (Some(old), _) => *old = item,
            (None, Some(key)) => {
                table.insert_formatted(&key, item);
            }
            (None, None) => {
                table.insert(last, item);
            }
        },
        _ => {}
    }
}

fn remove_item(item: &mut Item, parents: &[String], last: &str) {
    let Some(table) = item.as_table_like_mut() else {
        return;
    };
    match parents.split_first() {
        None => {
            table.remove(last);
        }
        Some((key, rest)) => {
            let Some(child) = table.get_mut(key) else {
                return;
            };
            remove_item(child, rest, last);
            if child.as_table_like().is_some_and(|t| t.is_empty()) {
                table.remove(key);
            }
        }
    }
}

/// The paths of the values `doc` sets, through its tables and inline tables. An array of tables
/// is one value.
pub fn value_paths(doc: &DocumentMut) -> Vec<Vec<String>> {
    fn walk(
        table: &dyn toml_edit::TableLike,
        prefix: &mut Vec<String>,
        out: &mut Vec<Vec<String>>,
    ) {
        for (key, item) in table.iter() {
            prefix.push(key.to_string());
            match item.as_table_like() {
                Some(child) => walk(child, prefix, out),
                None => out.push(prefix.clone()),
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(doc.as_table(), &mut Vec::new(), &mut out);
    out
}

/// `base` with every value `over` sets written over it, comments of both kept where they can be.
/// A table `base` doesn't have is copied whole, with its comments.
pub fn overlay(base: &str, over: &DocumentMut) -> Result<String, Error> {
    let mut doc = parse_document(base, "the collection's settings")?;
    let mut copied: Vec<Vec<String>> = Vec::new();
    for path in value_paths(over) {
        if copied.iter().any(|table| path.starts_with(table)) {
            continue;
        }
        // The outermost table on the way that `doc` lacks
        let missing = (1..path.len())
            .find(|&depth| !item_at(&doc, &path[..depth]).is_some_and(Item::is_table_like));
        let path = match missing {
            Some(depth) => {
                copied.push(path[..depth].to_vec());
                path[..depth].to_vec()
            }
            None => path,
        };
        copy_item(&mut doc, &path, over);
    }
    Ok(doc.to_string())
}

/// A document as JSON, for comparing values whatever their formatting.
fn as_json(doc: &DocumentMut) -> Result<Value, Error> {
    toml_edit::de::from_str::<Value>(&doc.to_string()).map_err(|e| invalid(e.to_string()))
}

fn json_at<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, key| value.get(key))
}

/// Which side a key both sides changed takes in [`merge3`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Prefer {
    Local,
    Remote,
}

/// `local` with the changes `remote` made since `base` written into it, key by key: a value
/// `remote` changed (or added, or removed) and `local` didn't takes `remote`'s, and one both
/// changed differently takes the side `prefer` names. Returns the merged text, comments and
/// layout of `local` kept, and the keys both sides changed differently, with dots between their
/// parts.
pub fn merge3(
    base: &str,
    local: &str,
    remote: &str,
    prefer: Prefer,
) -> Result<(String, Vec<String>), Error> {
    let base_doc = parse_document(base, "the settings as last synced")?;
    let mut local_doc = parse_document(local, "this side's settings")?;
    let remote_doc = parse_document(remote, "the other side's settings")?;
    let base_json = as_json(&base_doc)?;
    let mut local_json = as_json(&local_doc)?;
    let remote_json = as_json(&remote_doc)?;
    let mut paths = value_paths(&base_doc);
    paths.extend(value_paths(&remote_doc));
    paths.sort();
    paths.dedup();
    let mut conflicts = Vec::new();
    for path in paths {
        let b = json_at(&base_json, &path);
        let r = json_at(&remote_json, &path);
        let l = json_at(&local_json, &path);
        if r == b || l == r {
            continue;
        }
        if l != b {
            conflicts.push(path.join("."));
            if prefer == Prefer::Local {
                continue;
            }
        }
        copy_item(&mut local_doc, &path, &remote_doc);
        local_json = as_json(&local_doc)?;
    }
    Ok((local_doc.to_string(), conflicts))
}

/// The keys whose values differ between two settings texts, with dots between their parts.
pub fn changed_keys(before: &str, after: &str) -> Result<Vec<String>, Error> {
    let before_doc = parse_document(before, "the settings before")?;
    let after_doc = parse_document(after, "the settings after")?;
    let before_json = as_json(&before_doc)?;
    let after_json = as_json(&after_doc)?;
    let mut paths = value_paths(&before_doc);
    paths.extend(value_paths(&after_doc));
    paths.sort();
    paths.dedup();
    Ok(paths
        .into_iter()
        .filter(|path| json_at(&before_json, path) != json_at(&after_json, path))
        .map(|path| path.join("."))
        .collect())
}

/// The top-level keys of the collection's settings that rendering a note reads: a change to one
/// may leave every note's files out of date.
pub const RENDER_KEYS: [&str; 3] = ["parser", "image_occlusion", "overlapper"];

/// Whether rendering reads the setting `key` (with dots between its parts, as
/// [`changed_keys`] gives them): one under [`RENDER_KEYS`].
pub fn affects_rendering(key: &str) -> bool {
    RENDER_KEYS
        .iter()
        .any(|top| key == *top || key.starts_with(&format!("{top}.")))
}

/// Whether `before` → `after` changes a setting rendering reads ([`RENDER_KEYS`]).
pub fn changes_rendering(before: &str, after: &str) -> bool {
    changed_keys(before, after).is_ok_and(|keys| keys.iter().any(|key| affects_rendering(key)))
}

// --- The cache -------------------------------------------------------------------------------

/// The collection's settings text as last loaded or changed here, or `None` if they were never
/// loaded in this process (then [`crate::config::read_external_config`] reads `config.toml`
/// alone).
pub fn cached() -> Option<Arc<str>> {
    #[cfg(not(target_family = "wasm"))]
    if outside_a_collection_in_a_test() {
        return TEST_CACHE.with(|cache| cache.borrow().clone());
    }
    crate::collection::state().settings()
}

fn set_cached(text: Option<&str>) {
    let text = text.map(Arc::from);
    #[cfg(not(target_family = "wasm"))]
    if outside_a_collection_in_a_test() {
        TEST_CACHE.with(|cache| *cache.borrow_mut() = text);
        return;
    }
    crate::collection::state().set_settings(text);
}

#[cfg(not(target_family = "wasm"))]
thread_local! {
    /// A test that runs in no collection shares the state of every other such test (see
    /// [`crate::collection::state`]), which run at the same time on threads of their own. Their
    /// settings are kept for each thread instead, so one test's settings never reach another's.
    static TEST_CACHE: std::cell::RefCell<Option<Arc<str>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(not(target_family = "wasm"))]
fn outside_a_collection_in_a_test() -> bool {
    crate::config::is_test_mode() && crate::collection::current().is_none()
}

/// The settings [`crate::config::read_external_config`] reads, as TOML text: the collection's
/// with this machine's `config.toml` (`file`, if it has one) over it.
pub fn effective_text(file: Option<&str>) -> Result<String, Error> {
    let collection = cached();
    let Some(file) = file else {
        return Ok(collection.as_deref().unwrap_or_default().to_string());
    };
    match collection {
        None => Ok(file.to_string()),
        Some(collection) => overlay(&collection, &parse_document(file, "config.toml")?),
    }
}

// --- The database ----------------------------------------------------------------------------

/// The `collection_settings` row's text, if there is one.
pub async fn read_text(conn: &mut SqliteConnection) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT text FROM collection_settings WHERE id = 1")
        .fetch_optional(&mut *conn)
        .await
        .map_err(sqlx_error)
}

/// Writes the row's text, which the triggers mark as changed.
async fn write_text(conn: &mut SqliteConnection, text: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO collection_settings (id, text, base, updated_at, usn) VALUES (1, ?1, NULL, ?2, -1)
         ON CONFLICT (id) DO UPDATE SET text = excluded.text, updated_at = excluded.updated_at
         WHERE text IS NOT excluded.text",
    )
    .bind(text)
    .bind(Utc::now().timestamp())
    .execute(&mut *conn)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

/// [`reload`], logging a failure rather than returning it, for after a sync that has already
/// committed.
pub async fn reload_after_sync(db: &SqlitePool) {
    if let Err(e) = reload(db).await {
        log::error!("Failed to read the collection's settings after a sync: {e}");
    }
}

/// Reads the row again into the cache, after a sync wrote it, for example.
pub async fn reload(db: &SqlitePool) -> Result<(), Error> {
    let mut conn = db.acquire().await.map_err(sqlx_error)?;
    let text = read_text(&mut conn).await?;
    // A collection that has never had settings reads `config.toml` alone until it does, as a
    // collection whose settings were never loaded
    if text.is_some() || cached().is_some() {
        set_cached(Some(text.as_deref().unwrap_or_default()));
    }
    Ok(())
}

/// Checks that `text` is a valid set of the collection's settings: TOML, no keys of
/// [`LOCAL_KEYS`], and valid with this machine's `config.toml` over it.
pub fn validate(text: &str) -> Result<(), Error> {
    let doc = parse_document(text, "the collection's settings")?;
    if let Some((key, _)) = doc.iter().find(|(key, _)| is_local_key(key)) {
        return Err(invalid(format!(
            "`{key}` is this machine's setting, kept in its config.toml, not the collection's"
        )));
    }
    // On their own first, so an error's line is the line of `text`
    crate::config::parse_named(text, "the settings")?;
    let Some(file) = read_config_file()? else {
        return Ok(());
    };
    let effective = overlay(text, &local_part(&file)?)?;
    crate::config::parse_named(&effective, "the settings, with this machine's config.toml,")
        .map(|_| ())
}

/// The collection's `config.toml`, if it has one.
fn read_config_file() -> Result<Option<String>, Error> {
    let path = crate::config::get_external_config_file();
    if !crate::platform::is_file(&path) {
        return Ok(None);
    }
    crate::platform::read_to_string(&path)
        .map(Some)
        .map_err(|e| Error::Io {
            description: format!("Failed to read {}.", path.display()),
            source: e,
        })
}

/// Reads `collection`'s settings into its cache without moving anything out of `config.toml` or
/// migrating its database, as the CLI does with the collection its host may be serving. A database
/// without them (one not migrated yet) leaves `config.toml` read alone.
#[cfg(not(target_family = "wasm"))]
pub async fn load_read_only(collection: &crate::collection::Collection) {
    let text = match collection.db.acquire().await {
        Ok(mut conn) => read_text(&mut conn).await,
        Err(e) => Err(sqlx_error(e)),
    };
    match text {
        Ok(text) => collection
            .state
            .set_settings(Some(Arc::from(text.as_deref().unwrap_or_default()))),
        Err(e) => log::debug!("Reading config.toml alone, without the collection's settings: {e}"),
    }
}

/// Loads the collection's settings into the cache, after first moving the collection's keys
/// found in `config.toml` into the database (see the [module docs](self)). A host, the CLI and a
/// device's copy do this when they open a collection.
pub async fn load(db: &SqlitePool) -> Result<(), Error> {
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    let text = read_text(&mut tx).await?;
    #[cfg(not(target_family = "wasm"))]
    let text = match move_in_from_file(text.as_deref())? {
        Some(moved) => {
            write_text(&mut tx, &moved.text).await?;
            tx.commit().await.map_err(sqlx_error)?;
            moved.finish()?;
            set_cached(Some(&moved.text));
            return Ok(());
        }
        None => text,
    };
    tx.commit().await.map_err(sqlx_error)?;
    // A copy that never had settings and has no collection keys in its config.toml: the
    // defaults, written nowhere until they change
    set_cached(Some(text.as_deref().unwrap_or_default()));
    Ok(())
}

/// The collection's keys of `config.toml` moved into `current` (the row's text), with what is left
/// of the file to write once the database has them.
#[cfg(not(target_family = "wasm"))]
struct MovedIn {
    text: String,
    file: std::path::PathBuf,
    old: String,
    left: String,
}

#[cfg(not(target_family = "wasm"))]
impl MovedIn {
    /// Keeps the old file as `config.toml.collection-settings.bak`, and leaves this machine's keys in `config.toml`.
    fn finish(&self) -> Result<(), Error> {
        let io = |e| Error::Io {
            description: format!("Failed to write {}", self.file.display()),
            source: e,
        };
        std::fs::write(backup_path(&self.file), &self.old).map_err(io)?;
        let staging = self.file.with_extension("toml.moving");
        std::fs::write(&staging, &self.left)
            .and_then(|()| std::fs::rename(&staging, &self.file))
            .map_err(io)
    }
}

#[cfg(not(target_family = "wasm"))]
fn backup_path(file: &std::path::Path) -> std::path::PathBuf {
    file.with_extension("toml.collection-settings.bak")
}

#[cfg(not(target_family = "wasm"))]
fn move_in_from_file(current: Option<&str>) -> Result<Option<MovedIn>, Error> {
    // Tests in no collection share one directory's config.toml, which others read meanwhile
    if outside_a_collection_in_a_test() {
        return Ok(None);
    }
    let Some(old) = read_config_file()? else {
        return Ok(None);
    };
    let collection = collection_part(&old)?;
    if !has_keys(&collection) {
        return Ok(None);
    }
    let text = overlay(current.unwrap_or_default(), &collection)?;
    // Left where it is if it isn't valid, so reading it says what is wrong, as before
    if crate::config::parse_external_config(&overlay(&text, &local_part(&old)?)?).is_err() {
        log::warn!(
            "Left the collection's settings in config.toml, which isn't valid: correct it, and \
             they move into the collection the next time it opens"
        );
        return Ok(None);
    }
    let mut left = local_part(&old)?.to_string();
    if !left.starts_with(MOVED_HEADER) {
        left = format!("{MOVED_HEADER}\n{}", left.trim_start());
    }
    Ok(Some(MovedIn {
        text,
        file: crate::config::get_external_config_file(),
        old,
        left,
    }))
}

/// A change to the collection's settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SettingsChange {
    pub before: String,
    pub after: String,
    /// The [`EventType::UpdateSettings`] event it was logged as, if it was
    pub event_id: Option<i64>,
}

impl SettingsChange {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// Changes the collection's settings with `change`, which edits them as a TOML document, after
/// moving in what `config.toml` has of them. Fails, changing nothing, if the result isn't valid
/// ([`validate`]). With `log`, the change is an [`EventType::UpdateSettings`] event, so it can be
/// undone.
pub async fn edit(
    db: &SqlitePool,
    log: bool,
    change: impl FnOnce(&mut DocumentMut) -> Result<(), Error>,
) -> Result<SettingsChange, Error> {
    load(db).await?;
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    // A write first, so two changes at once can't both start from the same text
    sqlx::query("UPDATE collection_settings SET updated_at = updated_at WHERE id = 1")
        .execute(&mut *tx)
        .await
        .map_err(sqlx_error)?;
    let before = read_text(&mut tx).await?.unwrap_or_default();
    let mut doc = parse_document(&before, "the collection's settings")?;
    change(&mut doc)?;
    let after = doc.to_string();
    let mut result = SettingsChange {
        before,
        after,
        event_id: None,
    };
    if !result.changed() {
        return Ok(result);
    }
    validate(&result.after)?;
    write_text(&mut tx, &result.after).await?;
    if log {
        let payload = crate::api::undo::payloads::UpdateSettingsPayload {
            text: crate::api::undo::payloads::Transition {
                before: result.before.clone(),
                after: result.after.clone(),
            },
        };
        let ids = crate::api::undo::insert_events_on(
            &mut tx,
            &[(
                EventType::UpdateSettings,
                serde_json::to_value(&payload).expect("serializes"),
            )],
            Utc::now(),
            None,
        )
        .await?;
        result.event_id = ids.first().copied();
    }
    tx.commit().await.map_err(sqlx_error)?;
    set_cached(Some(&result.after));
    Ok(result)
}

/// Replaces the collection's settings with `text`, as `PUT /api/settings` and `spares config
/// edit` do, logged so it can be undone.
pub async fn replace(db: &SqlitePool, text: &str) -> Result<SettingsChange, Error> {
    let new = parse_document(text, "the collection's settings")?;
    edit(db, true, |doc| {
        *doc = new;
        Ok(())
    })
    .await
}

/// Sets (or with `None` removes) the values at these paths, as `PATCH /api/settings` and `spares
/// config set` do, logged so it can be undone. Each value is TOML, such as `20`, `"due"` or
/// `[60, 600]`.
pub async fn set_values(
    db: &SqlitePool,
    values: &[(Vec<String>, Option<String>)],
) -> Result<SettingsChange, Error> {
    let mut items = Vec::with_capacity(values.len());
    for (path, value) in values {
        if path.is_empty() || path.iter().any(String::is_empty) {
            return Err(invalid("a setting's name can't be empty"));
        }
        let item = match value {
            None => None,
            Some(value) => Some(
                parse_value(value).map_err(|e| invalid(format!("`{}`: {e}", path.join("."))))?,
            ),
        };
        items.push((path.clone(), item));
    }
    edit(db, true, move |doc| {
        for (path, item) in items {
            set_item(doc, &path, item);
        }
        Ok(())
    })
    .await
}

/// A TOML value written on its own, such as `20`, `"due"`, `[60, 600]` or `{ days = 3 }`.
pub fn parse_value(value: &str) -> Result<Item, String> {
    let doc = format!("v = {value}")
        .parse::<DocumentMut>()
        .map_err(|e| format!("not a TOML value: {}", e.message()))?;
    doc.get("v")
        .cloned()
        .ok_or_else(|| "not a TOML value".to_string())
}

/// Applies `before` → `after`, a change logged as an [`EventType::UpdateSettings`], to the
/// current settings key by key, for its undo (with the two swapped) and redo. Refuses, changing
/// nothing, if a key it changes has changed again since. Not logged: the undo is.
pub(crate) async fn apply_change(db: &SqlitePool, before: &str, after: &str) -> Result<(), Error> {
    let mut conflicts = Vec::new();
    edit(db, false, |doc| {
        let current = doc.to_string();
        let (merged, found) = merge3(before, &current, after, Prefer::Local)?;
        conflicts = found;
        if conflicts.is_empty() {
            *doc = parse_document(&merged, "the collection's settings")?;
        }
        Ok(())
    })
    .await?;
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(invalid(format!(
            "the settings {} changed again since. Undo the later change first, or change them \
             by hand.",
            conflicts
                .iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

/// The keys `apply_change` would refuse over: those `before` → `after` changes whose current
/// value isn't `before`'s any more.
pub(crate) fn changed_since(before: &str, after: &str) -> Result<Vec<String>, Error> {
    let current = cached().as_deref().unwrap_or_default().to_string();
    let (_, conflicts) = merge3(before, &current, after, Prefer::Local)?;
    Ok(conflicts)
}

// --- The API --------------------------------------------------------------------------------

/// `GET /api/settings`: the collection's settings, as the web app and `spares config` show them.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SettingsView {
    /// The collection's settings as written, comments included
    pub text: String,
    /// This machine's settings (the keys of [`LOCAL_KEYS`] in its `config.toml`), which can only
    /// be changed there, without the sync token
    pub local: String,
    /// Every setting in effect, defaults included, as JSON, without the sync token
    pub values: Value,
    /// Every setting's default, as JSON
    pub defaults: Value,
    pub local_keys: Vec<String>,
}

/// The current [`SettingsView`], after moving in what `config.toml` has of the collection's
/// settings.
pub async fn view(db: &SqlitePool) -> Result<SettingsView, Error> {
    load(db).await?;
    let text = cached().as_deref().unwrap_or_default().to_string();
    let local = match read_config_file()? {
        Some(file) => {
            let mut local = local_part(&file)?;
            if let Some(sync) = local.get_mut("sync").and_then(Item::as_table_like_mut) {
                sync.remove("token");
                if sync.is_empty() {
                    local.remove("sync");
                }
            }
            local.to_string()
        }
        None => String::new(),
    };
    let to_json = |config: &crate::config::SparesExternalConfig| {
        serde_json::to_value(config).map_err(|e| invalid(e.to_string()))
    };
    let mut values = to_json(&crate::config::read_external_config()?)?;
    if let Some(sync) = values.get_mut("sync").and_then(Value::as_object_mut) {
        sync.remove("token");
    }
    Ok(SettingsView {
        text,
        local,
        values,
        defaults: to_json(&crate::config::SparesExternalConfig::default())?,
        local_keys: LOCAL_KEYS.iter().map(ToString::to_string).collect(),
    })
}

/// `PUT /api/settings`: replaces the collection's settings with `text`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ReplaceSettingsRequest {
    pub text: String,
    /// The text the change was made from. If the settings are no longer it (changed on another
    /// device meanwhile, say), nothing changes and the answer is a conflict.
    #[serde(default)]
    pub base: Option<String>,
}

/// One value of `PATCH /api/settings`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SettingValue {
    /// Its path, such as `["queue", "review_order"]`
    pub key: Vec<String>,
    /// Its new value as JSON, such as `20`, `"due"` or `[60, 600]`
    #[serde(default)]
    pub value: Option<Value>,
    /// Or as TOML, as `spares config set` sends it. Neither removes the setting, so it takes its
    /// default.
    #[serde(default)]
    pub toml: Option<String>,
}

/// `PATCH /api/settings`: sets or removes some of the collection's settings.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct UpdateSettingsRequest {
    pub values: Vec<SettingValue>,
}

/// The answer to `PUT` and `PATCH /api/settings`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SettingsChangeResponse {
    pub settings: SettingsView,
    /// The keys whose values changed, with dots between their parts
    pub changed_keys: Vec<String>,
    /// The event the change was logged as, to undo it with
    pub event_id: Option<i64>,
}

/// Why a `PUT /api/settings` with a `base` changed nothing.
#[derive(Debug)]
pub enum ReplaceError {
    /// The settings are no longer `base`: they are this
    ChangedSince(String),
    Other(Error),
}

impl From<Error> for ReplaceError {
    fn from(e: Error) -> Self {
        Self::Other(e)
    }
}

async fn change_response(
    db: &SqlitePool,
    change: &SettingsChange,
) -> Result<SettingsChangeResponse, Error> {
    Ok(SettingsChangeResponse {
        settings: view(db).await?,
        changed_keys: changed_keys(&change.before, &change.after)?,
        event_id: change.event_id,
    })
}

/// Answers `PUT /api/settings`.
pub async fn replace_settings(
    db: &SqlitePool,
    request: ReplaceSettingsRequest,
) -> Result<SettingsChangeResponse, ReplaceError> {
    let new = parse_document(&request.text, "the collection's settings")?;
    let mut changed_since = None;
    let change = edit(db, true, |doc| {
        let current = doc.to_string();
        match &request.base {
            Some(base) if *base != current => changed_since = Some(current),
            _ => *doc = new,
        }
        Ok(())
    })
    .await?;
    if let Some(current) = changed_since {
        return Err(ReplaceError::ChangedSince(current));
    }
    Ok(change_response(db, &change).await?)
}

/// Answers `PATCH /api/settings`.
pub async fn update_settings(
    db: &SqlitePool,
    request: UpdateSettingsRequest,
) -> Result<SettingsChangeResponse, Error> {
    let mut values = Vec::with_capacity(request.values.len());
    for value in request.values {
        let toml = match (value.toml, value.value) {
            (Some(toml), _) => Some(toml),
            (None, None | Some(Value::Null)) => None,
            (None, Some(json)) => Some(
                json_to_toml(&json)
                    .map_err(|e| invalid(format!("`{}`: {e}", value.key.join("."))))?,
            ),
        };
        values.push((value.key, toml));
    }
    let change = set_values(db, &values).await?;
    change_response(db, &change).await
}

/// A JSON value written as TOML.
fn json_to_toml(json: &Value) -> Result<String, String> {
    use serde::Serialize as _;
    json.serialize(toml_edit::ser::ValueSerializer::new())
        .map(|value| value.to_string().trim().to_string())
        .map_err(|e| e.to_string())
}

/// A setting's name as written in TOML, such as `queue.review_order` or
/// `queue.presets."my preset".new_order`, as its parts.
pub fn parse_key(key: &str) -> Result<Vec<String>, Error> {
    toml_edit::Key::parse(key)
        .map(|keys| keys.iter().map(|key| key.get().to_string()).collect())
        .map_err(|e| invalid(format!("`{key}` is not a setting's name: {}", e.message())))
}

#[cfg(test)]
mod tests;
