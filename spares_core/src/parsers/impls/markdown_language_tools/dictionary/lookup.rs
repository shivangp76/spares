//! Looking up the word a text starts with.
//!
//! - **Languages written without spaces** (Japanese, Chinese), and text in Han or kana: up to
//!   [`MAX_CHARS`] characters of the text are tried from the longest prefix to the shortest, each
//!   as written and as the dictionary forms the language's deinflector gives (`食べた` →
//!   `食べる`), against both the terms and their readings.
//! - **Languages written with spaces**: the text's first word, as written and lower case. Inflected
//!   forms are found only if the dictionary lists them, as Wiktionary-based dictionaries do.
//!
//! The dictionaries are the enabled ones of the language (the request's, or else the one the
//! text's script shows) and those without a language. With no language, all enabled ones.

use std::collections::HashMap;
use std::collections::HashSet;

use serde::Deserialize;
use serde::Serialize;
use sqlx::Row;
use sqlx::SqlitePool;

use super::DictionaryError;
use super::DictionaryInfo;
use super::list;
use crate::model::NoteId;
use crate::parsers::impls::markdown_language_tools::language::Deinflection;
use crate::parsers::impls::markdown_language_tools::language::Language;
use crate::parsers::impls::markdown_language_tools::language::is_unspaced;

/// The most characters of a text that a lookup tries.
pub const MAX_CHARS: usize = 20;

/// The most entries a lookup returns.
const MAX_ENTRIES: usize = 50;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LookupRequest {
    /// The text from the word looked up on, such as the rest of a card's line from the tapped
    /// character.
    pub text: String,
    /// The note's language, such as `ja`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct LookupResponse {
    /// How many characters of the text the longest entry matched, for highlighting them.
    pub matched: usize,
    /// The language looked up in, if any.
    pub language: Option<String>,
    /// Longest match first, then in the dictionaries' order, then by their score.
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Entry {
    pub expression: String,
    pub reading: String,
    pub glosses: Vec<String>,
    /// Its tags, such as parts of speech (`n`, `v1`), separated by spaces.
    pub tags: String,
    /// The text it matched, as written.
    pub source: String,
    /// How many characters of the text it matched.
    pub matched: usize,
    /// How the text was inflected from it, innermost first, such as `["negative", "past"]` for
    /// 食べなかった.
    pub inflections: Vec<String>,
    pub dictionary_id: i64,
    pub dictionary: String,
    pub attribution: String,
    /// The notes with the entry's expression as a keyword (see [`mark_notes`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteId>,
}

/// A form of a prefix of the text to look for.
struct Candidate {
    term: String,
    /// The prefix, as written
    source: String,
    chars: usize,
    deinflection: Option<Deinflection>,
}

/// Whether a term whose rules are `rules` (Yomitan's identifiers, separated by spaces) is the
/// kind of word `deinflection` must be.
fn rules_match(rules: &str, deinflection: Option<&Deinflection>) -> bool {
    match deinflection {
        None => true,
        Some(d) if d.rules.is_empty() => true,
        Some(d) => rules
            .split_whitespace()
            .any(|rule| d.rules.iter().any(|wanted| rule.starts_with(wanted))),
    }
}

fn candidates(text: &str, language: Option<&Language>) -> Vec<Candidate> {
    let text = text.trim_start();
    let unspaced = match language {
        Some(language) => !language.written_with_spaces,
        None => text.chars().next().is_some_and(is_unspaced),
    };
    if unspaced {
        let chars = text.chars().take(MAX_CHARS).collect::<Vec<_>>();
        let mut candidates = Vec::new();
        for len in (1..=chars.len()).rev() {
            let prefix = chars[..len].iter().collect::<String>();
            if prefix.chars().last().is_some_and(char::is_whitespace) {
                continue;
            }
            if let Some(language) = language {
                for deinflection in (language.deinflect)(&prefix) {
                    candidates.push(Candidate {
                        term: deinflection.term.clone(),
                        source: prefix.clone(),
                        chars: len,
                        deinflection: Some(deinflection),
                    });
                }
            }
            candidates.push(Candidate {
                term: prefix.clone(),
                source: prefix,
                chars: len,
                deinflection: None,
            });
        }
        candidates
    } else {
        let word = text
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '\'' | '-' | '’'))
            .collect::<String>();
        let word = word.trim_end_matches(['\'', '-', '’']).to_string();
        if word.is_empty() {
            return Vec::new();
        }
        let chars = word.chars().count();
        let lower = word.to_lowercase();
        let mut candidates = vec![Candidate {
            term: word.clone(),
            source: word.clone(),
            chars,
            deinflection: None,
        }];
        if lower != word {
            candidates.push(Candidate {
                term: lower,
                source: word,
                chars,
                deinflection: None,
            });
        }
        candidates
    }
}

/// The entries `request`'s text starts with (see the [module docs](self)).
#[allow(clippy::too_many_lines, reason = "one step after another")]
pub async fn lookup(
    pool: Option<&SqlitePool>,
    request: &LookupRequest,
) -> Result<LookupResponse, DictionaryError> {
    let language = match request.language.as_deref().filter(|l| !l.is_empty()) {
        Some(code) => Language::from_code(code),
        None => Language::detect(&request.text.chars().take(MAX_CHARS).collect::<String>()),
    };
    let language_code = request
        .language
        .as_deref()
        .filter(|l| !l.is_empty())
        .map(super::import::normalize_language)
        .transpose()?
        .flatten()
        .or_else(|| language.map(|l| l.code.to_string()));
    let Some(pool) = pool else {
        return Ok(LookupResponse {
            language: language_code,
            ..LookupResponse::default()
        });
    };
    let dictionaries: HashMap<i64, DictionaryInfo> = list(pool)
        .await?
        .into_iter()
        .filter(|d| d.enabled)
        .filter(|d| match (&language_code, &d.language) {
            (Some(wanted), Some(language)) => wanted == language,
            _ => true,
        })
        .map(|d| (d.id, d))
        .collect();
    let candidates = candidates(&request.text, language);
    if dictionaries.is_empty() || candidates.is_empty() {
        return Ok(LookupResponse {
            language: language_code,
            ..LookupResponse::default()
        });
    }

    let terms = candidates
        .iter()
        .map(|c| c.term.as_str())
        .collect::<HashSet<_>>();
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT dict_id, expression, reading, glosses_json, pos, rules, score FROM terms WHERE dict_id IN ",
    );
    builder.push_tuples(dictionaries.keys(), |mut b, id| {
        b.push_bind(*id);
    });
    builder.push(" AND (expression IN ");
    builder.push_tuples(&terms, |mut b, term| {
        b.push_bind(*term);
    });
    builder.push(" OR reading IN ");
    builder.push_tuples(&terms, |mut b, term| {
        b.push_bind(*term);
    });
    builder.push(")");
    let rows = builder.build().fetch_all(pool).await?;

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for row in &rows {
        let dict_id: i64 = row.get("dict_id");
        let expression: String = row.get("expression");
        let reading: String = row.get("reading");
        let rules: String = row.get("rules");
        // The longest candidate it is, and among those, one as written over a deinflection
        let Some(candidate) = candidates
            .iter()
            .filter(|c| c.term == expression || c.term == reading)
            .filter(|c| rules_match(&rules, c.deinflection.as_ref()))
            .max_by_key(|c| (c.chars, c.deinflection.is_none()))
        else {
            continue;
        };
        let glosses_json: String = row.get("glosses_json");
        if !seen.insert((
            dict_id,
            expression.clone(),
            reading.clone(),
            glosses_json.clone(),
        )) {
            continue;
        }
        let dictionary = &dictionaries[&dict_id];
        entries.push((
            dictionary.priority,
            row.get::<i64, _>("score"),
            Entry {
                glosses: serde_json::from_str(&glosses_json).unwrap_or_default(),
                tags: row.get("pos"),
                source: candidate.source.clone(),
                matched: candidate.chars,
                inflections: candidate
                    .deinflection
                    .as_ref()
                    .map(|d| d.reasons.iter().map(ToString::to_string).collect())
                    .unwrap_or_default(),
                dictionary_id: dict_id,
                dictionary: dictionary.title.clone(),
                attribution: dictionary.attribution.clone(),
                notes: Vec::new(),
                expression,
                reading,
            },
        ));
    }
    entries.sort_by(|(priority_a, score_a, a), (priority_b, score_b, b)| {
        b.matched
            .cmp(&a.matched)
            // As written before deinflected
            .then(a.inflections.len().cmp(&b.inflections.len()))
            .then(priority_a.cmp(priority_b))
            .then(score_b.cmp(score_a))
    });
    let entries = entries
        .into_iter()
        .map(|(_, _, entry)| entry)
        .take(MAX_ENTRIES)
        .collect::<Vec<_>>();
    Ok(LookupResponse {
        matched: entries.first().map_or(0, |e| e.matched),
        language: language_code,
        entries,
    })
}

/// Fills in each entry's [`Entry::notes`]: the notes of the collection `db` whose keywords include
/// its expression, so a client can say the word is already in them.
pub async fn mark_notes(
    db: &SqlitePool,
    response: &mut LookupResponse,
) -> Result<(), DictionaryError> {
    let expressions = response
        .entries
        .iter()
        .map(|e| e.expression.as_str())
        .collect::<HashSet<_>>();
    if expressions.is_empty() {
        return Ok(());
    }
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT DISTINCT keyword, note_id FROM note_keyword WHERE keyword IN ",
    );
    builder.push_tuples(&expressions, |mut b, expression| {
        b.push_bind(*expression);
    });
    let mut notes: HashMap<String, Vec<NoteId>> = HashMap::new();
    for row in builder.build().fetch_all(db).await? {
        notes
            .entry(row.get("keyword"))
            .or_default()
            .push(row.get("note_id"));
    }
    for entry in &mut response.entries {
        if let Some(ids) = notes.get(&entry.expression) {
            entry.notes.clone_from(ids);
        }
    }
    Ok(())
}
