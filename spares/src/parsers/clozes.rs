use crate::helpers::{GroupByInsertion, find_pairs, split_inclusive_following};
use crate::parsers::{
    NoteSettingsKeys, RegexMatch, get_settings_pairs, image_occlusion::ImageOcclusionCloze,
};
use crate::{CardErrorKind, DelimiterErrorKind, LibraryError};
use fancy_regex::Regex;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::ops::Range;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq)]
pub struct ClozeMatch {
    // Both `start_match_range` and `end_match_range` are needed. We can't do just `range: (start_match_range.start..end_match_range.end)`. This is because when parsing cards, we create `NotePart::ClozeStart` and `NotePart::ClozeEnd`.
    pub start_match: Range<usize>,
    pub end_match: Range<usize>,
    /// This must be contained within either `start_match` or `end_match`.
    pub settings_match: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClozeData {
    /// Original order the cloze appeared in the note.
    pub index: usize,
    pub start_delim: Range<usize>,
    pub end_delim: Range<usize>,
    pub settings: ClozeSettings,
    pub image_occlusion: Option<ImageOcclusionCloze>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClozeSettings {
    pub hint: Option<String>,
    /// Internal
    all_groupings: bool,
}

#[derive(Debug)]
pub enum ClozeSettingsSide {
    Start,
    End,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub struct ClozeGroupingSettings {
    /// Any unique string that is shared among all clozes that are meant to be a part of the same card. The special keyword `*` indicates this cloze is a part of all groups.
    /// If this is not specified, then the cloze will be in its own group. In other words, a card will be created for this cloze and the card will only contain this one cloze.
    pub grouping: ClozeGrouping,
    /// `orders` is used to specify the order of the cards created from a cloze. Using this information, a card's parameters can be properly lined up when a cloze is added, deleted, or moved.
    /// It is only specified on the first cloze in a card, since that determines the order of how the cards are parsed.
    /// This is typically 1 number unless the cloze is a part of multiple cards, such as if the option to include the reverse card is enabled. This is so actions like adding a reverse card are properly accounted for as a card creation.
    ///
    /// For example, consider a note created with the following data, where `{{` and `}}` are used to create a cloze:
    /// ```md
    /// {{[g:1] Original: Card 1, Cloze 1 }}
    /// {{ Original: Card 2, Cloze 1 }}
    /// {{ Original: Card 3, Cloze 1 }}
    /// {{[g:1] Original: Card 1, Cloze 2 }}
    /// {{ Original: Card 4, Cloze 1 }}
    /// ```
    /// After adding this note, spares will automatically add the correct order to the first cloze of each card. Thus, the note's file will look like:
    /// ```md
    /// {{[o:1;g:1] Original: Card 1, Cloze 1 }}
    /// {{[o:2] Original: Card 2, Cloze 1 }}
    /// {{[o:3] Original: Card 3, Cloze 1 }}
    /// {{[g:1] Original: Card 1, Cloze 2 }}
    /// {{[o:4] Original: Card 4, Cloze 1 }}
    /// ```
    /// This is used for tracking each cloze as updates are later made. For example, consider some changes are made to the note so it now looks like:
    /// ```md
    /// {{[o:1] Original: Card 1, Cloze 1 }}
    /// {{ New cloze }}
    /// {{[o:3] Original: Card 3, Cloze 1 }}
    /// {{[o:2] Original: Card 2, Cloze 1 }}
    /// {{[g:1] Original: Card 1, Cloze 2 }}
    /// {{[o:4] Original: Card 4, Cloze 1 }}
    /// ```
    /// From this, spares will understand that:
    /// 1. The first cloze in cards 2 and 3 were swapped, so the cards should also be swapped.
    /// 2. A new cloze was created after the first cloze that is not a part of an existing group. Thus, a new card should be created.
    ///
    /// The returned note after the update will have its orders be sequential once again, so the note is ready for future changes.
    /// ```md
    /// {{[o:1;g:1] Original: Card 1, Cloze 1 }}
    /// {{[o:2] New cloze }}
    /// {{[o:3] Original: Card 3, Cloze 1 }}
    /// {{[o:4] Original: Card 2, Cloze 1 }}
    /// {{[g:1] Original: Card 1, Cloze 2 }}
    /// {{[o:5] Original: Card 4, Cloze 1 }}
    /// ```
    ///
    /// While you generally do not need to modify the order added to a cloze, there are some scenarios where changing the order will be helpful.
    /// For example, if you heavily modify a cloze in a card, you may think that the card should be reset since its old information no longer strongly matches with the new information. To do so, you can remove the order on the cloze which will create a new card.
    pub orders: Option<Vec<usize>>,
    pub include_forward_card: bool,
    pub include_backward_card: bool,
    // Ex. `s:` is true, `s:n` is false, `` (empty string) is `None`.
    // These 3 states are needed for updating a note to work correctly. The `None` option here
    // represents that we don't want to change the existing option. For example, a note may be
    // created with a suspended card. When updating that note, we specify `None` for this setting
    // to signify that we don't want to change it. If we defaulted to false, then this would
    // unsuspend that card which is not what we want, unless explicitly stated.
    pub is_suspended: Option<bool>,
    /// For clozes that should be hidden, but don't require an answer. For example, consider the note:
    /// ```md
    /// a{{[g:1;hide:]b}}{{[g:1]c}}
    /// ```
    /// The single card created from this note will only require "c" to be answered, even though "b" is also hidden.
    pub hidden_no_answer: bool,
    pub front_conceal: FrontConceal,
    pub back_reveal: BackReveal,
    /// Internal
    /// Will not serialize this grouping in the cloze settings string
    pub hidden: bool,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Serialize,
    Deserialize,
)]
pub enum FrontConceal {
    #[default]
    #[strum(serialize = "")]
    OnlyGrouping,
    #[strum(serialize = "all")]
    AllGroupings,
}

impl FrontConceal {
    pub fn image_occlusion_default() -> Self {
        FrontConceal::AllGroupings
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Default,
    strum::EnumString,
    strum_macros::Display,
    Serialize,
    Deserialize,
)]
pub enum BackReveal {
    #[default]
    #[strum(serialize = "n")]
    FullNote,
    #[strum(to_string = "a", serialize = "answered")]
    OnlyAnswered,
}

impl BackReveal {
    pub fn image_occlusion_default() -> Self {
        BackReveal::OnlyAnswered
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Default, Copy, Serialize, Deserialize, sqlx::Type)]
#[repr(u8)]
pub enum BackType {
    #[default]
    FullNote = 1,
    OnlyAnswered = 2,
}

impl BackType {
    pub fn from_back_reveal(back_reveal: &BackReveal, groupings_count: usize) -> Self {
        match back_reveal {
            BackReveal::FullNote => BackType::FullNote,
            BackReveal::OnlyAnswered => {
                if groupings_count == 1 {
                    BackType::FullNote
                } else {
                    BackType::OnlyAnswered
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ClozeGrouping {
    All,
    Auto(u32),
    Custom(String),
}

// impl Serialize for ClozeGrouping {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         match self {
//             ClozeGrouping::All => serializer.serialize_str("All"),
//             ClozeGrouping::Auto(num) => serializer.serialize_str(&format!("Auto({})", num)),
//             ClozeGrouping::Custom(s) => serializer.serialize_str(&format!("Custom({})", s)),
//         }
//     }
// }
//
// impl<'de> Deserialize<'de> for ClozeGrouping {
//     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
//     where
//         D: Deserializer<'de>,
//     {
//         let s = String::deserialize(deserializer)?;
//         if s == "All" {
//             Ok(ClozeGrouping::All)
//         } else if s.starts_with("Auto(") {
//             let num = s[5..s.len() - 1]
//                 .parse::<i64>()
//                 .map_err(serde::de::Error::custom)?;
//             Ok(ClozeGrouping::Auto(num))
//         } else if s.starts_with("Custom(") {
//             let custom_str = &s[7..s.len() - 1];
//             Ok(ClozeGrouping::Custom(custom_str.to_string()))
//         } else {
//             Err(serde::de::Error::custom("Unknown variant"))
//         }
//     }
// }

impl ClozeGrouping {
    fn default(current_grouping_number: &mut u32) -> Self {
        // ClozeGrouping::Auto(Uuid::new_v4())
        let result = ClozeGrouping::Auto(*current_grouping_number);
        *current_grouping_number += 1;
        result
    }
}

impl Display for ClozeGrouping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClozeGrouping::All => write!(f, "*"),
            ClozeGrouping::Auto(_) => write!(f, ""),
            ClozeGrouping::Custom(group) => write!(f, "{}", group),
        }
    }
}

impl ClozeGroupingSettings {
    pub fn default(
        current_grouping_number: &mut u32,
        modify_defaults_fn: ModifyDefaultsFn,
    ) -> Self {
        let mut result = Self {
            grouping: ClozeGrouping::default(current_grouping_number),
            orders: None,
            include_forward_card: true,
            include_backward_card: false,
            is_suspended: None,
            hidden_no_answer: false,
            front_conceal: FrontConceal::default(),
            back_reveal: BackReveal::default(),
            hidden: false,
        };
        if let Some((front_conceal, back_reveal)) = modify_defaults_fn {
            result.front_conceal = front_conceal;
            result.back_reveal = back_reveal;
        }
        result
    }
}

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
        }
    }
}

#[allow(clippy::too_many_lines)]
pub fn construct_cloze_string(
    global_settings: &ClozeSettings,
    grouping_settings: &[ClozeGroupingSettings],
    cloze_settings_keys: &ClozeSettingsKeys,
    settings_delim: &str,
    settings_key_value_delim: &str,
    modify_defaults_fn: ModifyDefaultsFn,
) -> String {
    // Global settings
    let mut parts: Vec<String> = Vec::new();
    if let Some(ref hint) = global_settings.hint {
        parts.push(format!(
            "{}{}{}",
            cloze_settings_keys.hint, settings_key_value_delim, hint
        ));
    }

    // Grouping setting
    let default = ClozeGroupingSettings::default(&mut 0, modify_defaults_fn);
    let mut all_grouping_parts: Vec<String> = Vec::new();
    let mut only_groups = Vec::new();
    if global_settings.all_groupings {
        all_grouping_parts.push(format!(
            "{}{}{}",
            cloze_settings_keys.grouping,
            settings_key_value_delim,
            ClozeGrouping::All
        ));
    }
    for (
        i,
        ClozeGroupingSettings {
            grouping,
            orders,
            include_forward_card,
            include_backward_card,
            is_suspended: _,
            hidden_no_answer,
            front_conceal,
            back_reveal,
            hidden,
        },
    ) in grouping_settings.iter().enumerate()
    {
        if *hidden {
            continue;
        }
        let mut grouping_parts: Vec<String> = Vec::new();
        let parse_grouping = !matches!(grouping, ClozeGrouping::Auto(_));
        if parse_grouping {
            let grouping_str = grouping.to_string();
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

        // Push settings
        if parse_grouping && grouping_parts.len() == 1 {
            if !global_settings.all_groupings {
                only_groups.push(grouping.clone());
            }
            grouping_parts.clear();
        }
        if ((parse_grouping && grouping_parts.len() > 1) || i == grouping_settings.len() - 1)
            && !only_groups.is_empty()
        {
            let groups_str = only_groups
                .drain(0..)
                .map(|grouping| grouping.to_string())
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

fn parse_grouping(input: &str, current_grouping_number: &mut u32) -> Vec<ClozeGrouping> {
    let values = input.split(',').collect::<Vec<_>>();
    if values.contains(&ClozeGrouping::All.to_string().as_str()) {
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
        front_conceal: front_key,
        back_reveal: back_key,
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
        } else if key == front_key {
            current_grouping_settings.front_conceal =
                FrontConceal::from_str(value).map_err(|e| {
                    LibraryError::Card(CardErrorKind::InvalidSettings {
                        description: format!("The card back `{}` is invalid. Error: {}", value, e),
                        src: data.to_string(),
                        at: card_settings_indices.clone().into(),
                    })
                })?;
        } else if key == back_key {
            current_grouping_settings.back_reveal = BackReveal::from_str(value).map_err(|e| {
                LibraryError::Card(CardErrorKind::InvalidSettings {
                    description: format!("The card back `{}` is invalid. Error: {}", value, e),
                    src: data.to_string(),
                    at: card_settings_indices.clone().into(),
                })
            })?;
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

// These don't work because a closure may capture variables that are not necessarily cloneable.
// type ModifyDefaultsFn = Option<fn(&mut ClozeGroupingSettings)>;
// type ModifyDefaultsFn = Option<impl Fn(&mut ClozeGroupingSettings)>;
// type ModifyDefaultsFn = Option<Arc<dyn Fn(&mut ClozeGroupingSettings)>>;
type ModifyDefaultsFn = Option<(FrontConceal, BackReveal)>;

#[allow(clippy::too_many_lines)]
pub fn parse_card_settings(
    data: &str,
    card_settings_indices: &Range<usize>,
    current_grouping_number: &mut u32,
    NoteSettingsKeys {
        settings_delim,
        settings_key_value_delim,
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
        .flat_map(|(_, v)| parse_grouping(v, current_grouping_number))
        .collect::<Vec<_>>();
    let mut settings_split_by_grouping =
        split_inclusive_following(&settings_split, |(k, _)| *k == grouping_key);
    let mut local_settings = None;
    if let Some(first_grouping) = settings_split_by_grouping.first() {
        if let Some(first_setting) = first_grouping.first() {
            if first_setting.0 != grouping_key {
                local_settings = Some(settings_split_by_grouping.remove(0));
                if local_groups.is_empty() {
                    local_groups.push(ClozeGrouping::Auto(*current_grouping_number));
                    *current_grouping_number += 1;
                }
            }
        }
    }
    if local_groups.contains(&ClozeGrouping::All) {
        settings.all_groupings = true;
    }
    let mut grouped_settings = settings_split_by_grouping
        .clone()
        .into_iter()
        .map(|mut grouping_settings| {
            // SAFETY: At this point we know that the first element in each grouping is a grouping key since we removed the one that possibly wasn't.
            let grouping_value = grouping_settings.first().map(|x| x.1).unwrap();
            grouping_settings.remove(0);
            (
                parse_grouping(grouping_value, current_grouping_number),
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

pub fn get_matched_clozes(
    data: &str,
    cloze_start_regex: &Regex,
    settings_capture_group_index: usize,
    cloze_end_regex: &Regex,
    cloze_settings_side: &ClozeSettingsSide,
) -> Result<Vec<ClozeMatch>, LibraryError> {
    let start_settings = cloze_start_regex
        .captures_iter(data)
        .map(|c| {
            c.unwrap()
                .get(settings_capture_group_index)
                .map(|x| (x.start()..x.end()))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let start_matches = cloze_start_regex
        .find_iter(data)
        .map(|m| m.unwrap())
        .map(|m| (m.start()..m.end()))
        .zip(start_settings)
        .map(|(match_range, capture_range)| RegexMatch {
            match_range,
            capture_range,
        })
        .collect::<Vec<_>>();
    let end_settings = cloze_end_regex
        .captures_iter(data)
        .map(|c| {
            c.unwrap()
                .get(settings_capture_group_index)
                .map(|x| (x.start()..x.end()))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let end_matches = cloze_end_regex
        .find_iter(data)
        .map(|m| m.unwrap())
        .map(|m| (m.start()..m.end()))
        .zip(end_settings)
        .map(|(match_range, capture_range)| RegexMatch {
            match_range,
            capture_range,
        })
        .collect::<Vec<_>>();
    if start_matches.len() != end_matches.len() {
        dbg!(&start_matches);
        dbg!(&end_matches);
        return Err(LibraryError::Delimiter(
            DelimiterErrorKind::UnequalMatches {
                src: data.to_string(),
            },
        ));
    }
    let matches = find_pairs(data, &start_matches, &end_matches)?;
    let result = matches
        .into_iter()
        .map(|(s, e)| ClozeMatch {
            start_match: s.match_range,
            end_match: e.match_range,
            settings_match: match cloze_settings_side {
                ClozeSettingsSide::Start => s.capture_range,
                ClozeSettingsSide::End => e.capture_range,
            },
        })
        .collect::<Vec<_>>();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::parsers::{Parseable, impls::markdown::MarkdownParser};

    use super::*;

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
        );
        let expected_result = "h:Test";
        assert_eq!(result, expected_result.to_string());
    }
}
