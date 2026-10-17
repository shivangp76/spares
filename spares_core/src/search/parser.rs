use std::borrow::Cow;

use chrono::DateTime;
use chrono::NaiveDate;
use chrono::TimeZone;
use chrono::Utc;
use miette::Error;
use miette::LabeledSpan;
use miette::WrapErr;
use miette::miette;

use crate::search::Atom;
use crate::search::Op;
use crate::search::RelativeDate;
use crate::search::Token;
use crate::search::TokenKind;
use crate::search::TokenTree;
use crate::search::lexer::Lexer;

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

pub(crate) struct Parser<'de> {
    whole: &'de str,
    lexer: Lexer<'de>,
}

impl<'de> Parser<'de> {
    pub(crate) fn new(input: &'de str) -> Self {
        Self {
            whole: input,
            lexer: Lexer::new(input),
        }
    }

    pub(crate) fn parse_expression(mut self) -> Result<TokenTree<'de>, Error> {
        Ok(hoist_modifiers(self.parse_expression_within(0)?))
    }

    #[allow(clippy::too_many_lines, reason = "main parser method")]
    pub(crate) fn parse_expression_within(&mut self, min_bp: u8) -> Result<TokenTree<'de>, Error> {
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
            Token {
                kind: TokenKind::RelativeDate,
                span,
            } => TokenTree::Atom(Atom::RelativeDate(
                RelativeDate::parse(&self.whole[span.clone()]).map_err(|e| {
                    miette::miette! {
                        labels = vec![LabeledSpan::at(span, "this date")],
                        "{e}",
                    }
                    .with_source_code(self.whole.to_string())
                })?,
            )),
            Token {
                kind: TokenKind::Regex,
                span,
            } => TokenTree::Atom(Atom::Regex(Token::unescape(&self.whole[span]))),

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

/// Whether `tree` is a `sort_by_asc`, `sort_by_desc` or `limit` term, which changes how results
/// are returned rather than which items match.
fn is_modifier(tree: &TokenTree) -> bool {
    matches!(
        tree,
        TokenTree::Cons(Op::Equal, children)
            if matches!(
                children.first(),
                Some(TokenTree::Atom(Atom::Field(field)))
                    if matches!(field.as_ref(), "sort_by_asc" | "sort_by_desc" | "limit")
            )
    )
}

/// Moves each sort and limit to the end of the parentheses it is written in, or of the whole
/// query, so that it applies to all of them however `and` and `or` group the rest. For example,
/// `(a limit=2) or (b limit=1) limit=10` limits the whole `or` to 10, and
/// `tag=a or tag=b sort_by_asc=c.due` sorts both tags.
fn hoist_modifiers(tree: TokenTree<'_>) -> TokenTree<'_> {
    fn strip<'de>(
        tree: TokenTree<'de>,
        modifiers: &mut Vec<TokenTree<'de>>,
    ) -> Option<TokenTree<'de>> {
        match tree {
            TokenTree::Cons(op @ (Op::And | Op::Or), children) => {
                let mut kept = children
                    .into_iter()
                    .filter_map(|child| strip(child, modifiers))
                    .collect::<Vec<_>>();
                match kept.len() {
                    0 => None,
                    1 => kept.pop(),
                    _ => Some(TokenTree::Cons(op, kept)),
                }
            }
            tree if is_modifier(&tree) => {
                modifiers.push(tree);
                None
            }
            tree => Some(hoist_within(tree)),
        }
    }
    // Groups start a new scope. A modifier directly under `-` is left for the evaluator to reject.
    fn hoist_within(tree: TokenTree<'_>) -> TokenTree<'_> {
        match tree {
            TokenTree::Cons(Op::Group, children) => TokenTree::Cons(
                Op::Group,
                children.into_iter().map(hoist_modifiers).collect(),
            ),
            TokenTree::Cons(Op::Minus, children) => {
                TokenTree::Cons(Op::Minus, children.into_iter().map(hoist_within).collect())
            }
            tree => tree,
        }
    }
    let mut modifiers = Vec::new();
    let rest = strip(tree, &mut modifiers);
    let mut terms = rest.into_iter().chain(modifiers).collect::<Vec<_>>();
    match terms.len() {
        0 => TokenTree::Atom(Atom::Nil),
        1 => terms.pop().expect("one term"),
        _ => TokenTree::Cons(Op::And, terms),
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
        // `and`, written or implied by a space, binds tighter than `or`, so `a or b c` is
        // `a or (b and c)`.
        Op::Or => (1, 2),
        Op::And => (3, 4),
        Op::LessThan | Op::LessThanEqual | Op::GreaterThan | Op::GreaterThanEqual => (6, 5),
        Op::Equal | Op::Tilde => (11, 10),
        Op::Colon => (13, 12),
        // Op::Field => (16, 15),
        _ => return None,
    };
    Some(res)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    #[test]
    fn test_parser_basic() {
        let inputs = [
            "dog and tag or tag~math or -card.special_state=scheduler_buried a b",
            r##"tag~"math \"test" and tag~#"math "test"# and (c.stability>=2 or -c.special_state=suspended)"##,
            r#"a="b"=c"#,
            r"-(tag=how-a-car-works parser_name=markdown)",
            "",
        ];
        for input in inputs {
            let parser = Parser::new(input);
            let token_tree = parser.parse_expression();
            assert!(token_tree.is_ok());
        }
    }

    #[test]
    fn test_parser_advanced_1() {
        let input = r#"dog and tag~"math\" test" or -c.special_state=scheduler_buried"#;
        let parser = Parser::new(input);
        let token_tree_res = parser.parse_expression();
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
        let input = r"-tag=how-a-car-works parser_name=markdown";
        let parser = Parser::new(input);
        let token_tree_res = parser.parse_expression();
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
    fn and_binds_tighter_than_or() {
        let parse = |input| Parser::new(input).parse_expression().unwrap().to_string();
        assert_eq!(
            parse("a or b c"),
            r#"(or (~  "a") (and (~  "b") (~  "c")))"#
        );
        assert_eq!(
            parse("a b or c"),
            r#"(or (and (~  "a") (~  "b")) (~  "c"))"#
        );
        assert_eq!(
            parse("-a or b and c"),
            r#"(or (not (~  "a")) (and (~  "b") (~  "c")))"#
        );
    }

    #[test]
    fn sorts_and_limits_apply_to_their_parentheses() {
        let parse = |input| Parser::new(input).parse_expression().unwrap().to_string();
        assert_eq!(
            parse("(a limit=2) or (b limit=1) limit=10"),
            r#"(and (or (group (and (~  "a") (= limit 2))) (group (and (~  "b") (= limit 1)))) (= limit 10))"#
        );
        assert_eq!(
            parse("a or b sort_by_asc=c.due"),
            r#"(and (or (~  "a") (~  "b")) (= sort_by_asc "c.due"))"#
        );
        assert_eq!(parse("limit=5"), "(= limit 5)");
        assert_eq!(parse("-limit=5"), "(not (= limit 5))");
    }

    #[test]
    fn test_parser_equals_associativity() {
        let input = r#"custom_data:"$.array[0].key">=1"#;
        let parser = Parser::new(input);
        let token_tree_res = parser.parse_expression();
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
