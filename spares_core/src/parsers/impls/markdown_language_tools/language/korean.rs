//! Korean: written with spaces between words. Inflected forms are found only if the dictionary
//! lists them.

use super::CjkFont;
use super::Language;
use super::no_deinflection;

pub static KOREAN: Language = Language {
    code: "ko",
    name: "Korean",
    written_with_spaces: true,
    speak_reading: false,
    // Un fonts, in TeX Live
    cjk_font: Some(CjkFont {
        regular: "UnBatang.ttf",
        bold: "UnBatangBold.ttf",
    }),
    deinflect: no_deinflection,
};
