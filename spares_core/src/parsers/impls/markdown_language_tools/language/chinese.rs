//! Chinese: no inflection, and pinyin readings, which text to speech reads less well than the
//! characters.

use super::CjkFont;
use super::Language;
use super::no_deinflection;

pub static CHINESE: Language = Language {
    code: "zh",
    name: "Chinese",
    written_with_spaces: false,
    speak_reading: false,
    // Simplified and traditional characters (Fandol, in TeX Live)
    cjk_font: Some(CjkFont {
        regular: "FandolSong-Regular.otf",
        bold: "FandolSong-Bold.otf",
    }),
    deinflect: no_deinflection,
};
