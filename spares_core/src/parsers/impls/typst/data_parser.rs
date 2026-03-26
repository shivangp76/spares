use crate::parsers::ClozeMatch;
use std::ops::Range;
use unscanny::Scanner;

const CLOZE_FUNC_NAME: &str = "cl";
const LINKED_NOTE_FUNC_NAME: &str = "lin";
const SETTINGS_FUNC_NAME: &str = "se";
const KEYWORD_FUNC_NAME: &str = "key";

#[derive(Debug, Default, PartialEq, Eq)]
enum DataMode {
    #[default]
    Markup,
    Code,
    Math,
}

pub struct TypstDataParser<'de> {
    s: Scanner<'de>,
    modes: Vec<DataMode>,
    /// This counts open square brackets not in math mode. By not counting math mode, we can ensure that this count will always be balanced at the end of a document. This may be from a function call (`#strong[`), start of code mode (`#[`), or just bracketed text (`[`).
    open_square_bracket_count: u32,
}

#[derive(Debug)]
enum OutputType {
    Cloze,
    LinkedNote,
    Settings,
    Keyword,
}

#[derive(Debug)]
enum Output {
    Cloze(Vec<ClozeMatch>),
    LinkedNote(Range<usize>),
    Settings(Range<usize>),
    Keyword(Range<usize>),
}

#[derive(Debug)]
struct Cloze {
    // `#cl`
    start: Range<usize>,
    // Eventually, `]` or `][`
    first_arg_end: Option<Range<usize>>,
    // `]` or `None`
    // second_arg_end: Option<Range<usize>>,
    depth: u32,
}

impl<'a> TypstDataParser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            s: Scanner::new(input),
            modes: vec![],
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

    pub fn next_keyword(&mut self) -> Option<Range<usize>> {
        self.next_data(&OutputType::Keyword).map(|x| match x {
            Output::Keyword(res) => res,
            _ => unreachable!(),
        })
    }

    #[expect(clippy::too_many_lines)]
    fn next_data(&mut self, output_type: &OutputType) -> Option<Output> {
        let mut cloze_nesting_level = 0;
        let mut all_clozes = Vec::new();
        let mut current_clozes = Vec::new();

        let mut current_linked_notes = Vec::new();
        let mut current_settings = Vec::new();
        let mut current_keywords = Vec::new();

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
                    if let Some(DataMode::Math) = self.modes.last() {
                        self.modes.pop();
                    } else {
                        self.modes.push(DataMode::Math);
                    }
                }
                // Handle function argument opening
                Some('#') => {
                    let func_name = self
                        .s
                        .eat_while(|c| char::is_alphanumeric(c) || c == '_' || c == '-');
                    let open_delim = if let Some('(') = self.s.peek() {
                        self.s.eat();
                        Some('(')
                    } else if let Some('[') = self.s.peek() {
                        self.s.eat();
                        Some('[')
                    } else {
                        None
                    };
                    if func_name == CLOZE_FUNC_NAME && matches!(output_type, OutputType::Cloze) {
                        cloze_nesting_level += 1;
                        current_clozes.push(Cloze {
                            start: cursor_start..self.s.cursor(),
                            first_arg_end: None,
                            depth: self.open_square_bracket_count + 1,
                        });
                    } else if func_name == LINKED_NOTE_FUNC_NAME
                        && matches!(output_type, OutputType::LinkedNote)
                    {
                        current_linked_notes.push((
                            cursor_start..self.s.cursor(),
                            self.open_square_bracket_count + 1,
                        ));
                    } else if func_name == SETTINGS_FUNC_NAME
                        && matches!(output_type, OutputType::Settings)
                    {
                        current_settings.push((
                            cursor_start..self.s.cursor(),
                            self.open_square_bracket_count + 1,
                        ));
                    } else if func_name == KEYWORD_FUNC_NAME
                        && matches!(output_type, OutputType::Keyword)
                    {
                        current_keywords.push((
                            cursor_start..self.s.cursor(),
                            self.open_square_bracket_count + 1,
                        ));
                    }
                    if open_delim.is_some() {
                        self.s.uneat();
                    }
                    if open_delim == Some('[') {
                        self.modes.push(DataMode::Markup);
                    } else if open_delim == Some('(') {
                        self.modes.push(DataMode::Code);
                    }
                }
                Some(')') if matches!(self.modes.last(), Some(DataMode::Code)) => {
                    self.modes.pop();
                }
                Some('[') if self.modes.last().is_none_or(|x| *x != DataMode::Math) => {
                    self.open_square_bracket_count += 1;
                }
                // Handle function argument closing
                Some(']') if self.modes.last().is_none_or(|x| *x != DataMode::Math) => {
                    if self.s.eat_if('[') {
                        // Second argument
                        // Don't increment `open_square_bracket_count` here.
                        let first_arg_end = self.s.cursor();
                        if let Some(cloze) = current_clozes.last_mut()
                            && cloze.depth == self.open_square_bracket_count
                        {
                            cloze.first_arg_end = Some(cursor_start..first_arg_end);
                        }
                    } else {
                        // End of all arguments (could be 1 or 2 args)
                        self.open_square_bracket_count -= 1;
                        self.modes.pop();

                        if matches!(output_type, OutputType::LinkedNote)
                            && let Some(linked_note_start) = current_linked_notes
                                .pop_if(|x| x.1 == self.open_square_bracket_count + 1)
                        {
                            return Some(Output::LinkedNote(linked_note_start.0.end..cursor_start));
                        } else if matches!(output_type, OutputType::Keyword)
                            && let Some(keyword_start) = current_keywords
                                .pop_if(|x| x.1 == self.open_square_bracket_count + 1)
                        {
                            return Some(Output::Keyword(keyword_start.0.end..cursor_start));
                        } else if matches!(output_type, OutputType::Settings)
                            && let Some(setting_start) = current_settings
                                .pop_if(|x| x.1 == self.open_square_bracket_count + 1)
                        {
                            return Some(Output::Settings(setting_start.0.end..cursor_start));
                        } else if let Some(mut cloze) = current_clozes
                            .pop_if(|c| c.depth == (self.open_square_bracket_count + 1))
                        {
                            cloze_nesting_level -= 1;
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
                            if cloze_nesting_level == 0 && matches!(output_type, OutputType::Cloze)
                            {
                                break;
                            }
                        }
                    }
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
            OutputType::LinkedNote | OutputType::Keyword | OutputType::Settings => None,
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
        let input = "Test #lin[basic] #test[p]asd";
        let mut parser = TypstDataParser::new(input);
        let mut all_linked_notes = Vec::new();
        while let Some(linked_note) = parser.next_linked_note() {
            all_linked_notes.push(linked_note);
        }
        assert_eq!(all_linked_notes, vec![10..15],);
    }

    #[test]
    fn test_advanced_linked_note() {
        let input = "Test #lin[basic [a] b] #test[p]asd";
        let mut parser = TypstDataParser::new(input);
        let mut all_linked_notes = Vec::new();
        while let Some(linked_note) = parser.next_linked_note() {
            all_linked_notes.push(linked_note);
        }
        assert_eq!(all_linked_notes, vec![10..21],);
    }

    #[test]
    fn test_linked_note_in_math() {
        let input = "Test $ a &= b #[(bc of #lin[Rule A])] \\ b &=c $";
        let mut parser = TypstDataParser::new(input);
        let mut all_linked_notes = Vec::new();
        while let Some(linked_note) = parser.next_linked_note() {
            all_linked_notes.push(linked_note);
        }
        assert_eq!(all_linked_notes, vec![28..34],);
    }

    #[test]
    fn test_basic_keyword() {
        let input = "Test #key[basic] #test[p]asd";
        let mut parser = TypstDataParser::new(input);
        let mut all_keywords = Vec::new();
        while let Some(linked_note) = parser.next_keyword() {
            all_keywords.push(linked_note);
        }
        assert_eq!(all_keywords, vec![10..15],);
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
        let input = "test #cl[$\n( b]\n\n$][g:1] test";
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
    fn test_cloze_in_math_1() {
        let input = "Test $ a &= b #[(bc of #cl[Rule A])] \\ b &=c #cl[a][g:1] $";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![
                vec![ClozeMatch {
                    start_match: 23..27,
                    end_match: 33..34,
                    settings_match: Range::default(),
                }],
                vec![ClozeMatch {
                    start_match: 45..49,
                    end_match: 50..56,
                    settings_match: 52..55
                }],
            ]
        );
    }

    #[test]
    fn test_cloze_in_math_2_with_settings() {
        let input = "Test $ a &= b #[(bc of #cl[Rule A][g:[1]])] \\ b &=c #cl[a][g:1] $";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![
                vec![ClozeMatch {
                    start_match: 23..27,
                    end_match: 33..41,
                    settings_match: 35..40,
                }],
                vec![ClozeMatch {
                    start_match: 52..56,
                    end_match: 57..63,
                    settings_match: 59..62
                }],
            ]
        );
    }

    #[test]
    fn test_cloze_in_math_with_code() {
        let input = "#cl[ $ #table() $ ]";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 18..19,
                settings_match: Range::default(),
            }],]
        );
    }

    #[test]
    fn test_cloze_in_math_with_code_2() {
        let input = "#cl[ $ #let a = b \\ $ ]";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(all_clozes.len(), 1);
    }

    #[test]
    fn test_cloze_in_command() {
        let input = "#strong[bc of #cl[Rule A])]";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 14..18,
                end_match: 24..25,
                settings_match: Range::default(),
            }],]
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
    fn test_other_func_in_cloze_1() {
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

    #[test]
    fn test_comment_in_code_mode_within_cloze() {
        let input = "#cl[test ```py\n m.sample() # equal probability\n  ```\n]";
        let mut parser = TypstDataParser::new(input);
        let mut all_clozes = Vec::new();
        while let Some(cloze) = parser.next_cloze() {
            all_clozes.push(cloze);
        }
        assert_eq!(
            all_clozes,
            vec![vec![ClozeMatch {
                start_match: 0..4,
                end_match: 53..54,
                settings_match: Range::default(),
            },]]
        );
    }

    // #[test]
    // fn test_typst_as_library() {
    //     // TODO: Look into: https://github.com/tfachmann/typst-as-library
    //     use typst_syntax::ast::FuncCall;
    //     use typst_syntax::{SyntaxKind, parse};
    //     use typst_syntax::ast::{AstNode, Expr, Ident};
    //     // use typst::syntax::ast::FuncCall;
    //     // use typst::syntax::{SyntaxKind, parse};
    //     // use typst::{
    //     //     WorldExt,
    //     //     syntax::{
    //     //         Source,
    //     //         ast::{AstNode, Expr},
    //     //     },
    //     // };
    //     let input = "test #cl[#cl[b][g:1]#cl[$a s (d)$]][g:1] test";
    //     let a = parse(input);
    //     dbg!(&a);
    //     // let source = Source::detached(input);
    //     // dbg!(&source);
    //     let b = &a
    //         .children()
    //         .filter(|x| matches!(x.kind(), SyntaxKind::FuncCall))
    //         .map(|x| x.cast::<FuncCall>().unwrap())
    //         .filter_map(|x| {
    //             if let Expr::Ident(ident) = x.callee()
    //                 && ident.as_str() == CLOZE_FUNC_NAME
    //             {
    //                 return Some(x);
    //             }
    //             None
    //         })
    //         .map(|y| {
    //             let a = &y.args().items().map(|x| x).collect::<Vec<_>>();
    //             dbg!(&a);
    //             // dbg!(&x.span());
    //             // dbg!(x.span().range());
    //             // let res = x.span().into_raw();
    //             // dbg!(&res);
    //             // if let Expr::Ident(e) = x.callee() {
    //             //     dbg!(&e);
    //             //     if e.as_str() == "cl" {
    //             //         let span = e.span().range();
    //             //         dbg!(&span);
    //             //     }
    //             // }
    //             // let c = x.args().items();
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
