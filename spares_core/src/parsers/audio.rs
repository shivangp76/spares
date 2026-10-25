//! Audio clips: links to audio files in a note (e.g. `[🔊](/path/clip.mp3)` in Markdown or
//! `#link("/path/clip.mp3")[🔊]` in Typst). The rendered card shows the link's text, and clients
//! play the clips visible on each side of a card (see [`card_audio`]).
//!
//! ```md
//! How is "bonjour" pronounced? {{ [listen](/Users/me/media/bonjour.mp3) }}
//! ```
//! ```typst
//! #link("/Users/me/media/q.mp3")[🔊] Translate: #cl[#link("/Users/me/media/a.mp3")[🔊] good morning]
//! ```
//!
//! - Audio files are recognised by their extension: mp3, m4a, aac, ogg, oga, opus, wav and flac.
//!   Other links, including linked notes and images, are not audio.
//! - Paths must be absolute, as for images. A relative path gives an import warning, since neither
//!   a PDF viewer nor the server can find the file.
//! - In Markdown, leave a space between `{{` and a link at the start of a cloze, since `{{[` starts
//!   the cloze's settings.
//! - The link's text is typeset like any other text, so the PDF engine must support it. pdflatex,
//!   pandoc's default, fails on an emoji such as 🔊, and a font without it leaves the link blank.
//!   Use words such as `[listen](…)`, or an engine and font with the symbol.
//! - The LaTeX parser has no audio clips.
//! - In a rendered PDF, the link points at the clip's file, so clicking it depends on the PDF
//!   viewer. Some viewers, such as sioyek 2.0, do not open links to local files.
//!
//! # Which clips play
//!
//! - **The front** plays the clips it shows: those in the surrounding text, and in clozes of other
//!   cards that are not concealed (see `f:`).
//! - **The back** plays the clips it reveals: those in the card's answer clozes, and in other
//!   clozes if the back is the whole note (see `b:`). Clips already on the front are not played
//!   again.
//! - A clip inside a hidden cloze never plays on the front.
//! - **Replay** plays the front's clips, and then the back's if the card is flipped. In the web
//!   app, press `r` or click Replay in the audio bar above the card. In `spares card review`, pick
//!   Replay Audio.
//! - With `t`, audio links are not part of the expected answer.
//!
//! In the web app, clicking an audio link inside the card also plays it. If the browser blocks
//! autoplay before the page has been used, a Play button appears instead. Clips are fetched from
//! the server's render assets: rendering a note records its clips, and `spares sync cloud` sends
//! them, like the other files notes read. Clips are downloaded with the cards for offline review.
//! `spares card review` plays clips with
//! [`audio_player`](crate::config::SparesExternalConfig::audio_player).
//!
//! Text can also be read aloud by the device's voices (see [`super::tts`]). It is played along with
//! the clips, in the order both appear in the note.

use std::ops::Range;
use std::path::Path;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

use crate::helpers::get_or_compile_regex;
use crate::parsers::BackType;
use crate::parsers::CardData;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::NotePart;
use crate::parsers::NoteSettingsKeys;
use crate::parsers::Parseable;
use crate::parsers::get_settings_pairs;
use crate::parsers::image_occlusion::ConstructImageOcclusionType;
use crate::parsers::tts::TtsSetting;
use crate::parsers::tts::TtsVoice;

/// Extensions of the files whose links are audio clips, in lowercase.
pub const AUDIO_EXTENSIONS: [&str; 8] = ["mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac"];

/// Whether `path` names an audio file, judged by its extension.
pub fn is_audio_path(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// A link to an audio file in a note.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioRef {
    /// The whole link.
    pub range: Range<usize>,
    /// As written in the link.
    pub path: String,
}

/// Something a client plays for a card: an audio clip, or text to speak (see [`super::tts`]).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AudioItem {
    /// The absolute path of an audio clip.
    Clip(String),
    Speech(Speech),
}

/// Text a client speaks, without markup.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Speech {
    pub text: String,
    #[serde(flatten)]
    pub voice: TtsVoice,
}

/// What a client plays for a card, in the order it appears in the note.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CardAudio {
    /// Visible on the front. Played when the card is shown.
    pub front: Vec<AudioItem>,
    /// Revealed by the back. Played when the card is flipped. Clips already on the front are not
    /// repeated.
    pub back: Vec<AudioItem>,
}

/// The ranges of the matches of `regex` in `text`.
pub(crate) fn regex_ranges(regex: &str, text: &str) -> Vec<Range<usize>> {
    get_or_compile_regex(regex)
        .unwrap()
        .find_iter(text)
        .filter_map(Result::ok)
        .map(|m| m.range())
        .collect()
}

/// The links in `text` matched by `link_regex`, whose path is capture group 1 (or 2, as an
/// alternative spelling) and names an audio file. Links starting inside a match of
/// `ignored_regex` (e.g. code or comments) are skipped.
pub(crate) fn find_audio_refs(text: &str, link_regex: &str, ignored_regex: &str) -> Vec<AudioRef> {
    let ignored = regex_ranges(ignored_regex, text);
    get_or_compile_regex(link_regex)
        .unwrap()
        .captures_iter(text)
        .filter_map(Result::ok)
        .filter_map(|caps| {
            let link = caps.get(0)?;
            let path = caps.get(1).or_else(|| caps.get(2))?.as_str();
            (is_audio_path(path) && !ignored.iter().any(|r| r.contains(&link.start()))).then(|| {
                AudioRef {
                    range: link.range(),
                    path: path.to_string(),
                }
            })
        })
        .collect()
}

/// Where a part of a card is shown.
#[derive(Clone, Copy, PartialEq)]
enum Visibility {
    /// On both sides.
    Front,
    /// Hidden on the front, revealed on the back.
    Answer,
    /// Hidden on the front, and revealed on the back only if the back is the whole note.
    Other,
    /// Holds no audio.
    None,
}

/// What to play for `card`, or `None` if it has nothing: its audio clips, and its text to speak
/// (see [`super::tts`]), with `note_tts` the note's setting. Clips with relative paths are left
/// out, since they cannot be found (see the import warning in [`crate::parsers::get_notes`]).
pub fn card_audio(
    parser: &dyn Parseable,
    card: &CardData,
    note_tts: Option<&TtsSetting>,
) -> Option<CardAudio> {
    if card.back_type == BackType::Cli {
        return None;
    }
    let back_is_note = card.back_type == BackType::NoteFilePath;

    // Clips are found in the whole note, so code and comments spanning several parts are skipped.
    let mut note_data = String::new();
    let mut parts: Vec<(Range<usize>, Visibility)> = Vec::new();
    for part in &card.data {
        let (text, visibility) = match part {
            NotePart::SurroundingData(text)
            | NotePart::ClozeStart(text)
            | NotePart::ClozeEnd(text) => (text.clone(), Visibility::Front),
            NotePart::ClozeData(text, ClozeHiddenReplacement::ToAnswer { .. }) => {
                (text.clone(), Visibility::Answer)
            }
            NotePart::ClozeData(text, ClozeHiddenReplacement::NotToAnswer) => {
                (text.clone(), Visibility::Other)
            }
            NotePart::ImageOcclusion { data, .. } => (
                parser.construct_image_occlusion(data, ConstructImageOcclusionType::Note),
                Visibility::None,
            ),
            NotePart::Cli { data } => (parser.construct_cli_block(data), Visibility::None),
        };
        parts.push((note_data.len()..note_data.len() + text.len(), visibility));
        note_data.push_str(&text);
    }
    let visibility_at = |position: usize| {
        parts
            .iter()
            .find(|(range, _)| range.contains(&position))
            .map_or(Visibility::None, |(_, visibility)| *visibility)
    };
    // Whether something at `position` is played on the back, or on the front, or not at all
    let side_at = |position: usize| match visibility_at(position) {
        Visibility::Front => Some(false),
        Visibility::Answer => Some(true),
        Visibility::Other if back_is_note => Some(true),
        Visibility::Other | Visibility::None => None,
    };

    // Positions in the note, so clips and speech are played in order
    let mut front: Vec<(usize, AudioItem)> = Vec::new();
    let mut back: Vec<(usize, AudioItem)> = Vec::new();
    let audio_refs = parser.audio_refs(&note_data);
    for audio_ref in &audio_refs {
        if !Path::new(&audio_ref.path).is_absolute() {
            continue;
        }
        let Some(is_back) = side_at(audio_ref.range.start) else {
            continue;
        };
        let clip = AudioItem::Clip(audio_ref.path.clone());
        let items = if is_back { &mut back } else { &mut front };
        if !items.iter().any(|(_, item)| *item == clip) {
            items.push((audio_ref.range.start, clip));
        }
    }
    back.retain(|(_, clip)| !front.iter().any(|(_, item)| item == clip));
    for (position, speech, is_back) in speech_runs(
        parser,
        &note_data,
        &audio_refs,
        note_tts,
        &parts
            .iter()
            .map(|(range, _)| range.start)
            .collect::<Vec<_>>(),
        &side_at,
    ) {
        if is_back {
            back.push((position, AudioItem::Speech(speech)));
        } else {
            front.push((position, AudioItem::Speech(speech)));
        }
    }

    let in_order = |mut items: Vec<(usize, AudioItem)>| {
        items.sort_by_key(|(position, _)| *position);
        items.into_iter().map(|(_, item)| item).collect::<Vec<_>>()
    };
    let audio = CardAudio {
        front: in_order(front),
        back: in_order(back),
    };
    (!audio.front.is_empty() || !audio.back.is_empty()).then_some(audio)
}

/// The text of `note_data` to speak, as runs of text spoken on the same side by the same voice,
/// with where each starts and whether it is spoken on the back. `side_at` tells where text is
/// shown, as in [`card_audio`], which changes only at `part_starts`.
fn speech_runs(
    parser: &dyn Parseable,
    note_data: &str,
    audio_refs: &[AudioRef],
    note_tts: Option<&TtsSetting>,
    part_starts: &[usize],
    side_at: &dyn Fn(usize) -> Option<bool>,
) -> Vec<(usize, Speech, bool)> {
    let Ok(clozes) = parser.get_clozes(note_data) else {
        return Vec::new();
    };
    // Each cloze's text, with its own setting
    let NoteSettingsKeys {
        settings_delim,
        settings_key_value_delim,
        ..
    } = parser.note_settings_keys();
    let tts_key = parser.cloze_settings_keys().tts;
    let cloze_settings = clozes
        .iter()
        .filter_map(|cloze| {
            let tts = get_settings_pairs(
                note_data,
                &cloze.settings_match,
                settings_delim,
                settings_key_value_delim,
                true,
            )
            .into_iter()
            .filter_map(Result::ok)
            .find(|(key, _)| *key == tts_key)
            .and_then(|(_, value)| TtsSetting::from_str(value).ok())?;
            Some((cloze.start_match.end..cloze.end_match.start, tts))
        })
        .collect::<Vec<_>>();
    if note_tts.is_none() && cloze_settings.is_empty() {
        return Vec::new();
    }
    let setting_at = |position: usize| {
        cloze_settings
            .iter()
            .filter(|(range, _)| range.contains(&position))
            // The innermost cloze
            .min_by_key(|(range, _)| range.len())
            .map(|(_, tts)| tts.or(note_tts))
            .or_else(|| note_tts.cloned())
    };

    // Not spoken, but read past: cloze delimiters, comments and the like
    let mut skipped: Vec<Range<usize>> = clozes
        .iter()
        .flat_map(|cloze| [cloze.start_match.clone(), cloze.end_match.clone()])
        .chain(parser.unspoken_ranges(note_data))
        .collect();
    // Not spoken, and ending a run, so a clip is played in between
    let breaks: Vec<Range<usize>> = audio_refs.iter().map(|r| r.range.clone()).collect();
    skipped.extend(breaks.iter().cloned());

    // The note is cut wherever what is spoken may change, and the pieces are joined into runs
    let mut cuts: Vec<usize> = vec![0, note_data.len()];
    for range in skipped
        .iter()
        .chain(cloze_settings.iter().map(|(range, _)| range))
    {
        cuts.extend([range.start, range.end]);
    }
    cuts.extend(part_starts);
    cuts.retain(|cut| note_data.is_char_boundary(*cut));
    cuts.sort_unstable();
    cuts.dedup();

    let mut runs: Vec<(usize, Speech, bool)> = Vec::new();
    // The run being added to, and its text so far
    let mut current: Option<(usize, TtsVoice, bool, String)> = None;
    let mut end_run = |current: &mut Option<(usize, TtsVoice, bool, String)>| {
        if let Some((start, voice, is_back, text)) = current.take() {
            let text = parser
                .spoken_text(&text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if text.chars().any(char::is_alphanumeric) {
                runs.push((start, Speech { text, voice }, is_back));
            }
        }
    };
    for piece in cuts.windows(2).map(|w| w[0]..w[1]) {
        if piece.is_empty() {
            continue;
        }
        if breaks.iter().any(|r| r.contains(&piece.start)) {
            end_run(&mut current);
            continue;
        }
        if skipped.iter().any(|r| r.contains(&piece.start)) {
            continue;
        }
        let spoken = match (side_at(piece.start), setting_at(piece.start)) {
            (Some(is_back), Some(TtsSetting::Speak(voice))) => Some((is_back, voice)),
            _ => None,
        };
        match (spoken, &mut current) {
            (Some((is_back, voice)), Some((_, run_voice, run_is_back, text)))
                if *run_is_back == is_back && *run_voice == voice =>
            {
                text.push_str(&note_data[piece]);
            }
            (Some((is_back, voice)), _) => {
                end_run(&mut current);
                current = Some((piece.start, voice, is_back, note_data[piece].to_string()));
            }
            (None, _) => end_run(&mut current),
        }
    }
    end_run(&mut current);
    runs
}

/// `text` without its audio links, e.g. to find the answer a card expects to be typed.
pub(crate) fn remove_audio_refs(parser: &dyn Parseable, text: &str) -> String {
    let mut result = String::new();
    let mut start = 0;
    for audio_ref in parser.audio_refs(text) {
        result.push_str(&text[start..audio_ref.range.start]);
        start = audio_ref.range.end;
    }
    result.push_str(&text[start..]);
    result
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::parsers::get_cards;
    use crate::parsers::impls::latex::LatexParserNote;
    use crate::parsers::impls::markdown::MarkdownParser;
    use crate::parsers::impls::typst::TypstParser;

    fn audios(parser: &dyn Parseable, data: &str) -> Vec<Option<CardAudio>> {
        get_cards(parser, None, data, false, false)
            .unwrap()
            .iter()
            .map(|card| card_audio(parser, card, None))
            .collect()
    }

    #[expect(clippy::unnecessary_wraps, reason = "compared with `card_audio`")]
    fn audio(front: &[&str], back: &[&str]) -> Option<CardAudio> {
        let clips = |paths: &[&str]| {
            paths
                .iter()
                .map(|path| AudioItem::Clip((*path).to_string()))
                .collect()
        };
        Some(CardAudio {
            front: clips(front),
            back: clips(back),
        })
    }

    fn paths(parser: &dyn Parseable, text: &str) -> Vec<String> {
        parser
            .audio_refs(text)
            .into_iter()
            .map(|audio_ref| {
                assert!(text[audio_ref.range.clone()].contains(&audio_ref.path));
                audio_ref.path
            })
            .collect()
    }

    #[test]
    fn test_is_audio_path() {
        for path in ["/a/b.mp3", "c.M4A", "d.ogg", "e.opus", "f.wav", "g.flac"] {
            assert!(is_audio_path(path), "{path}");
        }
        for path in ["/a/b.png", "li", "mp3", "https://example.com"] {
            assert!(!is_audio_path(path), "{path}");
        }
    }

    #[test]
    fn test_markdown_audio_refs() {
        let parser = MarkdownParser::new();
        assert_eq!(
            paths(
                &parser,
                "[🔊](/m/a.mp3) [x](</m/with space.ogg>) [note][li] ![img](/m/b.png) \
                 ![not a link](/m/c.mp3) [x](https://example.com) [x](/m/d.WAV \"title\")"
            ),
            vec!["/m/a.mp3", "/m/with space.ogg", "/m/d.WAV"]
        );
        // Code, math and comments are skipped
        assert_eq!(
            paths(
                &parser,
                "`[x](/m/a.mp3)` $[x](/m/b.mp3)$ <!-- [x](/m/c.mp3) -->\n```\n[x](/m/d.mp3)\n```\n[x](/m/e.mp3)"
            ),
            vec!["/m/e.mp3"]
        );
    }

    #[test]
    fn test_typst_audio_refs() {
        let parser = TypstParser::new();
        assert_eq!(
            paths(
                &parser,
                r#"#link("/m/a.mp3")[🔊] #link( "/m/b.ogg" )[x] #link("https://example.com")[x] #image("/m/c.png") #cl[#link("/m/d.mp3")[🔊]][g:1]"#
            ),
            vec!["/m/a.mp3", "/m/b.ogg", "/m/d.mp3"]
        );
        assert_eq!(
            paths(
                &parser,
                "`#link(\"/m/a.mp3\")` // #link(\"/m/b.mp3\")\n/* #link(\"/m/c.mp3\") */ #link(\"/m/e.mp3\")"
            ),
            vec!["/m/e.mp3"]
        );
    }

    #[test]
    fn test_latex_has_no_audio() {
        let parser = LatexParserNote::new();
        assert_eq!(paths(&parser, r"\href{/m/a.mp3}{x}"), Vec::<String>::new());
    }

    #[test]
    fn test_card_audio() {
        let markdown = MarkdownParser::new();
        let typst = TypstParser::new();
        let cases: [(&dyn Parseable, &str, Vec<Option<CardAudio>>); 8] = [
            // J: audio as the answer
            (
                &markdown,
                "How is \"bonjour\" pronounced? {{ [🔊](/m/bonjour.mp3) }}",
                vec![audio(&[], &["/m/bonjour.mp3"])],
            ),
            // K: audio as the prompt
            (
                &markdown,
                "[🔊](/m/phrase12.mp3) What was said? {{[t:ci] je voudrais un café}}",
                vec![audio(&["/m/phrase12.mp3"], &[])],
            ),
            // L: audio in both places
            (
                &typst,
                r#"#link("/m/q.mp3")[🔊] Translate: #cl[#link("/m/a.mp3")[🔊] good morning][g:1]"#,
                vec![audio(&["/m/q.mp3"], &["/m/a.mp3"])],
            ),
            // M: another card's cloze is shown on the front
            (
                &markdown,
                "{{[g:1] [🔊](/m/one.mp3) uno}} and {{[g:2] dos}}",
                vec![audio(&[], &["/m/one.mp3"]), audio(&["/m/one.mp3"], &[])],
            ),
            // M: unless it is concealed, and then the back (the whole note) reveals it
            (
                &markdown,
                "{{[g:1;f:all] [🔊](/m/one.mp3) uno}} and {{[g:2;f:all] dos}}",
                vec![audio(&[], &["/m/one.mp3"]), audio(&[], &["/m/one.mp3"])],
            ),
            // ... but not a back showing only the answered clozes
            (
                &markdown,
                "{{[g:1;f:all;b:a] [🔊](/m/one.mp3) uno}} and {{[g:2;f:all;b:a] dos}}",
                vec![audio(&[], &["/m/one.mp3"]), None],
            ),
            // A clip on both sides plays only on the front, and relative paths are left out
            (
                &markdown,
                "[🔊](/m/a.mp3) [🔊](m/rel.mp3) {{ [🔊](/m/a.mp3) [🔊](/m/b.mp3) }}",
                vec![audio(&["/m/a.mp3"], &["/m/b.mp3"])],
            ),
            (&markdown, "No {{audio}}", vec![None]),
        ];
        for (parser, data, expected) in cases {
            assert_eq!(audios(parser, data), expected, "{data}");
        }
    }

    #[test]
    fn test_card_audio_reverse() {
        let parser = MarkdownParser::new();
        assert_eq!(
            audios(&parser, "[🔊](/m/q.mp3) {{[r] [🔊](/m/a.mp3) answer}}"),
            vec![
                audio(&["/m/q.mp3"], &["/m/a.mp3"]),
                audio(&["/m/a.mp3"], &["/m/q.mp3"]),
            ]
        );
    }

    /// What is played for each card of `data`, with clips written as their path and speech as
    /// `"LANG: text"`.
    fn played(parser: &dyn Parseable, data: &str, note_tts: Option<&str>) -> Played {
        let note_tts = note_tts.map(|tts| TtsSetting::from_str(tts).unwrap());
        let describe = |items: &[AudioItem]| {
            items
                .iter()
                .map(|item| match item {
                    AudioItem::Clip(path) => path.clone(),
                    AudioItem::Speech(Speech { text, voice }) => {
                        let mut tts = TtsSetting::Speak(voice.clone()).to_string();
                        tts.push_str(": ");
                        tts.push_str(text);
                        tts
                    }
                })
                .collect::<Vec<_>>()
        };
        get_cards(parser, None, data, false, false)
            .unwrap()
            .iter()
            .map(|card| {
                let audio = card_audio(parser, card, note_tts.as_ref()).unwrap_or_default();
                [describe(&audio.front), describe(&audio.back)]
            })
            .collect()
    }

    /// What a case of [`test_card_speech`] plays for each card: the front and the back.
    type Played = Vec<[Vec<String>; 2]>;

    fn sides(front: &[&str], back: &[&str]) -> [Vec<String>; 2] {
        [
            front.iter().map(ToString::to_string).collect(),
            back.iter().map(ToString::to_string).collect(),
        ]
    }

    #[test]
    fn test_card_speech() {
        let markdown = MarkdownParser::new();
        let typst = TypstParser::new();
        let latex = LatexParserNote::new();
        let cases: [(&dyn Parseable, &str, Option<&str>, Played); 12] = [
            // A cloze's own setting speaks its text on the back
            (
                &markdown,
                "Hello = {{[tts:fr_FR] *bonjour*}}",
                None,
                vec![sides(&[], &["fr_FR: bonjour"])],
            ),
            // The note's setting also speaks the text around the clozes, on the front
            (
                &markdown,
                "Hello = {{ bonjour }}",
                Some("en_US"),
                vec![sides(&["en_US: Hello ="], &["en_US: bonjour"])],
            ),
            // A cloze overrides the note part by part, and `off` silences it
            (
                &markdown,
                "Hi {{[tts:de_DE,0.8] Guten Tag}} {{[tts:off] (literal)}}",
                Some("en_US,1.2"),
                vec![
                    sides(&["en_US,1.2: Hi"], &["de_DE,0.8: Guten Tag"]),
                    sides(&["en_US,1.2: Hi", "de_DE,0.8: Guten Tag"], &[]),
                ],
            ),
            // A bare key uses the note's language, and the device's without one
            (
                &markdown,
                "{{[tts] uno}}",
                None,
                vec![sides(&[], &[": uno"])],
            ),
            // Another card's cloze is spoken on the front with the text around it
            (
                &markdown,
                "{{[g:1;tts:es] uno}} and {{[g:2] dos}}",
                Some("es"),
                vec![
                    sides(&["es: and dos"], &["es: uno"]),
                    sides(&["es: uno and"], &["es: dos"]),
                ],
            ),
            // ... unless it is concealed, and then a back showing only the answer does not speak it
            (
                &markdown,
                "{{[g:1;f:all;b:a] uno}} and {{[g:2;f:all;b:a] dos}}",
                Some("es"),
                vec![
                    sides(&["es: and"], &["es: uno"]),
                    sides(&["es: and"], &["es: dos"]),
                ],
            ),
            // Clips and speech in order, with a clip ending a run of text
            (
                &markdown,
                "Listen [🔊](/m/q.mp3) then answer {{ [🔊](/m/a.mp3) good morning }}",
                Some("en"),
                vec![sides(
                    &["en: Listen", "/m/q.mp3", "en: then answer"],
                    &["/m/a.mp3", "en: good morning"],
                )],
            ),
            // Comments, code and math are not spoken
            (
                &markdown,
                "<!-- hidden --> Area `code` $x^2$ of {{ a circle }}",
                Some("en"),
                vec![sides(&["en: Area of"], &["en: a circle"])],
            ),
            // The reverse card speaks the other way round
            (
                &markdown,
                "Dog {{[r] Hund}}",
                Some("de"),
                vec![
                    sides(&["de: Dog"], &["de: Hund"]),
                    sides(&["de: Hund"], &["de: Dog"]),
                ],
            ),
            (
                &typst,
                "Hello = #cl[#strong[bonjour]][tts:fr_FR] // comment",
                None,
                vec![sides(&[], &["fr_FR: bonjour"])],
            ),
            (
                &latex,
                r"Capital of \textbf{France}: \begin{cl}[tts:fr]Paris\end{cl} % comment",
                Some("en"),
                vec![sides(&["en: Capital of France:"], &["fr: Paris"])],
            ),
            // Nothing to speak
            (&markdown, "No {{audio}}", None, vec![sides(&[], &[])]),
        ];
        for (parser, data, note_tts, expected) in cases {
            assert_eq!(played(parser, data, note_tts), expected, "{data}");
        }
    }

    #[test]
    fn test_audio_item_json() {
        let audio = CardAudio {
            front: vec![AudioItem::Clip("/m/a.mp3".to_string())],
            back: vec![AudioItem::Speech(Speech {
                text: "bonjour".to_string(),
                voice: TtsVoice {
                    lang: Some("fr_FR".to_string()),
                    speed: None,
                    voice: None,
                },
            })],
        };
        let json = serde_json::to_value(&audio).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "front": ["/m/a.mp3"],
                "back": [{"text": "bonjour", "lang": "fr_FR"}],
            })
        );
        assert_eq!(serde_json::from_value::<CardAudio>(json).unwrap(), audio);
    }
}
