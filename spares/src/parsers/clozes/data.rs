use super::DEFAULT_BACK_EMPHASIS;
use crate::model::NoteId;
use crate::parsers::image_occlusion::ImageOcclusionCloze;
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// See [`ClozeGroupingSettings`] for documentation.
pub type ModifyDefaultsFn = Option<(FrontConceal, BackReveal, bool)>;

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
    pub all_groupings: Option<ClozeGroupingSettings>,
}

#[derive(Debug)]
pub enum ClozeSettingsSide {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadableCardIdentifier {
    pub note_id: NoteId,
    pub order: usize,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub struct ClozeGroupingSettings {
    /// Any unique string that is shared among all clozes that are meant to be a part of the same card. The special keyword `*` indicates this cloze is a part of all groups.
    /// If this is not specified, then the cloze will be in its own group. In other words, a card will be created for this cloze and the card will only contain this one cloze.
    pub grouping: ClozeGrouping,
    /// When the card produced by this cloze is *newly created*, copy SRS fields from the card at the given note id and order. This field is not serialized, i.e., it is stripped from stored note data after processing.
    ///
    /// This is added to support 2 workflows:
    /// - Moving a card to a different note
    /// - Splitting a card into multiple cards
    pub inherit: Option<ReadableCardIdentifier>,
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
    /// Whether the clozes that should be answered should be emphasized on the back of the card.
    pub back_emphasis: bool,
    /// Internal
    /// Will not serialize this grouping in the cloze settings string
    pub skip_serialization: bool,
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
#[serde(rename_all = "snake_case")]
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
#[serde(rename_all = "snake_case")]
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
    NoteFilePath = 1,
    CardFilePath = 2,
}

impl BackType {
    pub fn from_back_reveal(
        back_reveal: &BackReveal,
        groupings_count: usize,
        emphasis: bool,
    ) -> Self {
        if emphasis {
            return BackType::CardFilePath;
        }
        match back_reveal {
            BackReveal::FullNote => BackType::NoteFilePath,
            BackReveal::OnlyAnswered => {
                if groupings_count == 1 {
                    // We can just use the full note as the back to avoid having to generate an extra card back.
                    BackType::NoteFilePath
                } else {
                    BackType::CardFilePath
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

    pub fn to_parser_string(&self, groupings_all: &str) -> String {
        match self {
            ClozeGrouping::All => groupings_all.to_string(),
            ClozeGrouping::Auto(_) => String::new(),
            ClozeGrouping::Custom(group) => group.clone(),
        }
    }
}

impl ClozeGroupingSettings {
    pub fn default_from_grouping(
        grouping: ClozeGrouping,
        modify_defaults_fn: ModifyDefaultsFn,
    ) -> Self {
        let mut result = Self {
            grouping,
            inherit: None,
            orders: None,
            include_forward_card: true,
            include_backward_card: false,
            is_suspended: None,
            hidden_no_answer: false,
            front_conceal: FrontConceal::default(),
            back_reveal: BackReveal::default(),
            back_emphasis: DEFAULT_BACK_EMPHASIS,
            skip_serialization: false,
        };
        if let Some((front_conceal, back_reveal, back_emphasis)) = modify_defaults_fn {
            result.front_conceal = front_conceal;
            result.back_reveal = back_reveal;
            result.back_emphasis = back_emphasis;
        }
        result
    }

    pub fn default(
        current_grouping_number: &mut u32,
        modify_defaults_fn: ModifyDefaultsFn,
    ) -> Self {
        ClozeGroupingSettings::default_from_grouping(
            ClozeGrouping::default(current_grouping_number),
            modify_defaults_fn,
        )
    }
}
