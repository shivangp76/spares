use miette::Diagnostic;
use miette::Error;
use miette::LabeledSpan;
use miette::SourceSpan;
use thiserror::Error;
use unscanny::Scanner;

use crate::search::Token;
use crate::search::TokenKind;
use crate::search::evaluator::is_boolean_field;

pub(crate) struct Lexer<'de> {
    /// The scanner: contains the underlying string and location as a "cursor".
    s: Scanner<'de>,
    buffered: Vec<Result<Token, miette::Error>>,
    prev: Option<Token>,
    normalize: bool,
    // peeked: Option<Result<Token, miette::Error>>,
}

impl<'de> Lexer<'de> {
    pub(crate) fn new(text: &'de str) -> Self {
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
pub(crate) struct StringTerminationError {
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
            Some('!') if self.s.eat_if('=') => just(TokenKind::NotEqual, self.s.cursor()),
            Some(':') => return Some(Err(self.colon_error(cursor_start))),
            Some('~') => just(TokenKind::Tilde, self.s.cursor()),
            Some(open) if is_quote(open) => match self.parse_string(open, false) {
                Ok(close_len) => Ok(Token {
                    kind: TokenKind::String,
                    span: cursor_start + open.len_utf8()..self.s.cursor() - close_len,
                }),
                Err(e) => return Some(Err(e)),
            },
            Some('#') if self.quote_after("") => {
                let open = self.s.eat()?;
                match self.parse_string(open, true) {
                    Ok(close_len) => Ok(Token {
                        kind: TokenKind::String,
                        span: cursor_start + 1 + open.len_utf8()..self.s.cursor() - close_len - 1,
                    }),
                    Err(e) => return Some(Err(e)),
                }
            }
            Some('r') if self.quote_after("e:") => {
                self.s.eat_if("e:");
                let open = self.s.eat()?;
                let close_len = self.parse_string(open, false).ok()?;
                Ok(Token {
                    kind: TokenKind::Regex,
                    span: cursor_start + 3 + open.len_utf8()..self.s.cursor() - close_len,
                })
            }
            Some(c) if char::is_ascii_digit(&c) || c == '-' => {
                just(self.parse_date_or_number(c), self.s.cursor())
            }
            Some(ch) if char::is_alphanumeric(ch) || ch == '.' || ch == '_' => {
                self.s.eat_while(is_word_char);
                // A sort can list several fields, as in `sort_by_asc=c.due,c.stability`.
                if self.is_sort_value() {
                    self.s.eat_while(|c| is_word_char(c) || c == ',');
                }
                let kind = match self.eat_reference(cursor_start) {
                    Ok(true) => TokenKind::RelativeDate,
                    Ok(false) => match self
                        .eat_custom_data_path(cursor_start)
                        .and_then(|path| Ok(path || self.eat_rated_time(cursor_start)?))
                    {
                        // A path on its own, like `custom_data.flag`, means the key is there, and
                        // `c.rated[today]` that the card was rated then.
                        Ok(true) if !self.prev_takes_value() => TokenKind::Field,
                        Ok(true) => TokenKind::String,
                        Ok(false) => self.classify_word(cursor_start),
                        Err(e) => return Some(Err(e)),
                    },
                    Err(e) => return Some(Err(e)),
                };
                just(kind, self.s.cursor())
            }
            Some(e) => {
                return Some(Err(SingleTokenError {
                    src: self.s.string().to_string(),
                    token: e,
                    err_span: SourceSpan::from(cursor_start..self.s.cursor()),
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

/// Whether `c` can continue an unquoted word. `+` is allowed for offsets like `today+3d`.
fn is_word_char(c: char) -> bool {
    char::is_alphanumeric(c) || c == '.' || c == '_' || c == '-' || c == '+'
}

/// Whether a token of `kind` is followed by a value, like the `=` in `tag=math`.
fn takes_value(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Equal
            | TokenKind::Tilde
            | TokenKind::GreaterThan
            | TokenKind::GreaterThanEqual
            | TokenKind::LessThan
            | TokenKind::LessThanEqual
            | TokenKind::NotEqual
    )
}

/// Whether `c` delimits a string. Curly double quotes are accepted since phone keyboards insert them in place of `"`.
pub(crate) fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\u{201C}' | '\u{201D}')
}

impl Lexer<'_> {
    fn normalize(&mut self, current_token: &mut Token) {
        if self.normalize {
            // A value with no field before it is searched for in the note's text. That includes
            // numbers, dates and `true`, so `dog 2024` finds notes containing both words.
            if matches!(
                current_token.kind,
                TokenKind::String
                    | TokenKind::Regex
                    | TokenKind::Integer(_)
                    | TokenKind::Float(_)
                    | TokenKind::Date
                    | TokenKind::RelativeDate
                    | TokenKind::True
                    | TokenKind::False
            ) {
                let add_implied_field = !self.prev_takes_value();
                if add_implied_field {
                    if current_token.kind != TokenKind::Regex {
                        current_token.kind = TokenKind::String;
                    }
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
                // A value in parentheses, as in `linked_to_note=(tag=math)`, follows an operator.
                && !self.prev_takes_value()
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

    /// Whether the previous token is followed by a value, like the `=` in `tag=math`.
    fn prev_takes_value(&self) -> bool {
        self.prev
            .as_ref()
            .is_some_and(|prev| takes_value(prev.kind))
    }

    /// Classifies the word from `start` to the cursor. A keyword is only one when it is the
    /// whole word, so `orbit` and `android` are ordinary words.
    fn classify_word(&mut self, start: usize) -> TokenKind {
        let word = self.s.from(start);
        match word {
            "and" | "AND" => TokenKind::And,
            "or" | "OR" => TokenKind::Or,
            // A word for `-`, except as a value, as in `tag=not`.
            "not" | "NOT" if !self.prev_takes_value() => TokenKind::Minus,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ if self.operator_follows() => TokenKind::Field,
            // A boolean field on its own, like `c.suspended`, means it is true.
            _ if !self.prev_takes_value() && is_boolean_field(word) => {
                let end = self.s.cursor();
                self.buffered.push(Ok(Token {
                    kind: TokenKind::True,
                    span: end..end,
                }));
                self.buffered.push(Ok(Token {
                    kind: TokenKind::Equal,
                    span: end..end,
                }));
                TokenKind::Field
            }
            _ => TokenKind::String,
        }
    }

    /// Whether the word being lexed is the value of `sort_by_asc=` or `sort_by_desc=`.
    fn is_sort_value(&self) -> bool {
        self.prev.as_ref().is_some_and(|prev| {
            prev.kind == TokenKind::Equal
                && matches!(
                    self.s.string()[..prev.span.start].trim_end().rsplit(char::is_whitespace).next(),
                    Some(field) if field.ends_with("sort_by_asc") || field.ends_with("sort_by_desc")
                )
        })
    }

    /// Whether a comparison comes next, as in `c.due<=today`. Spaces may come before it, as in
    /// `c.due <= today`, unless the word is itself a value.
    fn operator_follows(&self) -> bool {
        let is_operator =
            |rest: &str| rest.starts_with(['=', '>', '<', '~', ':']) || rest.starts_with("!=");
        let rest = self.s.after();
        is_operator(rest) || (!self.prev_takes_value() && is_operator(rest.trim_start()))
    }

    /// After the word starting at `start`, consumes the rest of a reference like
    /// `note[gir].created_at-5d` or `card[12].due`, returning whether there was one. `[` is not
    /// allowed anywhere else, so this can't change the meaning of another query. The parser
    /// validates the reference.
    fn eat_reference(&mut self, start: usize) -> Result<bool, Error> {
        if !(matches!(self.s.from(start), "note" | "card") && self.s.eat_if('[')) {
            return Ok(false);
        }
        self.s.eat_until(']');
        if !self.s.eat_if(']') {
            return Err(miette::miette! {
                labels = vec![LabeledSpan::at(start..self.s.cursor(), "this reference")],
                "Missing `]` after the id",
            }
            .with_source_code(self.s.string().to_string()));
        }
        self.s.eat_while(is_word_char);
        Ok(true)
    }

    /// After the word starting at `start`, consumes the rest of a path into custom data, like the
    /// `[1].z` of `custom_data.x.y[1].z` or the `["a's key"]` of `c.custom_data["a's key"]`,
    /// returning whether the word is such a path. The evaluator validates the path.
    fn eat_custom_data_path(&mut self, start: usize) -> Result<bool, Error> {
        let word = self.s.from(start);
        let is_path = ["custom_data", "c.custom_data", "card.custom_data"]
            .iter()
            .any(|prefix| {
                word.strip_prefix(prefix).is_some_and(|rest| {
                    rest.starts_with('.') || (rest.is_empty() && self.s.peek() == Some('['))
                })
            });
        if !is_path {
            return Ok(false);
        }
        while self.s.eat_if('[') {
            let bracket = self.s.cursor() - 1;
            match self.s.peek() {
                Some(open) if is_quote(open) => {
                    self.s.eat();
                    self.parse_string(open, false)?;
                }
                _ => {
                    self.s.eat_while(|c: char| c != ']' && !c.is_whitespace());
                }
            }
            if !self.s.eat_if(']') {
                return Err(miette::miette! {
                    labels = vec![LabeledSpan::at(bracket..self.s.cursor(), "this `[`")],
                    "Missing `]`",
                }
                .with_source_code(self.s.string().to_string()));
            }
            self.s.eat_while(is_word_char);
        }
        Ok(true)
    }

    /// After the word starting at `start`, consumes the time in brackets of ratings at a time, like
    /// the `[>=today-7d]` of `c.rated[>=today-7d]`, returning whether there was one. The evaluator
    /// validates the time.
    fn eat_rated_time(&mut self, start: usize) -> Result<bool, Error> {
        if !(matches!(self.s.from(start), "c.rated" | "card.rated") && self.s.eat_if('[')) {
            return Ok(false);
        }
        self.s.eat_until(|c: char| c == ']' || c.is_whitespace());
        if !self.s.eat_if(']') {
            return Err(miette::miette! {
                labels = vec![LabeledSpan::at(start..self.s.cursor(), "this field")],
                help = "Write the time with no spaces, as in `c.rated[>=today-7d]`",
                "Missing `]` after the time",
            }
            .with_source_code(self.s.string().to_string()));
        }
        Ok(true)
    }

    /// The error for a `:` at `start`, which used to start a path into custom data, as in
    /// `custom_data:"$.x"`, and is now never valid outside quotes.
    fn colon_error(&self, start: usize) -> Error {
        miette::miette! {
            labels = vec![LabeledSpan::at(start..start + 1, "this `:`")],
            help = "Compare a field with `=` or `~`, as in `tag=math`, and write a path into custom data after a `.`, as in `custom_data.x=1`. Quote text that has a `:`, as in `tag=\"math:algebra\"`",
            "Unexpected `:`",
        }
        .with_source_code(self.s.string().to_string())
    }

    /// Whether the input after `prefix` starts with a quote character.
    fn quote_after(&self, prefix: &str) -> bool {
        self.s
            .after()
            .strip_prefix(prefix)
            .and_then(|rest| rest.chars().next())
            .is_some_and(is_quote)
    }

    /// Consumes the rest of a string opened by `open`, returning the byte length of the closing quote.
    ///
    /// A string opened with a straight quote is only closed by a straight quote. A string opened with a curly quote
    /// (e.g. from a phone keyboard's smart punctuation) is closed by any double quote.
    fn parse_string(&mut self, open: char, hash_quote: bool) -> Result<usize, Error> {
        let mut result = String::new();
        while let Some(c) = self.s.eat() {
            match c {
                c if c == '"' || (open != '"' && is_quote(c)) => {
                    if !hash_quote || self.s.eat_if('#') {
                        return Ok(c.len_utf8());
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

    /// Lexes a word starting with a digit or `-`, whose first character `c` was consumed. A `-`
    /// negates what follows, except right after an operator like `>=`, where it starts a value
    /// such as `-2` or `-math`. A number or date running into other word characters, like `3d`
    /// or `2024-25`, is an ordinary word.
    fn parse_date_or_number(&mut self, c: char) -> TokenKind {
        let start = self.s.cursor() - 1;
        if c == '-' && !self.prev_takes_value() {
            return TokenKind::Minus;
        }

        self.s.eat_while(char::is_ascii_digit);
        let mut is_date = false;
        if self.s.eat_if('-') {
            self.s.eat_while(char::is_ascii_digit);
            if self.s.eat_if('-') {
                self.s.eat_while(char::is_ascii_digit);
                is_date = true;

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
            }
        } else if self.s.eat_if('.') {
            self.s.eat_while(char::is_ascii_digit);
        }
        let number_end = self.s.cursor();
        self.s.eat_while(is_word_char);
        let text = self.s.from(start);
        if self.s.cursor() != number_end {
            TokenKind::String
        } else if is_date {
            TokenKind::Date
        } else if let Ok(n) = text.parse::<i64>() {
            TokenKind::Integer(n)
        } else if let Ok(n) = text.parse::<f64>() {
            TokenKind::Float(n)
        } else {
            TokenKind::String
        }
    }
}

impl Lexer<'_> {
    pub(crate) fn peek(&mut self) -> Option<&Result<Token, miette::Error>> {
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

    pub(crate) fn extract_tag_dependencies(&mut self) -> Result<Vec<String>, miette::Error> {
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
    fn test_relative_dates() {
        let input = "created_at>=note[gir].created_at-5d c.due<=today+3d note(dog)";
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
                (TokenKind::RelativeDate, "note[gir].created_at-5d"),
                (TokenKind::And, ""),
                (TokenKind::Field, "c.due"),
                (TokenKind::LessThanEqual, "<="),
                (TokenKind::String, "today+3d"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "note"),
                (TokenKind::And, ""),
                (TokenKind::LeftParen, "("),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "dog"),
                (TokenKind::RightParen, ")"),
            ]
        );
        assert!(
            Lexer::new("created_at>=note[gir")
                .collect::<Result<Vec<_>, _>>()
                .is_err()
        );
    }

    #[test]
    fn test_implied_and() {
        let input = r"tag=math test and tag";
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
        let input = r"created_at>=2020-01-01 and c.stability>=-2.0";
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
        let input = r"created_at>=2020-01-01T12:12:12Z and c.stability>=2.0";
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
        let input = r"created_at>=2020-01-01T12: and c.stability>=2.0";
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
    fn test_custom_data_paths() {
        assert_eq!(
            lex(r#"custom_data.x.y[1]=zz -c.custom_data["a's \"key"][0].b custom_data"#),
            vec![
                (TokenKind::Field, "custom_data.x.y[1]"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "zz"),
                (TokenKind::And, ""),
                (TokenKind::Minus, "-"),
                (TokenKind::Field, r#"c.custom_data["a's \"key"][0].b"#),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "custom_data"),
            ]
        );
        assert_eq!(
            lex("sort_by_asc=custom_data.x"),
            vec![
                (TokenKind::Field, "sort_by_asc"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "custom_data.x"),
            ]
        );
        for input in [
            "custom_data[0",
            r#"custom_data["a"=1"#,
            r#"custom_data:"$.x"=1"#,
        ] {
            assert!(
                Lexer::new(input).collect::<Result<Vec<_>, _>>().is_err(),
                "{input}"
            );
        }
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
    fn test_string_with_colon() {
        let input = r#"tag="math:measure-theory:homework""#;
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
                (TokenKind::String, "math:measure-theory:homework"),
            ]
        );
    }

    #[test]
    fn test_curly_quotes() {
        let input = "tag=\u{201C}math:homework\u{201D} #\u{201C}a \"b\u{201D}\u{201D}# re:\u{201C}x.y\u{201D} \"\u{201C}c\u{201D}\"";
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
                (TokenKind::String, "math:homework"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "a \"b\u{201D}"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::Regex, "x.y"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "\u{201C}c\u{201D}"),
            ]
        );
    }

    fn lex(input: &str) -> Vec<(TokenKind, &str)> {
        Lexer::new(input)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|t| (t.kind, &input[t.span]))
            .collect()
    }

    #[test]
    fn keywords_are_whole_words() {
        assert_eq!(
            lex("orbit android falsehood trueno"),
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "orbit"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "android"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "falsehood"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "trueno"),
            ]
        );
        assert_eq!(
            lex("a OR b"),
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "a"),
                (TokenKind::Or, "OR"),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "b"),
            ]
        );
    }

    #[test]
    fn values_without_a_field_are_text() {
        assert_eq!(
            lex("dog 2024 1.5 2020-01-01 3d 2024-25 true"),
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "dog"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "2024"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "1.5"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "2020-01-01"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "3d"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "2024-25"),
                (TokenKind::And, ""),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "true"),
            ]
        );
    }

    #[test]
    fn minus_negates_unless_it_starts_a_value() {
        assert_eq!(
            lex("-5 c.stability>=-2 tag=-math"),
            vec![
                (TokenKind::Minus, "-"),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "5"),
                (TokenKind::And, ""),
                (TokenKind::Field, "c.stability"),
                (TokenKind::GreaterThanEqual, ">="),
                (TokenKind::Integer(-2), "-2"),
                (TokenKind::And, ""),
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "-math"),
            ]
        );
    }

    #[test]
    fn not_and_not_equal() {
        assert_eq!(
            lex("not tag=math c.state!=0 NOT dog tag=not"),
            vec![
                (TokenKind::Minus, "not"),
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "math"),
                (TokenKind::And, ""),
                (TokenKind::Field, "c.state"),
                (TokenKind::NotEqual, "!="),
                (TokenKind::Integer(0), "0"),
                (TokenKind::And, ""),
                (TokenKind::Minus, "NOT"),
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "dog"),
                (TokenKind::And, ""),
                (TokenKind::Field, "tag"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "not"),
            ]
        );
    }

    #[test]
    fn spaces_around_operators() {
        assert_eq!(
            lex("c.due <= today tag = math"),
            lex("c.due<=today tag=math")
        );
        assert_eq!(lex("c.state != new"), lex("c.state!=new"));
        assert_eq!(lex("c.suspended = false"), lex("c.suspended=false"));
        assert_eq!(lex("c.stability >= -2"), lex("c.stability>=-2"));
    }

    #[test]
    fn sorts_list_fields() {
        assert_eq!(
            lex("sort_by_asc=c.due,c.stability"),
            vec![
                (TokenKind::Field, "sort_by_asc"),
                (TokenKind::Equal, "="),
                (TokenKind::String, "c.due,c.stability"),
            ]
        );
        assert!(
            Lexer::new("dog,cat")
                .collect::<Result<Vec<_>, _>>()
                .is_err()
        );
    }

    #[test]
    fn boolean_field_alone_means_true() {
        assert_eq!(
            lex("dog -c.user_buried card.suspended"),
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "dog"),
                (TokenKind::And, ""),
                (TokenKind::Minus, "-"),
                (TokenKind::Field, "c.user_buried"),
                (TokenKind::Equal, ""),
                (TokenKind::True, ""),
                (TokenKind::And, ""),
                (TokenKind::Field, "card.suspended"),
                (TokenKind::Equal, ""),
                (TokenKind::True, ""),
            ]
        );
        // Quoted, it is still text.
        assert_eq!(
            lex("\"c.suspended\""),
            vec![
                (TokenKind::Field, ""),
                (TokenKind::Tilde, ""),
                (TokenKind::String, "c.suspended"),
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
