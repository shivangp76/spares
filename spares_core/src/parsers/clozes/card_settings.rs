use std::ops::Range;
use std::str::FromStr;

use indexmap::IndexMap;

use super::data::BackReveal;
use super::data::ClozeGrouping;
use super::data::ClozeGroupingSettings;
use super::data::ClozeSettings;
use super::data::ClozeUid;
use super::data::FrontConceal;
use super::data::ModifyDefaultsFn;
use super::data::ReadableCardIdentifier;
use crate::CardErrorKind;
use crate::LibraryError;
use crate::helpers::GroupByInsertion;
use crate::helpers::split_inclusive_following;
use crate::parsers::NoteSettingsKeys;
use crate::parsers::get_settings_pairs;

#[derive(Clone, Debug)]
pub struct ClozeSettingsKeys {
    pub orders: &'static str,
    pub grouping: &'static str,
    /// Creates another card which is the complement of this card. In other words, everything that was not a cloze becomes a cloze and everything that was a cloze becomes not a cloze. This can be used to mimic the "Basic (and reversed card)" functionality of Anki, but is capable of much more.
    pub include_reverse: &'static str,
    pub reverse_only: &'static str,
    pub is_suspended: &'static str,
    pub hint: &'static str,
    pub hidden_no_answer: &'static str,
    pub front_conceal: &'static str,
    pub back_reveal: &'static str,
    pub back_emphasis: &'static str,
    pub inherit: &'static str,
    pub overlapper: &'static str,
    pub id: &'static str,
}

impl Default for ClozeSettingsKeys {
    fn default() -> Self {
        Self {
            orders: "o",
            grouping: "g",
            include_reverse: "r",
            reverse_only: "ro",
            is_suspended: "s",
            hint: "h",
            hidden_no_answer: "hide",
            front_conceal: "f",
            back_reveal: "b",
            back_emphasis: "be",
            inherit: "inh",
            overlapper: "ov",
            id: "id",
        }
    }
}

#[expect(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub fn construct_cloze_string(
    global_settings: &ClozeSettings,
    grouping_settings: &[ClozeGroupingSettings],
    cloze_settings_keys: &ClozeSettingsKeys,
    settings_delim: &str,
    settings_key_value_delim: &str,
    modify_defaults_fn: ModifyDefaultsFn,
    groupings_all: &str,
    serialize_ephemeral: bool,
) -> String {
    // Global settings
    let mut parts: Vec<String> = Vec::new();
    if let Some(ref hint) = global_settings.hint {
        parts.push(format!(
            "{}{}{}",
            cloze_settings_keys.hint, settings_key_value_delim, hint
        ));
    }
    if global_settings.is_overlapper {
        parts.push(format!(
            "{}{}",
            cloze_settings_keys.overlapper, settings_key_value_delim
        ));
    }
    if let Some(ref cloze_uid) = global_settings.cloze_uid {
        parts.push(format!(
            "{}{}{}",
            cloze_settings_keys.id, settings_key_value_delim, cloze_uid
        ));
    }

    // Grouping setting
    let default = ClozeGroupingSettings::default(&mut 0, modify_defaults_fn);
    let mut all_grouping_parts: Vec<String> = Vec::new();
    let mut only_groups = Vec::new();
    let grouping_settings_with_all =
        if let Some(ref all_groupings_settings) = global_settings.all_groupings {
            std::iter::once(all_groupings_settings)
                .chain(grouping_settings.iter())
                .collect::<Vec<_>>()
        } else {
            grouping_settings.iter().collect::<Vec<_>>()
        };
    for (
        i,
        ClozeGroupingSettings {
            grouping,
            inherit,
            orders,
            include_forward_card,
            include_backward_card,
            is_suspended: _,
            hidden_no_answer,
            front_conceal,
            back_reveal,
            back_emphasis,
            skip_serialization,
        },
    ) in grouping_settings_with_all.iter().enumerate()
    {
        if *skip_serialization {
            continue;
        }
        let mut grouping_parts: Vec<String> = Vec::new();
        let parse_grouping = !matches!(grouping, ClozeGrouping::Auto(_));
        if parse_grouping {
            let grouping_str = grouping.to_parser_string(groupings_all);
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.grouping, settings_key_value_delim, grouping_str
            ));
        }
        if let Some(orders) = orders {
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.orders,
                settings_key_value_delim,
                orders
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if *include_forward_card && *include_backward_card {
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.include_reverse, settings_key_value_delim, ""
            ));
        }
        if !*include_forward_card && *include_backward_card {
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.reverse_only, settings_key_value_delim, ""
            ));
        }
        // Don't serialize `is_suspended`. Otherwise, sending a request to update a card and suspend it would require modifying the note's data. Instead, this field now only *de*serialized, not serialized.
        // if *is_suspended != default.is_suspended {
        //     grouping_parts.push(format!(
        //         "{}{}{}",
        //         cloze_settings_keys.is_suspended, settings_key_value_delim, ""
        //     ));
        // }
        if *hidden_no_answer != default.hidden_no_answer {
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.hidden_no_answer, settings_key_value_delim, ""
            ));
        }
        if *front_conceal != default.front_conceal {
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.front_conceal, settings_key_value_delim, front_conceal
            ));
        }
        if *back_reveal != default.back_reveal {
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.back_reveal, settings_key_value_delim, back_reveal
            ));
        }
        if *back_emphasis != default.back_emphasis {
            grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.back_emphasis, settings_key_value_delim, back_emphasis
            ));
        }
        if serialize_ephemeral && let Some(identifier) = inherit {
            grouping_parts.push(format!(
                "{}{}{}/{}",
                cloze_settings_keys.inherit,
                settings_key_value_delim,
                identifier.note_id,
                identifier.order,
            ));
        }

        // Push settings
        if parse_grouping && grouping_parts.len() == 1 && *grouping != ClozeGrouping::All {
            if global_settings.all_groupings.is_none() {
                only_groups.push(grouping.clone());
            }
            grouping_parts.clear();
        }
        if ((parse_grouping && grouping_parts.len() > 1) || i == grouping_settings.len() - 1)
            && !only_groups.is_empty()
        {
            let groups_str = only_groups
                .drain(0..)
                .map(|grouping| grouping.to_parser_string(groupings_all))
                .collect::<Vec<_>>()
                .join(",");
            all_grouping_parts.push(format!(
                "{}{}{}",
                cloze_settings_keys.grouping, settings_key_value_delim, groups_str
            ));
        }
        if !grouping_parts.is_empty() {
            let grouping_parts_str = grouping_parts.join(settings_delim);
            all_grouping_parts.push(grouping_parts_str);
        }
    }
    if !all_grouping_parts.is_empty() {
        let delim = format!("{} ", settings_delim);
        let all_grouping_parts_str = all_grouping_parts.join(delim.as_str());
        parts.push(all_grouping_parts_str);
    }

    parts.join(settings_delim)
}

fn parse_grouping(
    input: &str,
    current_grouping_number: &mut u32,
    groupings_all: &str,
) -> Vec<ClozeGrouping> {
    let values = input.split(',').collect::<Vec<_>>();
    if values.contains(&groupings_all) {
        vec![ClozeGrouping::All]
    } else if values.is_empty() {
        *current_grouping_number += 1;
        vec![ClozeGrouping::Auto(*current_grouping_number - 1)]
    } else {
        values
            .into_iter()
            .map(|x| ClozeGrouping::Custom(x.to_string()))
            .collect::<Vec<_>>()
    }
}

fn parse_card_source(
    value: &str,
    data: &str,
    card_settings_indices: &Range<usize>,
) -> Result<ReadableCardIdentifier, LibraryError> {
    let mut parts = value.splitn(2, '/');
    let note_id_str = parts.next().unwrap_or("").trim();
    let order_str = parts.next().unwrap_or("").trim();
    let note_id = note_id_str.parse::<i64>().map_err(|e| {
        LibraryError::Card(CardErrorKind::InvalidSettings {
            description: format!(
                "`inh:` must be in the format `inh:NOTE_ID/ORDER`. Invalid note id `{}`. Error: {}",
                note_id_str, e
            ),
            src: data.to_string(),
            at: card_settings_indices.clone().into(),
        })
    })?;
    let order = order_str.parse::<usize>().map_err(|e| {
        LibraryError::Card(CardErrorKind::InvalidSettings {
            description: format!(
                "`inh:` must be in the format `inh:NOTE_ID/ORDER`. Invalid order `{}`. Error: {}",
                order_str, e
            ),
            src: data.to_string(),
            at: card_settings_indices.clone().into(),
        })
    })?;
    if order == 0 {
        return Err(LibraryError::Card(CardErrorKind::InvalidSettings {
            description: "`inh:` order must be >= 1.".to_string(),
            src: data.to_string(),
            at: card_settings_indices.clone().into(),
        }));
    }
    Ok(ReadableCardIdentifier { note_id, order })
}

fn parse_grouping_settings(
    grouping_settings: &mut Vec<(&str, &str)>,
    settings: &mut ClozeSettings,
    current_grouping_number: &mut u32,
    data: &str,
    card_settings_indices: &Range<usize>,
    ClozeSettingsKeys {
        orders: orders_key,
        grouping: _,
        include_reverse: include_reverse_key,
        reverse_only: reverse_only_key,
        is_suspended: is_suspended_key,
        hint: hint_key,
        hidden_no_answer: hidden_no_answer_key,
        front_conceal: front_conceal_key,
        back_reveal: back_reveal_key,
        back_emphasis: back_emphasis_key,
        inherit: inherit_key,
        overlapper: overlapper_key,
        id: id_key,
    }: &ClozeSettingsKeys,
    modify_defaults_fn: ModifyDefaultsFn,
) -> Result<ClozeGroupingSettings, LibraryError> {
    let (mut include_reverse, mut reverse_only) = (false, false);
    // We don't want to increment `current_grouping_number` here, so we clone it first.
    let mut current_grouping_settings =
        ClozeGroupingSettings::default(&mut current_grouping_number.clone(), modify_defaults_fn);
    for (key, value) in grouping_settings {
        if key == include_reverse_key {
            include_reverse = true;
        } else if key == reverse_only_key {
            reverse_only = true;
        } else if key == is_suspended_key {
            // A negative option is provided to allow unsuspending a card when updating a note.
            current_grouping_settings.is_suspended = Some(*value != "n");
        } else if key == hint_key {
            settings.hint = Some((**value).to_string());
        } else if key == hidden_no_answer_key {
            current_grouping_settings.hidden_no_answer = true;
        } else if key == front_conceal_key {
            current_grouping_settings.front_conceal =
                FrontConceal::from_str(value).map_err(|e| {
                    LibraryError::Card(CardErrorKind::InvalidSettings {
                        description: format!("The card front `{}` is invalid. Error: {}", value, e),
                        src: data.to_string(),
                        at: card_settings_indices.clone().into(),
                    })
                })?;
        } else if key == back_reveal_key {
            current_grouping_settings.back_reveal = BackReveal::from_str(value).map_err(|e| {
                LibraryError::Card(CardErrorKind::InvalidSettings {
                    description: format!("The card back `{}` is invalid. Error: {}", value, e),
                    src: data.to_string(),
                    at: card_settings_indices.clone().into(),
                })
            })?;
        } else if key == back_emphasis_key {
            current_grouping_settings.back_emphasis = true;
        } else if key == overlapper_key {
            settings.is_overlapper = true;
        } else if key == inherit_key {
            current_grouping_settings.inherit =
                Some(parse_card_source(value, data, card_settings_indices)?);
        } else if key == orders_key {
            let orders = value
                .split(',')
                .map(|x| {
                    x.trim().parse::<usize>().map_err(|e| {
                        LibraryError::Card(CardErrorKind::InvalidSettings {
                            description: format!("The card order `{}` is invalid. Error: {}", x, e),
                            src: data.to_string(),
                            at: card_settings_indices.clone().into(),
                        })
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            current_grouping_settings.orders = Some(orders);
        } else if key == id_key {
            settings.cloze_uid = Some(ClozeUid::try_from(*value).map_err(|_| {
                LibraryError::Card(CardErrorKind::InvalidSettings {
                    description: format!("The cloze uid `{}` is invalid.", value),
                    src: data.to_string(),
                    at: card_settings_indices.clone().into(),
                })
            })?);
        } else {
            return Err(LibraryError::Card(CardErrorKind::InvalidSettings {
                description: format!("The key `{}` is not supported.", key),
                src: data.to_string(),
                at: card_settings_indices.clone().into(),
            }));
        }
    }

    // Validate settings
    if include_reverse && reverse_only {
        return Err(LibraryError::Card(CardErrorKind::InvalidSettings {
            description: "`include reverse` and `reverse only` are mutually exclusive settings."
                .to_string(),
            src: data.to_string(),
            at: card_settings_indices.clone().into(),
        }));
    }
    if include_reverse {
        current_grouping_settings.include_backward_card = true;
    } else if reverse_only {
        current_grouping_settings.include_forward_card = false;
        current_grouping_settings.include_backward_card = true;
    }
    // NOTE: This is not always true if a note changed from "o:1" to "o:1;r:" when it is being updated. In this case, a new order needs to be added.
    // if let Some(ref orders) = current_grouping_settings.orders {
    //     if current_grouping_settings.include_forward_card
    //         && current_grouping_settings.include_backward_card
    //         && orders.len() != 2
    //     {
    //         return Err(format!("Expected 2 orders, but found {}", orders.len()));
    //     }
    //     if (current_grouping_settings.include_forward_card
    //         ^ current_grouping_settings.include_backward_card)
    //         && orders.len() != 1
    //     {
    //         return Err(format!("Expected 1 order, but found {}", orders.len()));
    //     }
    // }
    Ok(current_grouping_settings)
}

pub fn parse_card_settings(
    data: &str,
    card_settings_indices: &Range<usize>,
    current_grouping_number: &mut u32,
    NoteSettingsKeys {
        settings_delim,
        settings_key_value_delim,
        groupings_all,
        ..
    }: &NoteSettingsKeys,
    cloze_settings_keys: &ClozeSettingsKeys,
    modify_defaults_fn: ModifyDefaultsFn,
) -> Result<(ClozeSettings, Vec<ClozeGroupingSettings>), LibraryError> {
    let mut settings = ClozeSettings::default();
    let grouping_key = cloze_settings_keys.grouping;
    let settings_split: Vec<(&str, &str)> = get_settings_pairs(
        data,
        card_settings_indices,
        settings_delim,
        settings_key_value_delim,
    )
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|(description, indices)| {
        LibraryError::Card(CardErrorKind::InvalidSettings {
            description,
            src: data.to_string(),
            at: indices.into(),
        })
    })?;
    let mut local_groups = settings_split
        .iter()
        .filter(|(k, _)| *k == grouping_key)
        .flat_map(|(_, v)| parse_grouping(v, current_grouping_number, groupings_all))
        .collect::<Vec<_>>();
    let mut settings_split_by_grouping =
        split_inclusive_following(&settings_split, |(k, _)| *k == grouping_key);
    let mut local_settings = None;
    if let Some(first_grouping) = settings_split_by_grouping.first()
        && let Some(first_setting) = first_grouping.first()
        && first_setting.0 != grouping_key
    {
        local_settings = Some(settings_split_by_grouping.remove(0));
        if local_groups.is_empty() {
            local_groups.push(ClozeGrouping::Auto(*current_grouping_number));
            *current_grouping_number += 1;
        }
    }
    if local_groups.contains(&ClozeGrouping::All) {
        settings.all_groupings = Some(ClozeGroupingSettings::default_from_grouping(
            ClozeGrouping::All,
            None,
        ));
    }
    let mut grouped_settings = settings_split_by_grouping
        .clone()
        .into_iter()
        .map(|mut grouping_settings| {
            // SAFETY: At this point we know that the first element in each grouping is a grouping key since we removed the one that possibly wasn't.
            let grouping_value = grouping_settings.first().map(|x| x.1).unwrap();
            grouping_settings.remove(0);
            (
                parse_grouping(grouping_value, current_grouping_number, groupings_all),
                grouping_settings,
            )
        })
        .flat_map(|(groupings, grouping_settings)| {
            groupings
                .into_iter()
                .map(|grouping| (grouping, grouping_settings.clone()))
                .collect::<Vec<_>>()
        })
        .into_group_by_insertion()
        .into_iter()
        .map(|(grouping, grouping_settings)| {
            (
                grouping,
                grouping_settings.into_iter().flatten().collect::<Vec<_>>(),
            )
        })
        .collect::<IndexMap<_, _>>();
    let mut all_grouping_settings = Vec::new();
    // Parse local settings first
    if let Some(local_settings) = local_settings {
        for grouping in local_groups {
            grouped_settings
                .entry(grouping)
                .and_modify(|v| {
                    v.extend(local_settings.clone());
                })
                .or_insert(local_settings.clone());
        }
    }

    // Parse grouping settings
    for (grouping, mut grouping_settings) in grouped_settings {
        let mut current_grouping_settings = parse_grouping_settings(
            &mut grouping_settings,
            &mut settings,
            current_grouping_number,
            data,
            card_settings_indices,
            cloze_settings_keys,
            modify_defaults_fn,
        )?;

        // Update groupings
        // let groupings = parse_grouping(grouping_value, current_grouping_number);
        // let groupings_parsed = groupings
        //     .iter()
        //     .map(|grouping| {
        //         let mut grouping_settings = current_grouping_settings.clone();
        //         grouping_settings.grouping = grouping.clone();
        //         grouping_settings
        //     })
        //     .collect::<Vec<_>>();
        current_grouping_settings.grouping = grouping;
        all_grouping_settings.push(current_grouping_settings);
    }
    if all_grouping_settings.is_empty() {
        let default_grouping_settings =
            ClozeGroupingSettings::default(current_grouping_number, modify_defaults_fn);
        all_grouping_settings.push(default_grouping_settings);
    }

    Ok((settings, all_grouping_settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::NoteSettingsKeys;
    use crate::parsers::Parseable;
    use crate::parsers::impls::markdown::MarkdownParser;

    #[test]
    fn test_construct_cloze_string_1() {
        let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let mut global_settings = ClozeSettings::default();
        global_settings.hint = Some("Test".to_string());

        let mut grouping_setting = ClozeGroupingSettings::default(&mut 1, None);
        grouping_setting.orders = Some(vec![1]);
        let all_grouping_settings = vec![grouping_setting];
        let NoteSettingsKeys {
            settings_delim,
            settings_key_value_delim,
            groupings_all,
            ..
        } = parser.note_settings_keys();
        let cloze_settings_keys = parser.cloze_settings_keys();
        let result = construct_cloze_string(
            &global_settings,
            &all_grouping_settings,
            &cloze_settings_keys,
            settings_delim,
            settings_key_value_delim,
            None,
            groupings_all,
            false,
        );
        let expected_result = "h:Test;o:1";
        assert_eq!(result, expected_result.to_string());
    }

    #[test]
    fn test_construct_cloze_string_2() {
        let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let mut global_settings = ClozeSettings::default();
        global_settings.hint = Some("Test".to_string());

        let grouping_setting = ClozeGroupingSettings::default(&mut 1, None);
        let all_grouping_settings = vec![grouping_setting];
        let NoteSettingsKeys {
            settings_delim,
            settings_key_value_delim,
            groupings_all,
            ..
        } = parser.note_settings_keys();
        let cloze_settings_keys = parser.cloze_settings_keys();
        let result = construct_cloze_string(
            &global_settings,
            &all_grouping_settings,
            &cloze_settings_keys,
            settings_delim,
            settings_key_value_delim,
            None,
            groupings_all,
            false,
        );
        let expected_result = "h:Test";
        assert_eq!(result, expected_result.to_string());
    }
}
