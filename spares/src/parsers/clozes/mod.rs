#[cfg(test)]
pub const DEFAULT_BACK_EMPHASIS: bool = false;

#[cfg(not(test))]
pub const DEFAULT_BACK_EMPHASIS: bool = true;

mod card_settings;
mod data;
mod matching;

pub use card_settings::*;
pub use data::*;
pub use matching::*;

#[cfg(test)]
mod tests {
    use crate::parsers::{NoteSettingsKeys, Parseable, impls::markdown::MarkdownParser};

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
        );
        let expected_result = "h:Test";
        assert_eq!(result, expected_result.to_string());
    }
}
