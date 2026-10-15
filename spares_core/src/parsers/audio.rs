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

use std::ops::Range;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::helpers::get_or_compile_regex;
use crate::parsers::BackType;
use crate::parsers::CardData;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::NotePart;
use crate::parsers::Parseable;
use crate::parsers::image_occlusion::ConstructImageOcclusionType;

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

/// The audio clips a client plays for a card, as absolute paths.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CardAudio {
    /// Visible on the front. Played when the card is shown.
    pub front: Vec<String>,
    /// Revealed by the back. Played when the card is flipped. Clips already on the front are not
    /// repeated.
    pub back: Vec<String>,
}

/// The links in `text` matched by `link_regex`, whose path is capture group 1 (or 2, as an
/// alternative spelling) and names an audio file. Links starting inside a match of
/// `ignored_regex` (e.g. code or comments) are skipped.
pub(crate) fn find_audio_refs(text: &str, link_regex: &str, ignored_regex: &str) -> Vec<AudioRef> {
    let ignored = get_or_compile_regex(ignored_regex)
        .unwrap()
        .find_iter(text)
        .filter_map(Result::ok)
        .map(|m| m.range())
        .collect::<Vec<_>>();
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

/// The audio clips to play for `card`, or `None` if it has none. Clips with relative paths are left
/// out, since they cannot be found (see the import warning in [`crate::parsers::get_notes`]).
pub fn card_audio(parser: &dyn Parseable, card: &CardData) -> Option<CardAudio> {
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

    let mut audio = CardAudio::default();
    for audio_ref in parser.audio_refs(&note_data) {
        if !Path::new(&audio_ref.path).is_absolute() {
            continue;
        }
        let visibility = parts
            .iter()
            .find(|(range, _)| range.contains(&audio_ref.range.start))
            .map_or(Visibility::None, |(_, visibility)| *visibility);
        let clips = match visibility {
            Visibility::Front => &mut audio.front,
            Visibility::Answer => &mut audio.back,
            Visibility::Other if back_is_note => &mut audio.back,
            Visibility::Other | Visibility::None => continue,
        };
        if !clips.contains(&audio_ref.path) {
            clips.push(audio_ref.path);
        }
    }
    let front = audio.front.clone();
    audio.back.retain(|path| !front.contains(path));
    (!audio.front.is_empty() || !audio.back.is_empty()).then_some(audio)
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
            .map(|card| card_audio(parser, card))
            .collect()
    }

    #[expect(clippy::unnecessary_wraps, reason = "compared with `card_audio`")]
    fn audio(front: &[&str], back: &[&str]) -> Option<CardAudio> {
        Some(CardAudio {
            front: front.iter().map(ToString::to_string).collect(),
            back: back.iter().map(ToString::to_string).collect(),
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
}
