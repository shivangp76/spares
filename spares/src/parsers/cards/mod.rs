use super::{BackReveal, BackType, FrontConceal};
use crate::helpers::merge_by_key;
use crate::parsers::image_occlusion::{
    ImageOcclusionCloze, ImageOcclusionClozeIndex, ParsedImageOcclusionCloze,
    ParsedImageOcclusionData, combine_image_occlusion_clozes, parse_image_occlusion_data,
};
use crate::parsers::{
    ClozeData, ClozeGrouping, ClozeGroupingSettings, ClozeHiddenReplacement, ClozeSettings,
    NotePart, NoteSettingsKeys, Parseable, image_occlusion::ConstructImageOcclusionType,
    parse_card_settings,
};
use crate::{Error, LibraryError};
use std::sync::Arc;

#[cfg(test)]
mod card_tests;
mod data;
mod grouping;
mod match_cards;
pub mod overlapper;
mod validation;

pub use data::*;
pub use match_cards::*;
pub use validation::*;

use crate::model::NoteId;
use crate::parsers::{DEFAULT_BACK_EMPHASIS, ReadableCardIdentifier};
use grouping::{apply_conceal_and_reveal, group_clozes, modify_card_settings};

#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Backward,
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
        data.to_string(),
        add_order,
        move_files,
        (
            FrontConceal::default(),
            BackReveal::default(),
            DEFAULT_BACK_EMPHASIS,
        ),
    )
}

// The order of the returned cards matters here and is used to reference cards in the database. Cloze number cannot be used in the database because 1 card can have multiple clozes (grouped clozes).
#[expect(clippy::too_many_lines)]
pub fn get_cards_main(
    parser: &dyn Parseable,
    to_parser: Option<&dyn Parseable>,
    data: String,
    add_order: bool,
    move_files: bool,
    defaults: (FrontConceal, BackReveal, bool),
) -> Result<Vec<CardData>, LibraryError> {
    let mut data = data;
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
                return Err(LibraryError::Card(crate::CardErrorKind::EmptyCloze {
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
    let image_occlusion_clozes = parse_image_occlusion_data(
        data.as_str(),
        parser,
        move_files,
        &mut current_grouping_number,
    )?
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
    let (mut cards_raw, groupings_count) = group_clozes(&mut all_clozes, &data)?;

    // Once cards are created by grouping clozes by their grouping, we can add other clozes that should be hidden if `FrontConceal::AllGroupings`.
    // This must be done after the image occlusions are interweaved since `FrontConceal` works across image occlusion clozes.
    apply_conceal_and_reveal(&mut cards_raw, &all_clozes);

    // Extract old orders before they're modified by `modify_card_settings()`
    let old_orders: Vec<Option<Vec<usize>>> = cards_raw
        .iter()
        .map(|clozes| {
            clozes
                .iter()
                .find(|(_, x)| !x.skip_serialization)
                .and_then(|(_, settings)| settings.orders.clone())
        })
        .collect::<Vec<_>>();

    // Modify card settings
    modify_card_settings(&mut cards_raw, &mut data, parser, to_parser, add_order)?;

    // Combine image occlusions
    // This is done in place since image occlusions containing grouped clozes are a relatively rare type of card. This means that it is rare that combining image occlusion clozes will change the data.
    for clozes in &mut cards_raw {
        combine_image_occlusion_clozes(clozes);
    }

    // Convert Vec<ClozeData> to CardData
    let mut cards: Vec<CardData> = Vec::new();
    let mut old_orders_iter = old_orders.into_iter();
    for clozes in cards_raw {
        // Since cloze settings are boiled up, just examine the first cloze for the settings.
        let ClozeGroupingSettings {
            grouping,
            inherit,
            orders,
            include_forward_card,
            include_backward_card,
            is_suspended,
            hidden_no_answer: _,
            front_conceal,
            back_reveal,
            back_emphasis,
            skip_serialization: _,
        } = &clozes
            .iter()
            .find(|(_, x)| !x.skip_serialization)
            .unwrap()
            .1;
        let ClozeSettings { hint, .. } = &clozes.first().unwrap().0.settings;
        let mut orders_iter = orders.as_ref().into_iter().flat_map(|v| v.iter().copied());

        // Extract old orders for this grouping
        let old_orders_for_grouping = old_orders_iter.next().flatten();
        let old_orders_vec = old_orders_for_grouping.clone();
        let mut old_orders_iter_grouping = old_orders_vec
            .as_ref()
            .map(|v| v.iter().copied())
            .into_iter()
            .flatten();

        // Construct directions
        let mut directions: Vec<Direction> = Vec::with_capacity(2);
        if *include_forward_card {
            directions.push(Direction::Forward);
        }
        if *include_backward_card {
            directions.push(Direction::Backward);
        }
        assert!(!directions.is_empty());

        // Create cards
        let clozes_num = clozes.len();
        let mut is_first_direction = true;
        for dir in directions {
            // Inline closures capture `hint` and `dir` without heap allocation.
            let outer = |text: String, hidden: bool| -> NotePart {
                match dir {
                    Direction::Forward => NotePart::SurroundingData(text),
                    Direction::Backward => {
                        if hidden {
                            NotePart::ClozeData(text, ClozeHiddenReplacement::NotToAnswer)
                        } else {
                            NotePart::ClozeData(
                                text,
                                ClozeHiddenReplacement::ToAnswer { hint: hint.clone() },
                            )
                        }
                    }
                }
            };
            let inner = |text: String, hidden: bool| -> NotePart {
                match dir {
                    Direction::Forward => {
                        if hidden {
                            NotePart::ClozeData(text, ClozeHiddenReplacement::NotToAnswer)
                        } else {
                            NotePart::ClozeData(
                                text,
                                ClozeHiddenReplacement::ToAnswer { hint: hint.clone() },
                            )
                        }
                    }
                    Direction::Backward => NotePart::SurroundingData(text),
                }
            };
            let mut card_data: Vec<NotePart> = Vec::new();
            for (i, (cloze, grouping_settings)) in clozes.iter().enumerate() {
                let hidden = grouping_settings.hidden_no_answer;
                if i == 0 && cloze.start_delim.start > 0 {
                    card_data.push(outer(data[..cloze.start_delim.start].to_string(), hidden));
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
                    card_data.push(inner(
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
                    card_data.push(outer(
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
                return Err(LibraryError::Card(crate::CardErrorKind::InvalidInput(
                    format!(
                        "All clozes cannot be hidden. See grouping `{}`.",
                        grouping.to_parser_string(note_settings_keys.groupings_all)
                    ),
                )));
            }
            if matches!(front_conceal, FrontConceal::OnlyGrouping)
                && matches!(back_reveal, BackReveal::OnlyAnswered)
                && groupings_count > 1
            {
                return Err(LibraryError::Card(crate::CardErrorKind::InvalidInput(
                    "If there is more than 1 grouping, then `FrontConceal::OnlyGrouping` and `BackReveal::OnlyAnswered` cannot both be set. This would mean the other groupings are visible on the front, but hidden on the back, even though they are not tested. Either change `front_conceal`, change `back_reveal`, or remove a grouping.".to_string()
                )));
            }
            cards.push(CardData {
                order: orders_iter.next(),
                previous_order: old_orders_iter_grouping.next(),
                grouping: grouping.clone(),
                is_suspended: *is_suspended,
                data: card_data,
                front_conceal: *front_conceal,
                back_reveal: *back_reveal,
                back_emphasis: *back_emphasis,
                back_type: BackType::from_back_reveal(back_reveal, groupings_count, *back_emphasis),
                // WORKAROUND: To reduce complexity, `inherit` only applies to the first (forward) card. For `r:` clozes that produce both a forward and backward card, the backward card starts fresh. If we wanted to implement it for the backwards card as well, we would need modify the syntax of `inh:` and also add a key to `ClozeGroupingSettings` which is already a large struct.
                inherit: if is_first_direction { *inherit } else { None },
            });
            is_first_direction = false;
        }
    }
    Ok(cards)
}

pub fn add_order_to_note_data(
    parser: &dyn Parseable,
    original_note_data: &str,
) -> Result<(String, Vec<CardData>), Error> {
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
                    | NotePart::ClozeData(text, _) => text.clone(),
                    NotePart::ImageOcclusion { data, .. } => {
                        parser.construct_image_occlusion(data, ConstructImageOcclusionType::Note)
                    }
                })
                .collect::<String>()
        });
    Ok((note_data, card_datas))
}
