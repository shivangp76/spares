use std::ops::Range;
use unscanny::Scanner;

const LINKED_NOTE_CMD: &str = "li";
const SETTINGS_CMD: &str = "se";
const KEYWORD_CMD: &str = "key";

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
