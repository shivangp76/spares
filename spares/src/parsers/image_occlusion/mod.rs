//! # Image Occlusion
//!
//! NOTE: The quality of the original image determines the quality of the rendered image occlusion.
//!
//! ## Unsupported
//! - Grouped objects as a method of grouping clozes for a card. Instead, the cloze setting string must be used to specify the group.
//! - Image occlusion as a parser. It is more advanced than this; it is a cloze type. This means that there is no way to make a note with _only_ image occlusion without picking a parser.
//!
//! # svg crate
//! - `xmltree`
//!
//! The following crates were not chosen for the given reason:
//! - `roxmltree`: Not writable.
//! - `svg`: No DOM-style reading support. See <https://github.com/bodoni/svg/issues/41>.

use super::generate_files::CardSide;
use super::{BackReveal, FrontConceal};
use crate::model::NoteId;
use crate::parsers::{ClozeGroupingSettings, ClozeHiddenReplacement, ClozeSettings, Parseable};
use crate::{LibraryError, NoteErrorKind};
use construct::{get_clozes_from_svg_str, read_image_occlusion_data};
use fancy_regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use shellexpand;
use std::fs::read_to_string;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use strum::EnumString;
use strum_macros::EnumIter;

mod construct;
mod utils;
pub use construct::{
    combine_image_occlusion_clozes, construct_image_occlusion_from_image,
    create_image_occlusion_cards, update_cloze_settings,
};
#[cfg(test)]
pub use construct::{get_clozes_from_svg, modify_clozes_for_card};
#[cfg(all(test, feature = "testing"))]
pub use utils::get_image_occlusion_directory;
pub use utils::{get_image_occlusion_card_filepath, get_image_occlusion_rendered_directory};

#[cfg(all(test, feature = "testing"))]
mod test;

const CLOZE_SETTINGS_KEY: &str = "data-cloze-settings";
const CLOZES_GROUP_ID: &str = "clozes-group";

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ImageOcclusionConfig {
    pub cloze_to_answer_color: String,
    pub cloze_not_to_answer_color: String,
    pub cloze_hint_font_size: u32,
}

impl Default for ImageOcclusionConfig {
    fn default() -> Self {
        Self {
            cloze_to_answer_color: "#FF7E7E".to_string(),
            cloze_not_to_answer_color: "#FFEBA2".to_string(),
            cloze_hint_font_size: 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageOcclusionCloze {
    pub index: ImageOcclusionClozeIndex,
    pub data: Arc<ImageOcclusionData>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImageOcclusionClozeIndex {
    /// Original order the cloze appeared in the image
    /// 0 based indexing
    OriginalIndex(usize),
    /// All clozes that should be rendered in the card
    /// 0 based indexing
    MultipleIndices(Vec<(usize, ClozeHiddenReplacement)>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ImageOcclusionData {
    #[serde(deserialize_with = "deserialize_path_buf")]
    pub original_image_filepath: PathBuf,
    // /// Annotations to the original image that will be shown behind the clozes
    // #[serde(skip_serializing_if = "Option::is_none")]
    // pub markup_filepath: Option<PathBuf>,
    /// A file that is the same height and width as `original_image_filepath` and contains 2
    /// layers: "Markup" and "Clozes". The markup layer allows annotations to the original
    /// image that will be shown behind the clozes.
    #[serde(deserialize_with = "deserialize_path_buf")]
    pub clozes_filepath: PathBuf,
    // This is added as an option, rather than forcing the clozes filepath to already account for this.
    // This is so that
    // - this feature is easily toggleable
    // - each cloze can have a shorter settings string when editing the clozes file
    // # `CardConceal::OnlyGrouping`
    // - Equivalent to `HideOneGuessOne` and `HideGroupingGuessGrouping`
    //
    // # `CardConceal::AllGroupings`
    // Example: Note that in the worst case, you have 3 disjoint segments: All the previous groupings that are hidden, the current grouping, and lastly all the future groupings that are hidden.
    // {{[g:1; g:2;hide:; g:3;hide:]a}}
    // {{[g:1;hide:; g:2; g:3;hide:]b}}
    // {{[g:1,2;hide:; g:3]c}}
    // front = original_image + all_clozes (different colors so the user knows which one needs to be answered)
    //
    // This is implemented manually when cards are generated. The grouping logic above is not used
    // for simplicity. There is also the issue of clozes that are part of the auto group. The other
    // clozes do not have a group to refer to that cloze by to hide itself.
    //
    // This is the default since you do not want other data in the image to spoil what the cloze
    // is. For example, if the image is labelling parts of a brain and the other data is visible,
    // then you can use process of elimination to figure out what the cloze is.
    // This is also configurable through grouping settings. It is provided here on the image
    // occlusion for convenience. This is because it is more likely that you can use process of
    // elimination on an image than in text. Thus, a user may want text clozes to be visible on the
    // front of a card, but not image occlusion clozes. This feature makes this easy to accomplish.
    // This also allows the default to differ from the default for text clozes.
    /// Applies to all groups present in the image occlusion. This setting will boil up throughout
    /// the grouping, like other settings. For example, for a note first containing a text cloze
    /// with grouping 1, and then an image occlusion cloze with grouping 1, the text cloze's
    /// settings will be modified to match this value, since the setting boiled up.
    #[serde(default = "FrontConceal::image_occlusion_default")]
    pub front_conceal: FrontConceal,
    /// Applies to all groups present in the image occlusion. This setting will boil up throughout
    /// the grouping, like other settings. For example, for a note first containing a text cloze
    /// with grouping 1, and then an image occlusion cloze with grouping 1, the text cloze's
    /// settings will be modified to match this value, since the setting boiled up.
    #[serde(default = "BackReveal::image_occlusion_default")]
    pub back_reveal: BackReveal,
}

fn deserialize_path_buf<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let expanded = shellexpand::full(&s).map_err(serde::de::Error::custom)?;
    Ok(PathBuf::from(expanded.to_string()))
}

#[derive(Clone, Debug)]
pub struct ParsedImageOcclusionData {
    pub image_occlusion: ImageOcclusionData,
    pub start_delim: Range<usize>,
    pub end_delim: Range<usize>,
    /// Ordered by occurrence
    pub clozes: Vec<ParsedImageOcclusionCloze>,
}

#[derive(Clone, Debug)]
pub struct ParsedImageOcclusionCloze {
    pub settings: ClozeSettings,
    pub grouping_settings: Vec<ClozeGroupingSettings>,
}

#[derive(Clone, Copy, Debug)]
pub enum ConstructImageOcclusionType {
    Note,
    Card {
        side: CardSide,
        note_id: NoteId,
        card_order: usize,
        /// Order within the card
        image_occlusion_order: usize,
    },
}

#[derive(Clone, Copy, Debug, strum::Display, EnumIter, EnumString)]
#[strum(serialize_all = "lowercase")]
enum SvgClozeType {
    #[strum(serialize = "rect")]
    Rectangle,
    Circle,
    Ellipse,
    // Line,
    // Polyline,
    Polygon,
    Path,
}

pub fn parse_image_occlusion_data(
    data: &str,
    parser: &dyn Parseable,
    move_files: bool,
) -> Result<Vec<ParsedImageOcclusionData>, LibraryError> {
    let start = parser.construct_comment("(.*)");
    let regex_string = format!(r"(?m){}", start.trim());
    let image_occlusion_settings_regex = Regex::new(&regex_string).unwrap();
    let image_occlusion_ranges = parser.get_image_occlusions(data)?;
    let image_occlusion_range_with_settings = image_occlusion_ranges
        .into_iter()
        .map(|range| {
            let settings = image_occlusion_settings_regex
                .captures_iter(&data[range.capture_range.start..range.capture_range.end])
                .map(|c| c.unwrap().get(1).map(|x| (x.start()..x.end())).unwrap())
                .map(|r| (r.start + range.capture_range.start)..(r.end + range.capture_range.start))
                .collect::<Vec<_>>();
            (range, settings)
        })
        .collect::<Vec<_>>();
    let mut clozes = Vec::new();
    for (image_occlusion_range, setting_ranges) in image_occlusion_range_with_settings {
        let image_occlusion_data = read_image_occlusion_data(
            data,
            &setting_ranges,
            image_occlusion_range.capture_range,
            move_files,
        )?;
        let clozes_file_contents =
            read_to_string(&image_occlusion_data.clozes_filepath).map_err(|_| {
                LibraryError::Note(NoteErrorKind::InvalidSettings {
                    description: format!(
                        "Failed to read {}.",
                        &image_occlusion_data.clozes_filepath.display()
                    ),
                    advice: None,
                    src: data.to_string(),
                    at: image_occlusion_range.match_range.clone().into(),
                })
            })?;
        let parsed_clozes = get_clozes_from_svg_str(
            clozes_file_contents.as_str(),
            image_occlusion_data.front_conceal,
            image_occlusion_data.back_reveal,
        )?;
        clozes.push(ParsedImageOcclusionData {
            image_occlusion: image_occlusion_data,
            start_delim: (image_occlusion_range.match_range.start
                ..image_occlusion_range.match_range.start + 1),
            end_delim: (image_occlusion_range.match_range.end - 1
                ..image_occlusion_range.match_range.end),
            clozes: parsed_clozes,
        });
    }
    Ok(clozes)
}
