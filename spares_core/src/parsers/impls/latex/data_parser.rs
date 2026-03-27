use std::ops::Range;
use unscanny::Scanner;

use crate::parsers::ClozeMatch;

const LINKED_NOTE_CMD: &str = "li";
const SETTINGS_CMD: &str = "se";
const KEYWORD_CMD: &str = "key";

enum ClMarker {
    Begin {
        match_range: Range<usize>,
        settings_match: Range<usize>,
    },
    End {
        match_range: Range<usize>,
    },
}

pub struct LatexDataParser<'de> {
    s: Scanner<'de>,
}

impl<'a> LatexDataParser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            s: Scanner::new(input),
        }
    }

    pub fn next_linked_note(&mut self) -> Option<Range<usize>> {
        self.find_command(LINKED_NOTE_CMD)
            .map(|(_, capture)| capture)
    }

    pub fn next_setting(&mut self) -> Option<Range<usize>> {
        self.find_command(SETTINGS_CMD).map(|(_, capture)| capture)
    }

    pub fn next_keyword(&mut self) -> Option<Range<usize>> {
        self.find_command(KEYWORD_CMD).map(|(_, capture)| capture)
    }

    /// Consume a `{...}` block (with nested brace tracking), starting after the opening `{` has
    /// already been eaten. Returns the byte range of the content inside the braces (excluding the
    /// braces themselves), or `None` if the braces are unmatched.
    fn eat_braced_content(&mut self) -> Option<Range<usize>> {
        let content_start = self.s.cursor();
        let mut depth = 1u32;
        loop {
            match self.s.eat() {
                Some('\\') => {
                    // Escape sequence: consume the next character unconditionally.
                    self.s.eat();
                }
                Some('%') => {
                    // LaTeX comment: skip until end of line.
                    self.s.eat_until('\n');
                }
                Some('{') => {
                    depth += 1;
                }
                Some('}') => {
                    depth -= 1;
                    if depth == 0 {
                        // cursor() now points past the closing `}`.
                        let content_end = self.s.cursor() - '}'.len_utf8();
                        return Some(content_start..content_end);
                    }
                }
                Some(_) => {}
                None => return None, // unmatched opening brace
            }
        }
    }

    /// Returns the next top-level `\begin{cl}…\end{cl}` group (with all nested clozes in the
    /// same `Vec`, sorted by `start_match.start`). Call repeatedly to iterate over all groups.
    pub fn next_cloze(&mut self) -> Option<Vec<ClozeMatch>> {
        // Find the first \begin{cl}, skipping any stray \end{cl}.
        let (first_start, first_settings) = loop {
            match self.find_next_cl_marker()? {
                ClMarker::Begin {
                    match_range,
                    settings_match,
                } => break (match_range, settings_match),
                ClMarker::End { .. } => {}
            }
        };

        let mut stack: Vec<(Range<usize>, Range<usize>)> = vec![(first_start, first_settings)];
        let mut results: Vec<ClozeMatch> = Vec::new();

        loop {
            match self.find_next_cl_marker() {
                Some(ClMarker::Begin {
                    match_range,
                    settings_match,
                }) => {
                    stack.push((match_range, settings_match));
                }
                Some(ClMarker::End {
                    match_range: end_match,
                }) => {
                    if let Some((start_match, settings_match)) = stack.pop() {
                        results.push(ClozeMatch {
                            start_match,
                            end_match,
                            settings_match,
                        });
                    }
                    if stack.is_empty() {
                        break;
                    }
                }
                None => break,
            }
        }

        if results.is_empty() {
            None
        } else {
            results.sort_by_key(|c| c.start_match.start);
            Some(results)
        }
    }

    /// Scan forward for the next `\begin{cl}` or `\end{cl}` marker, skipping comments and
    /// escape sequences.
    fn find_next_cl_marker(&mut self) -> Option<ClMarker> {
        loop {
            let cursor = self.s.cursor();
            match self.s.eat() {
                Some('%') => {
                    self.s.eat_until('\n');
                }
                Some('\\') => {
                    if self.s.peek().is_some_and(|c| c.is_alphabetic()) {
                        let cmd = self.s.eat_while(char::is_alphabetic);
                        match cmd {
                            "begin" => {
                                if self.s.eat_if("{cl}") {
                                    let (settings_match, match_end) =
                                        self.eat_optional_bracketed_settings();
                                    return Some(ClMarker::Begin {
                                        match_range: cursor..match_end,
                                        settings_match,
                                    });
                                }
                            }
                            "end" => {
                                if self.s.eat_if("{cl}") {
                                    return Some(ClMarker::End {
                                        match_range: cursor..self.s.cursor(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    } else {
                        self.s.eat();
                    }
                }
                Some(_) => {}
                None => return None,
            }
        }
    }

    /// Optionally consume a `[…]` block with nested-bracket tracking. Must be called when the
    /// cursor is positioned at the potential `[`.
    ///
    /// Returns `(settings_match, match_end)`:
    /// - `settings_match` is the range of content inside the outer `[…]` (or `Range::default()`
    ///   when no `[` is present).
    /// - `match_end` is the cursor position after the closing `]` (or the current position when
    ///   no `[` was found).
    fn eat_optional_bracketed_settings(&mut self) -> (Range<usize>, usize) {
        if !self.s.eat_if('[') {
            return (Range::default(), self.s.cursor());
        }
        let settings_start = self.s.cursor();
        let mut depth = 0u32;
        loop {
            match self.s.eat() {
                Some('[') => depth += 1,
                Some(']') => {
                    if depth == 0 {
                        let settings_end = self.s.cursor() - ']'.len_utf8();
                        return (settings_start..settings_end, self.s.cursor());
                    }
                    depth -= 1;
                }
                Some(_) => {}
                None => return (settings_start..self.s.cursor(), self.s.cursor()),
            }
        }
    }

    /// Scan forward, skipping comments and escaped characters, until the next
    /// `\{command_name}{...}` is found.
    ///
    /// Returns `(match_range, capture_range)` where `match_range` covers the full token from `\`
    /// to the closing `}`, and `capture_range` covers only the content inside `{...}`.
    fn find_command(&mut self, command_name: &str) -> Option<(Range<usize>, Range<usize>)> {
        loop {
            let cursor = self.s.cursor();
            match self.s.eat() {
                Some('%') => {
                    // LaTeX comment: skip until end of line.
                    self.s.eat_until('\n');
                }
                Some('\\') => {
                    if self.s.peek().is_some_and(|c| c.is_alphabetic()) {
                        // Read the command name (letters only).
                        let cmd = self.s.eat_while(char::is_alphabetic);
                        if cmd == command_name && self.s.eat_if('{') {
                            if let Some(capture) = self.eat_braced_content() {
                                let match_end = self.s.cursor();
                                return Some((cursor..match_end, capture));
                            }
                            // Unmatched braces — stop searching.
                            return None;
                        }
                        // Some other command; its name was already consumed; continue.
                    } else {
                        // Escaped non-letter (e.g. `\%`, `\\`, `\{`): consume one more char.
                        self.s.eat();
                    }
                }
                Some(_) => {}
                None => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── settings ──────────────────────────────────────────────────────────────

    #[test]
    fn test_basic_setting() {
        let input = r"text \se{key: value} more";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "key: value");
    }

    #[test]
    fn test_setting_not_in_comment() {
        // `%` starts a LaTeX comment; \se inside must be ignored.
        let input = "% \\se{ignored}\n\\se{real}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "real");
    }

    #[test]
    fn test_setting_after_escaped_percent() {
        // `\%` is a literal percent sign, NOT a comment; \se after it must be found.
        let input = r"\% \se{found}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "found");
    }

    #[test]
    fn test_setting_after_escaped_backslash() {
        // `\\` is an escaped backslash, the `\se` after it is a real command.
        let input = r"\\ \se{found}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "found");
    }

    #[test]
    fn test_setting_nested_braces() {
        let input = r"\se{custom-data: {nested}}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "custom-data: {nested}");
    }

    #[test]
    fn test_multiple_settings() {
        let input = r"\se{a} text \se{b}";
        let mut parser = LatexDataParser::new(input);
        let s1 = parser.next_setting().unwrap();
        let s2 = parser.next_setting().unwrap();
        assert_eq!(&input[s1], "a");
        assert_eq!(&input[s2], "b");
        assert!(parser.next_setting().is_none());
    }

    #[test]
    fn test_no_setting() {
        let input = r"just text, no commands";
        let mut parser = LatexDataParser::new(input);
        assert!(parser.next_setting().is_none());
    }

    #[test]
    fn test_setting_unmatched_braces_returns_none() {
        let input = r"\se{unclosed";
        let mut parser = LatexDataParser::new(input);
        assert!(parser.next_setting().is_none());
    }

    #[test]
    fn test_setting_with_comment_inside_braces() {
        // A `%` inside `{...}` should be treated as a comment (skip to end of line).
        let input = "\\se{key % comment\n: value}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "key % comment\n: value");
    }

    #[test]
    fn test_setting_other_command_before() {
        let input = r"\begin{note} \se{id: 1} \end{note}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "id: 1");
    }

    // ── linked notes ──────────────────────────────────────────────────────────

    #[test]
    fn test_basic_linked_note() {
        let input = r"text \li{note-id} more";
        let mut parser = LatexDataParser::new(input);
        let range = parser.next_linked_note().unwrap();
        assert_eq!(&input[range], "note-id");
    }

    #[test]
    fn test_linked_note_not_in_comment() {
        let input = "% \\li{ignored}\n\\li{real}";
        let mut parser = LatexDataParser::new(input);
        let range = parser.next_linked_note().unwrap();
        assert_eq!(&input[range], "real");
    }

    #[test]
    fn test_linked_note_after_escaped_percent() {
        let input = r"\% \li{found}";
        let mut parser = LatexDataParser::new(input);
        let range = parser.next_linked_note().unwrap();
        assert_eq!(&input[range], "found");
    }

    #[test]
    fn test_multiple_linked_notes() {
        let input = r"\li{a} \li{b}";
        let mut parser = LatexDataParser::new(input);
        assert_eq!(&input[parser.next_linked_note().unwrap()], "a");
        assert_eq!(&input[parser.next_linked_note().unwrap()], "b");
        assert!(parser.next_linked_note().is_none());
    }

    // ── keywords ──────────────────────────────────────────────────────────────

    #[test]
    fn test_basic_keyword() {
        let input = r"text \key{foo} more";
        let mut parser = LatexDataParser::new(input);
        let range = parser.next_keyword().unwrap();
        assert_eq!(&input[range], "foo");
    }

    #[test]
    fn test_keyword_not_in_comment() {
        let input = "% \\key{ignored}\n\\key{real}";
        let mut parser = LatexDataParser::new(input);
        let range = parser.next_keyword().unwrap();
        assert_eq!(&input[range], "real");
    }

    #[test]
    fn test_keyword_after_escaped_percent() {
        let input = r"\% \key{found}";
        let mut parser = LatexDataParser::new(input);
        let range = parser.next_keyword().unwrap();
        assert_eq!(&input[range], "found");
    }

    #[test]
    fn test_multiple_keywords() {
        let input = r"\key{foo} \key{bar}";
        let mut parser = LatexDataParser::new(input);
        assert_eq!(&input[parser.next_keyword().unwrap()], "foo");
        assert_eq!(&input[parser.next_keyword().unwrap()], "bar");
        assert!(parser.next_keyword().is_none());
    }

    // ── clozes ────────────────────────────────────────────────────────────────

    #[test]
    fn test_basic_cloze() {
        let input = r"\begin{cl}content\end{cl}";
        let mut parser = LatexDataParser::new(input);
        let group = parser.next_cloze().unwrap();
        assert_eq!(group.len(), 1);
        assert_eq!(&input[group[0].start_match.clone()], r"\begin{cl}");
        assert_eq!(&input[group[0].end_match.clone()], r"\end{cl}");
        assert!(group[0].settings_match.is_empty());
        assert!(parser.next_cloze().is_none());
    }

    #[test]
    fn test_cloze_with_settings() {
        // \begin{cl}[o:1]content\end{cl}
        // start_match covers \begin{cl}[o:1], settings_match covers o:1
        let input = r"\begin{cl}[o:1]content\end{cl}";
        let mut parser = LatexDataParser::new(input);
        let group = parser.next_cloze().unwrap();
        assert_eq!(group.len(), 1);
        assert_eq!(&input[group[0].start_match.clone()], r"\begin{cl}[o:1]");
        assert_eq!(&input[group[0].settings_match.clone()], "o:1");
        assert_eq!(&input[group[0].end_match.clone()], r"\end{cl}");
    }

    #[test]
    fn test_cloze_with_nested_bracket_settings() {
        let input = r"\begin{cl}[g:[1,2]]content\end{cl}";
        let mut parser = LatexDataParser::new(input);
        let group = parser.next_cloze().unwrap();
        assert_eq!(group.len(), 1);
        assert_eq!(&input[group[0].settings_match.clone()], "g:[1,2]");
        assert_eq!(&input[group[0].start_match.clone()], r"\begin{cl}[g:[1,2]]");
        assert_eq!(&input[group[0].end_match.clone()], r"\end{cl}");
    }

    #[test]
    fn test_cloze_not_in_comment() {
        let input = "% \\begin{cl}ignored\\end{cl}\n\\begin{cl}real\\end{cl}";
        let mut parser = LatexDataParser::new(input);
        let group = parser.next_cloze().unwrap();
        assert_eq!(group.len(), 1);
        // The matched start should be the non-commented \begin{cl}
        assert_eq!(&input[group[0].start_match.clone()], r"\begin{cl}");
        assert_eq!(&input[group[0].end_match.clone()], r"\end{cl}");
        assert!(parser.next_cloze().is_none());
    }

    #[test]
    fn test_nested_cloze() {
        let input = r"\begin{cl}[o:1]outer \begin{cl}inner\end{cl}\end{cl}";
        let mut parser = LatexDataParser::new(input);
        let group = parser.next_cloze().unwrap();
        assert_eq!(group.len(), 2);
        // Sorted by start: outer first, then inner.
        assert_eq!(&input[group[0].settings_match.clone()], "o:1");
        assert_eq!(&input[group[0].end_match.clone()], r"\end{cl}");
        assert_eq!(&input[group[1].start_match.clone()], r"\begin{cl}");
        assert_eq!(&input[group[1].end_match.clone()], r"\end{cl}");
        assert!(parser.next_cloze().is_none());
    }

    #[test]
    fn test_multiple_sequential_clozes() {
        let input = r"\begin{cl}first\end{cl} \begin{cl}second\end{cl}";
        let mut parser = LatexDataParser::new(input);
        let g1 = parser.next_cloze().unwrap();
        assert_eq!(g1.len(), 1);
        assert_eq!(&input[g1[0].start_match.clone()], r"\begin{cl}");
        let g2 = parser.next_cloze().unwrap();
        assert_eq!(g2.len(), 1);
        assert_eq!(&input[g2[0].start_match.clone()], r"\begin{cl}");
        assert!(parser.next_cloze().is_none());
    }

    #[test]
    fn test_unmatched_begin_returns_none() {
        let input = r"\begin{cl}unclosed";
        let mut parser = LatexDataParser::new(input);
        assert!(parser.next_cloze().is_none());
    }

    #[test]
    fn test_other_environment_does_not_interfere() {
        let input = r"\begin{equation}E=mc^2\end{equation}\begin{cl}content\end{cl}";
        let mut parser = LatexDataParser::new(input);
        let group = parser.next_cloze().unwrap();
        assert_eq!(group.len(), 1);
        assert_eq!(&input[group[0].start_match.clone()], r"\begin{cl}");
        assert_eq!(&input[group[0].end_match.clone()], r"\end{cl}");
    }

    // ── mixed / edge cases ────────────────────────────────────────────────────

    #[test]
    fn test_inline_percent_is_comment() {
        // A `%` not preceded by `\` in running text starts a comment.
        let input = "before % \\se{ignored}\nafter \\se{found}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], "found");
        assert!(parser.next_setting().is_none());
    }

    #[test]
    fn test_escaped_brace_does_not_confuse_depth() {
        // `\{` and `\}` are literal braces, not depth-changers.
        let input = r"\se{a \{ b \} c}";
        let mut parser = LatexDataParser::new(input);
        let setting = parser.next_setting().unwrap();
        assert_eq!(&input[setting], r"a \{ b \} c");
    }
}
