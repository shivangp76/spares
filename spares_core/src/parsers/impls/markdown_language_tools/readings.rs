//! Readings: `漢字[かんじ]`, a reading written in brackets after its base text, as in Anki's
//! furigana fields. It works the same for any script: `中文[zhōngwén]` for pinyin.
//!
//! - **The base** is the run of letters and digits right before the `[` (and the combining marks
//!   on them): `食[た]べる` gives 食 the reading た. It ends at a space, punctuation or markup, so
//!   `私は 日本[にほん]` and `私は「日本[にほん]」` give 日本 the reading. As in Anki, a single
//!   space before the base only separates it from the text before, and is left out, but only after
//!   text that isn't ASCII or another reading, so `the 漢字[かんじ]` and a card's
//!   `[card-2 漢字[かんじ]]{.mark}` keep theirs.
//! - **Not readings**: brackets right after anything but a letter or digit (`[a link](…)`,
//!   `{{[o:1]…}}`, `- [ ]`), brackets followed by `(`, `{` or `[` (links, spans, reference
//!   links), `[^1]` footnotes and `[@key]` citations, empty brackets, brackets over a line break,
//!   and anything in code, math and comments ([`Parseable::unspoken_ranges`]).

use std::fmt::Write as _;
use std::ops::Range;

use unicode_normalization::char::is_combining_mark;

/// One reading in a text, as [`find_readings`] finds it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_field_names, reason = "a reading's reading")]
pub struct Reading {
    /// The whole reading, from the space before its base that is left out, if any, to its `]`.
    pub range: Range<usize>,
    /// The base text, `漢字` in `漢字[かんじ]`.
    pub base: Range<usize>,
    /// The reading, `かんじ`, without the brackets and the spaces inside them.
    pub reading: Range<usize>,
}

fn is_base_char(c: char) -> bool {
    c.is_alphanumeric() || is_combining_mark(c)
}

/// The readings in `text`, in order, outside the `excluded` ranges (code, math and comments).
pub fn find_readings(text: &str, excluded: &[Range<usize>]) -> Vec<Reading> {
    let mut readings = Vec::new();
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find('[') {
        let open = search_from + offset;
        search_from = open + 1;
        if excluded.iter().any(|range| range.contains(&open)) {
            continue;
        }
        // The base: letters and digits right before the `[`
        let base_start = text[..open]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_base_char(*c))
            .last()
            .map_or(open, |(i, _)| i);
        if base_start == open {
            continue;
        }
        // The reading: up to the next `]`, on the same line
        let Some(close_offset) = text[open + 1..].find([']', '[', '\n']) else {
            continue;
        };
        let close = open + 1 + close_offset;
        if bytes[close] != b']' {
            continue;
        }
        if matches!(bytes.get(close + 1), Some(b'(' | b'{' | b'[')) {
            continue;
        }
        let inner = &text[open + 1..close];
        let trimmed = inner.trim();
        if trimmed.is_empty() || trimmed.starts_with(['^', '@']) {
            continue;
        }
        let reading_start = open + 1 + (inner.len() - inner.trim_start().len());
        let reading = reading_start..reading_start + trimmed.len();
        // The space before the base is left out after text that isn't ASCII, or right after
        // another reading (`毎日[まいにち] 日本語[にほんご]`)
        let before = &text[..base_start];
        let after_reading = readings
            .last()
            .is_some_and(|last: &Reading| last.range.end + 1 == base_start);
        let start = match before.strip_suffix(' ') {
            Some(rest)
                if after_reading || rest.chars().next_back().is_some_and(|c| !c.is_ascii()) =>
            {
                base_start - 1
            }
            _ => base_start,
        };
        // A reading can't start inside the one before
        if readings
            .last()
            .is_some_and(|last: &Reading| start < last.range.end)
        {
            continue;
        }
        readings.push(Reading {
            range: start..close + 1,
            base: base_start..open,
            reading,
        });
        search_from = close + 1;
    }
    readings
}

/// `text` with each of its `readings` replaced by `replace(base, reading)`.
pub fn replace_readings(
    text: &str,
    readings: &[Reading],
    mut replace: impl FnMut(&str, &str) -> String,
) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at = 0;
    for reading in readings {
        result.push_str(&text[at..reading.range.start]);
        result.push_str(&replace(
            &text[reading.base.clone()],
            &text[reading.reading.clone()],
        ));
        at = reading.range.end;
    }
    result.push_str(&text[at..]);
    result
}

/// A reading as pandoc's raw HTML and raw LaTeX, each of which pandoc keeps only in its own
/// output: `<ruby>` in the HTML, luatexja-ruby's `\ruby` in the PDF.
pub fn ruby(base: &str, reading: &str) -> String {
    let mut ruby = String::new();
    let _ = write!(
        ruby,
        "`<ruby>{}<rt>{}</rt></ruby>`{{=html}}`\\ruby{{{}}}{{{}}}`{{=latex}}",
        escape_html(base),
        escape_html(reading),
        escape_latex(base),
        escape_latex(reading),
    );
    ruby
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('`', "&#96;")
}

fn escape_latex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => escaped.push_str(r"\textbackslash{}"),
            '~' => escaped.push_str(r"\textasciitilde{}"),
            '^' => escaped.push_str(r"\textasciicircum{}"),
            '`' => escaped.push_str(r"\textasciigrave{}"),
            '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                escaped.push('\\');
                escaped.push(c);
            }
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bases_and_readings(text: &str) -> Vec<(&str, &str)> {
        find_readings(text, &[])
            .into_iter()
            .map(|r| (&text[r.base], &text[r.reading]))
            .collect()
    }

    fn with_ruby(text: &str) -> String {
        replace_readings(text, &find_readings(text, &[]), |base, reading| {
            format!("<{base}|{reading}>")
        })
    }

    #[test]
    fn test_japanese_bases() {
        assert_eq!(bases_and_readings("漢字[かんじ]"), [("漢字", "かんじ")]);
        assert_eq!(bases_and_readings("食[た]べる"), [("食", "た")]);
        assert_eq!(
            bases_and_readings("私は 日本[にほん]に 住[す]んでいる"),
            [("日本", "にほん"), ("住", "す")]
        );
        // As in Anki, without a space the base is the whole run
        assert_eq!(
            bases_and_readings("私は日本[にほん]"),
            [("私は日本", "にほん")]
        );
        // Punctuation ends the base
        assert_eq!(
            bases_and_readings("「日本[にほん]」、東京[とうきょう]。"),
            [("日本", "にほん"), ("東京", "とうきょう")]
        );
        assert_eq!(bases_and_readings("時々[ときどき]"), [("時々", "ときどき")]);
    }

    #[test]
    fn test_chinese_bases() {
        assert_eq!(
            bases_and_readings("我学 中文[zhōngwén] 和 汉字[hànzì]"),
            [("中文", "zhōngwén"), ("汉字", "hànzì")]
        );
        assert_eq!(bases_and_readings("你好[nǐ hǎo]"), [("你好", "nǐ hǎo")]);
    }

    #[test]
    fn test_space_before_base() {
        assert_eq!(with_ruby("私は 日本[にほん]"), "私は<日本|にほん>");
        assert_eq!(with_ruby("the 漢字[かんじ]"), "the <漢字|かんじ>");
        assert_eq!(
            with_ruby("私は 毎日[まいにち] 日本語[にほんご]を"),
            "私は<毎日|まいにち><日本語|にほんご>を"
        );
        assert_eq!(
            with_ruby("中文[zhōngwén] 汉字[hànzì]"),
            "<中文|zhōngwén><汉字|hànzì>"
        );
        assert_eq!(
            with_ruby("[card-2 漢字[かんじ]]{.mark}"),
            "[card-2 <漢字|かんじ>]{.mark}"
        );
        // Only one space, and not a line break
        assert_eq!(with_ruby("私は\n日本[にほん]"), "私は\n<日本|にほん>");
    }

    #[test]
    fn test_not_readings() {
        for text in [
            "[a link](https://example.com)",
            "a [span]{.mark} and text[span]{.mark}",
            "see [keyword][li] and [word][li2]",
            "{{[o:1]Paris}} and {{[o:1;h:x]Paris}}",
            "- [ ] todo\n- [x] done",
            "a footnote[^1] and a citation[@doe]",
            "empty[] and [ ]",
            "over[a\nline]",
            r"\[escaped\]",
            "[li1]: /notes/a.md",
        ] {
            assert!(bases_and_readings(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn test_excluded_ranges() {
        let text = "`code[x]` 漢字[かんじ] $a[i]$";
        let readings = find_readings(text, &[0..9, text.len() - 6..text.len()]);
        assert_eq!(readings.len(), 1);
        assert_eq!(&text[readings[0].base.clone()], "漢字");
    }

    #[test]
    fn test_readings_in_clozes() {
        // A cloze holding a reading, and readings around a cloze
        assert_eq!(
            bases_and_readings("{{[o:1]漢字[かんじ]}} and {{ 東京[とうきょう]}}"),
            [("漢字", "かんじ"), ("東京", "とうきょう")]
        );
        // As the note's render shows clozes
        assert_eq!(
            with_ruby(r"\[漢字[かんじ]\] is read"),
            r"\[<漢字|かんじ>\] is read"
        );
    }

    #[test]
    fn test_ruby() {
        assert_eq!(
            ruby("漢字", "かんじ"),
            "`<ruby>漢字<rt>かんじ</rt></ruby>`{=html}`\\ruby{漢字}{かんじ}`{=latex}"
        );
        assert_eq!(
            ruby("a", "b&c_%"),
            "`<ruby>a<rt>b&amp;c_%</rt></ruby>`{=html}`\\ruby{a}{b\\&c\\_\\%}`{=latex}"
        );
    }
}
