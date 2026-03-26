use crate::parsers::ClozeMatch;
use unscanny::Scanner;

pub struct ClozeParser<'de> {
    s: Scanner<'de>,
    math_mode: bool,
}

impl<'a> ClozeParser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            s: Scanner::new(input),
            math_mode: false,
        }
    }

    /// Returns clozes, ordered by their starting delim position. This means for nested clozes, the outer cloze will be returned first, then the inner cloze.
    pub fn next_cloze(&mut self) -> Option<Vec<ClozeMatch>> {
        let mut nesting_level = 0;
        let mut all_clozes = Vec::new();
        let mut current_clozes = Vec::new();

        loop {
            let cursor_start = self.s.cursor();
            match self.s.eat() {
                // Comment
                Some('<') if self.s.eat_if("!---") => {
                    self.s.eat_until("--->");
                }
                // Escaped character
                Some('\\') => {
                    self.s.eat(); // Consume backslash
                }
                // Handle math mode transitions
                Some('$') if !self.math_mode => {
                    self.s.eat_if('$');
                    self.math_mode = true;
                }
                Some('$') if self.math_mode => {
                    self.s.eat_if('$');
                    self.math_mode = false;
                }
                // Handle math blocks
                Some('`') if !self.math_mode && self.s.eat_if("``math") => {
                    self.math_mode = true;
                }
                Some('`') if self.math_mode && self.s.eat_if("``") => {
                    self.math_mode = false;
                }
                // Handle cloze opening
                Some('{') if self.s.eat_if('{') && !self.math_mode => {
                    let mut settings = None;
                    if self.s.eat_if('[') {
                        let settings_start_idx = self.s.cursor();
                        self.s.eat_until(']');
                        settings = Some(settings_start_idx..self.s.cursor());
                    }
                    let cursor_end = self.s.cursor();
                    current_clozes.push((
                        cursor_start..settings.clone().map_or(cursor_end, |x| x.end + 1),
                        settings.filter(|x| !x.is_empty()).unwrap_or_default(),
                    ));
                }
                // Handle cloze closing
                Some('}') if self.s.eat_if('}') && !self.math_mode => {
                    nesting_level -= 1;
                    if let Some((opener_range, settings)) = current_clozes.pop() {
                        all_clozes.push(ClozeMatch {
                            start_match: opener_range,
                            end_match: cursor_start..self.s.cursor(),
                            settings_match: settings.clone(),
                        });
                    }
                    if nesting_level == 0 {
                        break;
                    }
                }
                Some(_) => {}
                None => {
                    break;
                }
            }
        }

        if all_clozes.is_empty() {
            None
        } else {
            assert!(current_clozes.is_empty());
            all_clozes.sort_by_key(|x| x.start_match.start);
            Some(all_clozes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    #[test]
    fn test_basic_cloze() {
        let input = "Test {{basic}} cloze";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..7,
                end_match: 12..14,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_empty_settings() {
        let input = "Test {{[] basic}} cloze";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 15..17,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_comment() {
        let input = "Test <!--- {{[] basic}} cloze ---> word";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        let expected: Vec<Vec<ClozeMatch>> = vec![];
        assert_eq!(all_clozes, expected);
    }

    #[test]
    fn test_escaped_character() {
        let input = "Test \\{{[] basic}} cloze";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        let expected: Vec<Vec<ClozeMatch>> = vec![];
        assert_eq!(all_clozes, expected);
    }

    #[test]
    fn test_unmatched_open() {
        let input = "Test \\{{ test";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        let expected: Vec<Vec<ClozeMatch>> = vec![];
        assert_eq!(all_clozes, expected);
    }

    #[test]
    fn test_math_mode() {
        let input = "Test {{[o:1] $ \\$ 3^{2^{2}}$}} complex";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..12,
                end_match: 28..30,
                settings_match: 8..11,
            }]]
        );
    }

    #[test]
    fn test_nested_cloze_1() {
        let input = "Test {{[o:1] outer {{inner}}}} complex";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        // for clozes_group in &all_clozes {
        //     for cloze in clozes_group {
        //         dbg!(&cloze);
        //         dbg!(&input[cloze.start_match_range.clone()]);
        //         dbg!(&input[cloze.end_match_range.clone()]);
        //         dbg!(&input[cloze.settings_match_range.clone()]);
        //         println!("---");
        //     }
        // }
        assert_eq!(
            all_clozes,
            vec![vec![
                ClozeMatch {
                    start_match: 5..12,
                    end_match: 28..30,
                    settings_match: 8..11,
                },
                ClozeMatch {
                    start_match: 19..21,
                    end_match: 26..28,
                    settings_match: Range::default(),
                }
            ]]
        );
    }

    #[test]
    fn test_nested_cloze_2() {
        let input = "Test {{[o:1] outer {{inner}} {{sibling}}}} complex";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![
                ClozeMatch {
                    start_match: 5..12,
                    end_match: 40..42,
                    settings_match: 8..11,
                },
                ClozeMatch {
                    start_match: 19..21,
                    end_match: 26..28,
                    settings_match: Range::default(),
                },
                ClozeMatch {
                    start_match: 29..31,
                    end_match: 38..40,
                    settings_match: Range::default(),
                }
            ]]
        );
    }

    #[test]
    fn test_display_math() {
        let input = "Test $$\n  x = {{2}}\n$$ {{content}} end";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 23..25,
                end_match: 32..34,
                settings_match: Range::default(),
            },]]
        );
    }

    #[test]
    fn test_math_block() {
        let input = "Test ```math\nx = {{2}}\n``` {{content}} end";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 27..29,
                end_match: 36..38,
                settings_match: Range::default(),
            },]]
        );
    }

    #[test]
    fn test_empty_cloze() {
        let input = "Test {{}} end";
        let mut parser = ClozeParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..7,
                end_match: 7..9,
                settings_match: Range::default(),
            },]]
        );
    }
}
