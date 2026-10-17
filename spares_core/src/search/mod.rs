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
//! | `c.waiting_on_links`  | bool     |
//! | c.state             | u32      |
//! | `c.custom_data`       | Json     |
//! | `linked_to_note`      | i64      |
//! | `linked_to_keyword`   | String   |
//! | `linked_from_note`    | i64      |
//! | c.rated             | u32      |
//! | c.count             | u32      |
//! | c.cloze             | String   |
//!
//! `c.waiting_on_links` matches the cards that `new_after_linked_notes` and
//! `reviews_after_linked_notes` (see `QueueConfig` in `spares_core::config`) hold back today, so
//! `spares card search 'c.waiting_on_links=true'` shows why a card is not coming up. It matches
//! nothing while both are off.
//!
//! `c.rated` matches against graded reviews only. Forgetting a card is recorded in the review log too,
//! but it carries no rating, so it never makes a card match `c.rated`. Neither does a cram (a
//! rating in a filtered tag that does not reschedule), since it is not a review the card's
//! schedule is based on.
//!
//! ## Types
//!
//! ### Strings
//!
//! **Unquoted Strings:**
//! - Words made of letters, digits, `.`, `_`, `-` and `+` that don't start with `-` do not need
//!   quotes. Without a field, a number or date like `2024` is a word too.
//! - `and`, `or`, `AND`, `OR`, `true` and `false` are keywords only as whole words, so `orbit` is
//!   an ordinary word. Quote a keyword to search for it, as in `"or"`.
//!
//! **Quoted Strings:**
//! - Strings with non-alphanumeric characters must be quoted.
//! - Quotes can be escaped with a backslash.
//! - Alternatively, use `#"` and `"#` to delimit strings, where double quotes inside do not require escaping.
//! - Curly double quotes (`“` and `”`, as inserted by phone keyboards) may be used in place of `"`.
//!
//! **Regex String:**
//! - Strings delimited by `re:"` and `"`
//!
//! #### Tilde Operator (`~`)
//!
//! **With a normal String**:
//! - `field~"value"`: Searches if `field` contains `value` (case insensitive for ASCII letters).
//!   Every character is matched literally, including `%` and `_`.
//!   - Example: `tag~"math test"` finds notes with tags containing `math test`.
//!
//! **With a Regex String**:
//! - `field~re:"value"`: Searches if `field` matches the regex `value` (case sensitive).
//! - This uses the [`regex`](https://docs.rs/regex/latest/regex/) crate.
//!
//! #### Exact Match (`=`)
//! - `field="value"`: Searches for notes where `field` is exactly `value`.
//!   - Example: `keyword="Integration by parts"` finds notes with that keyword.
//! - `tag=value` finds notes and cards tagged `value` or one of its descendants (case insensitive
//!   for ASCII letters).
//!   - Example: `tag=math` finds the tags `math` and `math:algebra`, but not `physics:math` or
//!     `mathematics`.
//!
//! #### Default Field
//! - If no field is specified, `data~` is used by default, so a bare word searches the note's
//!   text.
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
//! - `c.suspended=true` or `c.suspended`: Returns suspended cards.
//! - `c.suspended=false` or `-c.suspended=true`: Returns non-suspended cards.
//!
//! ### Dates
//!
//! **Formats:**
//! - `YYYY-MM-DD`: a whole day, in local time.
//! - `YYYY-MM-DDTHH:MM:SSZ`: an exact time, in UTC.
//!
//! **Operators:**
//! - `=`
//! - `>`
//! - `>=`
//! - `<`
//! - `<=`
//!
//! Compared against a day, `=` matches any time in it, `>` is after the day and `<=` is up to the
//! end of it, so `created_at=2026-10-17` finds notes created any time that day.
//!
//! #### Relative dates
//!
//! In place of a fixed date, a date field can be compared against a date that is worked out when
//! the query runs:
//! - Another note's timestamp: `note[<id>].created_at` or `note[<id>].updated_at`, where `<id>` is
//!   the note's letters.
//! - A card's timestamp: `card[<id>].created_at`, `card[<id>].updated_at` or `card[<id>].due`.
//! - `today`: the current day, in local time.
//! - `now`.
//!
//! Each can be followed by one or more offsets, written with no spaces: `+` or `-`, a number, and a
//! unit of `m` (minutes), `h` (hours), `d` (days) or `w` (weeks), as in `-5d` or `+1w-12h`. Days
//! and weeks are calendar days in local time, so `today-1d` is the start of yesterday even across
//! a daylight saving change. Minutes and hours are exact.
//!
//! `today` with only day and week offsets is a whole day, like `YYYY-MM-DD`, so `c.due<=today`
//! includes cards due later today and `created_at=today-1d` finds notes created yesterday.
//!
//! A query that refers to a note or card that doesn't exist is an error. `today` and `now` are only
//! dates when compared against a date field, so `today` and `data~now` still search for the words.
//!
//! **Examples:**
//! - `created_at<=note[gir].created_at and created_at>=note[gir].created_at-5d`: notes created in
//!   the 5 days up to note `gir`.
//! - `c.due<=today+3d`: cards due by the end of the day 3 days from now.
//! - `updated_at>=now-12h`: notes edited in the last 12 hours.
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
//! - `-custom_data:"$.a.b"=true`
//! - `custom_data:"$.array[0].key">=1`
//!
//! ### Sorting
//!
//! - Use sorting keys to order results by numeric or `DateTime` fields:
//!   - Ascending: `sort_by_asc=created_at`
//!   - Descending: `sort_by_desc=c.stability`
//! - Supported sortable fields include `id`, `created_at`, `updated_at`, `linked_to_note`, `linked_from_note`, `linked_to_keyword`, and all numeric card fields like `c.id`, `c.created_at`, `c.updated_at`, `c.due`, `c.stability`, `c.difficulty`, `c.desired_retention`, `c.state`, `c.rated`, and computed `c.count`.
//! - Multiple sorts are allowed; later keys are appended to the ORDER BY list.
//! - When reviewing (`spares card review --query`, or a filtered tag's query), a sort outside any
//!   limited branch sets the order new and review cards are shown in, together, in place of the
//!   `review_order`, `new_order` and `new_review_mix` settings of `[queue]` in `config.toml` (see
//!   `QueueConfig` in `spares_core::config`). Learning cards whose step is over still come first.
//!   - Example: `tag=chess sort_by_desc=c.difficulty` reviews the hardest chess cards first.
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
//!   cards due the longest (`c.due`, then the note's `created_at`), or, when searching notes, the
//!   lowest note ids. A sort inside a branch does not change the order cards are reviewed in.
//!   - Example: `(tag=chess sort_by_desc=c.difficulty limit=5) or (tag=math limit=5)`
//! - A limit outside the parentheses still caps the combined result:
//!   `((tag=a limit=5) or (tag=b limit=5)) limit=8`.
//! - A sort or limit applies to the parentheses it is written in, or to the whole query, wherever
//!   it appears in them, so the limited conditions must be wrapped in parentheses:
//!   `dog limit=5 or cat` limits the whole `dog or cat` to 5.
//! - A parenthesized `or` branch with a sort but no limit is an error.
//! - `c.cloze` cannot be used inside a limited branch, since cloze matching runs after the
//!   database query.
//!
//! #### Limits when reviewing
//!
//! When `spares card review --query` uses `limit`, the limits count only the cards due today, and
//! the cards they pick are saved to a filtered tag named `review-<hash of the query>`:
//! - Running the same query again later that day resumes that set, including after finishing it,
//!   so finishing doesn't make room for more cards.
//! - The first run on a later day rebuilds the set from the cards due that day.
//! - Explicit limits take precedence over the daily new-card limit, which only stops new cards once
//!   it has already been reached today.
//! - `spares card review --tag-name review-<hash>` also resumes the set, but doesn't rebuild it on a
//!   new day.
//!
//! ### Other Operators
//!
//! - **Exclusion:** `-QUALIFIER`
//!   - Example: `-tag=math`
//! - **Logical Operators:** `and`, `or` (also `AND`, `OR`). `and`, whether written or implied by a
//!   space, binds tighter than `or`, so `a or b c` means `a or (b and c)`. Before October 2026 they
//!   bound equally from left to right, so a query mixing them without parentheses may now match
//!   differently.
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
//! - `dog` is equivalent to `data~dog` and `data~"dog"`.
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
use std::fmt::Write as _;
use std::ops::Range;

use chrono::DateTime;
use chrono::NaiveDate;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

pub(crate) mod evaluator;
pub(crate) mod lexer;
mod parser;
mod relative_date;

use relative_date::RelativeDate;

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
            Cow::Owned(
                s.replace("\\\"", "\"")
                    .replace("\\\u{201C}", "\u{201C}")
                    .replace("\\\u{201D}", "\u{201D}"),
            )
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
    /// A date resolved when the query runs, like `note[gir].created_at-5d`. See
    /// [`RelativeDate`].
    RelativeDate,
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

impl TokenKind {
    /// How the token is described in an error, like ``a field`` or ``` `(` ```.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            TokenKind::Field => "a field",
            TokenKind::String => "a word",
            TokenKind::Integer(_) | TokenKind::Float(_) => "a number",
            TokenKind::True => "`true`",
            TokenKind::False => "`false`",
            TokenKind::Date | TokenKind::RelativeDate => "a date",
            TokenKind::Regex => "a regex",
            TokenKind::And => "`and`",
            TokenKind::Or => "`or`",
            TokenKind::Minus => "`-`",
            TokenKind::Equal => "`=`",
            TokenKind::GreaterThan => "`>`",
            TokenKind::GreaterThanEqual => "`>=`",
            TokenKind::LessThan => "`<`",
            TokenKind::LessThanEqual => "`<=`",
            TokenKind::Colon => "`:`",
            TokenKind::Tilde => "`~`",
            TokenKind::LeftParen => "`(`",
            TokenKind::RightParen => "`)`",
        }
    }
}

/// Renders an error in `query` as plain text: what went wrong, the line of the query it is on with
/// the place marked, and any help. Context added with `wrap_err`, like "in bracketed expression",
/// is left out, since the marked place says where the problem is.
pub(crate) fn render_error(query: &str, report: &miette::Report) -> String {
    let mut out = report
        .chain()
        .last()
        .map_or_else(|| report.to_string(), ToString::to_string);
    if report.source_code().is_some() {
        for label in report.labels().into_iter().flatten() {
            let offset = label.offset().min(query.len());
            let line_start = query[..offset].rfind('\n').map_or(0, |i| i + 1);
            let line_end = query[offset..]
                .find('\n')
                .map_or(query.len(), |i| offset + i);
            let marked_end = (offset + label.len()).min(line_end);
            let column = query[line_start..offset].chars().count();
            let width = query[offset..marked_end].chars().count().max(1);
            let _ = write!(
                out,
                "\n\n    {}\n    {}{} {}",
                &query[line_start..line_end],
                " ".repeat(column),
                "^".repeat(width),
                label.label().unwrap_or_default(),
            );
        }
    }
    if let Some(help) = report.help() {
        let _ = write!(out, "\n\nhelp: {help}");
    }
    out
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
    /// A date without a time, like `2026-10-17`, which stands for that whole local day.
    Date(NaiveDate),
    RelativeDate(RelativeDate),
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
            Atom::Date(d) => write!(f, "{d}"),
            Atom::RelativeDate(d) => write!(f, "{d}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn render(query: &str) -> String {
        let report = parser::Parser::new(query).parse_expression().unwrap_err();
        render_error(query, &report)
    }

    #[test]
    fn errors_mark_where_the_problem_is() {
        assert_eq!(
            render("tag=math and (dog"),
            "Missing `)`\n\n    tag=math and (dog\n                 ^ this `(` is never closed"
        );
        assert_eq!(
            render("dog ) cat"),
            "This `)` has no matching `(`\n\n    dog ) cat\n        ^ unexpected `)`"
        );
        assert_eq!(
            render("tag="),
            "The query ends too early\n\n    tag=\n        ^ expected a search term here"
        );
        assert_eq!(
            render("a\nor or b"),
            "Expected a search term\n\n    or or b\n       ^^ found `or`"
        );
    }
}
