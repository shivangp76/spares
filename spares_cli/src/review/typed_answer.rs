//! Typed answers (the cloze key `t`): asked for on the card front, and compared with the expected
//! answers when the card is flipped.

use colored::ColoredString;
use colored::Colorize;
use inquire::Text;
use spares_core::parsers::TypeAnswer;
use spares_core::parsers::typed_answer::DiffSegment;
use spares_core::parsers::typed_answer::SegmentKind;
use spares_core::parsers::typed_answer::TypedAnswerInfo;
use spares_core::parsers::typed_answer::compare_typed_answer;

/// The prompt of blank `index`, e.g. `Blank 2 (city):`, or `Your answer:` if it is the only one.
fn blank_prompt(info: &TypedAnswerInfo, index: usize) -> String {
    let name = if info.blanks.len() > 1 {
        format!("Blank {}", index + 1)
    } else {
        "Your answer".to_string()
    };
    match &info.blanks[index].hint {
        Some(hint) => format!("{name} ({hint}):"),
        None => format!("{name}:"),
    }
}

/// Asks for the answer of each blank, starting from `previous`, e.g. after a flip was undone. Esc
/// skips the remaining blanks.
pub(super) fn ask_typed_answers(info: &TypedAnswerInfo, previous: &[String]) -> Vec<String> {
    let mut answers = Vec::with_capacity(info.blanks.len());
    for i in 0..info.blanks.len() {
        let prompt = blank_prompt(info, i);
        let previous = previous.get(i).map_or("", String::as_str);
        match Text::new(&prompt)
            .with_initial_value(previous)
            .with_help_message("Esc to skip")
            .prompt()
        {
            Ok(answer) => answers.push(answer),
            Err(_) => break,
        }
    }
    answers.resize(info.blanks.len(), String::new());
    answers
}

fn segments(segments: &[DiffSegment]) -> String {
    segments
        .iter()
        .map(|segment| -> ColoredString {
            match segment.kind {
                SegmentKind::Good => segment.text.green(),
                SegmentKind::Bad => segment.text.red().strikethrough(),
                SegmentKind::Missed => segment.text.dimmed().underline(),
            }
        })
        .map(|s| s.to_string())
        .collect()
}

/// Each typed answer, diffed against its expected answer, or as typed for a manual comparison.
pub(super) fn format_typed_answers(info: &TypedAnswerInfo, answers: &[String]) -> String {
    let mut lines = Vec::new();
    for (i, blank) in info.blanks.iter().enumerate() {
        if info.blanks.len() > 1 {
            lines.push(format!("Blank {}:", i + 1).bold().to_string());
        }
        let typed = answers.get(i).map_or("", |x| x.trim());
        match info.mode {
            TypeAnswer::Manual => {
                let typed = if typed.is_empty() {
                    "nothing".italic().to_string()
                } else {
                    typed.to_string()
                };
                lines.push(format!("You typed: {typed}"));
            }
            TypeAnswer::Diff {
                ignore_case,
                ignore_combining,
            } => {
                if typed.is_empty() {
                    lines.push(blank.expected.clone());
                    continue;
                }
                let diff =
                    compare_typed_answer(&blank.expected, typed, ignore_case, ignore_combining);
                lines.push(segments(&diff.typed));
                if !diff.is_exact() {
                    lines.push("↓".dimmed().to_string());
                    lines.push(segments(&diff.expected));
                }
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use spares_core::parsers::typed_answer::TypedBlank;

    use super::*;

    fn info(mode: TypeAnswer, expected: &[&str]) -> TypedAnswerInfo {
        TypedAnswerInfo {
            mode,
            blanks: expected
                .iter()
                .map(|expected| TypedBlank {
                    expected: (*expected).to_string(),
                    hint: None,
                })
                .collect(),
        }
    }

    const DIFF: TypeAnswer = TypeAnswer::Diff {
        ignore_case: false,
        ignore_combining: false,
    };

    #[test]
    fn test_format_typed_answers() {
        colored::control::set_override(false);
        let answers = |answers: &[&str]| answers.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            format_typed_answers(&info(DIFF, &["Paris"]), &answers(&["Paris"])),
            "Paris"
        );
        assert_eq!(
            format_typed_answers(&info(DIFF, &["Paris"]), &answers(&["paris"])),
            "paris\n↓\nParis"
        );
        assert_eq!(
            format_typed_answers(&info(DIFF, &["Paris"]), &answers(&[" "])),
            "Paris"
        );
        assert_eq!(
            format_typed_answers(&info(DIFF, &["red", "green"]), &answers(&["red", ""])),
            "Blank 1:\nred\nBlank 2:\ngreen"
        );
        assert_eq!(
            format_typed_answers(
                &info(TypeAnswer::Manual, &["Rayleigh scattering"]),
                &answers(&["blue light scatters more"])
            ),
            "You typed: blue light scatters more"
        );
    }
}
