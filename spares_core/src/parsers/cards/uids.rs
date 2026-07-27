use std::collections::HashMap;

use rand::RngExt;

use crate::Error;
use crate::parsers::ClozeGroupingSettings;
use crate::parsers::ClozeSettings;
use crate::parsers::ClozeUid;
use crate::parsers::Parseable;
use crate::parsers::construct_cloze_string;
use crate::parsers::parse_card_settings;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Internal: mint a 12-hex-char cloze uid using a single `random::<u64>()` call.
fn mint_cloze_uid(rng: &mut impl rand::Rng) -> ClozeUid {
    let val: u64 = rng.random();
    let mut uid_bytes = [0u8; 12];
    uid_bytes[0] = HEX[((val >> 44) & 0x0f) as usize];
    uid_bytes[1] = HEX[((val >> 40) & 0x0f) as usize];
    uid_bytes[2] = HEX[((val >> 36) & 0x0f) as usize];
    uid_bytes[3] = HEX[((val >> 32) & 0x0f) as usize];
    uid_bytes[4] = HEX[((val >> 28) & 0x0f) as usize];
    uid_bytes[5] = HEX[((val >> 24) & 0x0f) as usize];
    uid_bytes[6] = HEX[((val >> 20) & 0x0f) as usize];
    uid_bytes[7] = HEX[((val >> 16) & 0x0f) as usize];
    uid_bytes[8] = HEX[((val >> 12) & 0x0f) as usize];
    uid_bytes[9] = HEX[((val >> 8) & 0x0f) as usize];
    uid_bytes[10] = HEX[((val >> 4) & 0x0f) as usize];
    uid_bytes[11] = HEX[(val & 0x0f) as usize];
    ClozeUid(uid_bytes)
}

struct ClozeEdit {
    start_match_start: usize,
    start_match_end: usize,
    end_match_start: usize,
    end_match_end: usize,
    new_prefix: String,
    new_suffix: String,
}

/// Internal: iterate over clozes, select those matching `select`,
/// mutate them via `mutate`, and rebuild the data string in a single pass.
fn reapply_cloze_uid<T>(
    parser: &dyn Parseable,
    original_note_data: &str,
    select: impl Fn(&ClozeSettings) -> bool,
    mut mutate: impl FnMut(usize, &mut ClozeSettings) -> T,
) -> Result<(String, HashMap<usize, T>), Error> {
    let cloze_matches = parser.get_clozes(original_note_data)?;
    if cloze_matches.is_empty() {
        return Ok((original_note_data.to_owned(), HashMap::new()));
    }

    let note_settings_keys = parser.note_settings_keys();
    let cloze_settings_keys = parser.cloze_settings_keys();
    let mut current_grouping_number = 1u32;

    let mut selected: HashMap<usize, T> = HashMap::new();
    let mut settings_cache: Vec<Option<(ClozeSettings, Vec<ClozeGroupingSettings>)>> =
        vec![None; cloze_matches.len()];

    for (idx, cloze_match) in cloze_matches.iter().enumerate() {
        let (cloze_settings, grouping_settings) = parse_card_settings(
            original_note_data,
            &cloze_match.settings_match,
            &mut current_grouping_number,
            &note_settings_keys,
            &cloze_settings_keys,
            None,
        )?;
        if select(&cloze_settings) {
            let mut settings = cloze_settings;
            let t = mutate(idx, &mut settings);
            selected.insert(idx, t);
            settings_cache[idx] = Some((settings, grouping_settings));
        }
    }

    if selected.is_empty() {
        return Ok((original_note_data.to_owned(), selected));
    }

    // Build edits sorted by position (ascending) for a single left-to-right pass.
    let mut edits: Vec<(usize, ClozeEdit)> = Vec::new();
    {
        let mut indices: Vec<usize> = selected.keys().copied().collect();
        indices.sort_unstable();

        for &cloze_idx in &indices {
            let cm = &cloze_matches[cloze_idx];
            let (cloze_settings, grouping_settings) =
                settings_cache[cloze_idx].take().ok_or_else(|| {
                    Error::Library(crate::LibraryError::Parser(
                        crate::ParserErrorKind::FailedToGuess(format!(
                            "cloze index {cloze_idx} selected but missing from settings cache"
                        )),
                    ))
                })?;

            let settings_string = construct_cloze_string(
                &cloze_settings,
                &grouping_settings,
                &cloze_settings_keys,
                note_settings_keys.settings_delim,
                note_settings_keys.settings_key_value_delim,
                None,
                note_settings_keys.groupings_all,
                false,
            );

            let (new_prefix, new_suffix) = parser.construct_cloze(
                &settings_string,
                &original_note_data[cm.start_match.end..cm.end_match.start],
            );

            edits.push((
                cloze_idx,
                ClozeEdit {
                    start_match_start: cm.start_match.start,
                    start_match_end: cm.start_match.end,
                    end_match_start: cm.end_match.start,
                    end_match_end: cm.end_match.end,
                    new_prefix,
                    new_suffix,
                },
            ));
        }
    }

    // Single left-to-right pass building the output string.
    edits.sort_unstable_by_key(|(_, e)| e.start_match_start);
    let mut data = String::with_capacity(original_note_data.len());
    let mut last_end = 0;
    for (_, edit) in &edits {
        data.push_str(&original_note_data[last_end..edit.start_match_start]);
        data.push_str(&edit.new_prefix);
        data.push_str(&original_note_data[edit.start_match_end..edit.end_match_start]);
        data.push_str(&edit.new_suffix);
        last_end = edit.end_match_end;
    }
    data.push_str(&original_note_data[last_end..]);

    Ok((data, selected))
}

/// Strips `id:` keys from every cloze in `data` that carries one.
///
/// Returns the modified note data and a list of cloze indices that were stripped.
/// Clozes without an `id:` key are left unchanged.
///
/// Idempotent: repeated calls with the same data produce the same result
/// (clozes without an `id:` are skipped, and once stripped, subsequent calls
/// change nothing).
pub fn remove_cloze_uid_from_note_data(
    parser: &dyn Parseable,
    original_note_data: &str,
) -> Result<(String, Vec<usize>), Error> {
    let (data, selected) = reapply_cloze_uid(
        parser,
        original_note_data,
        |s| s.cloze_uid.is_some(),
        |_, s| s.cloze_uid = None,
    )?;
    let mut indices: Vec<usize> = selected.into_keys().collect();
    indices.sort_unstable();
    Ok((data, indices))
}

/// Mints 12-hex `id:` keys for any cloze in `data` that does not already have one.
///
/// Returns the modified note data and a map from cloze index to the new uid
/// for each cloze that was assigned one. Clozes that already carried an `id:`
/// key are left unchanged and do not appear in the map.
///
/// Idempotent: repeated calls with the same data produce the same result
/// (clozes that already have an `id:` are skipped).
pub fn add_cloze_uid_to_note_data(
    parser: &dyn Parseable,
    original_note_data: &str,
) -> Result<(String, HashMap<usize, ClozeUid>), Error> {
    let mut rng = rand::rng();
    reapply_cloze_uid(
        parser,
        original_note_data,
        |s| s.cloze_uid.is_none(),
        |_, s| {
            let uid = mint_cloze_uid(&mut rng);
            s.cloze_uid = Some(uid);
            uid
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::get_all_parsers;

    fn sample(parser_name: &str, scenario: &str) -> Option<&'static str> {
        Some(match (parser_name, scenario) {
            ("markdown", "with_id") => "{{[id:abc123def456] hello }} world",
            ("latex", "with_id") => "\\begin{cl}[id:abc123def456] hello \\end{cl} world",
            ("typst", "with_id") => "#cl[hello][id:abc123def456] world",

            ("markdown", "no_id") => "{{[o:1] hello }} world",
            ("latex", "no_id") => "\\begin{cl}[o:1] hello \\end{cl} world",
            ("typst", "no_id") => "#cl[hello][o:1] world",

            ("markdown", "two_no_ids") => "{{[o:1] hello }} world {{[g:1] foo }}",
            ("latex", "two_no_ids") => {
                "\\begin{cl}[o:1] hello \\end{cl} world \\begin{cl}[g:1] foo \\end{cl}"
            }
            ("typst", "two_no_ids") => "#cl[hello][o:1] world #cl[foo][g:1]",

            ("markdown", "no_id_plus_other_keys") => "{{[o:1;g:1] hello }}",
            ("latex", "no_id_plus_other_keys") => "\\begin{cl}[o:1;g:1] hello \\end{cl}",
            ("typst", "no_id_plus_other_keys") => "#cl[hello][o:1;g:1]",

            ("markdown", "has_id_plus_other_keys") => "{{[id:abc123def456;o:1;g:1] hello }}",
            ("latex", "has_id_plus_other_keys") => {
                "\\begin{cl}[id:abc123def456;o:1;g:1] hello \\end{cl}"
            }
            ("typst", "has_id_plus_other_keys") => "#cl[hello][id:abc123def456;o:1;g:1]",

            ("markdown", "mixed") => "{{[id:abc123def456] hello }} world {{[o:1] foo }}",
            ("latex", "mixed") => {
                "\\begin{cl}[id:abc123def456] hello \\end{cl} world \\begin{cl}[o:1] foo \\end{cl}"
            }
            ("typst", "mixed") => "#cl[hello][id:abc123def456] world #cl[foo][o:1]",

            ("markdown", "two_ids") => "{{[id:abc111111111] first }} {{[id:abc222222222] second }}",
            ("latex", "two_ids") => {
                "\\begin{cl}[id:abc111111111] first \\end{cl} \\begin{cl}[id:abc222222222] second \\end{cl}"
            }
            ("typst", "two_ids") => "#cl[first][id:abc111111111] #cl[second][id:abc222222222]",

            ("markdown", "two_ids_one_no_id") => {
                "{{[id:abc111111111] a }} {{[o:1] b }} {{[id:abc222222222] c }}"
            }
            ("latex", "two_ids_one_no_id") => {
                "\\begin{cl}[id:abc111111111] a \\end{cl} \\begin{cl}[o:1] b \\end{cl} \\begin{cl}[id:abc222222222] c \\end{cl}"
            }
            ("typst", "two_ids_one_no_id") => {
                "#cl[a][id:abc111111111] #cl[b][o:1] #cl[c][id:abc222222222]"
            }

            ("markdown", "surrounding") => "pre {{[o:1] A B }} C {{[g:1] D E }} post",
            ("latex", "surrounding") => {
                "pre \\begin{cl}[o:1] A B \\end{cl} C \\begin{cl}[g:1] D E \\end{cl} post"
            }
            ("typst", "surrounding") => "pre #cl[A B][o:1] C #cl[D E][g:1] post",

            _ => return None,
        })
    }

    #[test]
    fn no_change_when_opposite_state() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let pname = parser.get_parser_name();

            if let Some(data) = sample(pname, "with_id") {
                let (result, mint_map) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(result, data, "add: {pname}");
                assert!(mint_map.is_empty(), "add: {pname}");
            }

            if let Some(data) = sample(pname, "no_id") {
                let (result, stripped) =
                    remove_cloze_uid_from_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(result, data, "strip: {pname}");
                assert!(stripped.is_empty(), "strip: {pname}");
            }
        }
    }

    #[test]
    fn applies_to_target_clozes() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let pname = parser.get_parser_name();

            if let Some(data) = sample(pname, "two_no_ids") {
                let (_result, mint_map) =
                    add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(mint_map.len(), 2, "add: {pname}");
                for uid in mint_map.values() {
                    assert_eq!(uid.0.len(), 12, "add: {pname}");
                    assert!(uid.0.iter().all(|c| c.is_ascii_hexdigit()), "add: {pname}");
                }
            }

            if let Some(data) = sample(pname, "two_ids") {
                let (result, stripped) =
                    remove_cloze_uid_from_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(stripped.len(), 2, "strip: {pname}");
                assert!(!result.contains("id:"), "strip: {pname}");
                assert!(result.contains("first"), "strip: {pname}");
                assert!(result.contains("second"), "strip: {pname}");
            }
        }
    }

    #[test]
    fn idempotent() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let pname = parser.get_parser_name();

            if let Some(data) = sample(pname, "no_id") {
                let (result1, mint_map1) =
                    add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(mint_map1.len(), 1, "add first: {pname}");
                let (result2, mint_map2) =
                    add_cloze_uid_to_note_data(parser.as_ref(), &result1).unwrap();
                assert_eq!(result1, result2, "add: {pname}");
                assert!(mint_map2.is_empty(), "add second: {pname}");
            }

            if let Some(data) = sample(pname, "with_id") {
                let (result1, stripped1) =
                    remove_cloze_uid_from_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(stripped1.len(), 1, "strip first: {pname}");
                assert!(!result1.contains("id:"), "strip: {pname}");
                let (result2, stripped2) =
                    remove_cloze_uid_from_note_data(parser.as_ref(), &result1).unwrap();
                assert_eq!(result1, result2, "strip: {pname}");
                assert!(stripped2.is_empty(), "strip second: {pname}");
            }
        }
    }

    #[test]
    fn preserves_other_keys() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let pname = parser.get_parser_name();

            if let Some(data) = sample(pname, "no_id_plus_other_keys") {
                let (result, mint_map) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(mint_map.len(), 1, "add: {pname}");
                assert!(result.contains("o:1"), "add: {pname} result: {result}");
                assert!(result.contains("g:1"), "add: {pname} result: {result}");
                assert!(result.contains("id:"), "add: {pname} result: {result}");
            }

            if let Some(data) = sample(pname, "has_id_plus_other_keys") {
                let (result, stripped) =
                    remove_cloze_uid_from_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(stripped.len(), 1, "strip: {pname}");
                assert!(!result.contains("id:"), "strip: {pname} result: {result}");
                assert!(result.contains("o:1"), "strip: {pname} result: {result}");
                assert!(result.contains("g:1"), "strip: {pname} result: {result}");
            }
        }
    }

    #[test]
    fn mixed() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let pname = parser.get_parser_name();

            if let Some(data) = sample(pname, "mixed") {
                let (_result, mint_map) =
                    add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(mint_map.len(), 1, "add: {pname}");
            }

            if let Some(data) = sample(pname, "two_ids_one_no_id") {
                let (result, stripped) =
                    remove_cloze_uid_from_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(stripped.len(), 2, "strip: {pname}");
                assert!(!result.contains("id:"), "strip: {pname} result: {result}");
            }
        }
    }

    #[test]
    fn preserves_surrounding_content() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let pname = parser.get_parser_name();

            if let Some(data) = sample(pname, "surrounding") {
                let (result, mint_map) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
                assert_eq!(mint_map.len(), 2, "add: {pname}");

                assert!(result.contains("pre"), "add {pname}: missing 'pre'");
                assert!(result.contains("A B"), "add {pname}: missing 'A B'");
                assert!(result.contains(" C "), "add {pname}: missing ' C '");
                assert!(result.contains("D E"), "add {pname}: missing 'D E'");
                assert!(result.contains(" post"), "add {pname}: missing ' post'");
                assert_eq!(
                    result.matches("id:").count(),
                    2,
                    "add {pname}: expected 2 id: keys"
                );

                let pre_pos = result.find("pre").unwrap();
                let ab_pos = result.find("A B").unwrap();
                let c_pos = result.find(" C ").unwrap();
                assert!(
                    pre_pos < ab_pos,
                    "add {pname}: 'pre' should come before 'A B'"
                );
                assert!(
                    ab_pos < c_pos,
                    "add {pname}: 'A B' should come before ' C '"
                );

                let (result2, mint_map2) =
                    add_cloze_uid_to_note_data(parser.as_ref(), &result).unwrap();
                assert_eq!(result, result2, "add {pname}: idempotency failed");
                assert!(
                    mint_map2.is_empty(),
                    "add {pname}: second pass should mint nothing"
                );

                let (stripped, stripped_indices) =
                    remove_cloze_uid_from_note_data(parser.as_ref(), &result).unwrap();
                assert_eq!(stripped_indices.len(), 2, "strip {pname}");
                assert_eq!(
                    stripped, data,
                    "strip {pname}: strip(add(original)) should equal original"
                );
                assert!(
                    !stripped.contains("id:"),
                    "strip {pname}: id: keys should be stripped"
                );
            }
        }
    }
}
