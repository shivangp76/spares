use std::collections::HashMap;

use rand::RngExt;

use crate::Error;
use crate::parsers::ClozeGroupingSettings;
use crate::parsers::ClozeSettings;
use crate::parsers::ClozeUid;
use crate::parsers::Parseable;
use crate::parsers::construct_cloze_string;
use crate::parsers::parse_card_settings;

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
    let cloze_matches = parser.get_clozes(original_note_data)?;
    if cloze_matches.is_empty() {
        return Ok((original_note_data.to_owned(), HashMap::new()));
    }

    let note_settings_keys = parser.note_settings_keys();
    let cloze_settings_keys = parser.cloze_settings_keys();
    let mut current_grouping_number = 1u32;

    let mut mint_map: HashMap<usize, ClozeUid> = HashMap::with_capacity(cloze_matches.len());
    let mut settings_cache: HashMap<usize, (ClozeSettings, Vec<ClozeGroupingSettings>)> =
        HashMap::with_capacity(cloze_matches.len());

    let mut rng = rand::rng();

    for (idx, cloze_match) in cloze_matches.iter().enumerate() {
        let (cloze_settings, grouping_settings) = parse_card_settings(
            original_note_data,
            &cloze_match.settings_match,
            &mut current_grouping_number,
            &note_settings_keys,
            &cloze_settings_keys,
            None,
        )?;
        if cloze_settings.cloze_uid.is_none() {
            let hex = b"0123456789abcdef";
            let mut uid_bytes = [0u8; 12];
            for i in 0..6 {
                let b = rng.random::<u8>();
                uid_bytes[i * 2] = hex[(b >> 4) as usize];
                uid_bytes[i * 2 + 1] = hex[(b & 0x0f) as usize];
            }
            mint_map.insert(idx, ClozeUid(uid_bytes));
            settings_cache.insert(idx, (cloze_settings, grouping_settings));
        }
    }

    if mint_map.is_empty() {
        return Ok((original_note_data.to_owned(), mint_map));
    }

    let mut data = original_note_data.to_owned();

    // Process right-to-left so that positions before the current cloze remain valid.
    let mut indices_needing_id: Vec<usize> = mint_map.keys().copied().collect();
    indices_needing_id.sort_unstable_by(|a, b| b.cmp(a));

    for &cloze_idx in &indices_needing_id {
        let cm = &cloze_matches[cloze_idx];
        let uid = mint_map[&cloze_idx];
        let (mut cloze_settings, grouping_settings) = settings_cache.remove(&cloze_idx).unwrap();

        cloze_settings.cloze_uid = Some(uid);

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

        let body = &data[cm.start_match.end..cm.end_match.start];
        let (new_prefix, new_suffix) = parser.construct_cloze(&settings_string, body);

        let old_start_len = cm.start_match.len();
        let old_end_len = cm.end_match.len();
        let new_data_len =
            data.len() + new_prefix.len() - old_start_len + new_suffix.len() - old_end_len;
        let mut new_data = String::with_capacity(new_data_len);

        new_data.push_str(&data[..cm.start_match.start]);
        new_data.push_str(&new_prefix);
        new_data.push_str(body);
        new_data.push_str(&new_suffix);
        new_data.push_str(&data[cm.end_match.end..]);

        data = new_data;
    }

    Ok((data, mint_map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::get_all_parsers;

    #[test]
    fn test_add_cloze_uid_no_change_when_already_present() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let data = match parser.get_parser_name() {
                "markdown" => "{{[id:abc123def456] hello }} world",
                "latex" => "\\begin{cl}[id:abc123def456] hello \\end{cl} world",
                "typst" => "#cl[hello][id:abc123def456] world",
                _ => continue,
            };
            let (result, mint_map) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
            assert_eq!(result, data, "parser: {}", parser.get_parser_name());
            assert!(mint_map.is_empty(), "parser: {}", parser.get_parser_name());
        }
    }

    #[test]
    fn test_add_cloze_uid_mints_for_clozes_without_id() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let data = match parser.get_parser_name() {
                "markdown" => "{{[o:1] hello }} world {{[g:1] foo }}",
                "latex" => "\\begin{cl}[o:1] hello \\end{cl} world \\begin{cl}[g:1] foo \\end{cl}",
                "typst" => "#cl[hello][o:1] world #cl[foo][g:1]",
                _ => continue,
            };
            let (_result, mint_map) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
            assert_eq!(mint_map.len(), 2, "parser: {}", parser.get_parser_name());
            // Each uid should be 12 hex chars.
            for uid in mint_map.values() {
                assert_eq!(uid.0.len(), 12, "parser: {}", parser.get_parser_name());
                assert!(
                    uid.0.iter().all(|c| c.is_ascii_hexdigit()),
                    "parser: {}",
                    parser.get_parser_name()
                );
            }
        }
    }

    #[test]
    fn test_add_cloze_uid_idempotent() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let data = match parser.get_parser_name() {
                "markdown" => "{{[o:1] hello }} world",
                "latex" => "\\begin{cl}[o:1] hello \\end{cl} world",
                "typst" => "#cl[hello][o:1] world",
                _ => continue,
            };
            // First call mints ids
            let (result1, mint_map1) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
            assert_eq!(mint_map1.len(), 1, "parser: {}", parser.get_parser_name());

            // Second call should be a no-op
            let (result2, mint_map2) =
                add_cloze_uid_to_note_data(parser.as_ref(), &result1).unwrap();
            assert_eq!(result1, result2, "parser: {}", parser.get_parser_name());
            assert!(mint_map2.is_empty(), "parser: {}", parser.get_parser_name());
        }
    }

    #[test]
    fn test_add_cloze_uid_preserves_order_keys() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let data = match parser.get_parser_name() {
                "markdown" => "{{[o:1;g:1] hello }}",
                "latex" => "\\begin{cl}[o:1;g:1] hello \\end{cl}",
                "typst" => "#cl[hello][o:1;g:1]",
                _ => continue,
            };
            let (result, mint_map) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
            assert_eq!(mint_map.len(), 1, "parser: {}", parser.get_parser_name());
            assert!(
                result.contains("o:1"),
                "parser: {} result: {}",
                parser.get_parser_name(),
                result
            );
            assert!(
                result.contains("g:1"),
                "parser: {} result: {}",
                parser.get_parser_name(),
                result
            );
            assert!(
                result.contains("id:"),
                "parser: {} result: {}",
                parser.get_parser_name(),
                result
            );
        }
    }

    #[test]
    fn test_add_cloze_uid_mixed_existing_and_new() {
        for parser_fn in get_all_parsers() {
            let parser = parser_fn();
            let data = match parser.get_parser_name() {
                "markdown" => "{{[id:abc123def456] hello }} world {{[o:1] foo }}",
                "latex" => {
                    "\\begin{cl}[id:abc123def456] hello \\end{cl} world \\begin{cl}[o:1] foo \\end{cl}"
                }
                "typst" => "#cl[hello][id:abc123def456] world #cl[foo][o:1]",
                _ => continue,
            };
            let (_result, mint_map) = add_cloze_uid_to_note_data(parser.as_ref(), data).unwrap();
            assert_eq!(mint_map.len(), 1, "parser: {}", parser.get_parser_name());
        }
    }
}
