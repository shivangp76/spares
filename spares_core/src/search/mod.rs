//! # Searching
//!
//! `spares note search --fields` lists the fields with their kind and an example, and
//! `spares note search --examples` example queries (see [`examples`]). The web app's search box
//! shows both under "Syntax".
//! `spares note search --explain '<query>'` (or `spares card search --explain`) prints how the
//! query was parsed, with `and` and `or` grouped as they apply, and the SQL it runs, without
//! searching. A card state in the SQL is followed by its name, as in `c.state = 2 /* review */`.
//!
//! `spares saved-search save <name> '<query>'` saves a query by name, and `spares note search --saved
//! <name>` (or `spares card search --saved`) runs it. The web app's notes and cards pages list the
//! same saved searches, and save new ones. See [`crate::api::saved_search`].
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
//! | c.postponed         | bool     |
//! | c.reviewed          | `DateTime` |
//! | c.reps              | u32      |
//! | c.lapses            | u32      |
//! | c.leech             | bool     |
//! | c.interval          | f64      |
//! | c.retrievability    | f64      |
//! | c.count             | u32      |
//! | c.cloze             | String   |
//!
//! `c.waiting_on_links` matches the cards that `new_after_linked_notes` and
//! `reviews_after_linked_notes` (see `QueueConfig` in `spares_core::config`) hold back today, so
//! `spares card search 'c.waiting_on_links=true'` shows why a card is not coming up. It matches
//! nothing while both are off.
//!
//! `c.state` is the card's FSRS state, written by name or number: `new` (0), `learning` (1),
//! `review` (2) or `relearning` (3), as in `c.state=review` or `c.state!=new`. Names are matched
//! ignoring case.
//!
//! `c.rated` matches against graded reviews only. Forgetting a card is recorded in the review log too,
//! but it carries no rating, so it never makes a card match `c.rated`. Neither does a cram (a
//! rating in a filtered tag that does not reschedule), since it is not a review the card's
//! schedule is based on.
//!
//! ### Review history
//!
//! These fields are worked out from the card's reviews. Like `c.rated`, they leave out crams, and
//! a card that was never reviewed has no `c.reviewed`, `c.interval` or `c.retrievability`, so it
//! matches none of their comparisons, and `-` matches it.
//! - `c.reviewed`: when the card was last reviewed, a date, as in `c.reviewed>=today-7d`.
//! - `c.reps`: how many times it was reviewed.
//! - `c.lapses`: how many times it was forgotten (failed a review, by the ratings of the scheduler
//!   that rated it) after it had been learned, counted as for leeches.
//! - `c.leech`: whether it lapsed at least `lapses_threshold` times (see `LeechConfig` in
//!   `spares_core::config`), the cards `spares card leeches` lists, but suspended and buried ones
//!   too. Unlike `tag=leech`, which is on the note, it matches only the card, and it doesn't
//!   depend on the leech `action`: `c.leech -c.suspended` is the leeches still being reviewed.
//! - `c.interval`: the days from its last review to when it is due, as in statistics, as in
//!   `c.interval>=30`. A postponed card's interval includes the postponement.
//! - `c.retrievability`: how likely you are to recall it now, from 0 to 1, as the scheduler that
//!   last reviewed it predicts (FSRS from its stability and the time since its last review), as in
//!   `c.retrievability<0.8`. Cards of a scheduler with no forgetting curve have none. Sorting
//!   orders cards by it only among those of the same scheduler.
//!
//! A time in brackets after `c.rated` limits it to the reviews at that time: a date, which means
//! any time in it, or a comparison with a date, written with no spaces. With no value, it is any
//! rating:
//! - `c.rated[today]`: cards reviewed today.
//! - `c.rated[>=today-7d]=1`: cards forgotten in the last week.
//! - `c.rated[2026-10]>=3`: cards rated good or easy in October 2026.
//!
//! `c.postponed` matches the cards whose due date was moved later without a review, by
//! `spares card postpone` (see `ReviewLogKind::Reschedule` in `spares_core::model`), and like
//! `c.rated` it takes a time in brackets, which means the postponement was then, like Anki's
//! `resched:n`:
//! - `c.postponed[today]`: the cards postponed today, as in `c.postponed[today] tag=exam` to find
//!   the exam cards among them.
//! - `-c.postponed[>=today-30d]`: cards not postponed in the last month.
//!
//! A postponement or advance is not a review, so it doesn't count toward `c.rated`, `c.reviewed`
//! or `c.reps`.
//!
//! Sorting by `c.reviewed`, `c.interval` or `c.retrievability` puts the cards without one last,
//! so `sort_by_asc=c.retrievability limit=20` is the 20 cards you are least likely to recall.
//!
//! ## Types
//!
//! ### Strings
//!
//! **Unquoted Strings:**
//! - Words made of letters, digits, `.`, `_`, `-` and `+` that don't start with `-` do not need
//!   quotes. Without a field, a number or date like `2024` is a word too.
//! - `and`, `or`, `not`, `AND`, `OR`, `NOT`, `true` and `false` are keywords only as whole words,
//!   so `orbit` is an ordinary word. Quote a keyword to search for it, as in `"or"`.
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
//! - `field~"value"`: Searches if `field` contains `value`, ignoring case and accents in any
//!   script, so `cafe` and `CAFÉ` both find `Café`. Every other character is matched literally,
//!   including `%`, `_` and `.`.
//!   - Example: `tag~"world history"` finds notes with tags containing `world history`.
//!
//! **With a Regex String**:
//! - `field~re:"value"`: Searches if `field` matches the regex `value` (case sensitive).
//! - This uses the [`regex`](https://docs.rs/regex/latest/regex/) crate.
//!
//! #### Exact Match (`=`)
//! - `field="value"`: Searches for notes where `field` is exactly `value`.
//!   - Example: `keyword="Irregular verbs"` finds notes with that keyword.
//! - `tag=value` finds notes and cards tagged `value` or one of its descendants (case insensitive
//!   for ASCII letters).
//!   - Example: `tag=history` finds the tags `history` and `history:rome`, but not `art:history`
//!     or `prehistory`.
//!
//! **Words and quoted text in the note's text:**
//! - A value without quotes, like `data~verb`, finds words that start with it: `verb`, `verbs` and
//!   `verbal`, but not `adverb`. Quoted, as in `data~"verb"`, it is found anywhere, and
//!   `data~"past tense"` finds that phrase. This applies to `data` and `c.cloze`. For other fields,
//!   like `tag~history`, the value is found anywhere either way.
//!
//! #### Default Field
//! - If no field is specified, `data~` is used by default, so a bare word searches the note's
//!   text for words starting with it.
//!   - Example: `-verb` or `-"verb"` finds notes not containing `verb`.
//!   - Example: `"-ing"` finds notes containing `-ing`. This follows the quoting rule for non-alphanumeric characters like `-`.
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
//! - `!=`
//! - `>`
//! - `>=`
//! - `<`
//! - `<=`
//!
//! Numbers with a fraction are equal when they are within a millionth of each other (relative to
//! the larger of the value and 1), since a stored `0.9` may be `0.8999999761581421`.
//!
//! **Example:**
//! - `c.stability>=5`
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
//! - `YYYY-MM`: a whole month, in local time.
//! - `YYYY`: a whole year, in local time.
//! - `YYYY-MM-DDTHH:MM` or `YYYY-MM-DDTHH:MM:SS`: an exact time, in local time.
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
//! end of it, so `created_at=2026-10-17` finds notes created any time that day. A month or a year
//! works the same way: `created_at=2026-10` is any time in October 2026 and `c.due>2026` is after
//! the end of 2026. Against a text field, they are just text, so `keyword=2026` is the keyword
//! `2026`.
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
//! unit of `m` (minutes), `h` (hours), `d` (days), `w` (weeks), `mo` (months) or `y` (years), as in
//! `-5d`, `+1w-12h` or `-1mo`. Days, weeks, months and years follow the calendar in local time, so
//! `today-1d` is the start of yesterday even across a daylight saving change, and a month after
//! January 31st is the last day of February. Minutes and hours are exact.
//!
//! `today` with only calendar offsets (`d`, `w`, `mo` and `y`) is a whole day, like `YYYY-MM-DD`,
//! so `c.due<=today` includes cards due later today, `created_at=today-1d` finds notes created
//! yesterday and `created_at>=today-1mo` the ones created since this day last month.
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
//! A value in a note's custom data is written as a path after `custom_data` (or `c.custom_data`
//! for a card's), the way `c.due` and `note[gir].created_at` are: `.key` steps into an object and
//! `[n]` into an array. A key that isn't a plain word goes in quotes inside brackets, as in
//! `custom_data["a's key"]`. The path goes to SQLite's `json_extract`, which has only these two
//! kinds of steps, so there are no filters or `..`.
//!
//! Compare the value with the operators of its type. A path on its own means the key is there and
//! its value is not `false` or `null`, so `custom_data.flag` skips `"flag": false`, like a bare
//! `c.suspended` means `=true`, and `0` and `""` count as there. `-` matches the notes without the
//! key too: `-custom_data.flag` and `-custom_data.a.b=true` match a note with no custom data.
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
//!     ],
//!     "a's key": 2
//! }
//! ```
//!
//! **Examples:**
//! - `custom_data.x.y[1]=zz`
//! - `-custom_data.a.b=true`
//! - `custom_data.array[0].key>=1`
//! - `custom_data["a's key"]>=1`
//! - `custom_data.x`: the note has an `x`.
//! - `custom_data.card_key="preflop-hand|SB|K5o|default"`
//!
//! Before October 2026, paths were written `custom_data:"$.x.y[1]"`. That form is now an error.
//!
//! ### Links
//!
//! `linked_to_note=gir` finds the notes that link to note `gir`, and `linked_from_note=gir` the
//! notes `gir` links to. In place of a note, either can take a query in parentheses, which
//! searches notes, to find the notes linked to or from any note it matches:
//! - `linked_to_note=(tag=history)`: notes that link to a note tagged `history`.
//! - `linked_from_note=(keyword="Irregular verbs")`: notes linked from a note with that keyword.
//! - `-linked_to_note=(tag=history)`: notes that link to no note tagged `history`.
//! - `linked_to_note=(linked_to_note=(tag=history))`: queries nest, so this is notes that link to
//!   a note that links to a note tagged `history`.
//!
//! The query in parentheses can't have a sort, a limit or `c.cloze`, and no other field takes one.
//!
//! ### Sorting
//!
//! - Use sorting keys to order results by numeric or `DateTime` fields:
//!   - Ascending: `sort_by_asc=created_at`
//!   - Descending: `sort_by_desc=c.stability`
//! - Every field of numbers, dates or text can be sorted by, except `c.rated` and `c.cloze`.
//!   Sorting by a field with several values, like `tag`, uses the smallest.
//! - Multiple sorts are allowed; later keys are appended to the ORDER BY list. One sort can also
//!   list several fields, separated by `,`: `sort_by_asc=c.due,c.stability` is the same as
//!   `sort_by_asc=c.due sort_by_asc=c.stability`.
//! - Without a sort, notes come in the order of their ids, and cards in the order review shows them
//!   by default: the cards due the longest first (`c.due`, then the note's `created_at`). Results a
//!   sort ranks the same come in the order of their ids, so the order, and what a limit keeps, is
//!   the same each time.
//! - Notes found by words or quoted text, with no sort, come in order of relevance instead: the
//!   notes with more of the words, and with the rarer ones, first, by SQLite's full-text search
//!   (`bm25`), with a word in a note's keywords counting double. So `Paris limit=5` keeps the 5
//!   notes that match `Paris` best. Words under `-` don't count, and nor do regexes. The ranking
//!   only orders the notes the search finds: which notes match is decided as described above.
//! - Cards found by words come in the order of their notes' relevance when they are shown as
//!   results (`spares card search`, the web app's cards page), with the review order for cards
//!   that rank the same. Reviews, filtered tags and the other card commands keep the review order,
//!   and so do queries with `c.cloze`.
//! - When reviewing (`spares card review --query`, or a filtered tag's query), a sort outside any
//!   limited branch sets the order new and review cards are shown in, together, in place of the
//!   `review_order`, `new_order` and `new_review_mix` settings of `[queue]` in `config.toml` (see
//!   `QueueConfig` in `spares_core::config`). Learning cards whose step is over still come first.
//!   - Example: `tag=chess sort_by_desc=c.difficulty` reviews the hardest chess cards first.
//!
//! ### Limit
//!
//! - Use `limit=N` to return at most `N` results, where `N` is a non-negative integer.
//!   - Example: `Paris limit=5` returns at most 5 notes containing `Paris`.
//! - The limit is applied after filtering and sorting, so it combines with sorting to get the "top N" results.
//!   - Example: `c.stability>=2 sort_by_desc=c.due limit=100`
//! - It applies to whatever is being searched (notes or cards).
//! - Restrictions:
//!   - Only the `=` operator is allowed (`limit>=10` and `limit~10` are errors).
//!   - It can only be specified once per group (`limit=10 limit=20` is an error).
//!   - It cannot be negated (`-limit=10` and `-(Paris limit=5)` are errors).
//!
//! #### Limits per `or` branch
//!
//! - Each parenthesized branch of an `or` can have its own limit, and each branch keeps at most
//!   that many results.
//!   - Example: `(tag=chess limit=5) or (tag="spanish:verbs" limit=30)`
//! - Combine this with `c.state` to limit by card state.
//!   - Example: `(tag=a c.state=new limit=10) or (tag=a c.state=review limit=10) or (tag=b c.state=new limit=20)`
//! - A sort inside a branch decides which results its limit keeps. Without one, a branch keeps the
//!   cards due the longest (`c.due`, then the note's `created_at`), or, when searching notes, the
//!   lowest note ids. A sort inside a branch does not change the order cards are reviewed in.
//!   - Example: `(tag=chess sort_by_desc=c.difficulty limit=5) or (tag=history limit=5)`
//! - A limit outside the parentheses still caps the combined result:
//!   `((tag=a limit=5) or (tag=b limit=5)) limit=8`.
//! - A sort or limit applies to the parentheses it is written in, or to the whole query, wherever
//!   it appears in them, so the limited conditions must be wrapped in parentheses:
//!   `Paris limit=5 or Rome` limits the whole `Paris or Rome` to 5.
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
//! - **Exclusion:** `-QUALIFIER`, or `not QUALIFIER` (also `NOT`)
//!   - Example: `-tag=history` or `not tag=history`
//!   - `field!=value` is short for `-field=value`, as in `c.state!=0`.
//! - **Logical Operators:** `and`, `or` (also `AND`, `OR`). `and`, whether written or implied by a
//!   space, binds tighter than `or`, so `a or b c` means `a or (b and c)`. Before October 2026 they
//!   bound equally from left to right, so a query mixing them without parentheses may now match
//!   differently.
//! - **Grouping:** Use parentheses for grouping expressions.
//! - **Spaces** may go around a comparison, so `c.due <= today` is the same as `c.due<=today`.
//!
//! ## Examples
//!
//! **Search for notes with a word starting with "verb", like "verb" or "verbs"**
//! - `verb`
//!
//! **Search for notes containing "verb" anywhere, as in "adverb"**
//! - `"verb"`
//!
//! **Search for notes containing "multiword \"query" with an escaped quote inside**
//! - `"multiword \"query"`
//!
//! **Search for notes NOT containing "verb"**
//! - `-verb`
//!
//! **Search for notes containing both "capital" AND "France"**
//! - `capital and France`
//!
//! **Search for notes containing "Paris" OR "Rome"**
//! - `Paris or Rome`
//!
//! **Search for "capital" AND either "France" OR "Italy"**
//! - `capital and (France or Italy)`
//!
//! **Search for notes with the tag "spanish"**
//! - `tag=spanish`
//!
//! **Search for notes with the tag "-draft"**
//! - `tag=-draft`
//!
//! **Search for notes NOT tagged "spanish"**
//! - `-tag=spanish`
//!
//! **Search for notes tagged exactly `a`, with no descendants. In other words, notes tagged `a:b` won't match**
//! - `spares note search 'tag~re:"^a$"'`
//!
//! **Search for cards with stability ≥ 2**
//! - `c.stability>=2`
//!
//! **Get the 10 most recently created notes tagged "spanish"**
//! - `tag=spanish sort_by_desc=created_at limit=10`
//!
//! ### Equivalences
//! - `verb` is equivalent to `data~verb`, and `"verb"` to `data~"verb"`.
//! - `-Paris -Rome` is equivalent to `-(Paris or Rome)`.
//! - `capital France` is equivalent to `capital and France`. A space between terms implies an `and` operator unless part of a quoted string.
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
use chrono::NaiveDateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

pub(crate) mod evaluator;
mod examples;
pub(crate) mod lexer;
mod parser;
mod relative_date;

pub use examples::QueryExample;
pub use examples::examples;
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
    /// `!=`, which the parser turns into `-(field=value)`.
    NotEqual,
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
            TokenKind::NotEqual => "`!=`",
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

/// Where the first place an error in `query` marks is, as a start and end in UTF-16 code units.
pub(crate) fn error_span(query: &str, report: &miette::Report) -> Option<(usize, usize)> {
    report.source_code()?;
    let label = report.labels()?.next()?;
    let utf16_offset = |offset: usize| {
        let offset = offset.min(query.len());
        query
            .char_indices()
            .take_while(|(i, _)| *i < offset)
            .map(|(_, c)| c.len_utf16())
            .sum::<usize>()
    };
    Some((
        utf16_offset(label.offset()),
        utf16_offset(label.offset() + label.len()),
    ))
}

/// Returns true if `query` uses `limit` anywhere, either globally or in a limited `or` branch. An
/// unparseable query returns false; evaluating it reports the parse error.
pub fn query_has_limit(query: &str) -> bool {
    parser::Parser::new(query)
        .parse_expression()
        .is_ok_and(|tree| evaluator::tree_has_limit(&tree))
}

/// The parsed `query` and the SQL that runs it for `output_type`, to debug a query, as
/// `spares note search --explain` prints them. Nothing is read from the database, so a missing
/// note in `note[<id>]` is not reported.
pub fn explain(query: &str, output_type: QueryReturnItemType) -> Result<String, crate::Error> {
    // As `spares card search` runs it.
    evaluator::Evaluator::new(query)
        .ranked()
        .explain(match output_type {
            QueryReturnItemType::Notes => evaluator::EvaluatorReturnItemType::Notes,
            QueryReturnItemType::Cards => evaluator::EvaluatorReturnItemType::Cards,
        })
}

/// A field that queries can use, as listed by `spares note search --fields`.
#[derive(Debug, Clone, Serialize)]
pub struct FieldInfo {
    /// The name, as written in a query, like `c.due`.
    pub name: &'static str,
    /// The kind of value it holds, like `date`.
    pub kind: &'static str,
    /// A query that uses it, like `c.due<=today`.
    pub example: &'static str,
    /// What it matches.
    pub about: &'static str,
    /// Whether `sort_by_asc` and `sort_by_desc` can sort by it.
    pub sortable: bool,
    /// The values it can hold, if they are a few names, like `review` for `c.state`.
    pub values: Vec<&'static str>,
}

/// Every field that queries can use, in the order they are documented.
pub fn fields() -> Vec<FieldInfo> {
    evaluator::field_infos()
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
    /// Text written in quotes, like `"verb"`.
    String(Cow<'de, str>),
    /// Text written without quotes, like `verb`, which `data~` matches as the start of a word.
    Word(Cow<'de, str>),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    /// A date without a time, like `2026-10-17`, which stands for that whole local day.
    Date(NaiveDate),
    /// A time without a zone, like `2026-10-17T14:30`, in local time.
    LocalDateTime(NaiveDateTime),
    RelativeDate(RelativeDate),
    Regex(Cow<'de, str>),
    Nil,
}

impl fmt::Display for Atom<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Atom::Field(s) | Atom::Word(s) => write!(f, "{s}"),
            Atom::String(s) => write!(f, "\"{s}\""),
            Atom::Integer(n) => write!(f, "{n}"),
            Atom::Float(n) => write!(f, "{n}"),
            Atom::Boolean(b) => write!(f, "{b:?}"),
            Atom::DateTime(d) => write!(f, "{d:?}"),
            Atom::Date(d) => write!(f, "{d}"),
            Atom::LocalDateTime(d) => write!(f, "{}", d.format("%Y-%m-%dT%H:%M:%S")),
            Atom::RelativeDate(d) => write!(f, "{d}"),
            Atom::Regex(s) => write!(f, "re:\"{s}\""),
            Atom::Nil => write!(f, "nil"),
        }
    }
}

/// Where a term is written in the query, as byte offsets, so an error in it can mark it. Spans
/// never take part in comparisons, so two trees that search for the same thing are equal however
/// they are spaced.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Span {
    /// The span from the start of either span to the end of either.
    fn join(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

impl PartialEq for Span {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl From<Range<usize>> for Span {
    fn from(range: Range<usize>) -> Self {
        Span {
            start: range.start,
            end: range.end,
        }
    }
}

impl From<Span> for Range<usize> {
    fn from(span: Span) -> Self {
        span.start..span.end
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenTree<'de> {
    Atom(Atom<'de>, Span),
    Cons(Op, Vec<TokenTree<'de>>),
}

impl TokenTree<'_> {
    /// Where the tree is written in the query: an atom's own span, or the span of all the atoms
    /// under an operator. The implied field of a bare word, written nowhere, is left out.
    pub(crate) fn span(&self) -> Span {
        fn join(tree: &TokenTree<'_>, span: &mut Option<Span>) {
            match tree {
                TokenTree::Atom(Atom::Field(f), s) if f.is_empty() && s.start == s.end => {}
                TokenTree::Atom(Atom::Nil, _) => {}
                TokenTree::Atom(_, s) => *span = Some(span.map_or(*s, |span| span.join(*s))),
                TokenTree::Cons(_, children) => children.iter().for_each(|c| join(c, span)),
            }
        }
        let mut span = None;
        join(self, &mut span);
        span.unwrap_or_default()
    }
}

/// `error` with `span` marked by `label`, unless it already marks a place. The query's text is
/// attached where the error leaves the search, so `render_error` and `error_span` can show it.
pub(crate) fn label_error(error: miette::Report, span: Span, label: &str) -> miette::Report {
    if error
        .labels()
        .is_some_and(|mut labels| labels.next().is_some())
    {
        return error;
    }
    // As `render_error` shows it, without context added by `wrap_err`.
    let message = error
        .chain()
        .last()
        .map_or_else(|| error.to_string(), ToString::to_string);
    let labels = vec![miette::LabeledSpan::at(
        Range::from(span),
        label.to_string(),
    )];
    match error.help() {
        Some(help) => miette::miette!(labels = labels, help = help.to_string(), "{message}"),
        None => miette::miette!(labels = labels, "{message}"),
    }
}

impl fmt::Display for TokenTree<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenTree::Atom(i, _) => write!(f, "{}", i),
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
    fn error_spans_count_utf16_code_units() {
        let span = |query| {
            let report = parser::Parser::new(query).parse_expression().unwrap_err();
            error_span(query, &report)
        };
        assert_eq!(span("dog ) cat"), Some((4, 5)));
        // `é` is one code unit and `😀` two.
        assert_eq!(span("\"é😀\" ) cat"), Some((6, 7)));
        assert_eq!(span("é 😀"), Some((2, 4)));
        assert_eq!(span("tag="), Some((4, 4)));
    }

    /// The message and span of the error in evaluating `query`, which parses.
    fn evaluation_error(query: &str) -> (String, Option<(usize, usize)>) {
        match explain(query, QueryReturnItemType::Cards) {
            Err(crate::Error::Library(crate::LibraryError::Search { message, span, .. })) => {
                (message, span)
            }
            other => panic!("expected a search error for `{query}`, got {other:?}"),
        }
    }

    #[test]
    fn evaluation_errors_mark_where_the_problem_is() {
        for (query, marked) in [
            ("c.due<=tomorrow", "tomorrow"),
            ("dog c.reps~3", "c.reps~3"),
            ("dog foo=1", "foo"),
            ("c.state=nope", "nope"),
            ("created_at>2026-13-01", "2026-13-01"),
            ("sort_by_asc=c.nope", "c.nope"),
            ("dog limit=1 limit=2", "limit=2"),
            ("-sort_by_asc=c.due dog", "sort_by_asc=c.due"),
        ] {
            let (message, span) = evaluation_error(query);
            let (start, end) = span.unwrap_or_else(|| panic!("no span for `{query}`: {message}"));
            assert_eq!(&query[start..end], marked, "{message}");
        }
        assert_eq!(
            evaluation_error("c.due<=tomorrow").0,
            "`c.due` holds a date, so it can't be compared with text `tomorrow`\n\n    \
             c.due<=tomorrow\n           ^^^^^^^^ not a date\n\nhelp: Write a date, like \
             `2026-10-17`, `today+1d` or `now-12h`, as in `c.due<=today`"
        );
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
        // An unclosed regex used to end the query instead.
        let error = render("dog re:\"ca");
        assert!(error.starts_with("Unterminated string"), "{error}");
        assert_eq!(
            render("a\nor or b"),
            "Expected a search term\n\n    or or b\n       ^^ found `or`"
        );
    }
}
