//! Card orders in note text, for merging a note's text.
//!
//! A cloze names the cards it belongs to by position (`o:1,2`), and adding a cloze renumbers every
//! cloze after it. Merged as they are, two edits that each add a cloze would almost always
//! conflict. So before merging, each side's orders are replaced with the ids of the cards they
//! name on that side ([`to_card_ids`]); a cloze that only moved then reads the same on both
//! sides, and the merged text says directly which card each cloze keeps ([`from_card_ids`]).

use std::collections::HashMap;
use std::ops::Range;

use crate::Error;
use crate::model::CardId;
use crate::parsers::Parseable;

/// The prefix of a card id standing in for an order. A letter, so it cannot be read as an order.
const CARD_TOKEN: &str = "c";

/// The ranges of every order in the text's cloze settings (`1` and `2` in `o:1,2`).
fn order_ranges(parser: &dyn Parseable, text: &str) -> Result<Vec<Range<usize>>, Error> {
    let keys = parser.note_settings_keys();
    let orders_key = parser.cloze_settings_keys().orders;
    let mut ranges = Vec::new();
    for cloze in parser.get_clozes(text)? {
        let settings_start = cloze.settings_match.start;
        let settings = &text[cloze.settings_match];
        let mut offset = 0;
        for pair in settings.split(keys.settings_delim) {
            let pair_start = offset;
            offset += pair.len() + keys.settings_delim.len();
            let Some(split) = pair.find(keys.settings_key_value_delim) else {
                continue;
            };
            if pair[..split].trim() != orders_key {
                continue;
            }
            let mut item_start =
                settings_start + pair_start + split + keys.settings_key_value_delim.len();
            for item in pair[split + keys.settings_key_value_delim.len()..].split(',') {
                let trimmed = item.trim_start();
                let start = item_start + (item.len() - trimmed.len());
                let end = start + trimmed.trim_end().len();
                if end > start {
                    ranges.push(start..end);
                }
                item_start += item.len() + 1;
            }
        }
    }
    Ok(ranges)
}

fn replace_orders(
    parser: &dyn Parseable,
    text: &str,
    mut replace: impl FnMut(&str) -> Option<String>,
) -> Result<String, Error> {
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for range in order_ranges(parser, text)? {
        if let Some(replacement) = replace(&text[range.clone()]) {
            result.push_str(&text[last..range.start]);
            result.push_str(&replacement);
            last = range.end;
        }
    }
    result.push_str(&text[last..]);
    Ok(result)
}

/// Replaces each order that names a card in `cards` (order to card id) with that card's id.
pub(crate) fn to_card_ids(
    parser: &dyn Parseable,
    text: &str,
    cards: &HashMap<u32, CardId>,
) -> Result<String, Error> {
    replace_orders(parser, text, |order| {
        order
            .parse::<u32>()
            .ok()
            .and_then(|order| cards.get(&order))
            .map(|id| format!("{CARD_TOKEN}{id}"))
    })
}

/// Turns card ids written by [`to_card_ids`] back into orders, which name no card: each id gets
/// a number above every real order. Returns the text and which card each such number stands for,
/// so that after renumbering the text, a card's `previous_order` tells which card it keeps.
pub(crate) fn from_card_ids(
    parser: &dyn Parseable,
    text: &str,
) -> Result<(String, HashMap<usize, CardId>), Error> {
    const FIRST: usize = 1_000_000;
    let mut by_card: HashMap<CardId, usize> = HashMap::new();
    let mut numbers: HashMap<usize, CardId> = HashMap::new();
    let text = replace_orders(parser, text, |order| {
        let id: CardId = order.strip_prefix(CARD_TOKEN)?.parse().ok()?;
        let next = FIRST + by_card.len();
        let number = *by_card.entry(id).or_insert(next);
        numbers.insert(number, id);
        Some(number.to_string())
    })?;
    Ok((text, numbers))
}

/// Turns card ids written by [`to_card_ids`] into this note's current orders (`cards`: card id to
/// order). A card that no longer exists gets an order no card has, so editing the note creates a
/// new card for it. Orders that are numbers are kept.
pub(crate) fn to_current_orders(
    parser: &dyn Parseable,
    text: &str,
    cards: &HashMap<CardId, u32>,
) -> Result<String, Error> {
    let mut next = cards.values().copied().max().unwrap_or(0) + 1;
    let mut fresh: HashMap<CardId, u32> = HashMap::new();
    replace_orders(parser, text, |order| {
        let id: CardId = order.strip_prefix(CARD_TOKEN)?.parse().ok()?;
        let order = cards.get(&id).copied().unwrap_or_else(|| {
            *fresh.entry(id).or_insert_with(|| {
                next += 1;
                next - 1
            })
        });
        Some(order.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::find_parser;
    use crate::parsers::get_all_parsers;

    #[test]
    fn test_round_trip() {
        let parser = find_parser("markdown", &get_all_parsers()).unwrap();
        let text = "A {{[o:1] one }} B {{[o:2,3;g:1] two }} C {{ three }}";
        let cards = HashMap::from([(1, 40), (2, 48), (3, 57)]);
        let tokenized = to_card_ids(parser.as_ref(), text, &cards).unwrap();
        assert_eq!(
            tokenized,
            "A {{[o:c40] one }} B {{[o:c48,c57;g:1] two }} C {{ three }}"
        );
        let (numbered, numbers) = from_card_ids(parser.as_ref(), &tokenized).unwrap();
        assert_eq!(
            numbered,
            "A {{[o:1000000] one }} B {{[o:1000001,1000002;g:1] two }} C {{ three }}"
        );
        assert_eq!(numbers[&1_000_001], 48);
        // Orders without a card are left alone.
        assert_eq!(
            to_card_ids(parser.as_ref(), text, &HashMap::new()).unwrap(),
            text
        );
    }

    #[test]
    fn test_to_current_orders() {
        let parser = find_parser("markdown", &get_all_parsers()).unwrap();
        let text = "A {{[o:c40] one }} B {{[o:c99] two }} C {{[o:c99] three }} D {{[o:2] four }}";
        let current = HashMap::from([(40, 3), (48, 1)]);
        assert_eq!(
            to_current_orders(parser.as_ref(), text, &current).unwrap(),
            "A {{[o:3] one }} B {{[o:4] two }} C {{[o:4] three }} D {{[o:2] four }}"
        );
    }

    #[test]
    fn test_typst() {
        let parser = find_parser("typst", &get_all_parsers()).unwrap();
        let text = "A #cl[one][o:1] B #cl[two][o:2]";
        let tokenized =
            to_card_ids(parser.as_ref(), text, &HashMap::from([(1, 8), (2, 16)])).unwrap();
        assert_eq!(tokenized, "A #cl[one][o:c8] B #cl[two][o:c16]");
    }
}
