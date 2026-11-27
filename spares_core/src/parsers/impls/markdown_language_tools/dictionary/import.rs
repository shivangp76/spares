//! Importing a Yomitan dictionary zip (<https://github.com/yomidevs/yomitan>, `docs/making-yomitan-dictionaries.md`).
//!
//! Its `index.json` gives the title, revision, attribution and, in newer dictionaries,
//! `sourceLanguage`. Each `term_bank_*.json` is an array of entries,
//! `[expression, reading, definition tags, rules, score, glossary, sequence, term tags]` in format
//! 3, or with the glossary's strings after the score in format 1. A glossary's structured content
//! is kept as its text, a line per block, without images. Kanji banks and frequency lists are
//! left out.

use std::io::Cursor;
use std::io::Read;

use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;

use super::DictionaryError;
use super::DictionaryInfo;
use super::list;
use crate::parsers::impls::markdown_language_tools::language::Language;

/// Rows inserted by one statement, under SQLite's limit of variables.
const ROWS_PER_INSERT: usize = 500;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    title: String,
    revision: String,
    #[serde(default)]
    format: Option<u32>,
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    source_language: Option<String>,
    #[serde(default)]
    attribution: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// A term of a term bank.
#[derive(Debug, PartialEq)]
pub(crate) struct Term {
    pub expression: String,
    pub reading: String,
    pub glosses: Vec<String>,
    pub pos: String,
    pub rules: String,
    pub score: i64,
}

/// A language code as stored: lower case and without a region, or `None` for an empty one.
pub(crate) fn normalize_language(code: &str) -> Result<Option<String>, DictionaryError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    let base = code.split(['-', '_']).next().unwrap_or(code);
    if !(2..=3).contains(&base.len()) || !base.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(DictionaryError::Invalid(format!(
            "`{code}` is not a language code such as `ja`"
        )));
    }
    Ok(Some(base.to_ascii_lowercase()))
}

fn invalid(message: impl Into<String>) -> DictionaryError {
    DictionaryError::Invalid(message.into())
}

/// The text of a glossary entry, or `None` for an image.
fn gloss_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => match object.get("type").and_then(Value::as_str) {
            Some("text") => object
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string),
            Some("structured-content") => {
                let mut text = String::new();
                structured_text(object.get("content")?, &mut text);
                let text = text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        },
        // A deinflection, `[term, [rules]]`, in newer dictionaries
        _ => None,
    }
}

/// Appends the text of structured content to `text`, with a line break around each block.
fn structured_text(content: &Value, text: &mut String) {
    match content {
        Value::String(s) => text.push_str(s),
        Value::Array(items) => {
            for item in items {
                structured_text(item, text);
            }
        }
        Value::Object(element) => {
            let tag = element.get("tag").and_then(Value::as_str).unwrap_or("");
            match tag {
                // A ruby's reading, and images
                "rt" | "rp" | "img" => return,
                "br" => {
                    text.push('\n');
                    return;
                }
                _ => {}
            }
            let block = matches!(
                tag,
                "div" | "p" | "li" | "ul" | "ol" | "tr" | "table" | "details" | "summary"
            );
            if block {
                text.push('\n');
            }
            if tag == "li" {
                text.push_str("• ");
            }
            if let Some(content) = element.get("content") {
                structured_text(content, text);
            }
            if matches!(tag, "td" | "th") {
                text.push(' ');
            }
            if block {
                text.push('\n');
            }
        }
        _ => {}
    }
}

/// The terms of a term bank.
pub(crate) fn parse_term_bank(json: &str, format: u32) -> Result<Vec<Term>, DictionaryError> {
    let rows: Vec<Vec<Value>> =
        serde_json::from_str(json).map_err(|e| invalid(format!("Invalid term bank: {e}")))?;
    let string = |row: &[Value], i: usize| {
        row.get(i)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut terms = Vec::with_capacity(rows.len());
    for row in rows {
        let expression = string(&row, 0);
        if expression.is_empty() {
            continue;
        }
        let glosses: Vec<String> = if format >= 3 {
            row.get(5)
                .and_then(Value::as_array)
                .map(|glossary| glossary.iter().filter_map(gloss_text).collect())
                .unwrap_or_default()
        } else {
            row.iter().skip(5).filter_map(gloss_text).collect()
        };
        if glosses.is_empty() {
            continue;
        }
        let mut pos = string(&row, 2);
        if format >= 3 {
            let term_tags = string(&row, 7);
            if !term_tags.is_empty() {
                pos = format!("{pos} {term_tags}").trim().to_string();
            }
        }
        terms.push(Term {
            reading: string(&row, 1),
            pos,
            rules: string(&row, 3),
            score: row.get(4).and_then(Value::as_f64).unwrap_or(0.0) as i64,
            expression,
            glosses,
        });
    }
    Ok(terms)
}

/// Imports the Yomitan dictionary zip `zip` into the current collection's dictionaries. Its
/// language is `language`, or the one its index gives, or the one its first terms are written in.
/// A dictionary with the same title and revision is refused, and one with the same title and
/// another revision is replaced.
#[allow(clippy::too_many_lines, reason = "one step after another")]
pub async fn import(
    pool: &SqlitePool,
    zip: &[u8],
    language: Option<&str>,
) -> Result<DictionaryInfo, DictionaryError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip))
        .map_err(|e| invalid(format!("Not a zip file: {e}")))?;
    let read = |archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str| {
        let mut file = archive
            .by_name(name)
            .map_err(|e| invalid(format!("Can't read {name}: {e}")))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(|e| invalid(format!("Can't read {name}: {e}")))?;
        Ok::<_, DictionaryError>(contents)
    };
    let index: Index = serde_json::from_str(
        &read(&mut archive, "index.json")
            .map_err(|_| invalid("Not a Yomitan dictionary: the zip has no index.json"))?,
    )
    .map_err(|e| invalid(format!("Invalid index.json: {e}")))?;
    let format = index.format.or(index.version).unwrap_or(3);
    let mut bank_names = archive
        .file_names()
        .filter(|name| {
            name.starts_with("term_bank_")
                && std::path::Path::new(name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    // term_bank_2.json before term_bank_10.json
    bank_names.sort_by_key(|name| {
        name.trim_start_matches("term_bank_")
            .trim_end_matches(".json")
            .parse::<u64>()
            .unwrap_or(u64::MAX)
    });
    if bank_names.is_empty() {
        return Err(invalid(
            "The dictionary has no terms (term_bank_*.json): kanji and frequency dictionaries aren't supported",
        ));
    }

    let mut language = match language.or(index.source_language.as_deref()) {
        Some(code) => normalize_language(code)?,
        None => None,
    };
    let attribution = [
        index.attribution.clone(),
        index.author.clone().map(|a| format!("By {a}")),
        index.url.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.trim().is_empty())
    .collect::<Vec<_>>()
    .join(". ");

    if let Some(existing) = list(pool)
        .await?
        .into_iter()
        .find(|d| d.title == index.title)
    {
        if existing.revision == index.revision {
            return Err(invalid(format!(
                "{} (revision {}) is already imported",
                index.title, index.revision
            )));
        }
        super::remove(pool, existing.id).await?;
    }
    let priority: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(priority) + 1, 0) FROM dictionaries")
            .fetch_one(pool)
            .await?;
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO dictionaries
         (title, revision, language, attribution, enabled, priority, term_count, imported_at)
         VALUES (?, ?, ?, ?, 1, ?, 0, strftime('%s', 'now')) RETURNING id",
    )
    .bind(&index.title)
    .bind(&index.revision)
    .bind(&language)
    .bind(&attribution)
    .bind(priority)
    .fetch_one(&mut *tx)
    .await?;
    let mut count = 0_i64;
    for name in &bank_names {
        let terms = parse_term_bank(&read(&mut archive, name)?, format)?;
        if language.is_none() {
            let sample = terms
                .iter()
                .take(200)
                .map(|t| t.expression.as_str())
                .collect::<String>();
            language = Language::detect(&sample).map(|l| l.code.to_string());
        }
        for chunk in terms.chunks(ROWS_PER_INSERT) {
            insert_terms(&mut tx, id, chunk).await?;
        }
        count += i64::try_from(terms.len()).unwrap_or(i64::MAX);
    }
    sqlx::query("UPDATE dictionaries SET term_count = ?, language = ? WHERE id = ?")
        .bind(count)
        .bind(&language)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    sqlx::query("PRAGMA optimize").execute(pool).await?;
    list(pool)
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or(DictionaryError::NotFound(id))
}

async fn insert_terms(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    dict_id: i64,
    terms: &[Term],
) -> Result<(), DictionaryError> {
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "INSERT INTO terms (dict_id, expression, reading, glosses_json, pos, rules, score) ",
    );
    builder.push_values(terms, |mut row, term| {
        row.push_bind(dict_id)
            .push_bind(&term.expression)
            .push_bind(&term.reading)
            .push_bind(serde_json::to_string(&term.glosses).unwrap())
            .push_bind(&term.pos)
            .push_bind(&term.rules)
            .push_bind(term.score);
    });
    builder.build().execute(&mut **tx).await?;
    Ok(())
}
