//! # CLI cloze type
//!
//! Like image occlusion, this is *not* a parser. It is a cloze type embedded
//! natively in any host parser's note data. Each `spares: cli start…end` block
//! produces exactly one card whose review is driven by spawning an external
//! command rather than rendering a document.
//!
//! ## Block format
//! Inside a host parser's comment syntax (`<!--- … --->` for markdown, `% …`
//! for latex, `// …` for typst), the block looks like:
//! ```md
//! Some surrounding prompt text.
//! <!--- spares: cli start --->
//! <!--- exec = "pytest tests/" --->
//! <!--- spares: cli end --->
//! ```
//! The only required key is `exec`. The surrounding note text outside the
//! block is displayed in the terminal before exec runs.
//!
//! ## Score contract
//! The child process owns stdin/stderr (so it may prompt interactively) and
//! must emit a single trailing JSON object on stdout of the form
//! `{"score": <float in [0,1]>}`. Any other trailing line shape is an error.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;
use std::sync::OnceLock;

use serde::Deserialize;
use serde::Serialize;
use toml_edit::DocumentMut;

use crate::LibraryError;
use crate::NoteErrorKind;
use crate::parsers::Parseable;

/// Structured data parsed from a CLI block body.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CliData {
    /// The shell command (`sh -c "<exec>"`) run at review time.
    pub exec: String,
}

/// A range match for a CLI block, mirroring [`crate::parsers::image_occlusion::ImageOcclusionMatch`].
#[derive(Clone, Debug, PartialEq)]
pub struct CliBlockMatch {
    /// Full block range (including start/end delimiters).
    pub range: Range<usize>,
    /// Range of the body between start and end delimiters (the `key = "value"` lines).
    pub body_range: Range<usize>,
}

pub(crate) fn get_or_compile_cli_regex(pattern: &str) -> Result<fancy_regex::Regex, LibraryError> {
    type Cache = Mutex<HashMap<String, fancy_regex::Regex>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap();
    if let Some(re) = map.get(pattern) {
        return Ok(re.clone());
    }
    let re = fancy_regex::Regex::new(pattern).map_err(|e| {
        LibraryError::Note(NoteErrorKind::Other {
            description: e.to_string(),
        })
    })?;
    map.insert(pattern.to_string(), re.clone());
    Ok(re)
}

/// Scan for any occurrence of `start_marker` in `data` that is not inside
/// one of the `matched_ranges`, or that occurs *inside* the body of a
/// matched block (indicating a phantom cross-block capture). Returns an
/// error describing the first unterminated or phantom marker found. Used
/// by both the free-function and trait-default implementations of
/// `get_cli_blocks`.
pub(crate) fn check_unterminated_blocks(
    data: &str,
    start_marker: &str,
    matched_ranges: &[Range<usize>],
) -> Result<(), LibraryError> {
    // 1. Check for start markers not covered by any matched range
    //    (unterminated blocks).
    let mut remaining = data;
    let mut search_offset: usize = 0;
    while let Some(pos) = remaining.find(start_marker) {
        let abs_pos = search_offset + pos;
        if !matched_ranges.iter().any(|b| b.contains(&abs_pos)) {
            return Err(LibraryError::Note(NoteErrorKind::InvalidSettings {
                description: format!(
                    "Unterminated `spares: cli start` marker at byte {} — \
                     a matching `spares: cli end` is required.",
                    abs_pos,
                ),
                advice: Some(
                    "Add a matching `spares: cli end` comment after the CLI block body."
                        .to_string(),
                ),
                src: data.to_string(),
                at: (abs_pos..abs_pos + start_marker.len()).into(),
            }));
        }
        let advance = pos + start_marker.len();
        search_offset += advance;
        remaining = &remaining[advance..];
    }

    // 2. Check for phantom cross-block captures: a start marker appearing
    //    inside a matched block's body (after its opening start marker)
    //    means a previous block is missing its end delimiter, causing the
    //    regex to fuse two intended blocks into one.
    for block in matched_ranges {
        let body_start = block.start + start_marker.len();
        if body_start < block.end {
            let body = &data[body_start..block.end];
            if let Some(inner_pos) = body.find(start_marker) {
                let abs_inner = body_start + inner_pos;
                return Err(LibraryError::Note(NoteErrorKind::InvalidSettings {
                    description: format!(
                        "A CLI block starting at byte {} contains another `spares: cli start` \
                         marker at byte {} — a previous `spares: cli start` block may be missing \
                         its `spares: cli end`.",
                        block.start, abs_inner,
                    ),
                    advice: Some(
                        "Ensure every `spares: cli start` has a matching `spares: cli end`."
                            .to_string(),
                    ),
                    src: data.to_string(),
                    at: (abs_inner..abs_inner + start_marker.len()).into(),
                }));
            }
        }
    }

    Ok(())
}

/// Parse all `spares: cli start…end` blocks out of `data`. Returns one
/// [`CliBlockMatch`] per block, ordered by occurrence. The host parser's
/// `construct_comment` / `extract_comment` are used so this works for any
/// parser (markdown, latex, typst).
///
/// An unterminated `spares: cli start` (one without a matching `spares:
/// cli end`) produces an error — this is a strict parse, not a best-effort
/// scan.
pub fn get_cli_blocks(
    parser: &dyn Parseable,
    data: &str,
) -> Result<Vec<CliBlockMatch>, LibraryError> {
    let start = parser.construct_comment("spares: cli start");
    let end = parser.construct_comment("spares: cli end");
    let regex_string = format!(
        "(?s){}(.*?)\n{}",
        fancy_regex::escape(&start),
        fancy_regex::escape(&end),
    );
    let cli_regex = get_or_compile_cli_regex(&regex_string)?;
    let mut blocks = Vec::new();
    for captures in cli_regex.captures_iter(data) {
        let captures = captures.map_err(|e| {
            LibraryError::Note(NoteErrorKind::Other {
                description: e.to_string(),
            })
        })?;
        let full = captures.get(0).ok_or_else(|| {
            LibraryError::Note(NoteErrorKind::Other {
                description: "cli block regex produced no full match".to_string(),
            })
        })?;
        let body = captures.get(1).ok_or_else(|| {
            LibraryError::Note(NoteErrorKind::Other {
                description: "cli block regex produced no body capture".to_string(),
            })
        })?;
        blocks.push(CliBlockMatch {
            range: full.start()..full.end(),
            body_range: body.start()..body.end(),
        });
    }

    let ranges: Vec<Range<usize>> = blocks.iter().map(|b| b.range.clone()).collect();
    check_unterminated_blocks(data, &start, &ranges)?;

    Ok(blocks)
}

/// Parse the body of a CLI block (the `key = "value"` comment lines) into a
/// [`CliData`]. `body` should be the substring captured between the start/end
/// delimiters (i.e. [`CliBlockMatch::body_range`]).
pub fn parse_cli_block_body(
    parser: &dyn Parseable,
    body: &str,
    block_range: &Range<usize>,
    src: &str,
) -> Result<CliData, LibraryError> {
    let toml_str: String = body
        .lines()
        .map(|line| parser.extract_comment(line).trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if toml_str.is_empty() {
        return Err(LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: "CLI block is missing required `exec` setting".to_string(),
            advice: Some(
                "Add a commented line of the form `exec = \"<command>\"` inside the block."
                    .to_string(),
            ),
            src: src.to_string(),
            at: (block_range.start..block_range.start).into(),
        }));
    }
    let doc = toml_str.parse::<DocumentMut>().map_err(|e| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!("Failed to parse CLI block: {e}"),
            advice: None,
            src: src.to_string(),
            at: block_range.clone().into(),
        })
    })?;
    let cli_data: CliData = toml_edit::de::from_document(doc).map_err(|e| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!("Failed to parse CLI block: {e}"),
            advice: Some(
                "Add a commented line of the form `exec = \"<command>\"` inside the block."
                    .to_string(),
            ),
            src: src.to_string(),
            at: block_range.clone().into(),
        })
    })?;
    Ok(cli_data)
}

/// Parse all CLI blocks in `data` into structured [`CliData`] + range info,
/// ordered by occurrence. Convenience wrapper around [`get_cli_blocks`] +
/// [`parse_cli_block_body`].
pub fn parse_cli_data(
    parser: &dyn Parseable,
    data: &str,
) -> Result<Vec<(CliData, Range<usize>)>, LibraryError> {
    let blocks = get_cli_blocks(parser, data)?;
    blocks
        .into_iter()
        .map(|m| {
            let body = &data[m.body_range.clone()];
            let cli_data = parse_cli_block_body(parser, body, &m.range, data)?;
            Ok((cli_data, m.range))
        })
        .collect()
}

/// Compute the surrounding note text by stripping all CLI block ranges.
/// Returns the concatenation of text outside any CLI block.
pub fn compute_surrounding_text(data: &str, cli_blocks: &[(CliData, Range<usize>)]) -> String {
    let mut last_end: usize = 0;
    let mut surrounding_parts: Vec<String> = Vec::new();
    for (_, range) in cli_blocks {
        if range.start > last_end {
            surrounding_parts.push(data[last_end..range.start].to_string());
        }
        last_end = range.end;
    }
    if last_end < data.len() {
        surrounding_parts.push(data[last_end..].to_string());
    }
    surrounding_parts.join("")
}

#[cfg(test)]
mod test;
