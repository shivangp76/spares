use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::Range;

use itertools::Itertools;

use crate::CardErrorKind;
use crate::LibraryError;
use crate::helpers::GroupByInsertion;
use crate::parsers::BackReveal;
use crate::parsers::ClozeData;
use crate::parsers::ClozeGrouping;
use crate::parsers::ClozeGroupingSettings;
use crate::parsers::FrontConceal;
use crate::parsers::NoteSettingsKeys;
use crate::parsers::Parseable;
use crate::parsers::construct_cloze_string;
use crate::parsers::image_occlusion::ConstructImageOcclusionType;
use crate::parsers::image_occlusion::ImageOcclusionClozeIndex;
use crate::parsers::image_occlusion::update_cloze_settings;

#[allow(clippy::type_complexity, reason = "avoid creating extra struct")]
#[allow(clippy::ptr_arg)]
pub(super) fn group_clozes(
    all_clozes: &mut Vec<(ClozeData, Vec<ClozeGroupingSettings>)>,
    data: &str,
) -> Result<(Vec<Vec<(ClozeData, ClozeGroupingSettings)>>, usize), LibraryError> {
    // Group clozes into cards by examining their grouping
    // Note that the order should be preserved, so the clozes can NOT just be partitioned by whether they contain a grouping or not.
    let all_grouping_names = all_clozes
        .iter()
        .flat_map(|(_, grouping_settings)| grouping_settings)
        .map(|grouping_settings| &grouping_settings.grouping)
        .filter(|grouping| {
            matches!(grouping, ClozeGrouping::Auto(_))
                || matches!(grouping, ClozeGrouping::Custom(_))
        })
        .unique()
        .cloned()
        .collect::<Vec<_>>();

    let clozes_with_all_groupings = all_clozes
        .iter_mut()
        .filter_map(|(cloze_data, grouping_settings)| {
            grouping_settings
                .iter()
                .find(|g| g.grouping == ClozeGrouping::All)
                .cloned()
                .map(|all_groupings_settings| {
                    (cloze_data, grouping_settings, all_groupings_settings)
                })
        })
        .collect::<Vec<_>>();

    for (cloze_data, grouping_settings, all_groupings_settings) in clozes_with_all_groupings {
        // Update cloze data with settings for `ClozeGrouping::All`
        cloze_data.settings.all_groupings = Some(all_groupings_settings.clone());

        // Replace `ClozeGrouping::All` with each grouping
        let new_grouping_settings = all_grouping_names
            .iter()
            .map(|grouping| {
                let mut settings = all_groupings_settings.clone();
                settings.grouping = grouping.clone();
                settings
            })
            .collect::<Vec<_>>();
        *grouping_settings = new_grouping_settings;
    }

    let cards_raw: Vec<Vec<(ClozeData, ClozeGroupingSettings)>> = all_clozes
        .iter()
        .flat_map(|(cloze_data, grouping_settings)| {
            grouping_settings
                .iter()
                .map(|g| (g.grouping.clone(), (cloze_data.clone(), g.clone())))
                .collect::<Vec<_>>()
        })
        .into_group_by_insertion()
        .into_iter()
        .map(|(_, x)| x)
        .collect::<Vec<_>>();
    // Validate grouped clozes
    for clozes in &cards_raw {
        let flattened_matches = clozes
            .iter()
            .flat_map(|(cd, _)| {
                // The end points are not inclusive so they should be removed.
                [
                    cd.start_delim.start,
                    cd.start_delim.end - 1,
                    cd.end_delim.start,
                    cd.end_delim.end - 1,
                ]
            })
            // One image occlusion can have 2 clozes that are a part of the same card. In this case, we will have 2 `ClozeData`s with the same `start_delim` and `end_delim` that are consecutive. Calling `.unique()` removes these duplicates, while preserving order.
            .unique()
            .collect::<Vec<_>>();
        // Not strictly increasing because image occlusion clozes have the same value for `start_delim.start` and `start_delim.end`.
        let not_increasing = flattened_matches
            .iter()
            .tuple_windows()
            .find(|(cur, next)| cur > next);
        if let Some((cur, next)) = not_increasing {
            debug_assert!(!(*cur < 3 || *next + 3 >= data.len()));
            return Err(LibraryError::Card(
                CardErrorKind::SameGroupingNestedClozes {
                    src: data.to_string(),
                    // This is start_delim.start to end_delim.end of the outside cloze
                    cloze_1: (*cur - 3..*cur).into(),
                    // This is start_delim.start to end_delim.end of the intside cloze
                    cloze_2: (*next..*next + 3).into(),
                },
            ));
        }
    }
    // Validate multiple cards do not contain the same clozes
    let duplicates = cards_raw
        .iter()
        .map(|clozes| {
            clozes
                .iter()
                .map(|(cloze_data, grouping_settings)| {
                    (cloze_data.index, grouping_settings.hidden_no_answer)
                })
                .collect::<Vec<_>>()
        })
        .duplicates()
        .collect::<Vec<_>>();
    if !duplicates.is_empty() {
        return Err(LibraryError::Card(CardErrorKind::MultipleDuplicateCards {
            duplicates: duplicates
                .into_iter()
                .map(|x| {
                    x.into_iter()
                        .map(|(cloze_index, _)| cloze_index)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
        }));
    }
    Ok((cards_raw, all_grouping_names.len()))
}

/// Boil up settings to first cloze within the grouping.
/// Note that clozes in the same grouping can have different settings for convenience. Rather than be forced to copy and paste the settings for all clozes, they can be declared on each cloze where subsequent clozes override the previous one.
pub(super) fn boil_up_settings(
    cards_raw: &mut Vec<Vec<(ClozeData, ClozeGroupingSettings)>>,
    data: &str,
) -> Result<(), LibraryError> {
    for clozes in &mut *cards_raw {
        // Get first non-hidden cloze to determine if it is an image occlusion cloze. This
        // determines the default settings.
        let first_cloze = clozes.iter().find(|(_, x)| !x.skip_serialization).unwrap();
        let modify_defaults = first_cloze.0.image_occlusion.as_ref().map(|d| {
            (
                d.data.front_conceal,
                d.data.back_reveal,
                d.data.back_emphasis,
            )
        });
        let mut boiled_cloze_settings = ClozeGroupingSettings::default(&mut 0, modify_defaults);
        for (cloze_data, grouping_settings) in &mut *clozes {
            let modify_defaults = cloze_data.image_occlusion.as_ref().map(|d| {
                (
                    d.data.front_conceal,
                    d.data.back_reveal,
                    d.data.back_emphasis,
                )
            });
            let default_cloze_settings = ClozeGroupingSettings::default(&mut 0, modify_defaults);
            // Update `boiled_cloze_settings` with settings that deviated from main and reset settings to default
            let ClozeGroupingSettings {
                grouping: _,
                inherit,
                orders: _,
                include_forward_card,
                include_backward_card,
                is_suspended,
                front_conceal,
                back_reveal,
                back_emphasis,
                // Individual cloze settings. Don't boil up
                hidden_no_answer: _,
                skip_serialization,
            } = grouping_settings;
            if *skip_serialization {
                continue;
            }

            if *include_forward_card != default_cloze_settings.include_forward_card {
                boiled_cloze_settings.include_forward_card = *include_forward_card;
                grouping_settings.include_forward_card =
                    default_cloze_settings.include_forward_card;
            }
            if *include_backward_card != default_cloze_settings.include_backward_card {
                boiled_cloze_settings.include_backward_card = *include_backward_card;
                grouping_settings.include_backward_card =
                    default_cloze_settings.include_backward_card;
            }
            if *is_suspended != default_cloze_settings.is_suspended {
                boiled_cloze_settings.is_suspended = *is_suspended;
                grouping_settings.is_suspended = default_cloze_settings.is_suspended;
            }
            if *front_conceal != default_cloze_settings.front_conceal
                || cloze_data.image_occlusion.is_some()
            {
                boiled_cloze_settings.front_conceal = *front_conceal;
                grouping_settings.front_conceal = default_cloze_settings.front_conceal;
            }
            if *back_reveal != default_cloze_settings.back_reveal
                || cloze_data.image_occlusion.is_some()
            {
                boiled_cloze_settings.back_reveal = *back_reveal;
                grouping_settings.back_reveal = default_cloze_settings.back_reveal;
            }
            if *back_emphasis != default_cloze_settings.back_emphasis
                || cloze_data.image_occlusion.is_some()
            {
                boiled_cloze_settings.back_emphasis = *back_emphasis;
                grouping_settings.back_emphasis = default_cloze_settings.back_emphasis;
            }
            if inherit.is_some() {
                boiled_cloze_settings.inherit = *inherit;
                grouping_settings.inherit = None;
            }
        }

        // Update first non-hidden cloze with boiled settings
        let cloze = clozes
            .iter_mut()
            .find(|(_, x)| !x.skip_serialization)
            .unwrap();
        cloze.1.include_forward_card = boiled_cloze_settings.include_forward_card;
        cloze.1.include_backward_card = boiled_cloze_settings.include_backward_card;
        cloze.1.is_suspended = boiled_cloze_settings.is_suspended;
        cloze.1.front_conceal = boiled_cloze_settings.front_conceal;
        cloze.1.back_reveal = boiled_cloze_settings.back_reveal;
        cloze.1.back_emphasis = boiled_cloze_settings.back_emphasis;
        cloze.1.inherit = boiled_cloze_settings.inherit;

        // Validate settings
        let contains_image_occlusion = clozes
            .iter()
            .any(|(cloze_data, _)| cloze_data.image_occlusion.is_some());
        if contains_image_occlusion
            && (!boiled_cloze_settings.include_forward_card
                || boiled_cloze_settings.include_backward_card)
        {
            return Err(LibraryError::Card(CardErrorKind::InvalidSettings {
                description:
                    "`include reverse` and `reverse only` are not possible within Image Occlusion."
                        .to_string(),
                src: data.to_string(),
                at: (0..data.len()).into(),
            }));
        }
    }
    Ok(())
}

pub(super) fn update_first_cloze_with_order(
    cards_raw: &mut [Vec<(ClozeData, ClozeGroupingSettings)>],
) {
    let mut seen_clozes: HashSet<usize> = HashSet::new();
    let mut current_card_order = 1;
    for card_index in 0..cards_raw.len() {
        // Find first non-hidden cloze
        let cloze = &mut *cards_raw[card_index]
            .iter_mut()
            .find(|(_, x)| !x.skip_serialization)
            .unwrap();
        let index = cloze.0.index;
        // Update first non-hidden cloze with order
        if !seen_clozes.contains(&index) {
            let all_cloze_groupings = cards_raw
                .iter_mut()
                .map(|x| x.iter_mut().find(|(_, x)| !x.skip_serialization).unwrap())
                .filter(|cl| cl.0.index == index)
                .map(|x| &mut x.1)
                .collect::<Vec<_>>();

            for grouping_settings in all_cloze_groupings {
                let mut num_cards = 1;
                if grouping_settings.include_forward_card && grouping_settings.include_backward_card
                {
                    num_cards += 1;
                }
                let new_cloze_orders = Some(
                    (current_card_order..(current_card_order + num_cards)).collect::<Vec<_>>(),
                );
                // The orders may be overriden if an earlier card was changed to also include the reverse direction. In this case, all the future orders must be incremented by 1. Thus, it is fine to override here.
                // if grouping_settings.orders.is_some()
                //     && grouping_settings.orders != new_cloze_orders
                // {
                //     return Err(format!("Specified `cloze.settings.orders` as {:?} when it should be {:?} when calling `get_cards()` with `add_order = true`.", grouping_settings.orders, new_cloze_orders));
                // }
                grouping_settings.orders = new_cloze_orders;
                current_card_order += num_cards;
            }
            seen_clozes.insert(index);
        }
    }
}

#[derive(Debug)]
enum ReplacementKind {
    TextStart,
    TextEnd,
    // Stores inner delimiter positions so new start_delim.end / end_delim.start can be
    // recomputed to match the original offset-tracking behaviour.
    ImageOcclusion {
        orig_start_delim_end: usize,
        orig_end_delim_start: usize,
    },
}

#[derive(Debug)]
struct Replacement {
    cloze_index: usize,
    range: Range<usize>,
    new_text: String,
    kind: ReplacementKind,
}

#[derive(Debug, Default)]
struct NewPosition {
    ss: usize,
    se: usize,
    es: usize,
    ee: usize,
}

#[expect(clippy::too_many_lines)]
#[allow(clippy::cast_sign_loss)]
pub(super) fn modify_card_settings(
    cards_raw: &mut Vec<Vec<(ClozeData, ClozeGroupingSettings)>>,
    data: &mut String,
    parser: &dyn Parseable,
    to_parser: Option<&dyn Parseable>,
    add_order: bool,
    serialize_ephemeral: bool,
) -> Result<(), LibraryError> {
    let output_parser = to_parser.unwrap_or(parser);

    // Boil up settings to first cloze within the grouping
    boil_up_settings(cards_raw, &*data)?;

    if add_order {
        update_first_cloze_with_order(cards_raw);
    }

    let NoteSettingsKeys {
        settings_delim,
        settings_key_value_delim,
        groupings_all,
        ..
    } = output_parser.note_settings_keys();
    let cloze_settings_keys = output_parser.cloze_settings_keys();

    // Collect delimiter-level replacements — one or two per unique cloze.
    // For text clozes: two replacements covering only the start and end delimiters (not the body).
    // For image occlusion: one replacement covering the entire cloze range.
    // Because only delimiters (never bodies) are replaced, replacements never overlap,
    // which enables a single forward pass to rebuild `data` without incremental offset
    // tracking or cloning `cards_raw`.
    let mut seen: HashSet<usize> = HashSet::new();
    // Multiple IO clozes from the same note block share identical start/end delims. Track the
    // first cloze index seen per IO block so we add only one string replacement per block.
    let mut seen_io_range_start: HashSet<usize> = HashSet::new();
    let mut replacements: Vec<Replacement> = Vec::new();

    for card in &*cards_raw {
        for (cloze_data, _) in card {
            let cloze_index = cloze_data.index;
            if !seen.insert(cloze_index) {
                continue;
            }

            // Collect all grouping settings for this cloze across all cards.
            let all_groupings: Vec<ClozeGroupingSettings> = cards_raw
                .iter()
                .flatten()
                .filter(|(cd, _)| cd.index == cloze_index)
                .map(|(_, gs)| gs.clone())
                .collect();

            let modify_defaults = cloze_data.image_occlusion.as_ref().map(|d| {
                (
                    d.data.front_conceal,
                    d.data.back_reveal,
                    d.data.back_emphasis,
                )
            });
            let cloze_settings_string = construct_cloze_string(
                &cloze_data.settings,
                &all_groupings,
                &cloze_settings_keys,
                settings_delim,
                settings_key_value_delim,
                modify_defaults,
                groupings_all,
                serialize_ephemeral,
            );

            if let Some(ref image_occlusion_cloze) = cloze_data.image_occlusion {
                let image_occlusion_cloze_index =
                    if let ImageOcclusionClozeIndex::OriginalIndex(ref x) =
                        image_occlusion_cloze.index
                    {
                        *x
                    } else {
                        unreachable!()
                    };
                // Always update the SVG file for each individual IO cloze.
                update_cloze_settings(
                    image_occlusion_cloze_index,
                    &cloze_settings_string,
                    &image_occlusion_cloze.data.clozes_filepath,
                    data,
                    &(cloze_data.start_delim.start..cloze_data.end_delim.end),
                )?;
                // But only add one string replacement per IO block: all clozes from the same
                // block produce the same new text and share the same delim range.
                if seen_io_range_start.insert(cloze_data.start_delim.start) {
                    let new_text = output_parser.construct_image_occlusion(
                        &image_occlusion_cloze.data,
                        ConstructImageOcclusionType::Note,
                    );
                    replacements.push(Replacement {
                        cloze_index,
                        range: cloze_data.start_delim.start..cloze_data.end_delim.end,
                        new_text,
                        kind: ReplacementKind::ImageOcclusion {
                            orig_start_delim_end: cloze_data.start_delim.end,
                            orig_end_delim_start: cloze_data.end_delim.start,
                        },
                    });
                }
            } else {
                let cloze_body_range = cloze_data.start_delim.end..cloze_data.end_delim.start;
                let (new_prefix, new_suffix) =
                    output_parser.construct_cloze(&cloze_settings_string, &data[cloze_body_range]);
                replacements.push(Replacement {
                    cloze_index,
                    range: cloze_data.start_delim.start..cloze_data.start_delim.end,
                    new_text: new_prefix,
                    kind: ReplacementKind::TextStart,
                });
                replacements.push(Replacement {
                    cloze_index,
                    range: cloze_data.end_delim.start..cloze_data.end_delim.end,
                    new_text: new_suffix,
                    kind: ReplacementKind::TextEnd,
                });
            }
        }
    }

    // Sort by range start — delimiter ranges never overlap, so this is safe.
    replacements.sort_unstable_by_key(|r| r.range.start);

    // Single forward pass: build the new `data` string and record new absolute positions.
    // Text cloze positions are keyed by cloze_index.
    // IO cloze positions are keyed by original start_delim.start (all clozes from the same IO
    // block share that value, so they all get the same updated positions).
    let mut text_new_pos: HashMap<usize, NewPosition> = HashMap::with_capacity(seen.len());
    let mut io_new_pos: HashMap<usize, NewPosition> =
        HashMap::with_capacity(seen_io_range_start.len());
    let mut new_data = String::with_capacity(data.len());
    let mut prev_end = 0usize;
    let mut offset: i64 = 0;

    for replacement in &replacements {
        new_data.push_str(&data[prev_end..replacement.range.start]);
        let new_range_start = (i64::try_from(replacement.range.start).unwrap() + offset) as usize;
        match replacement.kind {
            ReplacementKind::TextStart => {
                let entry = text_new_pos.entry(replacement.cloze_index).or_default();
                entry.ss = new_range_start;
                entry.se = new_range_start + replacement.new_text.len();
            }
            ReplacementKind::TextEnd => {
                let entry = text_new_pos.entry(replacement.cloze_index).or_default();
                entry.es = new_range_start;
                entry.ee = new_range_start + replacement.new_text.len();
            }
            ReplacementKind::ImageOcclusion {
                orig_start_delim_end,
                orig_end_delim_start,
            } => {
                // Reproduce the same arithmetic as the original offset-tracking code:
                // cloze_start_diff = new_text.len() - replaced_range.len()
                let cloze_start_diff = i64::try_from(replacement.new_text.len()).unwrap()
                    - i64::try_from(replacement.range.len()).unwrap();
                let se = (i64::try_from(orig_start_delim_end).unwrap() + offset + cloze_start_diff)
                    as usize;
                let es = (i64::try_from(orig_end_delim_start).unwrap() + offset + cloze_start_diff)
                    as usize;
                let ee = new_range_start + replacement.new_text.len();
                // Key by original start_delim.start (= repl.range.start) so all IO clozes from
                // the same block are updated together in the step below.
                io_new_pos.insert(
                    replacement.range.start,
                    NewPosition {
                        ss: new_range_start,
                        se,
                        es,
                        ee,
                    },
                );
            }
        }
        new_data.push_str(&replacement.new_text);
        offset += i64::try_from(replacement.new_text.len()).unwrap()
            - i64::try_from(replacement.range.len()).unwrap();
        prev_end = replacement.range.end;
    }
    new_data.push_str(&data[prev_end..]);
    *data = new_data;

    // Update all cloze positions in cards_raw.
    for card in &mut *cards_raw {
        for (cloze_data, _) in card {
            let pos_opt = if cloze_data.image_occlusion.is_some() {
                io_new_pos.get(&cloze_data.start_delim.start)
            } else {
                text_new_pos.get(&cloze_data.index)
            };
            if let Some(pos) = pos_opt {
                cloze_data.start_delim.start = pos.ss;
                cloze_data.start_delim.end = pos.se;
                cloze_data.end_delim.start = pos.es;
                cloze_data.end_delim.end = pos.ee;
            }
        }
    }

    Ok(())
}

pub(super) fn apply_conceal_and_reveal(
    cards_raw: &mut Vec<Vec<(ClozeData, ClozeGroupingSettings)>>,
    all_clozes: &[(ClozeData, Vec<ClozeGroupingSettings>)],
) {
    for clozes in &mut *cards_raw {
        let mut new_clozes = Vec::new();
        for (current_cloze_data, cloze_grouping_settings) in &mut **clozes {
            if matches!(
                cloze_grouping_settings.front_conceal,
                FrontConceal::AllGroupings
            ) || matches!(
                cloze_grouping_settings.back_reveal,
                BackReveal::OnlyAnswered
            ) {
                // Find all other clozes which are either completely before or completely after this cloze and are NOT a part of this card's grouping
                let matching_clozes = all_clozes
                    .iter()
                    .filter(|(cloze_data, all_grouping_settings)| {
                        (cloze_data.end_delim.end < current_cloze_data.start_delim.start
                            || cloze_data.start_delim.start > current_cloze_data.end_delim.end)
                            && !all_grouping_settings
                                .iter()
                                .any(|x| x.grouping == cloze_grouping_settings.grouping)
                    })
                    .map(|(x, _)| x)
                    .collect::<Vec<_>>();
                // Filter these clozes to remove all nested clozes. This is because if the cloze is nested, then the outer cloze will hide the inner cloze. Note that this is not true for image occlusion clozes since they all have the same indicies, but are not nested, so we exclude them.
                let matching_clozes = matching_clozes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &cur_cloze_data)| {
                        if i == 0 || cur_cloze_data.image_occlusion.is_some() {
                            return Some(cur_cloze_data);
                        }
                        let prev_cloze_data = matching_clozes[i - 1];
                        if prev_cloze_data.end_delim.end <= cur_cloze_data.start_delim.start {
                            Some(cur_cloze_data)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                new_clozes.extend(matching_clozes);
            }
        }
        // Add these clozes along with a new grouping setting that has `hidden_no_answer` enabled.
        // NOTE: The value of `is_image_occlusion` doesn't matter here since that property won't be read.
        let mut new_grouping_settings = ClozeGroupingSettings::default(&mut 0, None);
        new_grouping_settings.grouping = clozes.first().unwrap().1.grouping.clone();
        // new_grouping_settings.front_conceal = clozes.first().unwrap().1.front_conceal;
        // new_grouping_settings.back_reveal = clozes.first().unwrap().1.back_reveal;
        new_grouping_settings.hidden_no_answer = true;
        new_grouping_settings.skip_serialization = true;
        clozes.extend(
            new_clozes
                .into_iter()
                .map(|x| (x.clone(), new_grouping_settings.clone()))
                .collect::<Vec<_>>(),
        );
        clozes.sort_by_key(|c| c.0.index);
    }
}
