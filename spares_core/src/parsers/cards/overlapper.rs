use crate::parsers::{ClozeData, ClozeGrouping, ClozeGroupingSettings};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct OverlapperConfig {
    pub context_before_item: u32,
    /// The number of clozes that require an answer at once
    pub prompts: u32,
    pub context_after_item: u32,
    /// Useful when you need to know the exact starting point of a sequence
    pub no_cues_for_first_item: bool,
    /// Useful when you need to know the exact ending point of a sequence
    pub no_cues_for_last_item: bool,
    /// For example, if `prompts = 4`, then it will show: 1 prompt, 2 prompt, 3 prompt, 4 prompt, 1 context 4 prompt, 2 context 4 prompt, etc. Instead of: 1 context 4 prompt, 2 context 4 prompt, etc.
    pub start_and_end_gradually: bool,
}

impl Default for OverlapperConfig {
    fn default() -> Self {
        Self {
            context_before_item: 1,
            prompts: 1,
            context_after_item: 0,
            no_cues_for_first_item: false,
            no_cues_for_last_item: false,
            start_and_end_gradually: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OverlapperGroupAssignment {
    /// Which card group (Auto grouping number) this assignment belongs to.
    pub card_group: u32,
    /// True only for the first prompt cloze in the card window. This cloze gets
    /// `skip_serialization = false` so that order numbers are written back to the note.
    pub is_first_prompt: bool,
    /// Whether this cloze is hidden (not required to answer) in this card.
    pub is_hidden: bool,
}

#[derive(Debug)]
struct Window {
    start: usize,
    end: usize,
    context_before_actual: usize,
    context_after_actual: usize,
}

/// Computes the group assignments for each of the `n` overlapper items.
///
/// Returns a `Vec` of length `n`. Each element is the list of card groups the item belongs to
/// and what role it plays. Items that fall in a visible context window have no assignment
/// (they appear as `SurroundingData` in the card).
///
/// `start_group` is the first Auto grouping number to use, allowing the caller to ensure no
/// collision with other cloze groupings in the same note.
pub fn generate_overlapper_groups(
    n: usize,
    config: &OverlapperConfig,
    start_group: u32,
) -> Vec<Vec<OverlapperGroupAssignment>> {
    let mut result: Vec<Vec<OverlapperGroupAssignment>> = vec![vec![]; n];

    if n == 0 {
        return result;
    }

    let prompts = config.prompts as usize;
    let context_before = config.context_before_item as usize;
    let context_after = config.context_after_item as usize;

    if prompts == 0 || n < prompts {
        return result;
    }

    let num_regular_cards = n - prompts + 1;
    let mut windows: Vec<Window> = Vec::new();

    if config.start_and_end_gradually {
        // Extra cards at start: gradually increase prompt window from 1 to P-1
        for extra_p in 1..prompts {
            windows.push(Window {
                start: 0,
                end: extra_p - 1,
                context_before_actual: 0,
                context_after_actual: 0,
            });
        }
    }

    // Regular sliding window cards
    for k in 0..num_regular_cards {
        let context_before_actual = if k == 0 && config.no_cues_for_first_item {
            0
        } else {
            context_before.min(k)
        };
        let context_after_actual = if k == num_regular_cards - 1 && config.no_cues_for_last_item {
            0
        } else {
            context_after.min(n - k - prompts)
        };
        windows.push(Window {
            start: k,
            end: k + prompts - 1,
            context_before_actual,
            context_after_actual,
        });
    }

    if config.start_and_end_gradually {
        // Extra cards at end: gradually decrease prompt window from P-1 to 1
        for extra_p in (1..prompts).rev() {
            windows.push(Window {
                start: n - extra_p,
                end: n - 1,
                context_before_actual: 0,
                context_after_actual: 0,
            });
        }
    }

    for (card_offset, window) in windows.iter().enumerate() {
        let card_group = start_group + card_offset as u32;
        for (i, entry) in result.iter_mut().enumerate().take(n) {
            if i >= window.start && i <= window.end {
                // Prompt: required to answer
                entry.push(OverlapperGroupAssignment {
                    card_group,
                    is_first_prompt: i == window.start,
                    is_hidden: false,
                });
            } else {
                // Check if in visible context window (no assignment needed)
                let in_context_before = window.context_before_actual > 0
                    && i < window.start
                    && i + window.context_before_actual >= window.start;
                let in_context_after = window.context_after_actual > 0
                    && i > window.end
                    && i <= window.end + window.context_after_actual;
                if !in_context_before && !in_context_after {
                    // Hidden: present in card but not required to answer
                    entry.push(OverlapperGroupAssignment {
                        card_group,
                        is_first_prompt: false,
                        is_hidden: true,
                    });
                }
            }
        }
    }
    result
}

/// Returns the total number of cards (windows) generated for `n` items with `config`.
pub fn overlapper_card_count(n: usize, config: &OverlapperConfig) -> usize {
    let p = config.prompts as usize;
    if p == 0 || n < p {
        return 0;
    }
    let num_regular = n - p + 1;
    let num_extra = if config.start_and_end_gradually && p > 1 {
        2 * (p - 1)
    } else {
        0
    };
    num_regular + num_extra
}

/// Finds all clozes in `all_clozes` with `is_overlapper = true`, computes the overlapper
/// grouping assignments using `config`, and replaces each such cloze's
/// `Vec<ClozeGroupingSettings>` with the computed entries.
///
/// Each overlapper card has exactly one "first prompt" cloze that gets
/// `skip_serialization = false`, allowing order numbers to be written back to the note.
/// All other assignments use `skip_serialization = true`.
///
/// `current_grouping_number` is advanced by the total number of overlapper cards so that
/// subsequent non-overlapper cloze numbering does not collide.
#[allow(clippy::ptr_arg)]
pub fn apply_overlapper_groupings(
    all_clozes: &mut Vec<(ClozeData, Vec<ClozeGroupingSettings>)>,
    config: &OverlapperConfig,
    current_grouping_number: &mut u32,
) -> Option<std::ops::Range<u32>> {
    // Collect positions of overlapper clozes in their document order
    let ov_indices: Vec<usize> = all_clozes
        .iter()
        .enumerate()
        .filter(|(_, (cd, _))| cd.settings.is_overlapper)
        .map(|(i, _)| i)
        .collect();

    if ov_indices.is_empty() {
        return None;
    }

    let n = ov_indices.len();
    let p = config.prompts as usize;
    if p == 0 || n < p {
        // Not enough overlapper clozes to form a card; leave original settings intact.
        return None;
    }

    let start_group = *current_grouping_number;
    let total_cards = overlapper_card_count(n, config);
    *current_grouping_number += total_cards as u32;

    let assignments = generate_overlapper_groups(n, config, start_group);

    for (ov_pos, &cloze_idx) in ov_indices.iter().enumerate() {
        let new_settings: Vec<ClozeGroupingSettings> = assignments[ov_pos]
            .iter()
            .map(|a| {
                let mut settings = ClozeGroupingSettings::default_from_grouping(
                    ClozeGrouping::Auto(a.card_group),
                    None,
                );
                settings.hidden_no_answer = a.is_hidden;
                settings.skip_serialization = !a.is_first_prompt;
                settings
            })
            .collect();
        all_clozes[cloze_idx].1 = new_settings;
    }

    Some(start_group..start_group + total_cards as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> OverlapperConfig {
        OverlapperConfig::default()
    }

    /// Summarises assignments as (card_group, is_first_prompt, is_hidden) for readability.
    fn summarise(assignments: &[Vec<OverlapperGroupAssignment>]) -> Vec<Vec<(u32, bool, bool)>> {
        assignments
            .iter()
            .map(|item| {
                item.iter()
                    .map(|a| (a.card_group, a.is_first_prompt, a.is_hidden))
                    .collect()
            })
            .collect()
    }

    /// Verify the standard example: 4 items, CB=1, P=1, CA=0.
    /// Expected cards:
    ///   Card 0 (group 0): prompt=a, hidden=[b,c,d]
    ///   Card 1 (group 1): context=[a], prompt=b, hidden=[c,d]
    ///   Card 2 (group 2): context=[b], prompt=c, hidden=[a,d]
    ///   Card 3 (group 3): context=[c], prompt=d, hidden=[a,b]
    #[test]
    fn test_standard_4_items_cb1_p1() {
        let config = OverlapperConfig {
            context_before_item: 1,
            prompts: 1,
            context_after_item: 0,
            ..default_config()
        };
        let result = generate_overlapper_groups(4, &config, 0);
        let s = summarise(&result);

        // Item 0 (a): prompt in card 0, hidden in cards 2 and 3
        assert!(s[0].contains(&(0, true, false)), "a: prompt card 0");
        assert!(s[0].contains(&(2, false, true)), "a: hidden card 2");
        assert!(s[0].contains(&(3, false, true)), "a: hidden card 3");
        assert!(
            !s[0].iter().any(|(g, _, _)| *g == 1),
            "a: no assignment card 1 (context)"
        );

        // Item 1 (b): hidden in card 0, prompt in card 1, hidden in card 3
        assert!(s[1].contains(&(0, false, true)), "b: hidden card 0");
        assert!(s[1].contains(&(1, true, false)), "b: prompt card 1");
        assert!(s[1].contains(&(3, false, true)), "b: hidden card 3");
        assert!(
            !s[1].iter().any(|(g, _, _)| *g == 2),
            "b: no assignment card 2 (context)"
        );

        // Item 2 (c): hidden in card 0 & 1, prompt in card 2
        assert!(s[2].contains(&(0, false, true)));
        assert!(s[2].contains(&(1, false, true)));
        assert!(s[2].contains(&(2, true, false)));
        assert!(
            !s[2].iter().any(|(g, _, _)| *g == 3),
            "c: no assignment card 3 (context)"
        );

        // Item 3 (d): hidden in cards 0,1,2; prompt in card 3
        assert!(s[3].contains(&(0, false, true)));
        assert!(s[3].contains(&(1, false, true)));
        assert!(s[3].contains(&(2, false, true)));
        assert!(s[3].contains(&(3, true, false)));
    }

    /// 5 items, CB=1, P=2, CA=0 — produces 4 cards (sliding window of 2).
    #[test]
    fn test_5_items_cb1_p2() {
        let config = OverlapperConfig {
            context_before_item: 1,
            prompts: 2,
            context_after_item: 0,
            ..default_config()
        };
        let result = generate_overlapper_groups(5, &config, 0);
        assert_eq!(result.len(), 5);
        // 4 regular cards (5-2+1)
        assert_eq!(overlapper_card_count(5, &config), 4);

        // Card 0 (k=0): prompts [0,1], no context (first item)
        // Item 0: first_prompt in card 0
        assert!(
            result[0]
                .iter()
                .any(|a| a.card_group == 0 && a.is_first_prompt && !a.is_hidden)
        );
        // Item 1: non-first prompt in card 0, first_prompt in card 1
        assert!(
            result[1]
                .iter()
                .any(|a| a.card_group == 0 && !a.is_first_prompt && !a.is_hidden)
        );
        assert!(
            result[1]
                .iter()
                .any(|a| a.card_group == 1 && a.is_first_prompt && !a.is_hidden)
        );
    }

    /// start_and_end_gradually with P=3 produces 2 extra cards at start and 2 at end.
    #[test]
    fn test_start_and_end_gradually_p3() {
        let config = OverlapperConfig {
            context_before_item: 1,
            prompts: 3,
            context_after_item: 0,
            start_and_end_gradually: true,
            ..default_config()
        };
        // N=6: regular cards = 6-3+1 = 4; extra = 2*(3-1) = 4; total = 8
        assert_eq!(overlapper_card_count(6, &config), 8);
        let result = generate_overlapper_groups(6, &config, 0);
        assert_eq!(result.len(), 6);
    }

    /// no_cues_for_first_item suppresses context on the first card.
    #[test]
    fn test_no_cues_for_first_item() {
        let config = OverlapperConfig {
            context_before_item: 2,
            prompts: 1,
            context_after_item: 0,
            no_cues_for_first_item: true,
            ..default_config()
        };
        let result = generate_overlapper_groups(4, &config, 0);
        // Card 0 (k=0): prompt=item0, no context before (suppressed) → items 1,2,3 are hidden
        // Without suppression, items 0 would normally be context for cards 1 and 2, but now:
        // Card 0 has no context before (k=0, no_cues_for_first_item=true → cb_actual=0)
        // So all non-prompt items in card 0 are hidden
        assert!(result[1].iter().any(|a| a.card_group == 0 && a.is_hidden));
        assert!(result[2].iter().any(|a| a.card_group == 0 && a.is_hidden));
        assert!(result[3].iter().any(|a| a.card_group == 0 && a.is_hidden));
    }

    /// Edge case: n < p returns empty assignments.
    #[test]
    fn test_n_less_than_p() {
        let config = OverlapperConfig {
            prompts: 3,
            ..default_config()
        };
        let result = generate_overlapper_groups(2, &config, 0);
        assert!(result.iter().all(|v| v.is_empty()));
    }
}
