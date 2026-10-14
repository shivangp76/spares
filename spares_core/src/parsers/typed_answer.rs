//! Typed answers (the cloze key `t`, see [`TypeAnswer`]): the answers a card expects to be typed,
//! and the comparison of a typed answer with its expected answer.
//!
//! The comparison is ported to the web app (`spares_frontend/frontend/src/typedAnswer.ts`), so it
//! also works offline. Both run the cases in `tests/fixtures/typed_answer_cases.json`.

use std::ops::Range;

use serde::Deserialize;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::LibraryError;
use crate::parsers::CardData;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::NotePart;
use crate::parsers::Parseable;
use crate::parsers::TypeAnswer;
use crate::parsers::image_occlusion::ConstructImageOcclusionType;

/// One answer cloze of a card, which gets an input box of its own.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypedBlank {
    pub expected: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// What a client needs to ask for a card's typed answers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypedAnswerInfo {
    pub mode: TypeAnswer,
    /// In reading order. Also sent for [`TypeAnswer::Manual`], so its input boxes match.
    pub blanks: Vec<TypedBlank>,
}

/// The answers `card` expects to be typed, one per answer cloze in reading order, or `None` if
/// the card does not ask for a typed answer or has no answer that can be typed.
pub fn typed_answer_info(
    parser: &dyn Parseable,
    card: &CardData,
) -> Result<Option<TypedAnswerInfo>, LibraryError> {
    let Some(mode) = card.type_answer else {
        return Ok(None);
    };
    let blanks = expected_typed_answers(parser, card)?;
    Ok((!blanks.is_empty()).then_some(TypedAnswerInfo { mode, blanks }))
}

/// The text of each answer cloze of `card`, without markup, hints or the delimiters of clozes
/// nested in it. Clozes without text, such as one holding only an image, are left out.
pub fn expected_typed_answers(
    parser: &dyn Parseable,
    card: &CardData,
) -> Result<Vec<TypedBlank>, LibraryError> {
    // The parts of a card make up the whole note, so the delimiters of nested clozes are found by
    // parsing the note again.
    let mut note_data = String::new();
    let mut answers: Vec<(Range<usize>, Option<String>)> = Vec::new();
    for part in &card.data {
        match part {
            NotePart::SurroundingData(text)
            | NotePart::ClozeStart(text)
            | NotePart::ClozeEnd(text)
            | NotePart::ClozeData(text, ClozeHiddenReplacement::NotToAnswer) => {
                note_data.push_str(text);
            }
            NotePart::ClozeData(text, ClozeHiddenReplacement::ToAnswer { hint }) => {
                answers.push((note_data.len()..note_data.len() + text.len(), hint.clone()));
                note_data.push_str(text);
            }
            NotePart::ImageOcclusion { data, .. } => note_data.push_str(
                &parser.construct_image_occlusion(data, ConstructImageOcclusionType::Note),
            ),
            NotePart::Cli { data } => note_data.push_str(&parser.construct_cli_block(data)),
        }
    }
    let delimiters = parser
        .get_clozes(&note_data)?
        .into_iter()
        .flat_map(|cloze| [cloze.start_match, cloze.end_match])
        .collect::<Vec<_>>();

    Ok(answers
        .into_iter()
        .filter_map(|(range, hint)| {
            let mut text = String::new();
            let mut start = range.start;
            let mut inner_delimiters = delimiters
                .iter()
                .filter(|d| range.start <= d.start && d.end <= range.end)
                .collect::<Vec<_>>();
            inner_delimiters.sort_by_key(|d| d.start);
            for delimiter in inner_delimiters {
                text.push_str(&note_data[start..delimiter.start]);
                start = delimiter.end;
            }
            text.push_str(&note_data[start..range.end]);
            let expected = clean_typed_answer(&parser.typed_answer_text(&text));
            (!expected.is_empty()).then_some(TypedBlank { expected, hint })
        })
        .collect())
}

/// Collapses whitespace and normalizes to NFC.
fn clean_typed_answer(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .nfc()
        .collect()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// Typed as expected.
    Good,
    /// Typed, but not expected.
    Bad,
    /// Expected, but not typed.
    Missed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiffSegment {
    pub kind: SegmentKind,
    pub text: String,
}

/// A typed answer compared with its expected answer, as two lines of segments: the typed answer
/// (`Good` and `Bad`) and the expected answer (`Good` and `Missed`).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TypedDiff {
    pub typed: Vec<DiffSegment>,
    pub expected: Vec<DiffSegment>,
}

impl TypedDiff {
    /// Whether the typed answer matches the expected answer.
    pub fn is_exact(&self) -> bool {
        self.typed
            .iter()
            .chain(&self.expected)
            .all(|segment| segment.kind == SegmentKind::Good)
    }
}

/// Above this many character pairs, a mismatch is not diffed, but shown as wholly wrong.
const MAX_DIFF_CELLS: usize = 1_000_000;

/// Compares `typed` with `expected` character by character. Segments hold the characters as
/// typed or expected, even where case or combining marks are ignored.
///
/// The diff is a longest common subsequence, which keeps it simple to port exactly to the web app.
pub fn compare_typed_answer(
    expected: &str,
    typed: &str,
    ignore_case: bool,
    ignore_combining: bool,
) -> TypedDiff {
    let key = |c: char| -> String {
        let mut key: String = if ignore_combining {
            c.to_string()
                .nfkd()
                .filter(|c| !is_combining_mark(*c))
                .collect()
        } else {
            c.to_string()
        };
        if ignore_case {
            key = key.to_lowercase();
        }
        key
    };
    let expected = expected.trim().nfc().collect::<Vec<_>>();
    let typed = typed.trim().nfc().collect::<Vec<_>>();
    let expected_keys = expected.iter().map(|c| key(*c)).collect::<Vec<_>>();
    let typed_keys = typed.iter().map(|c| key(*c)).collect::<Vec<_>>();
    let (n, m) = (expected.len(), typed.len());

    let mut diff = DiffBuilder::default();
    if n.saturating_mul(m) > MAX_DIFF_CELLS {
        let kinds = if expected_keys == typed_keys {
            (SegmentKind::Good, SegmentKind::Good)
        } else {
            (SegmentKind::Missed, SegmentKind::Bad)
        };
        for c in &expected {
            diff.expected(kinds.0, *c);
        }
        for c in &typed {
            diff.typed(kinds.1, *c);
        }
        return diff.finish();
    }

    // `lcs[i * (m + 1) + j]` is the length of the longest common subsequence of `expected[i..]`
    // and `typed[j..]`.
    let mut lcs = vec![0_usize; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * (m + 1) + j] = if expected_keys[i] == typed_keys[j] {
                lcs[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if expected_keys[i] == typed_keys[j] {
            diff.expected(SegmentKind::Good, expected[i]);
            diff.typed(SegmentKind::Good, typed[j]);
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1] {
            diff.expected(SegmentKind::Missed, expected[i]);
            i += 1;
        } else {
            diff.typed(SegmentKind::Bad, typed[j]);
            j += 1;
        }
    }
    for c in &expected[i..] {
        diff.expected(SegmentKind::Missed, *c);
    }
    for c in &typed[j..] {
        diff.typed(SegmentKind::Bad, *c);
    }
    diff.finish()
}

/// Builds the segments of a [`TypedDiff`], merging neighbouring characters of the same kind.
#[derive(Default)]
struct DiffBuilder(TypedDiff);

impl DiffBuilder {
    fn push(segments: &mut Vec<DiffSegment>, kind: SegmentKind, c: char) {
        match segments.last_mut() {
            Some(last) if last.kind == kind => last.text.push(c),
            _ => segments.push(DiffSegment {
                kind,
                text: c.to_string(),
            }),
        }
    }

    fn typed(&mut self, kind: SegmentKind, c: char) {
        Self::push(&mut self.0.typed, kind, c);
    }

    fn expected(&mut self, kind: SegmentKind, c: char) {
        Self::push(&mut self.0.expected, kind, c);
    }

    fn finish(self) -> TypedDiff {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::parsers::get_cards;
    use crate::parsers::impls::markdown::MarkdownParser;
    use crate::parsers::impls::typst::TypstParser;

    const DIFF: TypeAnswer = TypeAnswer::Diff {
        ignore_case: false,
        ignore_combining: false,
    };

    /// The typed answer info of each card of `data`.
    fn infos(parser: &dyn Parseable, data: &str) -> Vec<Option<TypedAnswerInfo>> {
        get_cards(parser, None, data, false, false)
            .unwrap()
            .iter()
            .map(|card| typed_answer_info(parser, card).unwrap())
            .collect()
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "compared with `typed_answer_info`"
    )]
    fn info(mode: TypeAnswer, expected: &[&str]) -> Option<TypedAnswerInfo> {
        Some(TypedAnswerInfo {
            mode,
            blanks: expected
                .iter()
                .map(|expected| TypedBlank {
                    expected: (*expected).to_string(),
                    hint: None,
                })
                .collect(),
        })
    }

    #[test]
    fn test_expected_typed_answers() {
        let markdown = MarkdownParser::new();
        let typst = TypstParser::new();
        let cases: [(&dyn Parseable, &str, Vec<Option<TypedAnswerInfo>>); 12] = [
            // A
            (
                &markdown,
                "The capital of France is {{[t] Paris}}.",
                vec![info(DIFF, &["Paris"])],
            ),
            // C: a box per cloze, in reading order
            (
                &markdown,
                "The primary colours are {{[g:1;t] red}}, {{[g:1] green}} and {{[g:1] blue}}.",
                vec![info(DIFF, &["red", "green", "blue"])],
            ),
            // C2
            (
                &markdown,
                "The US capital is {{[t] Washington, D.C.}}",
                vec![info(DIFF, &["Washington, D.C."])],
            ),
            // E
            (
                &markdown,
                "Why is the sky blue? {{[t:m] Rayleigh scattering of\n  shorter wavelengths}}",
                vec![info(
                    TypeAnswer::Manual,
                    &["Rayleigh scattering of shorter wavelengths"],
                )],
            ),
            // F
            (
                &typst,
                "The derivative of $x^2$ is #cl[$2x$][t:m]",
                vec![info(TypeAnswer::Manual, &["2x"])],
            ),
            (
                &typst,
                "The derivative of $x^2$ is #cl[$2x$][t]",
                vec![info(DIFF, &["2x"])],
            ),
            // G: the reverse card expects the rest of the note
            (
                &markdown,
                "The capital of France is {{[t;r] Paris}}.",
                vec![
                    info(DIFF, &["Paris"]),
                    info(DIFF, &["The capital of France is", "."]),
                ],
            ),
            // H: a nested cloze is part of the outer cloze's answer
            (
                &markdown,
                "{{[g:1;t] The {{[g:2] mitochondria}} is the powerhouse}}",
                vec![info(DIFF, &["The mitochondria is the powerhouse"]), None],
            ),
            (
                &typst,
                "#cl[The #cl[mitochondria][g:2] is the powerhouse][g:1;t]",
                vec![info(DIFF, &["The mitochondria is the powerhouse"]), None],
            ),
            // Markup is removed
            (
                &markdown,
                "{{[t] [Paris](https://example.com), `x_1` and [*a*]{.smallcaps} }}",
                vec![info(DIFF, &["Paris, x1 and a"])],
            ),
            (
                &typst,
                r#"#cl[#link("https://example.com/(a)")[Paris] and *b* _c_][t]"#,
                vec![info(DIFF, &["Paris and b c"])],
            ),
            // N: an answer without text gets no box
            (&markdown, "a {{[t] ![](/m/x.png)}}", vec![None]),
        ];
        for (parser, data, expected) in cases {
            assert_eq!(infos(parser, data), expected, "{data}");
        }
    }

    #[test]
    fn test_expected_typed_answers_hint() {
        // D
        let parser = MarkdownParser::new();
        assert_eq!(
            infos(&parser, "Water boils at {{[t;h:°C] **100**}} degrees."),
            vec![Some(TypedAnswerInfo {
                mode: DIFF,
                blanks: vec![TypedBlank {
                    expected: "100".to_string(),
                    hint: Some("°C".to_string()),
                }],
            })]
        );
    }

    #[derive(Deserialize)]
    #[expect(clippy::struct_field_names, reason = "named like the parameters")]
    struct Case {
        name: String,
        expected: String,
        typed: String,
        ignore_case: bool,
        ignore_combining: bool,
        diff: TypedDiff,
    }

    #[test]
    fn test_compare_typed_answer_fixture() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../tests/fixtures/typed_answer_cases.json"))
                .unwrap();
        for case in cases {
            let diff = compare_typed_answer(
                &case.expected,
                &case.typed,
                case.ignore_case,
                case.ignore_combining,
            );
            assert_eq!(diff, case.diff, "{}", case.name);
            let exact = case
                .diff
                .typed
                .iter()
                .chain(&case.diff.expected)
                .all(|s| s.kind == SegmentKind::Good);
            assert_eq!(diff.is_exact(), exact, "{}", case.name);
        }
    }

    #[test]
    fn test_compare_typed_answer_long() {
        let expected = "a".repeat(2000);
        let typed = format!("{}b", "a".repeat(1999));
        let diff = compare_typed_answer(&expected, &typed, false, false);
        assert_eq!(
            diff,
            TypedDiff {
                typed: vec![DiffSegment {
                    kind: SegmentKind::Bad,
                    text: typed.clone(),
                }],
                expected: vec![DiffSegment {
                    kind: SegmentKind::Missed,
                    text: expected.clone(),
                }],
            }
        );
        assert!(compare_typed_answer(&expected, &expected, false, false).is_exact());
    }
}
