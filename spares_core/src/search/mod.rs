//! # Searching
//!
//! | Field               | Type     |
//! |---------------------|----------|
//! | id                  | i64      |
//! | data                | String   |
//! | `created_at`          | `DateTime` |
//! | `updated_at`          | `DateTime` |
//! | `parser_name`         | String   |
//! | tag                 | String   |
//! | keyword             | String   |
//! | `custom_data`         | Json     |
//! | c.id                | i64      |
//! | `c.created_at`        | `DateTime` |
//! | `c.updated_at`        | `DateTime` |
//! | c.due               | `DateTime` |
//! | c.stability         | f64      |
//! | c.difficulty        | f64      |
//! | `c.desired_retention` | f64      |
//! | c.suspended         | bool     |
//! | `c.user_buried`       | bool     |
//! | `c.scheduler_buried`  | bool     |
//! | c.state             | u32      |
//! | `c.custom_data`       | Json     |
//! | `linked_to_note`      | i64      |
//! | `linked_to_keyword`   | String   |
//! | `linked_from_note`    | i64      |
//! | c.rated             | u32      |
//! | c.count             | u32      |
//! | c.cloze             | String   |
//!
//! `c.rated` matches against graded reviews only. Forgetting a card is recorded in the review log too,
//! but it carries no rating, so it never makes a card match `c.rated`.
//!
//! ## Types
//!
//! ### Strings
//!
//! **Unquoted Strings:**
//! - Strings containing only alphanumeric characters (`a-z`, `A-Z`, `0-9`) do not need quotes.
//!
//! **Quoted Strings:**
//! - Strings with non-alphanumeric characters must be quoted.
//! - Quotes can be escaped with a backslash.
//! - Alternatively, use `#"` and `"#` to delimit strings, where double quotes inside do not require escaping.
//!
//! **Regex String:**
//! - Strings delimited by `re:"` and `"`
//!
//! #### Tilde Operator (`~`)
//!
//! **With a normal String**:
//! - `field~"value"`: Searches if `field` contains `value` (case insensitive).
//!   - Example: `tag~"math test"` finds notes with tags containing `math test`.
//!
//! **With a Regex String**:
//! - `field~re:"value"`: Searches if `field` matches the regex `value` (case sensitive).
//! - This uses the [`regex`](https://docs.rs/regex/latest/regex/) crate.
//!
//! #### Exact Match (`=`)
//! - `field="value"`: Searches for notes where `field` matches `value` exactly and `value` appears in the note's body.
//!   - Example: `tag="math test"` finds notes tagged `math test` with `test` in the body.
//!
//! #### Default Field
//! - If no field is specified, `data` is used by default.
//!   - Example: `-dog` or `-"dog"` finds notes not containing `dog`.
//!   - Example: `"-dog"` finds notes containing `-dog`. This follows the quoting rule for non-alphanumeric characters like `-`.
//!
//! ### Numbers
//!
//! Supported Rust types:
//! - `i64`
//! - `f64`
//! - `u32`
//!
//! **Operators:**
//! - `=`
//! - `>`
//! - `>=`
//! - `<`
//! - `<=`
//!
//! **Example:**
//! - `id>=5`
//!
//! ### Booleans
//!
//! **Example:**
//! - `c.suspended=true`: Returns suspended cards.
//! - `c.suspended=false` or `-c.suspended=true`: Returns non-suspended cards.
//!
//! ### Dates
//!
//! **Formats:**
//! - `YYYY-MM-DD`
//! - `YYYY-MM-DDTHH:MM:SSZ`
//!
//! **Operators:**
//! - `=`
//! - `>`
//! - `>=`
//! - `<`
//! - `<=`
//!
//! ### JSON
//!
//! JSON data can be queried using [JSONPath syntax](https://jsonpath.com/). The query result must be a boolean, number, or string. Use corresponding operators to filter results.
//!
//! **Example JSON:**
//! ```json
//! {
//!     "x": {
//!         "y": ["z", "zz"]
//!     },
//!     "a": {
//!         "b": false
//!     },
//!     "array": [
//!         {
//!             "key": 1
//!         }
//!     ]
//! }
//! ```
//!
//! **Examples:**
//! - `custom_data:"$.x.y[1]"=zz`
//! - `-custom_data:"$.a.b"`
//! - `custom_data:"$.array[0].key">=1`
//!
//! ### Sorting
//!
//! - Use sorting keys to order results by numeric or `DateTime` fields:
//!   - Ascending: `sort_by_asc=created_at`
//!   - Descending: `sort_by_desc=c.stability`
//! - Supported sortable fields include `id`, `created_at`, `updated_at`, `linked_to_note`, `linked_from_note`, `linked_to_keyword`, and all numeric card fields like `c.id`, `c.created_at`, `c.updated_at`, `c.due`, `c.stability`, `c.difficulty`, `c.desired_retention`, `c.state`, `c.rated`, and computed `c.count`.
//! - Multiple sorts are allowed; later keys are appended to the ORDER BY list.
//!
//! ### Limit
//!
//! - Use `limit=N` to return at most `N` results, where `N` is a non-negative integer.
//!   - Example: `dog limit=5` returns at most 5 notes containing `dog`.
//! - The limit is applied after filtering and sorting, so it combines with sorting to get the "top N" results.
//!   - Example: `c.stability>=2 sort_by_desc=c.due limit=100`
//! - It applies to whatever is being searched (notes or cards).
//! - Restrictions:
//!   - Only the `=` operator is allowed (`limit>=10` and `limit~10` are errors).
//!   - It can only be specified once per group (`limit=10 limit=20` is an error).
//!   - It cannot be negated (`-limit=10` and `-(dog limit=5)` are errors).
//!
//! #### Limits per `or` branch
//!
//! - Each parenthesized branch of an `or` can have its own limit, and each branch keeps at most
//!   that many results.
//!   - Example: `(tag=chess limit=5) or (tag="measure-theory:grad" limit=30)`
//! - Combine this with `c.state` to limit by card state. FSRS states are `0` (new), `1` (learning),
//!   `2` (review), and `3` (relearning).
//!   - Example: `(tag=a c.state=0 limit=10) or (tag=a c.state=2 limit=10) or (tag=b c.state=0 limit=20)`
//! - A sort inside a branch decides which results its limit keeps. Without one, a branch keeps the
//!   cards review would show first (`c.due`, then the note's `created_at`), or, when searching
//!   notes, the lowest note ids.
//!   - Example: `(tag=chess sort_by_desc=c.difficulty limit=5) or (tag=math limit=5)`
//! - A limit outside the parentheses still caps the combined result:
//!   `((tag=a limit=5) or (tag=b limit=5)) limit=8`.
//! - The limited conditions must be wrapped in parentheses: `dog limit=5 or cat` is an error.
//! - A sort in an `or` branch without a limit is still an error.
//! - `c.cloze` cannot be used inside a limited branch, since cloze matching runs after the
//!   database query.
//!
//! ### Other Operators
//!
//! - **Exclusion:** `-QUALIFIER`
//!   - Example: `-tag=math`
//! - **Logical Operators:** `and`, `or`
//! - **Grouping:** Use parentheses for grouping expressions.
//!
//! ## Examples
//!
//! **Search for notes containing "dog"**
//! - `dog`
//!
//! **Search for notes containing "multiword \"query" with an escaped quote inside**
//! - `"multiword \"query"`
//!
//! **Search for notes NOT containing "dog"**
//! - `-dog`
//!
//! **Search for notes containing both "dog" AND "cat"**
//! - `dog and cat`
//!
//! **Search for notes containing "dog" OR "cat"**
//! - `dog or cat`
//!
//! **Search for "dog" AND either "cat" OR "mouse"**
//! - `dog and (cat or mouse)`
//!
//! **Search for notes with the tag "math"**
//! - `tag=math`
//!
//! **Search for notes with the tag "-math"**
//! - `tag=-math`
//!
//! **Search for notes NOT tagged "math"**
//! - `-tag=math`
//!
//! **Search for notes tagged exactly `a`, with no descendants. In other words, notes tagged `a:b` won't match**
//! - `spares note search 'tag~re:"^a$"'`
//!
//! **Search for cards with stability ≥ 2**
//! - `c.stability>=2`
//!
//! **Get the 10 most recently created notes tagged "math"**
//! - `tag=math sort_by_desc=created_at limit=10`
//!
//! ### Equivalences
//! - `dog` is equivalent to `data=dog` and `data="dog"`.
//! - `-cat -mouse` is equivalent to `-(cat or mouse)` (De Morgan's Laws).
//! - `dog cat` is equivalent to `dog and cat`. A space between terms implies an `and` operator unless part of a quoted string.
//!
//! ## Inspiration
//! - <https://github.com/github/docs/blob/main/content/search-github/getting-started-with-searching-on-github/understanding-the-search-syntax.md>
//! - <https://support.zendesk.com/hc/en-us/articles/4408835086106-Using-Zendesk-Support-advanced-search>
//! - <https://support.atlassian.com/trello/docs/searching-for-cards-all-boards/>
//! - <https://docs.ankiweb.net/searching.html>

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

pub(crate) mod evaluator;
pub(crate) mod lexer;
mod parser;

/// Design note: There is no need to store the token's value here. Value parsing
/// is done in the parser, and the value is stored in the Abstract Syntax Tree.
/// It is memory inefficient to add a new "value" field here.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    /// Token kind.
    pub(crate) kind: TokenKind,
    /// Byte offset range of the token in the source code.
    pub(crate) span: Range<usize>,
}

impl Token {
    pub(crate) fn unescape(s: &str) -> Cow<'_, str> {
        if s.contains('\\') {
            Cow::Owned(s.replace("\\\"", "\""))
        } else {
            Cow::Borrowed(s)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, strum::Display)]
pub(crate) enum TokenKind {
    // Field identifiers
    Field,
    // Literals
    String,
    Integer(i64),
    Float(f64),
    True,
    False,
    Date,
    Regex,
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

/// Returns true if `query` uses `limit` anywhere, either globally or in a limited `or` branch. An
/// unparseable query returns false; evaluating it reports the parse error.
pub fn query_has_limit(query: &str) -> bool {
    parser::Parser::new(query)
        .parse_expression()
        .is_ok_and(|tree| evaluator::tree_has_limit(&tree))
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Deserialize, Serialize)]
pub enum QueryReturnItemType {
    Cards,
    Notes,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Op {
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
pub(crate) enum Atom<'de> {
    // Field identifiers
    Field(Cow<'de, str>),
    // Literals
    String(Cow<'de, str>),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Regex(Cow<'de, str>),
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
            Atom::Regex(s) => write!(f, "re:\"{s}\""),
            Atom::Nil => write!(f, "nil"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenTree<'de> {
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
