//! Hints that hold the settings delimiter, `]` or quotes, written quoted or with `\` escapes (see
//! `SettingsSyntax::Cloze`).

use pretty_assertions::assert_eq;

use crate::CardErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::NotePart;
use crate::parsers::Parseable;
use crate::parsers::TypeAnswer;
use crate::parsers::add_order_to_note_data;
use crate::parsers::impls::latex::LatexParserNote;
use crate::parsers::impls::markdown::MarkdownParser;
use crate::parsers::impls::typst::TypstParser;
use crate::parsers::quote_setting_value;

/// The first card's hint, its clozes' text, and its typed answer
type Parsed = (Option<String>, Vec<String>, Option<TypeAnswer>);

fn parse(parser: &dyn Parseable, data: &str) -> Result<Parsed, Error> {
    let (_, cards) = add_order_to_note_data(parser, data, None)?;
    let card = cards.first().unwrap();
    let mut hint = None;
    let mut texts = Vec::new();
    for part in &card.data {
        if let NotePart::ClozeData(text, replacement) = part {
            texts.push(text.clone());
            if let ClozeHiddenReplacement::ToAnswer { hint: Some(h) } = replacement {
                hint = Some(h.clone());
            }
        }
    }
    Ok((hint, texts, card.type_answer))
}

fn hint(data: &str) -> String {
    parse(&MarkdownParser::new(), data).unwrap().0.unwrap()
}

fn error(data: &str) -> String {
    match parse(&MarkdownParser::new(), data) {
        Err(Error::Library(LibraryError::Card(CardErrorKind::InvalidSettings {
            description,
            ..
        }))) => description,
        other => panic!("expected invalid settings for {data}, got {other:?}"),
    }
}

#[test]
fn test_hint_quoted_or_escaped() {
    assert_eq!(hint(r#"{{[h:"it is;b"] a }}"#), "it is;b");
    assert_eq!(hint(r"{{[h:it is\;b] a }}"), "it is;b");
    assert_eq!(hint(r#"{{[h:" padded "] a }}"#), " padded ");
    assert_eq!(hint(r#"{{[h:"say \"hi\""] a }}"#), r#"say "hi""#);
    assert_eq!(hint(r#"{{[h:"a\\"] a }}"#), r"a\");
    // A quote that does not start the value, or a value not quoted to its end, is text
    assert_eq!(hint(r#"{{[h:5" screw] a }}"#), r#"5" screw"#);
    assert_eq!(hint(r#"{{[h:"Ode" by Keats] a }}"#), r#""Ode" by Keats"#);
    // A `\` before anything else belongs to the markup
    assert_eq!(hint(r"{{[h:\textbf{x}] a }}"), r"\textbf{x}");
}

#[test]
fn test_hint_settings_after_it() {
    let (hint, _, type_answer) = parse(&MarkdownParser::new(), r#"{{[h:"a;b";t] x }}"#).unwrap();
    assert_eq!(hint.as_deref(), Some("a;b"));
    assert!(type_answer.is_some());
    // An escaped `\` does not escape the delimiter after it
    let (hint, _, type_answer) = parse(&MarkdownParser::new(), r"{{[h:a\\;t] x }}").unwrap();
    assert_eq!(hint.as_deref(), Some(r"a\"));
    assert!(type_answer.is_some());
}

/// A `]` escaped in a hint does not end the cloze's settings, in each syntax
#[test]
fn test_hint_with_bracket() {
    for (parser, data) in [
        (
            Box::new(MarkdownParser::new()) as Box<dyn Parseable>,
            r"{{[h:a\]b] x }}",
        ),
        (Box::new(TypstParser::new()), r"#cl[x][h:a\]b]"),
        (
            Box::new(LatexParserNote::new()),
            r"\begin{cl}[h:a\]b]x\end{cl}",
        ),
    ] {
        let (hint, texts, _) = parse(parser.as_ref(), data).unwrap();
        assert_eq!(hint.as_deref(), Some("a]b"), "{data}");
        assert_eq!(texts.join("").trim(), "x", "{data}");
    }
}

#[test]
fn test_hint_unclosed_quote() {
    let description = error(r#"{{[h:"a;t] x }}"#);
    assert!(description.contains("no closing"), "{description}");
}

/// A setting after a hint that fails names the hint, as it may have been meant as part of it
#[test]
fn test_hint_note_on_error() {
    let description = error("{{[h:it is;b] x }}");
    assert!(description.contains("The card back"), "{description}");
    assert!(description.contains(r#"`h:"it is;b"`"#), "{description}");
    let description = error("{{[h:a;zz:1] x }}");
    assert!(description.contains(r#"`h:"a;zz:1"`"#), "{description}");
    // A quoted hint did not end at the delimiter, nor does a setting not right after a hint
    let description = error(r#"{{[h:"a";b] x }}"#);
    assert!(!description.contains("A hint ends"), "{description}");
    let description = error("{{[h:a;t;b] x }}");
    assert!(!description.contains("A hint ends"), "{description}");
}

/// A hint written by `quote_setting_value` reads back as itself, and the note's rewrite keeps it
#[test]
fn test_hint_round_trip() {
    let markdown: Box<dyn Parseable> = Box::new(MarkdownParser::new());
    let typst: Box<dyn Parseable> = Box::new(TypstParser::new());
    let latex: Box<dyn Parseable> = Box::new(LatexParserNote::new());
    let all = [&markdown, &typst, &latex];
    for (hint_text, parsers) in [
        ("city", &all[..]),
        ("a;b", &all),
        ("a]b", &all),
        ("a[b", &all),
        ("[a]", &all),
        (" padded ", &all),
        (r#""quoted""#, &all),
        (r#"x"y"#, &all),
        (r"a\", &all),
        (r"a\;", &all),
        (r"a\]", &all),
        (r"\\", &all),
        ("a;b]c\"d", &all),
        (r"\textbf{x}", &[&markdown, &latex][..]),
        ("$[0,1)$", &[&markdown, &latex][..]),
    ] {
        for parser in parsers {
            let settings = format!("h:{}", quote_setting_value(hint_text, ";"));
            let (start, end) = parser.construct_cloze(&settings);
            let data = format!("{start}x{end}");
            let (rewritten, _) = add_order_to_note_data(parser.as_ref(), &data, None)
                .unwrap_or_else(|e| panic!("{data}: {e:?}"));
            let (again, _) = add_order_to_note_data(parser.as_ref(), &rewritten, None).unwrap();
            assert_eq!(again, rewritten, "rewrite of {data}");
            let (hint, texts, _) = parse(parser.as_ref(), &rewritten).unwrap();
            assert_eq!(
                hint.as_deref(),
                Some(hint_text),
                "{data} rewritten as {rewritten}"
            );
            assert_eq!(texts.join(""), "x", "{data}");
        }
    }
}
