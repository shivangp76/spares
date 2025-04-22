use super::{BackReveal, BackType, FrontConceal};
use crate::helpers::{GroupByInsertion, change_offset, merge_by_key};
use crate::parsers::image_occlusion::{
    ImageOcclusionCloze, ImageOcclusionClozeIndex, ParsedImageOcclusionCloze,
    ParsedImageOcclusionData, combine_image_occlusion_clozes, parse_image_occlusion_data,
    update_cloze_settings,
};
use crate::parsers::{
    ClozeData, ClozeGrouping, ClozeGroupingSettings, ClozeHiddenReplacement, ClozeSettings,
    NotePart, NoteSettingsKeys, Parseable, construct_cloze_string,
    image_occlusion::ConstructImageOcclusionType, parse_card_settings,
};
use crate::{CardErrorKind, Error, LibraryError};
use itertools::Itertools;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

#[cfg(test)]
mod card_tests;
mod match_cards;
pub mod overlapper;
pub use match_cards::*;

#[derive(Clone, Debug, PartialEq)]
pub struct CardData {
    pub order: Option<usize>,
    pub grouping: ClozeGrouping,
    pub is_suspended: Option<bool>,
    pub front_conceal: FrontConceal,
    pub back_reveal: BackReveal,
    // NOTE: This does not always match `BackReveal`. If there is only 1 grouping and `back_reveal == BackReveal::OnlyGrouping`, then we can just use the full note as the back to avoid having to generate an extra card back.
    pub back_type: BackType,
    pub data: Vec<NotePart>,
}

pub fn validate_cards(cards: &[CardData]) -> Result<(), LibraryError> {
    if cards.iter().any(|cd| cd.data.is_empty()) {
        return Err(LibraryError::Card(CardErrorKind::Empty));
    }
    if cards.iter().any(|cd| {
        !cd.data
            .iter()
            .any(|p| matches!(p, NotePart::SurroundingData(_)))
    }) {
        return Err(LibraryError::Card(CardErrorKind::MissingField(
            "Surrounding data".to_string(),
        )));
    }

    if cards.iter().any(|cd| {
        !cd.data
            .iter()
            .any(|p| matches!(p, NotePart::ImageOcclusion { .. }))
            && !cd
                .data
                .iter()
                .any(|p| matches!(p, NotePart::ClozeData(_, _)))
    }) {
        return Err(LibraryError::Card(CardErrorKind::MissingField(
            "Cloze or Image Occlusion".to_string(),
        )));
    }
    Ok(())
}

#[allow(clippy::type_complexity, reason = "avoid creating extra struct")]
fn group_clozes(
    mut all_clozes: Vec<(ClozeData, Vec<ClozeGroupingSettings>)>,
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

    // Replace `ClozeGrouping::All` with all_groups
    let relevant_clozes = all_clozes
        .iter_mut()
        .map(|(_, grouping_settings)| grouping_settings)
        .filter(|grouping_settings| {
            grouping_settings
                .iter()
                .any(|g| g.grouping == ClozeGrouping::All)
        })
        .inspect(|grouping_settings| assert_eq!(grouping_settings.len(), 1))
        .collect::<Vec<_>>();
    for grouping_settings in relevant_clozes {
        let new_grouping_settings = all_grouping_names
            .iter()
            .map(|grouping| {
                let mut settings = grouping_settings.first().unwrap().clone();
                settings.grouping = grouping.clone();
                settings
            })
            .collect::<Vec<_>>();
        *grouping_settings = new_grouping_settings;
    }

    let cards_raw: Vec<Vec<(ClozeData, ClozeGroupingSettings)>> = all_clozes
        .into_iter()
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
            .map(|(cloze_data, _)| cloze_data)
            .flat_map(|cd| {
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
        // Not strictly increasing because image occlusion clozes have the samevalue for `start_delim.start` and `start_delim.end`.
        let not_increasing = flattened_matches
            .iter()
            .tuple_windows()
            .find(|(cur, next)| cur > next);
        if let Some((cur, next)) = not_increasing {
            dbg!(&flattened_matches);
            assert!(!(*cur < 3 || *next + 3 >= data.len()));
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
fn boil_up_settings(
    cards_raw: &mut Vec<Vec<(ClozeData, ClozeGroupingSettings)>>,
    data: &str,
) -> Result<(), LibraryError> {
    for clozes in &mut *cards_raw {
        // Get first non-hidden cloze to determine if it is an image occlusion cloze. This
        // determines the default settings.
        let first_cloze = clozes.iter().find(|(_, x)| !x.hidden).unwrap();
        let modify_defaults = first_cloze
            .0
            .image_occlusion
            .as_ref()
            .map(|d| (d.data.front_conceal, d.data.back_reveal));
        let mut boiled_cloze_settings = ClozeGroupingSettings::default(&mut 0, modify_defaults);
        for (cloze_data, grouping_settings) in &mut *clozes {
            let modify_defaults = cloze_data
                .image_occlusion
                .as_ref()
                .map(|d| (d.data.front_conceal, d.data.back_reveal));
            let default_cloze_settings = ClozeGroupingSettings::default(&mut 0, modify_defaults);
            // Update `boiled_cloze_settings` with settings that deviated from main and reset settings to default
            let ClozeGroupingSettings {
                grouping: _,
                orders: _,
                include_forward_card,
                include_backward_card,
                is_suspended,
                front_conceal,
                back_reveal,
                // Individual cloze settings. Don't boil up
                hidden_no_answer: _,
                hidden,
            } = grouping_settings;
            if *hidden {
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
        }

        // Update first non-hidden cloze with boiled settings
        let cloze = clozes.iter_mut().find(|(_, x)| !x.hidden).unwrap();
        cloze.1.include_forward_card = boiled_cloze_settings.include_forward_card;
        cloze.1.include_backward_card = boiled_cloze_settings.include_backward_card;
        cloze.1.is_suspended = boiled_cloze_settings.is_suspended;
        cloze.1.front_conceal = boiled_cloze_settings.front_conceal;
        cloze.1.back_reveal = boiled_cloze_settings.back_reveal;

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

fn update_first_cloze_with_order(cards_raw: &mut [Vec<(ClozeData, ClozeGroupingSettings)>]) {
    let mut seen_clozes: HashSet<usize> = HashSet::new();
    let mut current_card_order = 1;
    for card_index in 0..cards_raw.len() {
        // Find first non-hidden cloze
        let cloze = &mut *cards_raw[card_index]
            .iter_mut()
            .find(|(_, x)| !x.hidden)
            .unwrap();
        let index = cloze.0.index;
        // Update first non-hidden cloze with order
        if !seen_clozes.contains(&index) {
            let all_cloze_groupings = cards_raw
                .iter_mut()
                .map(|x| x.iter_mut().find(|(_, x)| !x.hidden).unwrap())
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

#[allow(clippy::too_many_lines)]
fn modify_card_settings(
    cards_raw: &mut Vec<Vec<(ClozeData, ClozeGroupingSettings)>>,
    data: &mut String,
    parser: &dyn Parseable,
    to_parser: Option<&dyn Parseable>,
    add_order: bool,
) -> Result<(), LibraryError> {
    let output_parser = to_parser.unwrap_or(parser);

    // Boil up settings to first cloze within the grouping
    boil_up_settings(cards_raw, &*data)?;

    if add_order {
        update_first_cloze_with_order(cards_raw);
    }

    let mut seen_clozes: HashSet<usize> = HashSet::new();
    let cards_raw_refcell = cards_raw
        .clone()
        .into_iter()
        .map(|clozes| {
            clozes
                .into_iter()
                .map(|(cloze_data, grouping_settings)| {
                    (RefCell::new(cloze_data), grouping_settings)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let NoteSettingsKeys {
        settings_delim,
        settings_key_value_delim,
        ..
    } = output_parser.note_settings_keys();
    let cloze_settings_keys = output_parser.cloze_settings_keys();
    for card_index in 0..cards_raw_refcell.len() {
        for cloze_index in 0..cards_raw_refcell[card_index].len() {
            if !seen_clozes.contains(&cards_raw[card_index][cloze_index].0.index) {
                // Modify the settings string for the cloze and change `data` accordingly
                let all_groupings = cards_raw_refcell
                    .iter()
                    .flatten()
                    .filter(|cloze| {
                        cloze.0.borrow().index
                            == cards_raw_refcell[card_index][cloze_index].0.borrow().index
                    })
                    .map(|cloze| cloze.1.clone())
                    .collect::<Vec<_>>();
                let current_cloze = &cards_raw_refcell[card_index][cloze_index];
                let modify_defaults = current_cloze
                    .0
                    .borrow()
                    .image_occlusion
                    .as_ref()
                    .map(|d| (d.data.front_conceal, d.data.back_reveal));
                let cloze_settings_string = construct_cloze_string(
                    &cards_raw_refcell[card_index][cloze_index]
                        .0
                        .borrow()
                        .settings,
                    &all_groupings,
                    &cloze_settings_keys,
                    settings_delim,
                    settings_key_value_delim,
                    modify_defaults,
                );
                let replaced_range = current_cloze.0.borrow().start_delim.start
                    ..current_cloze.0.borrow().end_delim.end;
                let (new_cloze_prefix, new_cloze_suffix) = if let Some(ref image_occlusion_cloze) =
                    current_cloze.0.borrow().image_occlusion
                {
                    // Update clozes file with new settings string
                    // The cloze most likely changed since the order changed.
                    let image_occlusion_cloze_index =
                        if let ImageOcclusionClozeIndex::OriginalIndex(ref x) =
                            image_occlusion_cloze.index
                        {
                            *x
                        } else {
                            unreachable!()
                        };
                    update_cloze_settings(
                        image_occlusion_cloze_index,
                        &cloze_settings_string,
                        &image_occlusion_cloze.data.clozes_filepath,
                        data,
                        &(current_cloze.0.borrow().start_delim.start
                            ..current_cloze.0.borrow().end_delim.end),
                    )?;

                    // Add image path for previewing
                    (
                        output_parser.construct_image_occlusion(
                            &image_occlusion_cloze.data,
                            ConstructImageOcclusionType::Note,
                        ),
                        String::new(),
                    )
                } else {
                    let cloze_body_range = current_cloze.0.borrow().start_delim.end
                        ..current_cloze.0.borrow().end_delim.start;
                    output_parser
                        .construct_cloze(cloze_settings_string.as_str(), &data[cloze_body_range])
                };
                let (cloze_start_diff_count, new_cloze) =
                    if current_cloze.0.borrow().image_occlusion.is_none() {
                        (
                            i64::try_from(new_cloze_prefix.len()).unwrap()
                                - i64::try_from(current_cloze.0.borrow().start_delim.len())
                                    .unwrap(),
                            format!(
                                "{}{}{}",
                                new_cloze_prefix,
                                &data[current_cloze.0.borrow().start_delim.end
                                    ..current_cloze.0.borrow().end_delim.start],
                                new_cloze_suffix
                            ),
                        )
                    } else {
                        (
                            i64::try_from(new_cloze_prefix.len()).unwrap()
                                - i64::try_from(replaced_range.len()).unwrap(),
                            new_cloze_prefix,
                        )
                    };
                let character_diff_count = i64::try_from(new_cloze.len()).unwrap()
                    - i64::try_from(replaced_range.len()).unwrap();
                data.replace_range(replaced_range, &new_cloze);

                // Modify all the future clozes to account for added/removed characters
                let current_start_start = current_cloze.0.borrow().start_delim.start;
                let current_start_end = current_cloze.0.borrow().start_delim.end;
                let start_delim_limit = current_cloze.0.borrow().start_delim.start;
                let end_delim_limit = current_cloze.0.borrow().end_delim.start;
                for (cloze, _) in cards_raw_refcell.iter().flatten() {
                    // Since the cloze we changed can be a part of multiple cards, we must update it everywhere.
                    if cloze.borrow().start_delim.start == current_start_start
                        && cloze.borrow().start_delim.end == current_start_end
                    {
                        change_offset(
                            &mut cloze.borrow_mut().start_delim.end,
                            cloze_start_diff_count,
                        );
                        change_offset(
                            &mut cloze.borrow_mut().end_delim.start,
                            cloze_start_diff_count,
                        );
                        change_offset(&mut cloze.borrow_mut().end_delim.end, character_diff_count);
                    } else {
                        if cloze.borrow().start_delim.start > end_delim_limit {
                            // Sibling cloze
                            change_offset(
                                &mut cloze.borrow_mut().start_delim.start,
                                character_diff_count,
                            );
                            change_offset(
                                &mut cloze.borrow_mut().start_delim.end,
                                character_diff_count,
                            );
                        } else if cloze.borrow().start_delim.start > start_delim_limit {
                            // Nested cloze
                            change_offset(
                                &mut cloze.borrow_mut().start_delim.start,
                                cloze_start_diff_count,
                            );
                            change_offset(
                                &mut cloze.borrow_mut().start_delim.end,
                                cloze_start_diff_count,
                            );
                        }
                        // The if blocks for start and end delims must be separate because we may
                        // be handling a nested cloze, in which case we still need to shift the end
                        // delim of the parent cloze.
                        if cloze.borrow().end_delim.start > end_delim_limit {
                            change_offset(
                                &mut cloze.borrow_mut().end_delim.start,
                                character_diff_count,
                            );
                            change_offset(
                                &mut cloze.borrow_mut().end_delim.end,
                                character_diff_count,
                            );
                        } else if cloze.borrow().end_delim.start > start_delim_limit {
                            change_offset(
                                &mut cloze.borrow_mut().end_delim.start,
                                cloze_start_diff_count,
                            );
                            change_offset(
                                &mut cloze.borrow_mut().end_delim.end,
                                cloze_start_diff_count,
                            );
                        }
                    }
                }
                seen_clozes.insert(cards_raw[card_index][cloze_index].0.index);
            }
        }
    }
    *cards_raw = cards_raw_refcell
        .into_iter()
        .map(|clozes| {
            clozes
                .into_iter()
                .map(|(x, y)| (x.into_inner(), y))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    Ok(())
}

fn apply_conceal_and_reveal(
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
                // Find all other clozes which are either completely before or completely after this cloze that are NOT a part of this card's grouping
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
                new_clozes.extend(matching_clozes);
            }
        }
        // Add these clozes along with a new grouping setting that has `hidden_no_answer` enabled.
        // NOTE: The value of `is_image_occlusion` doesn't matter here since that property won't be read.
        let mut new_grouping_settings = ClozeGroupingSettings::default(&mut 0, None);
        new_grouping_settings.grouping = clozes.first().unwrap().1.grouping.clone();
        new_grouping_settings.hidden_no_answer = true;
        new_grouping_settings.hidden = true;
        clozes.extend(
            new_clozes
                .into_iter()
                .map(|x| (x.clone(), new_grouping_settings.clone()))
                .collect::<Vec<_>>(),
        );
        clozes.sort_by_key(|c| c.0.index);
    }
}

pub fn get_cards(
    parser: &dyn Parseable,
    to_parser: Option<&dyn Parseable>,
    data: &str,
    add_order: bool,
    move_files: bool,
) -> Result<Vec<CardData>, LibraryError> {
    get_cards_main(
        parser,
        to_parser,
        data,
        add_order,
        move_files,
        (FrontConceal::default(), BackReveal::default()),
    )
}

// The order of the returned cards matters here and is used to reference cards in the database. Cloze number cannot be used in the database because 1 card can have multiple clozes (grouped clozes).
#[allow(clippy::too_many_lines)]
pub fn get_cards_main(
    parser: &dyn Parseable,
    to_parser: Option<&dyn Parseable>,
    data: &str,
    add_order: bool,
    move_files: bool,
    defaults: (FrontConceal, BackReveal),
) -> Result<Vec<CardData>, LibraryError> {
    let mut data = data.to_string();
    let cloze_matches = parser.get_clozes(&data)?;

    let mut current_grouping_number = 1;
    let note_settings_keys = parser.note_settings_keys();
    let cloze_settings_keys = parser.cloze_settings_keys();
    let text_clozes: Vec<(ClozeData, Vec<ClozeGroupingSettings>)> = cloze_matches
        .into_iter()
        .map(|cloze_match| -> Result<_, _> {
            let (card_settings, grouping_settings) = parse_card_settings(
                &data,
                &cloze_match.settings_match,
                &mut current_grouping_number,
                &note_settings_keys,
                &cloze_settings_keys,
                Some(defaults),
            )?;
            if (cloze_match.start_match.end..cloze_match.end_match.start).is_empty() {
                return Err(LibraryError::Card(CardErrorKind::EmptyCloze {
                    src: data.clone(),
                    at: (cloze_match.start_match.start..cloze_match.end_match.end).into(),
                }));
            }
            Ok((
                ClozeData {
                    // This will be renumbered anyway
                    index: 0,
                    start_delim: cloze_match.start_match,
                    end_delim: cloze_match.end_match,
                    settings: card_settings,
                    image_occlusion: None,
                },
                grouping_settings,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Parse image occlusion data
    let image_occlusion_clozes = parse_image_occlusion_data(data.as_str(), parser, move_files)?
        .into_iter()
        .flat_map(
            |ParsedImageOcclusionData {
                 start_delim,
                 end_delim,
                 image_occlusion,
                 clozes,
             }| {
                let shared_image_occlusion_data = Arc::new(image_occlusion);
                clozes
                    .into_iter()
                    .enumerate()
                    .map(
                        |(
                            i,
                            ParsedImageOcclusionCloze {
                                settings,
                                grouping_settings,
                            },
                        )| {
                            (
                                ClozeData {
                                    // This will be renumbered anyway
                                    index: 0,
                                    start_delim: start_delim.clone(),
                                    end_delim: end_delim.clone(),
                                    settings,
                                    image_occlusion: Some(ImageOcclusionCloze {
                                        index: ImageOcclusionClozeIndex::OriginalIndex(i),
                                        data: shared_image_occlusion_data.clone(),
                                    }),
                                },
                                grouping_settings,
                            )
                        },
                    )
                    .collect::<Vec<_>>()
            },
        )
        .collect::<Vec<_>>();

    // Interweave text and image occlusion clozes
    let mut all_clozes = merge_by_key(&text_clozes, &image_occlusion_clozes, |x| {
        x.0.start_delim.end
    });
    for cloze_data in all_clozes.iter().map(|(cloze_data, _)| cloze_data) {
        assert!(cloze_data.start_delim.start <= cloze_data.start_delim.end);
        assert!(cloze_data.start_delim.end <= cloze_data.end_delim.start);
        assert!(cloze_data.end_delim.start <= cloze_data.end_delim.end);
    }
    all_clozes
        .iter_mut()
        .enumerate()
        .for_each(|(i, x)| x.0.index = i);

    // Note the clozes are cloned if they are a part of multiple groups. They are NOT passed by reference, since their settings must be boiled up, which would be different for each card.
    let (mut cards_raw, groupings_count) = group_clozes(all_clozes.clone(), &data)?;

    // Once cards are created by grouping clozes by their grouping, we can add other clozes that should be hidden if `FrontConceal::AllGroupings`.
    // This must be done after the image occlusions are interweaved since `FrontConceal` works across image occlusion clozes.
    apply_conceal_and_reveal(&mut cards_raw, &all_clozes);

    // Modify card settings
    modify_card_settings(&mut cards_raw, &mut data, parser, to_parser, add_order)?;

    // Combine image occlusions
    // This is done in place since image occlusions containing grouped clozes are a relatively rare type of card. This means that it is rare that combining image occlusion clozes will change the data.
    for clozes in &mut cards_raw {
        combine_image_occlusion_clozes(clozes);
    }

    // Convert Vec<ClozeData> to CardData
    let mut cards: Vec<CardData> = Vec::new();
    for clozes in cards_raw {
        // Since cloze settings are boiled up, just examine the first cloze for the settings.
        let ClozeGroupingSettings {
            grouping,
            orders,
            include_forward_card,
            include_backward_card,
            is_suspended,
            hidden_no_answer: _,
            front_conceal,
            back_reveal,
            hidden: _,
        } = &clozes.iter().find(|(_, x)| !x.hidden).unwrap().1;
        let ClozeSettings { hint, .. } = &clozes.first().unwrap().0.settings;
        let mut orders_iter = orders.as_ref().into_iter().flat_map(|v| v.iter().copied());

        // Construct directions
        #[allow(clippy::type_complexity)]
        let mut directions: Vec<(
            Box<dyn Fn(String, bool) -> NotePart>,
            Box<dyn Fn(String, bool) -> NotePart>,
        )> = Vec::with_capacity(2);
        if *include_forward_card {
            directions.push((
                Box::new(|data, _hidden| NotePart::SurroundingData(data)),
                Box::new(|data, hidden| {
                    if hidden {
                        NotePart::ClozeData(data, ClozeHiddenReplacement::NotToAnswer)
                    } else {
                        NotePart::ClozeData(
                            data,
                            ClozeHiddenReplacement::ToAnswer { hint: hint.clone() },
                        )
                    }
                }),
            ));
        }
        if *include_backward_card {
            directions.push((
                Box::new(|data, hidden| {
                    if hidden {
                        NotePart::ClozeData(data, ClozeHiddenReplacement::NotToAnswer)
                    } else {
                        NotePart::ClozeData(
                            data,
                            ClozeHiddenReplacement::ToAnswer { hint: hint.clone() },
                        )
                    }
                }),
                Box::new(|data, _hidden| NotePart::SurroundingData(data)),
            ));
        }
        assert!(!directions.is_empty());

        // Create cards
        let clozes_num = clozes.len();
        for (side1, side2) in directions {
            let mut card_data: Vec<NotePart> = Vec::new();
            for (i, (cloze, grouping_settings)) in clozes.iter().enumerate() {
                let hidden = grouping_settings.hidden_no_answer;
                if i == 0 && cloze.start_delim.start > 0 {
                    card_data.push(side1(data[..cloze.start_delim.start].to_string(), hidden));
                }
                if let Some(image_occlusion_cloze) = &cloze.image_occlusion {
                    let cloze_indices = if let ImageOcclusionClozeIndex::MultipleIndices(ref x) =
                        image_occlusion_cloze.index
                    {
                        x.clone()
                    } else {
                        unreachable!()
                    };
                    card_data.push(NotePart::ImageOcclusion {
                        cloze_indices,
                        data: image_occlusion_cloze.data.clone(),
                    });
                } else {
                    card_data.push(NotePart::ClozeStart(
                        data[cloze.start_delim.start..cloze.start_delim.end].to_string(),
                    ));
                    card_data.push(side2(
                        data[cloze.start_delim.end..cloze.end_delim.start].to_string(),
                        hidden,
                    ));
                    card_data.push(NotePart::ClozeEnd(
                        data[cloze.end_delim.start..cloze.end_delim.end].to_string(),
                    ));
                }
                let clozes_end: usize = if i == clozes_num - 1 {
                    data.len()
                } else {
                    clozes[i + 1].0.start_delim.start
                };
                if cloze.end_delim.end < clozes_end {
                    card_data.push(side1(
                        data[cloze.end_delim.end..clozes_end].to_string(),
                        hidden,
                    ));
                }
            }

            if card_data
                .iter()
                .any(|x| matches!(x, NotePart::ClozeData(_, _)))
                && card_data
                    .iter()
                    .filter_map(|x| match x {
                        NotePart::SurroundingData(_)
                        | NotePart::ClozeStart(_)
                        | NotePart::ClozeEnd(_) => None,
                        NotePart::ImageOcclusion { cloze_indices, .. } => {
                            Some(cloze_indices.iter().map(|x| &x.1).collect::<Vec<_>>())
                        }
                        NotePart::ClozeData(_, y) => Some(vec![y]),
                    })
                    .flatten()
                    .all(|x| matches!(x, ClozeHiddenReplacement::NotToAnswer))
            {
                return Err(LibraryError::Card(CardErrorKind::InvalidInput(format!(
                    "All clozes cannot be hidden. See grouping `{}`.",
                    grouping
                ))));
            }
            if matches!(front_conceal, FrontConceal::OnlyGrouping)
                && matches!(back_reveal, BackReveal::OnlyAnswered)
                && groupings_count > 1
            {
                return Err(LibraryError::Card(CardErrorKind::InvalidInput(
                    "The front and back cannot both be set to `OnlyGrouping` if there is more than 1 grouping. This would mean the other groupings are visible on the front, but hidden on the back, even though they are not tested. Either change `front_conceal`, change `back_reveal`, or remove a grouping.".to_string()
                )));
            }
            cards.push(CardData {
                order: orders_iter.next(),
                grouping: grouping.clone(),
                is_suspended: *is_suspended,
                data: card_data,
                front_conceal: *front_conceal,
                back_reveal: *back_reveal,
                back_type: BackType::from_back_reveal(back_reveal, groupings_count),
            });
        }
    }
    Ok(cards)
}

pub fn add_order_to_note_data(
    parser: &dyn Parseable,
    original_note_data: &str,
) -> Result<(String, usize), Error> {
    let card_datas = get_cards(parser, None, original_note_data, true, true)?;
    let note_data = card_datas
        .first()
        .map_or(original_note_data.to_owned(), |card_data| {
            card_data
                .data
                .iter()
                .map(|p| match p {
                    NotePart::ClozeStart(text)
                    | NotePart::ClozeEnd(text)
                    | NotePart::SurroundingData(text)
                    | NotePart::ClozeData(text, _) => text.to_string(),
                    NotePart::ImageOcclusion { data, .. } => {
                        parser.construct_image_occlusion(data, ConstructImageOcclusionType::Note)
                    }
                })
                .collect::<String>()
        });
    Ok((note_data, card_datas.len()))
}
