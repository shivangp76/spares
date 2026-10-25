//! Examples of how a note is written for a parser, as the web app's note editor shows them (its
//! "Syntax" button).
//!
//! Each example is written with the parser's own functions ([`Parseable::construct_cloze`],
//! [`Parseable::construct_linked_note`], …) and settings keys ([`ClozeSettingsKeys`]), and its
//! cards are found by parsing it as a new note is ([`add_order_to_note_data`]). So the syntax and
//! the cards shown are the parser's, even after it changes. The tests check that each example
//! makes the cards its description says.

use serde::Serialize;

use crate::Error;
use crate::parsers::BackReveal;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::FrontConceal;
use crate::parsers::NotePart;
use crate::parsers::Parseable;
use crate::parsers::TypeAnswer;
use crate::parsers::add_order_to_note_data;
use crate::parsers::cards::overlapper::OverlapperConfig;

/// Shown in place of a hidden cloze without a hint.
const BLANK: &str = "[…]";

/// One way of writing something in a note, with the cards it makes.
#[derive(Clone, Debug, Serialize)]
pub struct SyntaxExample {
    /// What it is, like "Hint".
    pub title: &'static str,
    /// What it does. Text in backticks is code.
    pub about: String,
    /// The example, as written in a note.
    pub source: String,
    /// The cards the example makes, in order.
    pub cards: Vec<CardPreview>,
}

/// A card as plain text: its front, with what it hides replaced by a blank or its hint.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CardPreview {
    pub front: String,
    /// What the card asks for, in order.
    pub answers: Vec<String>,
}

/// The examples for `parser`. Overlapping clozes are split into cards by `overlapper`, as for the
/// notes the server creates (the `[overlapper]` table of `config.toml`).
#[expect(clippy::too_many_lines, reason = "A list of examples")]
pub fn syntax_examples(
    parser: &dyn Parseable,
    overlapper: Option<&OverlapperConfig>,
) -> Result<Vec<SyntaxExample>, Error> {
    let keys = parser.cloze_settings_keys();
    let note_keys = parser.note_settings_keys();
    // A cloze with the settings `settings`, each a key and a value, which may be empty
    let cloze = |text: &str, settings: &[(&str, &str)]| {
        let settings = settings
            .iter()
            .map(|(key, value)| {
                if value.is_empty() {
                    (*key).to_string()
                } else {
                    format!("{key}{}{value}", note_keys.settings_key_value_delim)
                }
            })
            .collect::<Vec<_>>()
            .join(note_keys.settings_delim);
        let (start, end) = parser.construct_cloze(&settings);
        format!("{start}{text}{end}")
    };
    let typed = |type_answer: TypeAnswer| type_answer.to_string();
    let ignore_case = typed(TypeAnswer::Diff {
        ignore_case: true,
        ignore_combining: false,
    });
    let ignore_combining = typed(TypeAnswer::Diff {
        ignore_case: false,
        ignore_combining: true,
    });
    let manual = typed(TypeAnswer::Manual);
    let all = FrontConceal::AllGroupings.to_string();
    let answered = BackReveal::OnlyAnswered.to_string();

    let mut examples: Vec<(&'static str, String, String)> = vec![
        (
            "Cloze",
            "Hides its text, making a card that asks for it.".to_string(),
            format!("The capital of France is {}.", cloze("Paris", &[])),
        ),
        (
            "Several clozes",
            "Each cloze makes its own card.".to_string(),
            format!(
                "{} is the capital of {}.",
                cloze("Paris", &[]),
                cloze("France", &[])
            ),
        ),
        (
            "Grouping",
            format!(
                "Clozes with the same `{}` are hidden together on one card. `{}{}{}` puts a cloze \
                 on every card of the note.",
                keys.grouping,
                keys.grouping,
                note_keys.settings_key_value_delim,
                note_keys.groupings_all,
            ),
            format!(
                "{} is the capital of {}.",
                cloze("Paris", &[(keys.grouping, "1")]),
                cloze("France", &[(keys.grouping, "1")])
            ),
        ),
        (
            "Hint",
            format!("`{}` shows a hint in place of the hidden text.", keys.hint),
            format!(
                "The capital of France is {}.",
                cloze("Paris", &[(keys.hint, "city")])
            ),
        ),
        (
            "Nested clozes",
            "A cloze inside another makes a card of its own.".to_string(),
            format!(
                "The capital of {}.",
                cloze(&format!("France is {}", cloze("Paris", &[])), &[])
            ),
        ),
        (
            "Reverse card",
            format!(
                "`{}` adds a card that shows the cloze and hides the rest of the note. `{}` makes \
                 only that card.",
                keys.include_reverse, keys.reverse_only
            ),
            format!("dog: {}", cloze("perro", &[(keys.include_reverse, "")])),
        ),
        (
            "Hidden, not asked",
            format!(
                "`{}` hides a cloze on its card without asking for it.",
                keys.hidden_no_answer
            ),
            format!(
                "{} is the capital of {}.",
                cloze("Paris", &[(keys.grouping, "1")]),
                cloze(
                    "France",
                    &[(keys.grouping, "1"), (keys.hidden_no_answer, "")]
                )
            ),
        ),
        (
            "Hide every cloze",
            format!(
                "`{}{}{all}` hides the note's other clozes too, not just the card's own.",
                keys.front_conceal, note_keys.settings_key_value_delim
            ),
            format!(
                "{} is the capital of {}.",
                cloze("Paris", &[(keys.front_conceal, &all)]),
                cloze("France", &[(keys.front_conceal, &all)])
            ),
        ),
        (
            "Back shows only the answer",
            format!(
                "`{}{}{answered}` shows only the answer on the back of the card, not the whole \
                 note. `{}` emphasizes the answer on the back.",
                keys.back_reveal, note_keys.settings_key_value_delim, keys.back_emphasis
            ),
            format!(
                "The capital of France is {}.",
                cloze("Paris", &[(keys.back_reveal, &answered)])
            ),
        ),
        (
            "Typed answer",
            format!(
                "`{t}` asks for the answer to be typed before the card is flipped, and compares \
                 it letter by letter. `{t}{d}{ignore_case}` ignores case, `{t}{d}{ignore_combining}` \
                 accents, and `{t}{d}{manual}` shows what you typed next to the answer, for you to \
                 judge.",
                t = keys.type_answer,
                d = note_keys.settings_key_value_delim,
            ),
            format!(
                "The capital of France is {}.",
                cloze("Paris", &[(keys.type_answer, &ignore_case)])
            ),
        ),
        (
            "Text to speech",
            format!(
                "`{tts}{d}fr_FR` reads the cloze aloud with the device's voice for French when the \
                 card is flipped. `{tts}{d}fr_FR,0.8,Thomas` also sets the speed and the voice, \
                 and `{tts}{d}off` keeps a cloze silent. The note setting `{tts}{d} en_US` reads \
                 the rest of the note too, and sets the language of a bare `{tts}`.",
                tts = keys.tts,
                d = note_keys.settings_key_value_delim,
            ),
            format!(
                "Good morning in French: {}",
                cloze("bonjour", &[(keys.tts, "fr_FR")])
            ),
        ),
        (
            "Overlapping clozes",
            format!(
                "Clozes marked `{}` are a sequence, like the lines of a poem. Each card asks for \
                 one, showing those around it as the `[overlapper]` table of `config.toml` sets.",
                keys.overlapper
            ),
            ["Roses are red", "Violets are blue", "Sugar is sweet"]
                .iter()
                .map(|line| cloze(line, &[(keys.overlapper, "")]))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "Suspended",
            format!(
                "`{}` creates the cloze's card suspended, so it is not reviewed until it is \
                 unsuspended.",
                keys.is_suspended
            ),
            format!(
                "The capital of France is {}.",
                cloze("Paris", &[(keys.is_suspended, "")])
            ),
        ),
        (
            "Inherit a card's reviews",
            format!(
                "`{}{}gir/1` gives the new card the scheduling and reviews of card 1 of note \
                 `gir`, as when moving a card to another note. It is removed from the note once \
                 the card is created.",
                keys.inherit, note_keys.settings_key_value_delim
            ),
            format!(
                "The capital of France is {}.",
                cloze("Paris", &[(keys.inherit, "gir/1")])
            ),
        ),
    ];
    if let Some(link) = parser.construct_linked_note("French cities") {
        examples.push((
            "Linked note",
            "Links to the note with this keyword, which can be opened from the card. Keywords \
             that are close match too."
                .to_string(),
            format!(
                "The capital of France is {}, one of the {link}.",
                cloze("Paris", &[])
            ),
        ));
    }
    if let Some(keyword) = parser.construct_keyword("capitals") {
        examples.push((
            "Keyword",
            "Adds its text to the note's keywords, and keeps it in the text.".to_string(),
            format!("Paris is one of the {keyword} of {}.", cloze("Europe", &[])),
        ));
    }

    examples
        .into_iter()
        .map(|(title, about, source)| {
            let (_, cards) = add_order_to_note_data(parser, &source, overlapper)?;
            let cards = cards
                .iter()
                .map(|card| preview(parser, &card.data))
                .collect::<Result<_, _>>()?;
            Ok(SyntaxExample {
                title,
                about,
                source,
                cards,
            })
        })
        .collect()
}

/// The front of a card made of `parts`, as plain text, and what it asks for. The delimiters of
/// the clozes that are not hidden on it, which other cards hide, are left out.
fn preview(parser: &dyn Parseable, parts: &[NotePart]) -> Result<CardPreview, Error> {
    let mut front = String::new();
    let mut answers = Vec::new();
    for part in parts {
        match part {
            NotePart::SurroundingData(text) => front.push_str(text),
            NotePart::ClozeData(text, ClozeHiddenReplacement::ToAnswer { hint }) => {
                match hint {
                    Some(hint) => {
                        front.push('[');
                        front.push_str(hint);
                        front.push(']');
                    }
                    None => front.push_str(BLANK),
                }
                answers.push(without_clozes(parser, text.trim())?);
            }
            NotePart::ClozeData(_, ClozeHiddenReplacement::NotToAnswer) => front.push_str(BLANK),
            NotePart::ClozeStart(_)
            | NotePart::ClozeEnd(_)
            | NotePart::ImageOcclusion { .. }
            | NotePart::Cli { .. } => {}
        }
    }
    Ok(CardPreview {
        front: without_clozes(parser, &front)?,
        answers,
    })
}

/// `text` without the delimiters of the clozes in it.
fn without_clozes(parser: &dyn Parseable, text: &str) -> Result<String, Error> {
    let mut delimiters = parser
        .get_clozes(text)?
        .into_iter()
        .flat_map(|cloze| [cloze.start_match, cloze.end_match])
        .collect::<Vec<_>>();
    delimiters.sort_by_key(|range| range.start);
    let mut result = String::new();
    let mut at = 0;
    for range in delimiters {
        result.push_str(&text[at..range.start]);
        at = range.end;
    }
    result.push_str(&text[at..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::parsers::extract_and_combine_keywords;
    use crate::parsers::get_all_parsers;
    use crate::parsers::get_cards;

    fn card(front: &str, answers: &[&str]) -> CardPreview {
        CardPreview {
            front: front.to_string(),
            answers: answers.iter().map(ToString::to_string).collect(),
        }
    }

    /// The cards of the example titled `title`, which every parser has.
    fn cards(examples: &[SyntaxExample], title: &str) -> Vec<CardPreview> {
        examples
            .iter()
            .find(|e| e.title == title)
            .unwrap_or_else(|| panic!("no example titled {title}"))
            .cards
            .clone()
    }

    /// Every example, of every parser, makes the cards its description says. The fronts are only
    /// compared for Markdown, whose clozes add no text of their own around a cloze.
    #[test]
    fn test_examples_make_the_cards_they_describe() {
        for parser in get_all_parsers() {
            let parser = parser();
            let name = parser.get_parser_name();
            let examples = syntax_examples(parser.as_ref(), Some(&OverlapperConfig::default()))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let count = |title| cards(&examples, title).len();
            let answers = |title| {
                cards(&examples, title)
                    .into_iter()
                    .map(|c| c.answers)
                    .collect::<Vec<_>>()
            };
            assert_eq!(answers("Cloze"), [["Paris"]], "{name}");
            assert_eq!(answers("Several clozes"), [["Paris"], ["France"]], "{name}");
            assert_eq!(answers("Grouping"), [["Paris", "France"]], "{name}");
            assert!(
                cards(&examples, "Hint")[0].front.contains("[city]"),
                "{name}"
            );
            assert_eq!(count("Nested clozes"), 2, "{name}");
            assert_eq!(count("Reverse card"), 2, "{name}");
            assert_eq!(answers("Hidden, not asked"), [["Paris"]], "{name}");
            assert_eq!(
                cards(&examples, "Hidden, not asked")[0]
                    .front
                    .matches(BLANK)
                    .count(),
                2,
                "{name}"
            );
            for card in cards(&examples, "Hide every cloze") {
                assert_eq!(card.answers.len(), 1, "{name}");
                assert_eq!(card.front.matches(BLANK).count(), 2, "{name}");
            }
            assert_eq!(count("Overlapping clozes"), 3, "{name}");
            assert!(
                examples.iter().any(|e| e.title == "Linked note")
                    == parser.construct_linked_note("x").is_some(),
                "{name}"
            );

            // The settings the descriptions name are the ones the cards get
            let source = |title| {
                examples
                    .iter()
                    .find(|e| e.title == title)
                    .unwrap()
                    .source
                    .clone()
            };
            let parsed = |title| get_cards(parser.as_ref(), None, &source(title), false, false);
            let parsed = |title| parsed(title).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                parsed("Back shows only the answer")[0].back_reveal,
                BackReveal::OnlyAnswered
            );
            assert_eq!(
                parsed("Typed answer")[0].type_answer,
                Some(TypeAnswer::Diff {
                    ignore_case: true,
                    ignore_combining: false
                })
            );
            let speech = parsed("Text to speech")
                .iter()
                .map(|card| crate::parsers::audio::card_audio(parser.as_ref(), card, None))
                .collect::<Vec<_>>();
            assert!(
                matches!(
                    &speech[..],
                    [Some(audio)] if audio.front.is_empty() && audio.back.len() == 1
                ),
                "{name}: {speech:?}"
            );
            assert_eq!(parsed("Suspended")[0].is_suspended, Some(true));
            assert_eq!(
                parsed("Inherit a card's reviews")[0]
                    .inherit
                    .map(|c| (c.note_id.letters(), c.order)),
                Some(("gir".to_string(), 1))
            );
            if parser.construct_linked_note("x").is_some() {
                let source = source("Linked note");
                let links = parser.get_linked_notes(&source).unwrap();
                let links = links.into_iter().map(|r| &source[r]).collect::<Vec<_>>();
                assert_eq!(links, ["French cities"], "{name}");
            }
            if parser.construct_keyword("x").is_some() {
                let keywords =
                    extract_and_combine_keywords(parser.as_ref(), &source("Keyword"), &[]).unwrap();
                let keywords = keywords.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
                assert_eq!(keywords, ["capitals"], "{name}");
            }
        }
    }

    /// The other options the typed answer's description names are ones it takes.
    #[test]
    fn test_type_answer_options() {
        for (option, expected) in [
            (
                "ci",
                TypeAnswer::Diff {
                    ignore_case: true,
                    ignore_combining: false,
                },
            ),
            (
                "nc",
                TypeAnswer::Diff {
                    ignore_case: false,
                    ignore_combining: true,
                },
            ),
            ("m", TypeAnswer::Manual),
        ] {
            assert_eq!(option.parse::<TypeAnswer>(), Ok(expected));
            assert_eq!(expected.to_string(), option);
        }
    }

    #[test]
    fn test_markdown_fronts() {
        let parser = crate::parsers::impls::markdown::MarkdownParser::new();
        let examples = syntax_examples(&parser, Some(&OverlapperConfig::default())).unwrap();
        assert_eq!(
            cards(&examples, "Cloze"),
            [card("The capital of France is […].", &["Paris"])]
        );
        assert_eq!(
            cards(&examples, "Hint"),
            [card("The capital of France is [city].", &["Paris"])]
        );
        assert_eq!(
            cards(&examples, "Several clozes"),
            [
                card("[…] is the capital of France.", &["Paris"]),
                card("Paris is the capital of […].", &["France"]),
            ]
        );
        assert_eq!(
            cards(&examples, "Nested clozes"),
            [
                card("The capital of […].", &["France is Paris"]),
                card("The capital of France is […].", &["Paris"]),
            ]
        );
    }
}
