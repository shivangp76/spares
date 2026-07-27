use std::sync::Arc;

use overlapper::OverlapperConfig;
use overlapper::apply_overlapper_groupings;

use super::BackReveal;
use super::BackType;
use super::FrontConceal;
use crate::Error;
use crate::LibraryError;
use crate::helpers::merge_by_key;
use crate::model::NoteId;
use crate::parsers::CliData;
use crate::parsers::ClozeData;
use crate::parsers::ClozeGrouping;
use crate::parsers::ClozeGroupingSettings;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::ClozeSettings;
use crate::parsers::DEFAULT_BACK_EMPHASIS;
use crate::parsers::NotePart;
use crate::parsers::NoteSettingsKeys;
use crate::parsers::Parseable;
use crate::parsers::ReadableCardIdentifier;
use crate::parsers::cli;
use crate::parsers::image_occlusion::ConstructImageOcclusionType;
use crate::parsers::image_occlusion::ImageOcclusionCloze;
use crate::parsers::image_occlusion::ImageOcclusionClozeIndex;
use crate::parsers::image_occlusion::ParsedImageOcclusionCloze;
use crate::parsers::image_occlusion::ParsedImageOcclusionData;
use crate::parsers::image_occlusion::combine_image_occlusion_clozes;
use crate::parsers::image_occlusion::parse_image_occlusion_data;
use crate::parsers::parse_card_settings;

#[cfg(test)]
mod card_tests;
mod data;
mod grouping;
mod match_cards;
pub mod overlapper;
mod uids;
mod validation;

use std::ops::Range;

pub use data::*;
use grouping::apply_conceal_and_reveal;
use grouping::group_clozes;
use grouping::modify_card_settings;
pub use match_cards::*;
pub use uids::*;
pub use validation::*;

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
        None,
        false,
    )
}

// The order of the returned cards matters here and is used to reference cards in the database. Cloze number cannot be used in the database because 1 card can have multiple clozes (grouped clozes).
#[expect(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub fn get_cards_main(
    parser: &dyn Parseable,
    to_parser: Option<&dyn Parseable>,
    data: String,
    add_order: bool,
    move_files: bool,
    defaults: (FrontConceal, BackReveal, bool),
    overlapper: Option<&OverlapperConfig>,
    serialize_ephemeral: bool,
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

    // CLI cards (native block type, like image occlusion). A CLI block produces
    // exactly one card reviewed by spawning an external command. Mixing CLI
    // blocks with text clozes or image occlusions in the same note is rejected
    // since a CLI card has no rendered form.
    let cli_blocks = cli::parse_cli_data(parser, &data)?;
    if !cli_blocks.is_empty() {
        if !all_clozes.is_empty() {
            return Err(LibraryError::Card(crate::CardErrorKind::InvalidInput(
                "A note cannot contain both a CLI block and text/image-occlusion clozes. \
                 Move the CLI block into its own note."
                    .to_string(),
            )));
        }
        return Ok(build_cli_cards(parser, &data, &cli_blocks, add_order));
    }
    for cloze_data in all_clozes.iter().map(|(cloze_data, _)| cloze_data) {
        assert!(cloze_data.start_delim.start <= cloze_data.start_delim.end);
        assert!(cloze_data.start_delim.end <= cloze_data.end_delim.start);
        assert!(cloze_data.end_delim.start <= cloze_data.end_delim.end);
    }
    all_clozes
        .iter_mut()
        .enumerate()
        .for_each(|(i, x)| x.0.index = i);

    // Apply overlapper groupings if any cloze is marked with `ov:`.
    let ov_group_range = if let Some(ov_config) = overlapper
        && all_clozes.iter().any(|(cd, _)| cd.settings.is_overlapper)
    {
        apply_overlapper_groupings(&mut all_clozes, ov_config, &mut current_grouping_number)
    } else {
        None
    };

    // Note the clozes are cloned if they are a part of multiple groups. They are NOT passed by reference, since their settings must be boiled up, which would be different for each card.
    let (mut cards_raw, groupings_count) = group_clozes(&mut all_clozes, &data)?;

    // Overlapper cards may come out of `group_clozes` in non-sequential order (because context
    // items have no assignment for a group, so that group may be "discovered" out of order).
    // Sort overlapper cards by their Auto group number to restore the expected sequence order.
    if let Some(range) = ov_group_range {
        cards_raw.sort_by(|a, b| {
            let auto_in_range = |clozes: &Vec<(ClozeData, ClozeGroupingSettings)>| -> Option<u32> {
                clozes
                    .iter()
                    .find(|(_, x)| !x.skip_serialization)
                    .and_then(|(_, x)| {
                        if let ClozeGrouping::Auto(n) = &x.grouping {
                            Some(*n)
                        } else {
                            None
                        }
                    })
                    .filter(|n| range.contains(n))
            };
            match (auto_in_range(a), auto_in_range(b)) {
                (Some(an), Some(bn)) => an.cmp(&bn),
                _ => std::cmp::Ordering::Equal,
            }
        });
    }

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
    modify_card_settings(
        &mut cards_raw,
        &mut data,
        parser,
        to_parser,
        add_order,
        serialize_ephemeral,
    )?;

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
                        // CLI cards contribute no cloze replacements; skip the
                        // "all clozes hidden" check for them.
                        NotePart::SurroundingData(_)
                        | NotePart::ClozeStart(_)
                        | NotePart::ClozeEnd(_)
                        | NotePart::Cli { .. } => None,
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

/// Returns the note data substring for the cloze group corresponding to `card_order` (1-based),
/// prefixed with up to `CONTEXT_CHARS` bytes of preceding note data.
///
/// The prefix allows callers to match patterns that appear just before the cloze (e.g.
/// `#proof[\n\s+#cl[`). Returns `None` when `card_order` is 0 or out of range.
pub(crate) fn get_cloze_context_for_card_order(
    parser: &dyn Parseable,
    data: &str,
    card_order: u32,
) -> Result<Option<String>, LibraryError> {
    const CONTEXT_CHARS: usize = 500;

    if card_order == 0 {
        return Ok(None);
    }

    let cloze_matches = parser.get_clozes(data)?;
    if cloze_matches.is_empty() {
        return Ok(None);
    }

    let note_settings_keys = parser.note_settings_keys();
    let cloze_settings_keys = parser.cloze_settings_keys();
    let mut current_grouping_number = 1u32;

    // Parse settings for each non-empty cloze and build ClozeData
    let mut all_clozes: Vec<(ClozeData, Vec<ClozeGroupingSettings>)> = cloze_matches
        .into_iter()
        .filter(|m| !(m.start_match.end..m.end_match.start).is_empty())
        .enumerate()
        .map(|(i, cloze_match)| {
            let (card_settings, grouping_settings) = parse_card_settings(
                data,
                &cloze_match.settings_match,
                &mut current_grouping_number,
                &note_settings_keys,
                &cloze_settings_keys,
                None,
            )?;
            Ok((
                ClozeData {
                    index: i,
                    start_delim: cloze_match.start_match,
                    end_delim: cloze_match.end_match,
                    settings: card_settings,
                    image_occlusion: None,
                },
                grouping_settings,
            ))
        })
        .collect::<Result<Vec<_>, LibraryError>>()?;

    let (cards_raw, _) = group_clozes(&mut all_clozes, data)?;

    let card_order_usize = card_order as usize;

    // Prefer matching by stored `[o:N]` annotations; fall back to sequential index.
    let card_clozes = cards_raw
        .iter()
        .find(|clozes| {
            clozes
                .iter()
                .find(|(_, gs)| !gs.skip_serialization)
                .and_then(|(_, gs)| gs.orders.as_ref())
                .is_some_and(|orders| orders.contains(&card_order_usize))
        })
        .or_else(|| cards_raw.get(card_order_usize - 1));

    let Some(card_clozes) = card_clozes else {
        return Ok(None);
    };

    let cloze_start = card_clozes
        .iter()
        .filter(|(_, gs)| !gs.skip_serialization)
        .map(|(cd, _)| cd.start_delim.start)
        .min()
        .unwrap_or(0);
    let cloze_end = card_clozes
        .iter()
        .filter(|(_, gs)| !gs.skip_serialization)
        .map(|(cd, _)| cd.end_delim.end)
        .max()
        .unwrap_or(data.len());

    // Subtract context bytes and snap forward to a UTF-8 char boundary.
    let mut context_start = cloze_start.saturating_sub(CONTEXT_CHARS);
    while context_start < data.len() && !data.is_char_boundary(context_start) {
        context_start += 1;
    }

    Ok(Some(data[context_start..cloze_end].to_string()))
}

pub fn add_order_to_note_data(
    parser: &dyn Parseable,
    original_note_data: &str,
    overlapper: Option<&OverlapperConfig>,
) -> Result<(String, Vec<CardData>), Error> {
    let card_datas = get_cards_main(
        parser,
        None,
        original_note_data.to_string(),
        true,
        true,
        (
            FrontConceal::default(),
            BackReveal::default(),
            DEFAULT_BACK_EMPHASIS,
        ),
        overlapper,
        false,
    )?;
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
                    NotePart::Cli { exec } => {
                        parser.construct_cli_block(&CliData { exec: exec.clone() })
                    }
                })
                .collect::<String>()
        });
    Ok((note_data, card_datas))
}

/// Build one [`CardData`] per CLI block. Each card's `data` consists of the
/// surrounding text (everything outside the block, joined) followed by a
/// [`NotePart::Cli`] with the parsed `exec`. Cards are sequenced 1..N in the
/// order the blocks appear in the note so the database can reference them by
/// `(note_id, order)`.
///
/// `_add_order` is ignored because CLI blocks have no grouping or order
/// markers to inject — ordering is always sequential from the block's
/// position in the note. Callers pass the same `add_order` value that would
/// be used for cloze-grouped cards; for CLI cards it is a safe no-op.
fn build_cli_cards(
    _parser: &dyn Parseable,
    data: &str,
    cli_blocks: &[(cli::CliData, Range<usize>)],
    _add_order: bool,
) -> Vec<CardData> {
    let surrounding_text = cli::compute_surrounding_text(data, cli_blocks);
    let mut cards: Vec<CardData> = Vec::with_capacity(cli_blocks.len());
    for (order_n, (cli_data, _range)) in cli_blocks.iter().enumerate() {
        let order_n = order_n + 1;
        let grouping = ClozeGrouping::Auto(order_n as u32);
        cards.push(CardData {
            order: Some(order_n),
            previous_order: Some(order_n),
            grouping,
            is_suspended: None,
            front_conceal: FrontConceal::default(),
            back_reveal: BackReveal::default(),
            back_emphasis: DEFAULT_BACK_EMPHASIS,
            back_type: BackType::Cli,
            inherit: None,
            data: vec![
                NotePart::SurroundingData(surrounding_text.clone()),
                NotePart::Cli {
                    exec: cli_data.exec.clone(),
                },
            ],
        });
    }
    cards
}
