use crate::parsers::ClozeMatch;
use std::ops::Range;
use unscanny::Scanner;

const CLOZE_FUNC_NAME: &str = "cl";
const LINKED_NOTE_FUNC_NAME: &str = "lin";
const SETTINGS_FUNC_NAME: &str = "se";

pub struct TypstDataParser<'de> {
    s: Scanner<'de>,
    math_mode: bool,
    open_square_bracket_count: u32,
}

#[derive(Debug)]
enum OutputType {
    Cloze,
    LinkedNote,
    Settings,
}

#[derive(Debug)]
enum Output {
    Cloze(Vec<ClozeMatch>),
    LinkedNote(Range<usize>),
    Settings(Range<usize>),
}

#[derive(Debug)]
struct Cloze {
    // `#cl`
    start: Range<usize>,
    // Eventually, `]` or `][`
    first_arg_end: Option<Range<usize>>,
    // `]` or `None`
    // second_arg_end: Option<Range<usize>>,
}

impl<'a> TypstDataParser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            s: Scanner::new(input),
            math_mode: false,
            open_square_bracket_count: 0,
        }
    }

    /// Returns clozes, ordered by their starting delim position. This means for nested clozes, the outer cloze will be returned first, then the inner cloze.
    pub fn next_cloze(&mut self) -> Option<Vec<ClozeMatch>> {
        self.next_data(&OutputType::Cloze).map(|x| match x {
            Output::Cloze(vec) => vec,
            _ => unreachable!(),
        })
    }

    pub fn next_linked_note(&mut self) -> Option<Range<usize>> {
        self.next_data(&OutputType::LinkedNote).map(|x| match x {
            Output::LinkedNote(res) => res,
            _ => unreachable!(),
        })
    }

    pub fn next_setting(&mut self) -> Option<Range<usize>> {
        self.next_data(&OutputType::Settings).map(|x| match x {
            Output::Settings(res) => res,
            _ => unreachable!(),
        })
    }

    #[allow(clippy::too_many_lines, reason = "off by a few")]
    fn next_data(&mut self, output_type: &OutputType) -> Option<Output> {
        let mut nesting_level = 0;
        let mut all_clozes = Vec::new();
        let mut current_clozes = Vec::new();

        let mut current_linked_notes = Vec::new();
        let mut current_settings = Vec::new();

        loop {
            let cursor_start = self.s.cursor();
            // dbg!(&self.s.peek());
            // dbg!(&self.s.string()[cursor_start..]);
            match self.s.eat() {
                // Comments
                Some('/') if self.s.eat_if('/') => {
                    self.s.eat_until("\n");
                }
                // Escaped character
                Some('\\') => {
                    self.s.eat(); // Consume backslash
                }
                // Handle math mode transitions
                Some('$') => {
                    self.math_mode = !self.math_mode;
                }
                // Handle function argument opening
                Some('#') if !self.math_mode => {
                    let func_name = self
                        .s
                        .eat_while(|c| char::is_alphanumeric(c) || c == '_' || c == '-');
                    //.eat_until('[');
                    self.s.eat();
                    // dbg!(&func_name);
                    // dbg!(&self.s.string()[self.s.cursor()..]);
                    if func_name == CLOZE_FUNC_NAME && matches!(output_type, OutputType::Cloze) {
                        nesting_level += 1;
                        current_clozes.push(Cloze {
                            start: cursor_start..self.s.cursor(),
                            first_arg_end: None,
                            // second_arg_end: None,
                        });
                    } else if func_name == LINKED_NOTE_FUNC_NAME
                        && matches!(output_type, OutputType::LinkedNote)
                    {
                        current_linked_notes.push(cursor_start..self.s.cursor());
                    } else if func_name == SETTINGS_FUNC_NAME
                        && matches!(output_type, OutputType::Settings)
                    {
                        current_settings.push(cursor_start..self.s.cursor());
                    } else {
                        self.s.uneat();
                    }
                }
                Some('[') if !self.math_mode && !current_clozes.is_empty() => {
                    self.open_square_bracket_count += 1;
                }
                // Handle function argument closing
                Some(']') if !self.math_mode && self.open_square_bracket_count == 0 => {
                    if self.s.eat_if('[') {
                        // Second argument
                        let first_arg_end = self.s.cursor();
                        if let Some(last) = current_clozes.last_mut() {
                            last.first_arg_end = Some(cursor_start..first_arg_end);
                        }
                    } else {
                        nesting_level -= 1;

                        // End of all arguments (could be 1 or 2 args)
                        if let Some(mut cloze) = current_clozes.pop() {
                            let second_arg_end = cloze
                                .first_arg_end
                                .as_ref()
                                .map(|_| cursor_start..self.s.cursor());
                            if cloze.first_arg_end.is_none() {
                                cloze.first_arg_end = Some(cursor_start..self.s.cursor());
                            }
                            let first_arg_end = cloze.first_arg_end.unwrap();
                            all_clozes.push(ClozeMatch {
                                start_match: cloze.start,
                                end_match: first_arg_end.start
                                    ..second_arg_end.as_ref().map_or(first_arg_end.end, |x| x.end),
                                settings_match: second_arg_end
                                    .map(|x| first_arg_end.start + 2..x.start)
                                    .filter(|x| !x.is_empty())
                                    .unwrap_or_default(),
                            });
                        }

                        if matches!(output_type, OutputType::LinkedNote) {
                            if let Some(linked_note_start) = current_linked_notes.pop() {
                                return Some(Output::LinkedNote(
                                    linked_note_start.end..cursor_start,
                                ));
                            }
                        }

                        if matches!(output_type, OutputType::Settings) {
                            if let Some(setting_start) = current_settings.pop() {
                                return Some(Output::Settings(setting_start.end..cursor_start));
                            }
                        }

                        if nesting_level == 0 && matches!(output_type, OutputType::Cloze) {
                            break;
                        }
                    }
                }
                Some(']') if !self.math_mode => {
                    self.open_square_bracket_count -= 1;
                }
                Some(_) => {}
                None => {
                    break;
                }
            }
        }

        match output_type {
            OutputType::Cloze => {
                if all_clozes.is_empty() {
                    None
                } else {
                    assert!(current_clozes.is_empty());
                    all_clozes.sort_by_key(|x| x.start_match.start);
                    Some(Output::Cloze(all_clozes))
                }
            }
            OutputType::LinkedNote | OutputType::Settings => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::ops::Range;

    #[test]
    fn test_basic_setting() {
        let input = "Test #se[basic] asd";
        let mut parser = TypstDataParser::new(input);
        let mut all_settings = Vec::new();
        while let Some(setting) = parser.next_setting() {
            all_settings.push(setting);
        }
        assert_eq!(all_settings, vec![9..14],);
    }

    #[test]
    fn test_basic_linked_note() {
        let input = "Test #lin[basic] asd";
        let mut parser = TypstDataParser::new(input);
        let mut all_linked_notes = Vec::new();
        while let Some(linked_note) = parser.next_linked_note() {
            all_linked_notes.push(linked_note);
        }
        assert_eq!(all_linked_notes, vec![10..15],);
    }

    #[test]
    fn test_setting_with_cloze() {
        let input = "Test #se[basic] asd #cl[test] #se[key]";
        let mut parser = TypstDataParser::new(input);
        let mut all_settings = Vec::new();
        while let Some(setting) = parser.next_setting() {
            all_settings.push(setting);
        }
        assert_eq!(all_settings, vec![9..14, 34..37],);
    }

    #[test]
    fn test_basic_cloze() {
        let input = "Test #cl[basic] asd";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 14..15,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_empty_settings() {
        let input = "Test #cl[basic][] cloze";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 14..17,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_escaped_character() {
        let input = "Test \\#cl[basic] cloze";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        let expected: Vec<Vec<ClozeMatch>> = vec![];
        assert_eq!(all_clozes, expected);
    }

    #[test]
    fn test_commented() {
        let input = "// Test #cl[basic] cloze\n #cl[b]";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 26..30,
                end_match: 31..32,
                settings_match: Range::default(),
            }]]
        );
    }

    #[test]
    fn test_unmatched_open() {
        let input = "Test #cl[ test";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        let expected: Vec<Vec<ClozeMatch>> = vec![];
        assert_eq!(all_clozes, expected);
    }

    #[test]
    fn test_math_mode() {
        let input = "test #cl[$\na b)\n\n$][g:1] test";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 18..24,
                settings_match: 20..23,
            }]]
        );
    }

    #[test]
    fn test_code_mode() {
        let input = "test #cl[#(let a = 2)][g:1] test";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 21..27,
                settings_match: 23..26,
            }]]
        );
    }

    #[test]
    fn test_nested_cloze() {
        let input = "test #cl[#cl[b][g:1]#cl[$a s (d)$]][g:1] test";
        let mut parser = TypstDataParser::new(input);
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
                    start_match: 5..9,
                    end_match: 34..40,
                    settings_match: 36..39,
                },
                ClozeMatch {
                    start_match: 9..13,
                    end_match: 14..20,
                    settings_match: 16..19,
                },
                ClozeMatch {
                    start_match: 20..24,
                    end_match: 33..34,
                    settings_match: Range::default(),
                }
            ]]
        );
    }

    #[test]
    fn test_empty_cloze() {
        let input = "Test #cl[] end";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 5..9,
                end_match: 9..10,
                settings_match: Range::default(),
            },]]
        );
    }

    #[test]
    fn test_other_func_in_cloze() {
        let input = indoc! { r#"#cl[ - mnemonic: #strong[c]url = #strong[c]ross product ]"# };
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 56..57,
                settings_match: Range::default(),
            },]]
        );
    }

    #[test]
    fn test_other_func_in_cloze_2() {
        let input = indoc! { r#"#cl[ #lin[upper envelope] test ][o:1] "# };
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 31..37,
                settings_match: 33..36,
            },]]
        );
    }

    // #[test]
    // fn test_apple() {
    //     // TODO: Look into: https://github.com/tfachmann/typst-as-library
    //     use typst::syntax::ast::FuncCall;
    //     use typst::syntax::{SyntaxKind, parse};
    //     use typst::{
    //         WorldExt,
    //         syntax::{
    //             Source,
    //             ast::{AstNode, Expr},
    //         },
    //     };
    //     let input = "test #cl[#cl[b][g:1]#cl[$a s (d)$]][g:1] test";
    //     let a = parse(input);
    //     let source = Source::detached(input);
    //     let b = &a
    //         .children()
    //         .filter(|x| matches!(x.kind(), SyntaxKind::FuncCall))
    //         .map(|x| x.cast::<FuncCall>().unwrap())
    //         .map(|x| {
    //             dbg!(x.span().range());
    //             let res = x.span().into_raw();
    //             dbg!(&res);
    //             if let Expr::Ident(e) = x.callee() {
    //                 dbg!(&e);
    //                 if e.as_str() == "cl" {
    //                     let span = e.span().range();
    //                     dbg!(&span);
    //                 }
    //             }
    //             let c = x.args().items();
    //             // dbg!(&c);
    //         })
    //         // .map(|x| x.clone().into_text())
    //         // .map(|x|
    //         //     dbg!(&x);
    //         //     x.kind()
    //         // })
    //         .collect::<Vec<_>>();
    //     dbg!(&b);
    //     assert!(false);
    // }
}
