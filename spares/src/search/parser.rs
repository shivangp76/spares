use crate::search::{Atom, Op, Token, TokenKind, TokenTree, lexer::Lexer};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use miette::{Error, LabeledSpan, WrapErr, miette};
use std::borrow::Cow;

fn parse_date(date_str: &str) -> Result<DateTime<Utc>, Error> {
    // Try parsing as NaiveDate (YYYY-MM-DD)
    if let Ok(naive_date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        // SAFETY: Hard coded hours, minutes, and seconds.
        let naive_date_time = naive_date.and_hms_opt(0, 0, 0).unwrap();
        return Ok(TimeZone::from_utc_datetime(&Utc, &naive_date_time));
    }

    // Try parsing as DateTime<Utc> (YYYY-MM-DDTHH:MM:SSZ)
    if let Ok(datetime) = DateTime::parse_from_rfc3339(date_str) {
        return Ok(datetime.with_timezone(&Utc));
    }

    // Return an error if neither format matches
    Err(miette!("Invalid date format: {}", date_str))
}

pub struct Parser<'de> {
    whole: &'de str,
    lexer: Lexer<'de>,
}

impl<'de> Parser<'de> {
    pub fn new(input: &'de str) -> Self {
        Self {
            whole: input,
            lexer: Lexer::new(input),
        }
    }

    pub fn parse_expression(mut self) -> Result<TokenTree<'de>, Error> {
        self.parse_expression_within(0)
    }

    #[allow(clippy::too_many_lines, reason = "main parser method")]
    pub fn parse_expression_within(&mut self, min_bp: u8) -> Result<TokenTree<'de>, Error> {
        let lhs = match self.lexer.next() {
            Some(Ok(token)) => token,
            None => return Ok(TokenTree::Atom(Atom::Nil)),
            Some(Err(e)) => {
                return Err(e).wrap_err("on left-hand side");
            }
        };
        let mut lhs = match lhs {
            // atoms
            Token {
                kind: TokenKind::String,
                span,
            } => TokenTree::Atom(Atom::String(Token::unescape(&self.whole[span]))),
            Token {
                kind: TokenKind::Integer(n),
                ..
            } => TokenTree::Atom(Atom::Integer(n)),
            Token {
                kind: TokenKind::Float(n),
                ..
            } => TokenTree::Atom(Atom::Float(n)),
            Token {
                kind: TokenKind::True,
                ..
            } => TokenTree::Atom(Atom::Boolean(true)),
            Token {
                kind: TokenKind::False,
                ..
            } => TokenTree::Atom(Atom::Boolean(false)),
            Token {
                kind: TokenKind::Field,
                span,
                ..
            } => TokenTree::Atom(Atom::Field(Cow::Borrowed(&self.whole[span]))),
            Token {
                kind: TokenKind::Date,
                span,
                ..
            } => TokenTree::Atom(Atom::DateTime(parse_date(&self.whole[span])?)),

            // groups
            Token {
                kind: TokenKind::LeftParen,
                ..
            } => {
                let lhs = self
                    .parse_expression_within(0)
                    .wrap_err("in bracketed expression")?;
                self.lexer
                    .expect(
                        TokenKind::RightParen,
                        "Unexpected end to bracketed expression",
                    )
                    .wrap_err("after bracketed expression")?;
                TokenTree::Cons(Op::Group, vec![lhs])
            }

            // unary prefix expressions
            Token {
                kind: TokenKind::Minus,
                // | TokenKind::GreaterThan
                // | TokenKind::GreaterThanEqual
                // | TokenKind::LessThan
                // | TokenKind::LessThanEqual,
                ..
            } => {
                let op = match lhs.kind {
                    TokenKind::Minus => Op::Minus,
                    // TokenKind::GreaterThan => Op::GreaterThan,
                    // TokenKind::GreaterThanEqual => Op::GreaterThanEqual,
                    // TokenKind::LessThan => Op::LessThan,
                    // TokenKind::LessThanEqual => Op::LessThanEqual,
                    _ => unreachable!("by the outer match arm pattern"),
                };
                let ((), r_bp) = prefix_binding_power(op);
                let rhs = self
                    .parse_expression_within(r_bp)
                    .wrap_err("in right-hand side")?;
                TokenTree::Cons(op, vec![rhs])
            }

            token => {
                return Err(miette::miette! {
                    labels = vec![
                        LabeledSpan::at(token.span.clone(), "here"),
                    ],
                    help = format!("Unexpected {token:?}"),
                    "Expected an expression",
                }
                .with_source_code(self.whole.to_string()));
            }
        };

        loop {
            let op = self.lexer.peek();
            if op.is_some_and(|op| op.is_err()) {
                return Err(self
                    .lexer
                    .next()
                    .expect("checked Some above")
                    .expect_err("checked Err above"))
                .wrap_err("in place of expected operator");
            }
            let op = match op.map(|res| res.as_ref().expect("handled Err above")) {
                Some(Token {
                    kind: TokenKind::RightParen,
                    ..
                })
                | None => break,
                Some(Token {
                    kind: TokenKind::Minus,
                    ..
                }) => Op::Minus,
                Some(Token {
                    kind: TokenKind::LessThanEqual,
                    ..
                }) => Op::LessThanEqual,
                Some(Token {
                    kind: TokenKind::GreaterThanEqual,
                    ..
                }) => Op::GreaterThanEqual,
                Some(Token {
                    kind: TokenKind::LessThan,
                    ..
                }) => Op::LessThan,
                Some(Token {
                    kind: TokenKind::GreaterThan,
                    ..
                }) => Op::GreaterThan,
                Some(Token {
                    kind: TokenKind::And,
                    ..
                }) => Op::And,
                Some(Token {
                    kind: TokenKind::Or,
                    ..
                }) => Op::Or,
                Some(Token {
                    kind: TokenKind::Equal,
                    ..
                }) => Op::Equal,
                Some(Token {
                    kind: TokenKind::Tilde,
                    ..
                }) => Op::Tilde,
                Some(Token {
                    kind: TokenKind::Colon,
                    ..
                }) => Op::Colon,
                Some(token) => {
                    return Err(miette::miette! {
                        labels = vec![
                            LabeledSpan::at(token.span.clone(), "here"),
                        ],
                        help = format!("Unexpected {token:?}"),
                        "Expected an infix operator",
                    }
                    .with_source_code(self.whole.to_string()));
                }
            };

            if let Some((l_bp, ())) = postfix_binding_power(op) {
                if l_bp < min_bp {
                    break;
                }
                self.lexer.next();

                // lhs = match op {
                //     // Op::Call => TokenTree::Call {
                //     //     callee: Box::new(lhs),
                //     //     arguments: self
                //     //         .parse_fun_call_arguments()
                //     //         .wrap_err("in function call arguments")?,
                //     // },
                //     _ => TokenTree::Cons(op, vec![lhs]),
                // };
                lhs = TokenTree::Cons(op, vec![lhs]);
                continue;
            }

            if let Some((l_bp, r_bp)) = infix_binding_power(op) {
                if l_bp < min_bp {
                    break;
                }
                self.lexer.next();

                lhs = {
                    let rhs = self
                        .parse_expression_within(r_bp)
                        .wrap_err_with(|| format!("on the right-hand side of {lhs} {op}"))?;
                    TokenTree::Cons(op, vec![lhs, rhs])
                };
                continue;
            }

            break;
        }

        Ok(lhs)
    }
}

fn prefix_binding_power(op: Op) -> ((), u8) {
    match op {
        Op::Minus => ((), 4),
        // Op::LessThan | Op::LessThanEqual | Op::GreaterThan | Op::GreaterThanEqual => ((), 11),
        _ => panic!("bad op: {:?}", op),
    }
}

fn postfix_binding_power(_op: Op) -> Option<(u8, ())> {
    // let res = match op {
    //     // Op::Call => (13, ()),
    //     _ => return None,
    // };
    // Some(res)
    None
}

fn infix_binding_power(op: Op) -> Option<(u8, u8)> {
    let res = match op {
        Op::And | Op::Or => (3, 4),
        Op::LessThan | Op::LessThanEqual | Op::GreaterThan | Op::GreaterThanEqual => (6, 5),
        Op::Equal | Op::Tilde => (11, 10),
        Op::Colon => (9, 8),
        // Op::Field => (16, 15),
        _ => return None,
    };
    Some(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    #[test]
    fn test_parser_basic() {
        let inputs = [
            "dog and tag or tag~math or -card.special_state=scheduler_buried a b",
            r##"tag~"math \"test" and tag~#"math "test"# and (c.stability>=2 or -c.special_state=suspended)"##,
            r#"a="b"=c"#,
            r#"-(tag=how-a-car-works parser_name=markdown)"#,
            "",
        ];
        for input in inputs {
            let parser = Parser::new(input);
            let token_tree = parser.parse_expression();
            // println!("{}", &token_tree.as_ref().unwrap());
            dbg!(&token_tree);
            assert!(token_tree.is_ok());
        }
    }

    #[test]
    fn test_parser_advanced_1() {
        let input = r#"dog and tag~"math\" test" or -c.special_state=scheduler_buried"#;
        let parser = Parser::new(input);
        let token_tree_res = parser.parse_expression();
        dbg!(&token_tree_res);
        assert!(token_tree_res.is_ok());
        let token_tree = token_tree_res.unwrap();
        assert_eq!(
            token_tree,
            TokenTree::Cons(
                Op::Or,
                vec![
                    TokenTree::Cons(
                        Op::And,
                        vec![
                            TokenTree::Cons(
                                Op::Tilde,
                                vec![
                                    TokenTree::Atom(Atom::Field(Cow::Borrowed(""))),
                                    TokenTree::Atom(Atom::String(Cow::Borrowed("dog"))),
                                ]
                            ),
                            TokenTree::Cons(
                                Op::Tilde,
                                vec![
                                    TokenTree::Atom(Atom::Field(Cow::Borrowed("tag"))),
                                    TokenTree::Atom(Atom::String(Cow::Borrowed("math\" test")))
                                ]
                            )
                        ]
                    ),
                    TokenTree::Cons(
                        Op::Minus,
                        vec![TokenTree::Cons(
                            Op::Equal,
                            vec![
                                TokenTree::Atom(Atom::Field(Cow::Borrowed("c.special_state"))),
                                TokenTree::Atom(Atom::String(Cow::Borrowed("scheduler_buried"))),
                            ]
                        )]
                    )
                ]
            )
        );
    }

    #[test]
    fn test_parser_minus_precedence() {
        let input = r#"-tag=how-a-car-works parser_name=markdown"#;
        let parser = Parser::new(input);
        let token_tree_res = parser.parse_expression();
        dbg!(&token_tree_res);
        assert!(token_tree_res.is_ok());
        let token_tree = token_tree_res.unwrap();
        assert_eq!(
            token_tree,
            TokenTree::Cons(
                Op::And,
                vec![
                    TokenTree::Cons(
                        Op::Minus,
                        vec![TokenTree::Cons(
                            Op::Equal,
                            vec![
                                TokenTree::Atom(Atom::Field(Cow::Borrowed("tag"))),
                                TokenTree::Atom(Atom::String(Cow::Borrowed("how-a-car-works"))),
                            ]
                        ),]
                    ),
                    TokenTree::Cons(
                        Op::Equal,
                        vec![
                            TokenTree::Atom(Atom::Field(Cow::Borrowed("parser_name"))),
                            TokenTree::Atom(Atom::String(Cow::Borrowed("markdown"))),
                        ]
                    ),
                ]
            )
        );
    }

    #[test]
    fn test_parser_equals_associativity() {
        let input = r#"custom_data:"$.array[0].key">=1"#;
        let parser = Parser::new(input);
        let token_tree_res = parser.parse_expression();
        dbg!(&token_tree_res);
        assert!(token_tree_res.is_ok());
        let token_tree = token_tree_res.unwrap();
        assert_eq!(
            token_tree,
            TokenTree::Cons(
                Op::GreaterThanEqual,
                vec![
                    TokenTree::Cons(
                        Op::Colon,
                        vec![
                            TokenTree::Atom(Atom::Field(Cow::Borrowed("custom_data"))),
                            TokenTree::Atom(Atom::String(Cow::Borrowed("$.array[0].key"))),
                        ]
                    ),
                    TokenTree::Atom(Atom::Integer(1)),
                ]
            )
        );
    }
}
