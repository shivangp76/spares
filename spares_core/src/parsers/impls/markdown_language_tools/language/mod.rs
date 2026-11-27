//! The languages the parser knows, one file each: what differs between them. Everything else in
//! the parser works the same for any language.
//!
//! To add a language, add a file with its [`Language`] and list it in [`LANGUAGES`].

use serde_json::Value;

use crate::model::CustomData;

mod chinese;
mod japanese;
mod korean;

/// The custom data key that sets a note's language, such as `"language": "ja"`.
pub const LANGUAGE_KEY: &str = "language";

/// What the parser needs to know about a language.
#[derive(Debug)]
pub struct Language {
    /// Its ISO 639-1 code, as written in a note's custom data and a dictionary's index.
    pub code: &'static str,
    pub name: &'static str,
    /// Whether words are separated by spaces, so a lookup takes the word under the caret, rather
    /// than trying each prefix of the text after it.
    pub written_with_spaces: bool,
    /// Whether text to speech reads a reading rather than its base, which is clearer when the
    /// reading is how the text sounds (Japanese kana) rather than a transcription (pinyin).
    pub speak_reading: bool,
    /// The PDF's CJK fonts, which TeX Live has.
    pub cjk_font: Option<CjkFont>,
    /// The dictionary forms `text` may be inflected from, as [`Deinflection`]s. The text itself
    /// is not among them.
    pub deinflect: fn(&str) -> Vec<Deinflection>,
}

/// A dictionary form an inflected text may come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deinflection {
    /// The dictionary form, such as `食べる` for `食べた`.
    pub term: String,
    /// The kinds of word it must be, as the rule identifiers of Yomitan dictionaries (`v1`, `v5`,
    /// `vs`, `vk`, `adj-i`). Any kind if empty.
    pub rules: Vec<&'static str>,
    /// How it was inflected, such as `past`, outermost last.
    pub reasons: Vec<&'static str>,
}

/// A PDF's CJK font: pandoc's `CJKmainfont`, and its bold (`CJKoptions`), which headings and
/// `**strong**` text need, or lualatex stops with "Missing font identifier".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CjkFont {
    pub regular: &'static str,
    pub bold: &'static str,
}

fn no_deinflection(_text: &str) -> Vec<Deinflection> {
    Vec::new()
}

/// Every language the parser knows.
pub const LANGUAGES: &[&Language] = &[&japanese::JAPANESE, &chinese::CHINESE, &korean::KOREAN];

impl Language {
    /// The language with the code `code`, such as `ja`. A region (`ja-JP`, `zh_CN`) is ignored.
    pub fn from_code(code: &str) -> Option<&'static Language> {
        let code = code
            .split(['-', '_'])
            .next()
            .unwrap_or(code)
            .to_ascii_lowercase();
        LANGUAGES.iter().copied().find(|l| l.code == code)
    }

    /// A note's language: its custom data's `language`, or else the one its text is written in
    /// ([`Self::detect`]).
    pub fn of_note(custom_data: &CustomData, text: &str) -> Option<&'static Language> {
        match custom_data.get(LANGUAGE_KEY).and_then(Value::as_str) {
            Some(code) => Self::from_code(code),
            None => Self::detect(text),
        }
    }

    /// The language `text` is written in, from its script: kana means Japanese, Hangul Korean.
    /// Text with only Han characters could be Chinese or Japanese, so it has none.
    pub fn detect(text: &str) -> Option<&'static Language> {
        if text.chars().any(is_kana) {
            Some(&japanese::JAPANESE)
        } else if text.chars().any(is_hangul) {
            Some(&korean::KOREAN)
        } else {
            None
        }
    }
}

pub fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}')
        // The prolonged sound mark and the iteration marks are shared with Han text
        && !matches!(c, '\u{30FB}')
}

pub fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7AF}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
}

pub fn is_han(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{2FA1F}' | '\u{3005}' | '\u{3007}')
}

/// Whether `c` is written in a script without spaces between words (Han and kana), where a lookup
/// starts at the character tapped rather than at the start of its word.
pub fn is_unspaced(c: char) -> bool {
    is_han(c) || is_kana(c)
}

/// Whether `text` has characters a PDF needs a CJK font for.
pub fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| is_han(c) || is_kana(c) || is_hangul(c))
}

/// The PDF's CJK font for a note in `language` whose text is `text`: the language's, or for text
/// with CJK characters but no language, the Japanese one, which has the Han characters.
pub fn cjk_font(language: Option<&Language>, text: &str) -> Option<CjkFont> {
    match language {
        Some(language) => language.cjk_font,
        None => has_cjk(text)
            .then_some(japanese::JAPANESE.cjk_font)
            .flatten(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_detect() {
        assert_eq!(Language::detect("日本語を話す").unwrap().code, "ja");
        assert_eq!(Language::detect("カタカナ").unwrap().code, "ja");
        assert_eq!(Language::detect("한국어").unwrap().code, "ko");
        assert!(Language::detect("中文").is_none());
        assert!(Language::detect("English").is_none());
    }

    #[test]
    fn test_of_note() {
        let custom_data = |value: Value| value.as_object().unwrap().clone();
        let zh = custom_data(json!({"language": "zh"}));
        assert_eq!(Language::of_note(&zh, "かな").unwrap().code, "zh");
        let region = custom_data(json!({"language": "ja-JP", "tts": "ja_JP"}));
        assert_eq!(Language::of_note(&region, "").unwrap().code, "ja");
        assert!(Language::of_note(&custom_data(json!({"language": "xx"})), "かな").is_none());
        assert_eq!(
            Language::of_note(&CustomData::default(), "かな")
                .unwrap()
                .code,
            "ja"
        );
    }

    #[test]
    fn test_cjk_font() {
        let zh = Language::from_code("zh");
        assert_eq!(cjk_font(zh, "").unwrap().regular, "FandolSong-Regular.otf");
        assert_eq!(
            cjk_font(None, "中文").unwrap().regular,
            "HaranoAjiMincho-Regular.otf"
        );
        assert_eq!(cjk_font(None, "English"), None);
    }
}
