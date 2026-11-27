//! Dictionaries for looking words up from cards: Yomitan dictionaries each user imports.
//!
//! - **Import** ([`import`]): a Yomitan dictionary zip (`index.json` and `term_bank_*.json`), the
//!   format `JMdict`, Jitendex, CC-CEDICT and Wiktionary-based dictionaries are published in.
//!   Nothing is bundled, since dictionaries are large and have their own licenses (`JMdict`'s
//!   CC BY-SA asks for attribution, which is shown with each entry).
//! - **Storage**: `language-tools/dictionaries.sqlite` in the collection's directory, created at
//!   the first import. It isn't the collection's database, so it isn't synced, and syncs and
//!   snapshots don't grow: each host imports its own. Its schema changes only through new
//!   [`MIGRATIONS`], counted in `PRAGMA user_version`.
//! - **Lookup** ([`lookup`]): the entries a text starts with, in the dictionaries of its language.
//!
//! Not in the browser's build: a device that keeps a copy of the collection asks its host.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

use serde::Deserialize;
use serde::Serialize;
use sqlx::Row;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

use crate::config::get_data_dir;

pub mod import;
pub mod lookup;

/// The dictionary database's migrations, in order. Only ever added to: a database at
/// `user_version` n has had the first n.
const MIGRATIONS: &[&str] = &[r"
CREATE TABLE dictionaries (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    revision TEXT NOT NULL,
    language TEXT,
    attribution TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    term_count INTEGER NOT NULL,
    imported_at INTEGER NOT NULL
) STRICT;
CREATE TABLE terms (
    dict_id INTEGER NOT NULL REFERENCES dictionaries(id),
    expression TEXT NOT NULL,
    reading TEXT NOT NULL,
    glosses_json TEXT NOT NULL,
    pos TEXT NOT NULL,
    rules TEXT NOT NULL,
    score INTEGER NOT NULL
) STRICT;
CREATE INDEX terms_expression ON terms(expression);
CREATE INDEX terms_reading ON terms(reading);
CREATE INDEX terms_dict_id ON terms(dict_id);
"];

#[derive(Debug, thiserror::Error)]
pub enum DictionaryError {
    /// What was given isn't valid, such as a zip that isn't a Yomitan dictionary.
    #[error("{0}")]
    Invalid(String),
    #[error("No dictionary with id {0}")]
    NotFound(i64),
    #[error("Dictionary database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("{description}: {source}")]
    Io {
        description: String,
        source: std::io::Error,
    },
}

/// A dictionary, as it is listed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DictionaryInfo {
    pub id: i64,
    pub title: String,
    pub revision: String,
    /// The language of its words (`ja`), if known. Lookups in that language use it, and lookups
    /// in any language use one without.
    pub language: Option<String>,
    pub attribution: String,
    pub enabled: bool,
    /// Lower first: entries from dictionaries earlier in the order are shown first.
    pub priority: i64,
    pub term_count: i64,
}

/// The current collection's dictionary database file.
pub fn database_path() -> PathBuf {
    get_data_dir()
        .join("language-tools")
        .join("dictionaries.sqlite")
}

/// The current collection's dictionary database, created and migrated if `create`. `None` if it
/// doesn't exist and isn't to be created.
pub async fn open(create: bool) -> Result<Option<SqlitePool>, DictionaryError> {
    open_at(database_path(), create).await
}

/// The dictionary database at `path` (see [`open`]). Its pool is kept for the next call.
pub async fn open_at(path: PathBuf, create: bool) -> Result<Option<SqlitePool>, DictionaryError> {
    static POOLS: Mutex<Option<HashMap<PathBuf, SqlitePool>>> = Mutex::new(None);
    if let Some(pool) = POOLS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&path)
        .cloned()
    {
        return Ok(Some(pool));
    }
    if !path.is_file() {
        if !create {
            return Ok(None);
        }
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|source| DictionaryError::Io {
            description: format!("Failed to create {}", dir.display()),
            source,
        })?;
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;
    migrate(&pool).await?;
    POOLS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(path, pool.clone());
    Ok(Some(pool))
}

async fn migrate(pool: &SqlitePool) -> Result<(), DictionaryError> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await?;
    let applied = usize::try_from(version).unwrap_or(0);
    for (i, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(*migration).execute(&mut *tx).await?;
        // A number of ours, which PRAGMA can't take as a parameter
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "PRAGMA user_version = {}",
            i + 1
        )))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

fn info_from_row(row: &sqlx::sqlite::SqliteRow) -> DictionaryInfo {
    DictionaryInfo {
        id: row.get("id"),
        title: row.get("title"),
        revision: row.get("revision"),
        language: row.get("language"),
        attribution: row.get("attribution"),
        enabled: row.get::<i64, _>("enabled") != 0,
        priority: row.get("priority"),
        term_count: row.get("term_count"),
    }
}

/// The dictionaries, in their order.
pub async fn list(pool: &SqlitePool) -> Result<Vec<DictionaryInfo>, DictionaryError> {
    let rows = sqlx::query(
        "SELECT id, title, revision, language, attribution, enabled, priority, term_count
         FROM dictionaries ORDER BY priority, id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(info_from_row).collect())
}

/// Removes the dictionary `id` and its entries.
pub async fn remove(pool: &SqlitePool, id: i64) -> Result<(), DictionaryError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM terms WHERE dict_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let removed = sqlx::query("DELETE FROM dictionaries WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(DictionaryError::NotFound(id));
    }
    tx.commit().await?;
    // Gives the space back
    sqlx::query("VACUUM").execute(pool).await?;
    Ok(())
}

/// Changes the dictionary `id`: whether lookups use it, and its language.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct UpdateDictionaryRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// An empty string for no language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

pub async fn update(
    pool: &SqlitePool,
    id: i64,
    request: &UpdateDictionaryRequest,
) -> Result<DictionaryInfo, DictionaryError> {
    if let Some(enabled) = request.enabled {
        sqlx::query("UPDATE dictionaries SET enabled = ? WHERE id = ?")
            .bind(i64::from(enabled))
            .bind(id)
            .execute(pool)
            .await?;
    }
    if let Some(language) = &request.language {
        let language = import::normalize_language(language)?;
        sqlx::query("UPDATE dictionaries SET language = ? WHERE id = ?")
            .bind(language)
            .bind(id)
            .execute(pool)
            .await?;
    }
    list(pool)
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or(DictionaryError::NotFound(id))
}

/// Puts the dictionaries in the order of `ids`. Those not in it go after, in their order.
pub async fn reorder(
    pool: &SqlitePool,
    ids: &[i64],
) -> Result<Vec<DictionaryInfo>, DictionaryError> {
    let current = list(pool).await?;
    if let Some(id) = ids.iter().find(|id| !current.iter().any(|d| d.id == **id)) {
        return Err(DictionaryError::NotFound(*id));
    }
    let order = ids
        .iter()
        .copied()
        .chain(current.iter().map(|d| d.id).filter(|id| !ids.contains(id)));
    let mut tx = pool.begin().await?;
    for (priority, id) in order.enumerate() {
        sqlx::query("UPDATE dictionaries SET priority = ? WHERE id = ?")
            .bind(i64::try_from(priority).unwrap_or(i64::MAX))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    list(pool).await
}

#[cfg(test)]
pub(crate) mod tests;
