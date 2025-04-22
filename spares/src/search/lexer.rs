use crate::search::{Token, TokenKind};
use miette::{Diagnostic, Error, LabeledSpan, SourceSpan};
use thiserror::Error;
use unscanny::Scanner;

pub struct Lexer<'de> {
    /// The scanner: contains the underlying string and location as a "cursor".
    s: Scanner<'de>,
    buffered: Vec<Result<Token, miette::Error>>,
    prev: Option<Token>,
    normalize: bool,
    // peeked: Option<Result<Token, miette::Error>>,
}

impl<'de> Lexer<'de> {
    pub fn new(text: &'de str) -> Self {
        Self {
            s: Scanner::new(text),
            buffered: Vec::with_capacity(5),
            prev: None,
            normalize: true,
            // peeked: None,
        }
    }
}

#[derive(Diagnostic, Debug, Error)]
#[error("Unexpected EOF")]
pub struct Eof;

#[derive(Diagnostic, Debug, Error)]
#[error("Unexpected token '{token}'")]
struct SingleTokenError {
    #[source_code]
    src: String,

    pub token: char,

    #[label = "this input character"]
    err_span: SourceSpan,
}

#[derive(Diagnostic, Debug, Error)]
#[error("Unterminated string")]
pub struct StringTerminationError {
    #[source_code]
    src: String,

    #[label = "this string literal"]
    err_span: SourceSpan,
}

impl Iterator for Lexer<'_> {
    type Item = Result<Token, Error>;

    /// Once the iterator returns `Err`, it will only return `None`.
    fn next(&mut self) -> Option<Self::Item> {
        // First return any buffered tokens
        if let Some(token) = self.buffered.pop() {
            if let Ok(t) = token {
                if self.normalize {
                    self.prev = Some(t.clone());
                }
                return Some(Ok(t));
            }
            return Some(token);
        }

        self.s.eat_whitespace();
        let cursor_start = self.s.cursor();
        let just = move |kind: TokenKind, end: usize| {
            Ok(Token {
                kind,
                span: cursor_start..end,
            })
        };
        let current_token_res: Result<Token, Error> = match self.s.eat() {
            Some('(') => just(TokenKind::LeftParen, self.s.cursor()),
            Some(')') => just(TokenKind::RightParen, self.s.cursor()),
            Some('=') => just(TokenKind::Equal, self.s.cursor()),
            Some('<') if self.s.eat_if('=') => just(TokenKind::LessThanEqual, self.s.cursor()),
            Some('<') => just(TokenKind::LessThan, self.s.cursor()),
            Some('>') if self.s.eat_if('=') => just(TokenKind::GreaterThanEqual, self.s.cursor()),
            Some('>') => just(TokenKind::GreaterThan, self.s.cursor()),
            Some(':') => just(TokenKind::Colon, self.s.cursor()),
            Some('~') => just(TokenKind::Tilde, self.s.cursor()),
            Some('a') if self.s.eat_if("nd") => just(TokenKind::And, self.s.cursor()),
            Some('A') if self.s.eat_if("ND") => just(TokenKind::And, self.s.cursor()),
            Some('o') if self.s.eat_if('r') => just(TokenKind::Or, self.s.cursor()),
            Some('O') if self.s.eat_if('R') => just(TokenKind::Or, self.s.cursor()),
            Some('t') if self.s.eat_if("rue") => just(TokenKind::True, self.s.cursor()),
            Some('f') if self.s.eat_if("alse") => just(TokenKind::False, self.s.cursor()),
            Some('"') => match self.parse_string(false) {
                Ok(kind) => Ok(Token {
                    kind,
                    span: cursor_start + 1..self.s.cursor() - 1,
                }),
                Err(e) => return Some(Err(e)),
            },
            Some('#') if self.s.eat_if('"') => match self.parse_string(true) {
                Ok(kind) => Ok(Token {
                    kind,
                    span: cursor_start + 2..self.s.cursor() - 2,
                }),
                Err(e) => return Some(Err(e)),
            },
            Some(c) if char::is_ascii_digit(&c) || c == '-' => match self.parse_date_or_number(c) {
                Ok(kind) => Ok(Token {
                    kind,
                    span: cursor_start..self.s.cursor(),
                }),
                Err(e) => return Some(Err(e)),
            },
            Some(ch) if char::is_alphanumeric(ch) || ch == '.' || ch == '_' => {
                self.s
                    .eat_while(|c| char::is_alphanumeric(c) || c == '.' || c == '_' || c == '-');
                let token_kind = match self.s.peek() {
                    Some('=' | '>' | '<' | '~' | ':') => TokenKind::Field,
                    _ => TokenKind::String,
                };
                Ok(Token {
                    kind: token_kind,
                    span: (cursor_start..self.s.cursor()),
                })
            }
            Some(e) => {
                return Some(Err(SingleTokenError {
                    src: self.s.string().to_string(),
                    token: e,
                    err_span: SourceSpan::from(self.s.cursor()..self.s.string().len()),
                }
                .into()));
            }
            None => return None,
        };
        match current_token_res {
            Err(e) => Some(Err(e)),
            Ok(mut current_token) => {
                self.normalize(&mut current_token);
                if self.normalize {
                    self.prev = Some(current_token.clone());
                }
                Some(Ok(current_token))
            }
        }
    }
}

impl Lexer<'_> {
    fn normalize(&mut self, current_token: &mut Token) {
        if self.normalize {
            if matches!(current_token.kind, TokenKind::String) {
                let add_implied_field = self.prev.as_ref().is_none_or(|prev| {
                    !matches!(
                        prev.kind,
                        TokenKind::Equal
                            | TokenKind::Tilde
                            | TokenKind::GreaterThan
                            | TokenKind::GreaterThanEqual
                            | TokenKind::LessThan
                            | TokenKind::LessThanEqual
                            | TokenKind::Colon
                    )
                });
                if add_implied_field {
                    self.buffered.push(Ok(current_token.clone()));
                    self.buffered.push(Ok(Token {
                        kind: TokenKind::Tilde,
                        span: current_token.span.start..current_token.span.start, // Zero-width span at start
                    }));
                    *current_token = Token {
                        kind: TokenKind::Field,
                        span: current_token.span.start..current_token.span.start, // Zero-width span at start
                    };
                }
            }

            let add_implied_and = matches!(
                current_token.kind,
                TokenKind::Field | TokenKind::LeftParen | TokenKind::Minus
            ) && self.prev.is_some()
                && !matches!(self.prev, Some(ref token) if matches!(token.kind, TokenKind::And | TokenKind::Or | TokenKind::Minus | TokenKind::LeftParen));
            if add_implied_and {
                self.buffered.push(Ok(current_token.clone()));
                *current_token = Token {
                    kind: TokenKind::And,
                    span: current_token.span.start..current_token.span.start, // Zero-width span at start
                };
            }
        }
    }

    fn parse_string(&mut self, hash_quote: bool) -> Result<TokenKind, Error> {
        let mut result = String::new();
        while let Some(c) = self.s.eat() {
            match c {
                '"' => {
                    if !hash_quote || self.s.eat_if('#') {
                        return Ok(TokenKind::String);
                    }
                    result.push(c);
                }
                '\\' => {
                    if let Some(c) = self.s.eat() {
                        result.push(c);
                    }
                }
                _ => {
                    result.push(c);
                }
            }
        }
        let err = StringTerminationError {
            src: self.s.string().to_string(),
            err_span: SourceSpan::from(self.s.cursor()..self.s.string().len()),
        };
        Err(err.into())
    }

    #[allow(clippy::unnecessary_wraps, reason = "For consistency")]
    fn parse_date_or_number(&mut self, c: char) -> Result<TokenKind, Error> {
        let start = self.s.cursor() - 1;

        // If it's a minus, check the next character
        if c == '-' {
            let next_char = self.s.peek();
            if next_char.is_some_and(|nc| char::is_ascii_digit(&nc)) {
                self.s.eat(); // Consume the '-'
            } else {
                return Ok(TokenKind::Minus);
            }
        }

        // At this point, it's a number (possibly negative)
        self.s.eat_while(char::is_ascii_digit);

        // Check for potential date format
        if self.s.eat_if('-') {
            self.s.eat_while(char::is_ascii_digit);
            if self.s.eat_if('-') {
                self.s.eat_while(char::is_ascii_digit);

                // Optional time parsing
                if self.s.eat_if('T') {
                    self.s.eat_while(char::is_ascii_digit); // HH
                    if self.s.eat_if(':') {
                        self.s.eat_while(char::is_ascii_digit); // MM
                        if self.s.eat_if(':') {
                            self.s.eat_while(char::is_ascii_digit); // SS
                            self.s.eat_if('Z');
                        }
                    }
                }
                return Ok(TokenKind::Date);
            }
        }
        // Handle as a float if there's a decimal point
        let contains_decimal = self.s.eat_if('.');
        if contains_decimal {
            self.s.eat_while(char::is_ascii_digit);
            let num = self.s.from(start).parse::<f64>().unwrap();
            Ok(TokenKind::Float(num))
        } else {
            let num = self.s.from(start).parse::<i64>().unwrap();
            Ok(TokenKind::Integer(num))
        }
    }
}

impl Lexer<'_> {
    pub fn expect(
        &mut self,
        expected: TokenKind,
        unexpected: &str,
    ) -> Result<Token, miette::Error> {
        self.expect_where(|next| next.kind == expected, unexpected)
    }

    pub fn expect_where(
        &mut self,
        mut check: impl FnMut(&Token) -> bool,
        unexpected: &str,
    ) -> Result<Token, miette::Error> {
        match self.next() {
            Some(Ok(token)) if check(&token) => Ok(token),
            Some(Ok(token)) => Err(miette::miette! {
                labels = vec![
                    LabeledSpan::at(token.span.clone(), "here"),
                ],
                help = format!("Expected {token:?}"),
                "{unexpected}",
            }
            .with_source_code(self.s.string().to_string())),
            Some(Err(e)) => Err(e),
            None => Err(Eof.into()),
        }
    }

    pub fn peek(&mut self) -> Option<&Result<Token, miette::Error>> {
        // if self.peeked.is_some() {
        //     return self.peeked.as_ref();
        // }
        //
        // self.peeked = self.next();
        // self.peeked.as_ref()
        if !self.buffered.is_empty() {
            return self.buffered.last();
        }
        if let Some(next_token) = self.next() {
            self.buffered.push(next_token);
            return self.buffered.last();
        }
        None
    }

    pub fn extract_tag_dependencies(&mut self) -> Result<Vec<String>, miette::Error> {
        let tokens = self
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|t| (t.kind, &self.s.string()[t.span]))
            .collect::<Vec<_>>();
        let mut tag_values = Vec::new();
        for window in tokens.windows(3) {
            if let [(TokenKind::Field, "tag"), _, (TokenKind::String, value)] = window {
                tag_values.push((*value).to_string());
            }
        }
        Ok(tag_values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::TokenKind;

    #[test]
    fn test_advanced_1() {
        let input = "dog and tag or tag=math or card.scheduler_buried=false";
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "dog"),
                (TokenKind::And, "and"),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "tag"),
                (TokenKind::Or, "or"),
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "math"),
                (TokenKind::Or, "or"),
                (TokenKind::Field, "card.scheduler_buried"),
                (TokenKind::Equal, "="),
                (TokenKind::False, "false"),
            ]
        );
    }

    #[test]
    fn test_advanced_2() {
        let input = r#"dog or tag~"math test" or tag~"dog.*[^1] park""#;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "dog"),
                (TokenKind::Or, "or"),
                (TokenKind::Field, "tag"),
                (TokenKind::Tilde, "~"),
                (TokenKind::String, "math test"),
                (TokenKind::Or, "or"),
                (TokenKind::Field, "tag"),
                (TokenKind::Tilde, "~"),
                (TokenKind::String, "dog.*[^1] park"),
            ]
        );
    }

    #[test]
    fn test_advanced_3() {
        let input = r##"tag~"math \"test" and tag~#"math "test"# and (c.stability>=2 or -c.special_state=suspended)"##;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "tag"),
                (TokenKind::Tilde, "~"),
                (TokenKind::String, "math \\\"test"),
                (TokenKind::And, "and"),
                (TokenKind::Field, "tag"),
                (TokenKind::Tilde, "~"),
                (TokenKind::String, "math \"test"),
                (TokenKind::And, "and"),
                (TokenKind::LeftParen, "("),
                (TokenKind::Field, "c.stability"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Integer(2), "2"),
                (TokenKind::Or, "or"),
                (TokenKind::Minus, "-"),
                (TokenKind::Field, "c.special_state"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "suspended"),
                (TokenKind::RightParen, ")"),
            ]
        );
    }

    #[test]
    fn test_implied_and() {
        let input = r##"tag=math test and tag"##;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "math"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "test"),
                (TokenKind::And, "and"),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "tag"),
            ]
        );
    }

    #[test]
    fn test_date() {
        let input = r##"created_at>=2020-01-01 and c.stability>=-2.0"##;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "created_at"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Date, "2020-01-01"),
                (TokenKind::And, "and"),
                (TokenKind::Field, "c.stability"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Float(-2.0), "-2.0"),
            ]
        );
    }

    #[test]
    fn test_date_with_time() {
        let input = r##"created_at>=2020-01-01T12:12:12Z and c.stability>=2.0"##;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "created_at"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Date, "2020-01-01T12:12:12Z"),
                (TokenKind::And, "and"),
                (TokenKind::Field, "c.stability"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Float(2.0), "2.0"),
            ]
        );
    }

    #[test]
    fn test_invalid_time() {
        let input = r##"created_at>=2020-01-01T12: and c.stability>=2.0"##;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "created_at"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Date, "2020-01-01T12:"),
                (TokenKind::And, "and"),
                (TokenKind::Field, "c.stability"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Float(2.0), "2.0"),
            ]
        );
    }

    #[test]
    fn test_json_pointer() {
        let input = r#"custom_data:"$.x.y[1]"=zz and -custom_data:"$.a.b"=true and custom_data:"$.array[0].key">=1"#;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "custom_data"),
                (TokenKind::Colon, ":"),
                (TokenKind::String, "$.x.y[1]"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "zz"),
                (TokenKind::And, "and"),
                (TokenKind::Minus, "-"),
                (TokenKind::Field, "custom_data"),
                (TokenKind::Colon, ":"),
                (TokenKind::String, "$.a.b"),
                (TokenKind::Equal, "="),
                (TokenKind::True, "true"),
                (TokenKind::And, "and"),
                (TokenKind::Field, "custom_data"),
                (TokenKind::Colon, ":"),
                (TokenKind::String, "$.array[0].key"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Integer(1), "1"),
            ]
        );
    }

    #[test]
    fn test_error() {
        let input = r"#";
        let lexer = Lexer::new(input);
        let tokens = lexer.into_iter().collect::<Result<Vec<_>, _>>();
        assert!(tokens.is_err());
    }

    #[test]
    fn test_and_added_before_minus() {
        let input = r#"tag="real-analysis-1" -data~"Prove""#;
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        dbg!(&tokens);
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "real-analysis-1"),
                (TokenKind::And, ""),
                (TokenKind::Minus, "-"),
                (TokenKind::Field, "data"),
                (TokenKind::Tilde, "~"),
                (TokenKind::String, "Prove"),
            ]
        );
    }

    #[test]
    fn test_string_with_dash() {
        let input = r"tag=potential-theory tag=math";
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "potential-theory"),
                (TokenKind::And, ""),
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "math"),
            ]
        );
    }

    #[test]
    fn test_normalize() {
        let input = "hello world tag=test";
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "hello"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "world"),
                (TokenKind::And, ""),
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "test"),
            ]
        );

        let mut lexer = Lexer::new(input);
        lexer.normalize = false;
        let tokens = lexer
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::String, "hello"),
                (TokenKind::String, "world"),
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "test"),
            ]
        );
    }

    #[test]
    fn test_normalize_with_existing_and() {
        let input = "hello and world";
        let lexer = Lexer::new(input);
        let tokens = lexer
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "hello"),
                (TokenKind::And, "and"),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "world"),
            ]
        );

        let mut lexer = Lexer::new(input);
        lexer.normalize = false;
        let tokens = lexer
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                (TokenKind::String, "hello"),
                (TokenKind::And, "and"),
                (TokenKind::String, "world"),
            ]
        );
    }
}
