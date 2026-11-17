//! Cards' own tags in note files.
//!
//! The database keeps a card's tags, and a note's stored text never has them (see
//! [`ClozeGroupingSettings::tags`](crate::parsers::ClozeGroupingSettings::tags)). A note's file
//! shows them on the cloze that keeps the card's orders, right after its `o:` key:
//!
//! ```md
//! {{[o:1;tag:hard,buried] The first card }} and {{[o:2,3;rtag:buried;r:] a card both ways }}
//! ```
//!
//! For a cloze with orders `o:2,3` (`r:`), `tag:` is the forward card's (order 2) and `rtag:` the
//! reverse card's (order 3). Image occlusions and CLI blocks keep their card settings elsewhere,
//! so their cards' tags are only in the database.

use std::collections::BTreeMap;
use std::ops::Range;

use super::Parseable;
use super::get_settings_pairs;
use super::quote_setting_value;
use super::settings::split_cloze_settings;
use crate::parsers::SettingsSyntax;
use crate::parsers::parse_tag_list;

/// Cards' own tags by card order, without the cards that have none
pub type CardTagsByOrder = BTreeMap<u32, Vec<String>>;

/// A key-value pair of a cloze's settings, by its place in the note's text
struct SettingPart<'a> {
    /// The whole part, as split at the settings delimiter
    range: Range<usize>,
    key: &'a str,
    value: &'a str,
}

/// The settings of each cloze of `data` that has any, in order. Notes that fail to parse have
/// none.
fn cloze_settings<'a>(parser: &dyn Parseable, data: &'a str) -> Vec<Vec<SettingPart<'a>>> {
    let Ok(clozes) = parser.get_clozes(data) else {
        return Vec::new();
    };
    let note_keys = parser.note_settings_keys();
    let offset = |part: &str| part.as_ptr() as usize - data.as_ptr() as usize;
    let mut settings = Vec::new();
    for cloze in clozes {
        let settings_range = cloze.settings_match;
        if settings_range.is_empty() {
            continue;
        }
        let Ok(parts) = split_cloze_settings(
            &data[settings_range.clone()],
            note_keys.settings_delim,
            note_keys.settings_key_value_delim,
        ) else {
            continue;
        };
        let mut cloze_parts: Vec<SettingPart<'a>> = Vec::new();
        for part in parts.into_iter().filter(|part| !part.trim().is_empty()) {
            let start = offset(part);
            let range = start..start + part.len();
            if let Some(Ok((key, value))) = get_settings_pairs(
                data,
                &range,
                note_keys.settings_delim,
                note_keys.settings_key_value_delim,
                SettingsSyntax::Cloze,
            )
            .into_iter()
            .next()
            {
                cloze_parts.push(SettingPart { range, key, value });
            }
        }
        if !cloze_parts.is_empty() {
            settings.push(cloze_parts);
        }
    }
    settings
}

/// The index ranges of a cloze's settings split into sections at each `g:` key, whose settings
/// belong to the same groupings
fn sections(parser: &dyn Parseable, parts: &[SettingPart<'_>]) -> Vec<Range<usize>> {
    let grouping_key = parser.cloze_settings_keys().grouping;
    let mut sections = Vec::new();
    let mut start = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.key == grouping_key && i > start {
            sections.push(start..i);
            start = i;
        }
    }
    if start < parts.len() {
        sections.push(start..parts.len());
    }
    sections
}

/// The settings of each section of each cloze of `data` (see [`sections`])
fn setting_sections<'a>(parser: &dyn Parseable, data: &'a str) -> Vec<Vec<SettingPart<'a>>> {
    cloze_settings(parser, data)
        .into_iter()
        .flat_map(|mut parts| {
            let mut split = Vec::new();
            for section in sections(parser, &parts).into_iter().rev() {
                split.push(parts.split_off(section.start));
            }
            split.reverse();
            split
        })
        .collect()
}

/// The orders in a section's `o:` key, if it has one
fn section_orders(parser: &dyn Parseable, section: &[SettingPart<'_>]) -> Option<Vec<u32>> {
    let orders_key = parser.cloze_settings_keys().orders;
    let part = section.iter().find(|part| part.key == orders_key)?;
    part.value
        .split(',')
        .map(|order| order.trim().parse::<u32>().ok())
        .collect()
}

/// `data` with each card's tags in `tags` written after its `o:` key, as `tag:` and `rtag:`.
/// Cards without tags, and cards whose orders `data` doesn't have, are left as they are.
pub fn show_card_tags(parser: &dyn Parseable, data: &str, tags: &CardTagsByOrder) -> String {
    if tags.is_empty() {
        return data.to_string();
    }
    let note_keys = parser.note_settings_keys();
    let cloze_keys = parser.cloze_settings_keys();
    let mut insertions: Vec<(usize, String)> = Vec::new();
    for section in setting_sections(parser, data) {
        let Some(orders) = section_orders(parser, &section) else {
            continue;
        };
        let Some(orders_part) = section.iter().find(|part| part.key == cloze_keys.orders) else {
            continue;
        };
        let mut text = String::new();
        for (key, order) in [cloze_keys.tags, cloze_keys.reverse_tags]
            .into_iter()
            .zip(orders)
        {
            if let Some(card_tags) = tags.get(&order).filter(|tags| !tags.is_empty()) {
                text.push_str(note_keys.settings_delim);
                text.push_str(key);
                text.push_str(note_keys.settings_key_value_delim);
                text.push_str(&quote_setting_value(
                    &card_tags.join(","),
                    note_keys.settings_delim,
                ));
            }
        }
        if !text.is_empty() {
            insertions.push((orders_part.range.end, text));
        }
    }
    let mut shown = data.to_string();
    for (at, text) in insertions.into_iter().rev() {
        shown.insert_str(at, &text);
    }
    shown
}

/// `data` without the `tag:` and `rtag:` keys of the clozes that have orders, and the tags they
/// gave each card by order. A key on a cloze without orders, such as one new to the note, is left
/// in place, since it names no card yet.
pub fn take_card_tags(parser: &dyn Parseable, data: &str) -> (String, CardTagsByOrder) {
    let cloze_keys = parser.cloze_settings_keys();
    let mut tags = CardTagsByOrder::new();
    let mut removals: Vec<Range<usize>> = Vec::new();
    for parts in cloze_settings(parser, data) {
        let mut removed = vec![false; parts.len()];
        for section in sections(parser, &parts) {
            let Some(orders) = section_orders(parser, &parts[section.clone()]) else {
                continue;
            };
            for i in section {
                let part = &parts[i];
                let order = if part.key == cloze_keys.tags {
                    orders.first()
                } else if part.key == cloze_keys.reverse_tags {
                    orders.get(1)
                } else {
                    continue;
                };
                if let Some(order) = order {
                    let card_tags = parse_tag_list(part.value);
                    if !card_tags.is_empty() {
                        tags.insert(*order, card_tags);
                    }
                }
                removed[i] = true;
            }
        }
        for (i, part) in parts.iter().enumerate().filter(|(i, _)| removed[*i]) {
            // With the delimiter before it, or, when nothing before it is kept, the one after it
            let removal = if removed[..i].iter().any(|removed| !removed) {
                parts[i - 1].range.end..part.range.end
            } else if let Some(next) = parts.get(i + 1) {
                let next_text = &data[next.range.clone()];
                let spaces = next_text.len() - next_text.trim_start().len();
                part.range.start..next.range.start + spaces
            } else {
                part.range.clone()
            };
            removals.push(removal);
        }
    }
    // Removals of neighboring keys overlap where they share a delimiter
    removals.sort_by_key(|removal| removal.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for removal in removals {
        match merged.last_mut() {
            Some(last) if removal.start <= last.end => last.end = last.end.max(removal.end),
            _ => merged.push(removal),
        }
    }
    let mut taken = data.to_string();
    for removal in merged.into_iter().rev() {
        taken.replace_range(removal, "");
    }
    (taken, tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::get_all_parsers;

    fn parser(name: &str) -> Box<dyn Parseable> {
        get_all_parsers()
            .into_iter()
            .map(|f| f())
            .find(|p| p.get_parser_name() == name)
            .unwrap()
    }

    fn tags(entries: &[(u32, &[&str])]) -> CardTagsByOrder {
        entries
            .iter()
            .map(|(order, tags)| (*order, tags.iter().map(ToString::to_string).collect()))
            .collect()
    }

    #[test]
    fn card_tags_are_shown_after_the_orders_and_taken_back() {
        let parser = parser("markdown");
        let data = "{{[o:1;g:1] a }} {{[o:2] b }} {{[g:1] c }} {{[o:3,4;r:] d }}";
        let card_tags = tags(&[
            (1, &["hard", "math:algebra"]),
            (3, &["x"]),
            (4, &["buried"]),
        ]);
        let shown = show_card_tags(parser.as_ref(), data, &card_tags);
        assert_eq!(
            shown,
            "{{[o:1;tag:hard,math:algebra;g:1] a }} {{[o:2] b }} {{[g:1] c }} \
             {{[o:3,4;tag:x;rtag:buried;r:] d }}"
        );
        assert_eq!(
            take_card_tags(parser.as_ref(), &shown),
            (data.to_string(), card_tags)
        );
    }

    #[test]
    fn taking_card_tags_removes_the_keys_wherever_they_are() {
        let parser = parser("markdown");
        let (data, card_tags) =
            take_card_tags(parser.as_ref(), "{{[tag:a ; o:1] a }} {{[o:2;tag:] b }}");
        assert_eq!(data, "{{[o:1] a }} {{[o:2] b }}");
        assert_eq!(card_tags, tags(&[(1, &["a"])]));
        let (data, card_tags) = take_card_tags(parser.as_ref(), "{{[tag:a;rtag:b; o:1,2;r:] a }}");
        assert_eq!(data, "{{[o:1,2;r:] a }}");
        assert_eq!(card_tags, tags(&[(1, &["a"]), (2, &["b"])]));
    }

    #[test]
    fn a_tag_on_a_cloze_without_orders_stays() {
        let parser = parser("markdown");
        let data = "{{[tag:a] new }}";
        assert_eq!(
            take_card_tags(parser.as_ref(), data),
            (data.to_string(), CardTagsByOrder::new())
        );
    }

    #[test]
    fn card_tags_with_the_settings_delimiter_are_quoted() {
        let parser = parser("markdown");
        let card_tags = tags(&[(1, &["a;b"])]);
        let shown = show_card_tags(parser.as_ref(), "{{[o:1] a }}", &card_tags);
        assert_eq!(shown, r#"{{[o:1;tag:"a;b"] a }}"#);
        assert_eq!(take_card_tags(parser.as_ref(), &shown).1, card_tags);
    }

    #[test]
    fn every_parser_round_trips_card_tags() {
        for parser in get_all_parsers().into_iter().map(|f| f()) {
            let Ok((data, cards)) = crate::parsers::add_order_to_note_data(
                parser.as_ref(),
                &cloze_note(&*parser),
                None,
            ) else {
                continue;
            };
            if cards.is_empty() {
                continue;
            }
            let card_tags = tags(&[(1, &["hard"])]);
            let shown = show_card_tags(parser.as_ref(), &data, &card_tags);
            assert_ne!(shown, data, "{}", parser.get_parser_name());
            assert_eq!(
                take_card_tags(parser.as_ref(), &shown),
                (data.clone(), card_tags),
                "{}",
                parser.get_parser_name()
            );
            // And importing the shown text reads them, then leaves them out of the note's text
            let (stored, cards) =
                crate::parsers::add_order_to_note_data(parser.as_ref(), &shown, None).unwrap();
            assert_eq!(stored, data, "{}", parser.get_parser_name());
            assert_eq!(cards[0].tags, Some(vec!["hard".to_string()]));
        }
    }

    /// A note with one cloze in the syntax of `parser`
    fn cloze_note(parser: &dyn Parseable) -> String {
        let (start, end) = parser.construct_cloze("");
        format!("The capital of France is {start}Paris{end}.")
    }
}
