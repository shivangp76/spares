use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt, ops::Range};

pub mod evaluator;
pub mod lexer;
mod parser;

/// Design note: There is no need to store the token's value here. Value parsing
/// is done in the parser, and the value is stored in the Abstract Syntax Tree.
/// It is memory inefficient to add a new "value" field here.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token kind.
    pub kind: TokenKind,
    /// Byte offset range of the token in the source code.
    pub span: Range<usize>,
}

impl Token {
    pub fn unescape(s: &str) -> Cow<'_, str> {
        if s.contains('\\') {
            Cow::Owned(s.replace("\\\"", "\""))
        } else {
            Cow::Borrowed(s)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, strum::Display)]
pub enum TokenKind {
    // Field identifiers
    Field,
    // Literals
    String,
    Integer(i64),
    Float(f64),
    True,
    False,
    Date,
    // Operators
    And,
    Or,
    Minus,
    Equal,
    GreaterThan,
    GreaterThanEqual,
    LessThan,
    LessThanEqual,
    Colon,
    Tilde,
    // Grouping
    LeftParen,
    RightParen,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Deserialize, Serialize)]
pub enum QueryReturnItemType {
    Cards,
    Notes,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    And,
    Or,
    Minus,
    Equal,
    GreaterThan,
    GreaterThanEqual,
    LessThan,
    LessThanEqual,
    Colon,
    Tilde,
    Group, // parens
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Op::And => "and",
                Op::Or => "or",
                Op::Minus => "not",
                Op::Equal => "=",
                Op::GreaterThan => ">",
                Op::GreaterThanEqual => ">=",
                Op::LessThan => "<",
                Op::LessThanEqual => "<=",
                Op::Colon => ":",
                Op::Tilde => "~",
                Op::Group => "group",
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Atom<'de> {
    // Field identifiers
    Field(Cow<'de, str>),
    // Literals
    String(Cow<'de, str>),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Nil,
}

impl fmt::Display for Atom<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Atom::Field(s) => write!(f, "{s}"),
            Atom::String(s) => write!(f, "\"{s}\""),
            Atom::Integer(n) => write!(f, "{n}"),
            Atom::Float(n) => write!(f, "{n}"),
            Atom::Boolean(b) => write!(f, "{b:?}"),
            Atom::DateTime(d) => write!(f, "{d:?}"),
            Atom::Nil => write!(f, "nil"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenTree<'de> {
    Atom(Atom<'de>),
    Cons(Op, Vec<TokenTree<'de>>),
}

impl fmt::Display for TokenTree<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenTree::Atom(i) => write!(f, "{}", i),
            TokenTree::Cons(head, rest) => {
                write!(f, "({}", head)?;
                for s in rest {
                    write!(f, " {s}")?;
                }
                write!(f, ")")
            }
        }
    }
}
