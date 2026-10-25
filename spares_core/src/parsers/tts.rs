//! Text to speech: card text read aloud by the device's voices, like Anki's `{{tts}}`. Nothing is
//! recorded or stored: clients speak the text when a card is shown, as they play
//! [audio clips](super::audio).
//!
//! The cloze key `tts` speaks a cloze's text, and the note setting `tts` speaks all of a note's
//! text, including the text around its clozes:
//!
//! ```md
//! <!--- # tts: fr_FR --->
//! Hello: {{ bonjour }} {{[tts:de_DE,0.8] Guten Tag}} {{[tts:off] (not read)}}
//! ```
//! ```typst
//! #cl[bonjour][tts:fr_FR]
//! ```
//!
//! The value is `LANG,SPEED,VOICE`, each part optional, or `off` (see [`TtsSetting`]):
//!
//! - `LANG` is a language such as `fr_FR` (Anki's spelling) or `fr-FR`. Without one, the device's
//!   default voice speaks.
//! - `SPEED` multiplies the voice's rate, e.g. `0.8` or `1.5`.
//! - `VOICE` is the name of a voice, as listed in the web app's settings. It is used when the
//!   device has it, and otherwise a voice for `LANG`.
//!
//! A cloze's value overrides the note's, part by part, so `{{[tts:,0.8] …}}` keeps the note's
//! language. `tts:off` keeps a cloze silent, and a cloze nested in it, unless that cloze has a
//! `tts` of its own. Without the note setting, only clozes with `tts` are spoken. The note setting
//! is stored in the note's custom data, so editing the note shows it as `custom-data: {"tts": …}`.
//!
//! # What is spoken
//!
//! Text is spoken on the sides it is shown, like audio clips:
//!
//! - **The front** speaks the text it shows: the text around the clozes (with the note setting),
//!   and other cards' clozes that are not concealed.
//! - **The back** speaks what it reveals: the card's answer clozes, and the other clozes if the
//!   back is the whole note (see `b:`).
//! - **Replay** speaks the front, and then the back if the card is flipped.
//!
//! Speech and clips play in the order they appear in the note. Text is spoken without its markup.
//! Cloze delimiters, audio links, comments, code and math are not spoken, and neither are image
//! occlusions or CLI blocks.
//!
//! The web app speaks with the browser's voices, which differ between devices. Speech can be turned
//! off in its settings. `spares card review` speaks with
//! [`tts_command`](crate::config::SparesExternalConfig::tts_command), and skips speech without it.
//! Anki's `[anki:tts]` tags become the note setting when a note is imported from Anki.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::model::CustomData;

/// Speeds a client can speak at, as for the web's `speechSynthesis`.
const SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.1..=10.0;

/// How text is spoken. Each part is optional, and a missing part is filled in by the note's setting
/// or the device.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TtsVoice {
    /// A language such as `fr_FR` or `fr-FR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// A multiplier of the voice's rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// The name of a voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

impl TtsVoice {
    /// `self`, with the parts it lacks taken from `defaults`.
    #[must_use]
    pub fn or(self, defaults: &TtsVoice) -> TtsVoice {
        TtsVoice {
            lang: self.lang.or_else(|| defaults.lang.clone()),
            speed: self.speed.or(defaults.speed),
            voice: self.voice.or_else(|| defaults.voice.clone()),
        }
    }
}

/// The value of the cloze key or note setting `tts` (see [the module docs](self)), written
/// `LANG,SPEED,VOICE` or `off`.
#[derive(Clone, Debug, PartialEq)]
pub enum TtsSetting {
    Off,
    Speak(TtsVoice),
}

impl TtsSetting {
    pub const KEY: &str = "tts";

    /// The setting for text under `self`, a cloze's setting, in a note whose setting is `note`.
    #[must_use]
    pub fn or(&self, note: Option<&TtsSetting>) -> TtsSetting {
        match (self, note) {
            (TtsSetting::Speak(voice), Some(TtsSetting::Speak(defaults))) => {
                TtsSetting::Speak(voice.clone().or(defaults))
            }
            _ => self.clone(),
        }
    }
}

impl TtsSetting {
    /// The note setting, from a note's custom data. A value that is not a setting is ignored.
    pub fn of_note(custom_data: &CustomData) -> Option<TtsSetting> {
        custom_data
            .get(Self::KEY)
            .and_then(Value::as_str)
            .and_then(|value| value.parse().ok())
    }
}

impl FromStr for TtsSetting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s == "off" {
            return Ok(TtsSetting::Off);
        }
        let mut parts = s.splitn(3, ',').map(str::trim);
        let non_empty = |part: Option<&str>| part.filter(|p| !p.is_empty()).map(str::to_string);
        let lang = non_empty(parts.next());
        if let Some(lang) = &lang
            && !lang
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "The language `{lang}` is not a language code such as `fr_FR`."
            ));
        }
        let speed = non_empty(parts.next())
            .map(|speed| {
                speed
                    .parse::<f32>()
                    .ok()
                    .filter(|speed| SPEED_RANGE.contains(speed))
                    .ok_or_else(|| {
                        format!(
                            "The speed `{speed}` is not a number from {} to {}.",
                            SPEED_RANGE.start(),
                            SPEED_RANGE.end()
                        )
                    })
            })
            .transpose()?;
        let voice = non_empty(parts.next());
        Ok(TtsSetting::Speak(TtsVoice { lang, speed, voice }))
    }
}

impl fmt::Display for TtsSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let TtsSetting::Speak(TtsVoice { lang, speed, voice }) = self else {
            return write!(f, "off");
        };
        let mut parts = vec![
            lang.clone().unwrap_or_default(),
            speed.map(|speed| speed.to_string()).unwrap_or_default(),
            voice.clone().unwrap_or_default(),
        ];
        while parts.last().is_some_and(String::is_empty) {
            parts.pop();
        }
        write!(f, "{}", parts.join(","))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn voice(lang: Option<&str>, speed: Option<f32>, name: Option<&str>) -> TtsSetting {
        TtsSetting::Speak(TtsVoice {
            lang: lang.map(ToString::to_string),
            speed,
            voice: name.map(ToString::to_string),
        })
    }

    #[test]
    fn test_parse_and_display() {
        let cases = [
            ("", voice(None, None, None), ""),
            ("off", TtsSetting::Off, "off"),
            ("fr_FR", voice(Some("fr_FR"), None, None), "fr_FR"),
            (
                " en-US , 1.5 ",
                voice(Some("en-US"), Some(1.5), None),
                "en-US,1.5",
            ),
            (
                "de_DE,0.8,Anna",
                voice(Some("de_DE"), Some(0.8), Some("Anna")),
                "de_DE,0.8,Anna",
            ),
            (
                ",,Google français",
                voice(None, None, Some("Google français")),
                ",,Google français",
            ),
        ];
        for (input, expected, displayed) in cases {
            let setting = TtsSetting::from_str(input).unwrap();
            assert_eq!(setting, expected, "{input}");
            assert_eq!(setting.to_string(), displayed, "{input}");
            assert_eq!(
                TtsSetting::from_str(displayed).unwrap(),
                expected,
                "{input}"
            );
        }
        for input in ["fr FR", "fr,fast", "fr,0", "fr,11"] {
            assert!(TtsSetting::from_str(input).is_err(), "{input}");
        }
    }

    #[test]
    fn test_or() {
        let note = voice(Some("fr_FR"), Some(0.8), None);
        assert_eq!(
            voice(None, Some(1.2), Some("Anna")).or(Some(&note)),
            voice(Some("fr_FR"), Some(1.2), Some("Anna"))
        );
        assert_eq!(TtsSetting::Off.or(Some(&note)), TtsSetting::Off);
        assert_eq!(
            voice(Some("de"), None, None).or(Some(&TtsSetting::Off)),
            voice(Some("de"), None, None)
        );
        assert_eq!(voice(None, None, None).or(None), voice(None, None, None));
    }
}
