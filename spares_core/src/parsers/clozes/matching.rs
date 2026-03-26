use super::data::ClozeSettingsSide;
use crate::helpers::find_pairs;
use crate::parsers::RegexMatch;
use crate::{DelimiterErrorKind, LibraryError};
use fancy_regex::Regex;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub struct ClozeMatch {
    // Both `start_match_range` and `end_match_range` are needed. We can't do just `range: (start_match_range.start..end_match_range.end)`. This is because when parsing cards, we create `NotePart::ClozeStart` and `NotePart::ClozeEnd`.
    pub start_match: Range<usize>,
    pub end_match: Range<usize>,
    /// This must be contained within either `start_match` or `end_match`.
    pub settings_match: Range<usize>,
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
                .map(|x| x.start()..x.end())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let start_matches = cloze_start_regex
        .find_iter(data)
        .map(|m| m.unwrap())
        .map(|m| m.start()..m.end())
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
                .map(|x| x.start()..x.end())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let end_matches = cloze_end_regex
        .find_iter(data)
        .map(|m| m.unwrap())
        .map(|m| m.start()..m.end())
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
