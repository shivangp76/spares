//! Date values that are resolved when a query runs: another item's timestamp (`note[gir].created_at`,
//! `card[12].due`), or `today` and `now`, each optionally shifted by offsets like `-5d`, `+1w-12h` or
//! `-1mo`.
//!
//! They compile to SQL expressions rather than constants, so the database resolves them in the same
//! statement as the rest of the query.

use std::fmt;

use chrono::NaiveDate;
use miette::Error;
use miette::miette;

use crate::model::CardId;
use crate::model::NoteId;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RelativeDate {
    pub(crate) anchor: Anchor,
    offsets: Vec<Offset>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Anchor {
    Note(NoteId, DateColumn),
    Card(CardId, DateColumn),
    /// The start of the current local day.
    Today,
    Now,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DateColumn {
    CreatedAt,
    UpdatedAt,
    Due,
}

impl DateColumn {
    fn name(self) -> &'static str {
        match self {
            DateColumn::CreatedAt => "created_at",
            DateColumn::UpdatedAt => "updated_at",
            DateColumn::Due => "due",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Offset {
    amount: i64,
    unit: OffsetUnit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OffsetUnit {
    Minutes,
    Hours,
    Days,
    Weeks,
    Months,
    Years,
}

impl OffsetUnit {
    fn suffix(self) -> &'static str {
        match self {
            OffsetUnit::Minutes => "m",
            OffsetUnit::Hours => "h",
            OffsetUnit::Days => "d",
            OffsetUnit::Weeks => "w",
            OffsetUnit::Months => "mo",
            OffsetUnit::Years => "y",
        }
    }

    /// Days, weeks, months and years move by the calendar in local time, so `today-1d` is always
    /// the start of yesterday, even across a daylight saving change. Minutes and hours are exact
    /// durations.
    fn is_calendar(self) -> bool {
        matches!(
            self,
            OffsetUnit::Days | OffsetUnit::Weeks | OffsetUnit::Months | OffsetUnit::Years
        )
    }
}

impl RelativeDate {
    /// Whether `s` starts with an anchor that is also an ordinary word (`today` or `now`). Such a
    /// value is only read as a date when compared against a date field, so `data~today` still
    /// searches for the word.
    pub(crate) fn starts_with_word_anchor(s: &str) -> bool {
        let word = s.split(['+', '-']).next().unwrap_or_default();
        matches!(word, "today" | "now")
    }

    pub(crate) fn parse(s: &str) -> Result<Self, Error> {
        let (anchor, rest) = parse_anchor(s)?;
        let offsets = parse_offsets(rest).map_err(|e| e.wrap_err(format!("in `{s}`")))?;
        Ok(Self { anchor, offsets })
    }

    /// When the date is the start of a local day, like `today` or `today-3d`, the start of the
    /// day after, so that comparing against the date can mean the whole day.
    pub(crate) fn next_day(&self) -> Option<Self> {
        if self.anchor != Anchor::Today || !self.offsets.iter().all(|o| o.unit.is_calendar()) {
            return None;
        }
        let mut next = self.clone();
        next.offsets.push(Offset {
            amount: 1,
            unit: OffsetUnit::Days,
        });
        Some(next)
    }

    /// An SQL expression for the date as Unix time, matching how dates are stored.
    pub(crate) fn to_sql(&self) -> String {
        let reference = |table: &str, column: DateColumn, id: i64| {
            format!("(SELECT {} FROM {table} WHERE id = {id})", column.name())
        };
        // The starting point as `strftime` arguments, and whether those arguments end in local time.
        let (mut args, mut local) = match self.anchor {
            Anchor::Note(id, column) => {
                let sql = reference("note", column, id.packed());
                if self.offsets.is_empty() {
                    return sql;
                }
                (vec![sql, "'unixepoch'".to_string()], false)
            }
            Anchor::Card(id, column) => {
                let sql = reference("card", column, id);
                if self.offsets.is_empty() {
                    return sql;
                }
                (vec![sql, "'unixepoch'".to_string()], false)
            }
            Anchor::Today => (
                vec![
                    "'now'".to_string(),
                    "'localtime'".to_string(),
                    "'start of day'".to_string(),
                ],
                true,
            ),
            Anchor::Now => (vec!["'now'".to_string()], false),
        };
        for offset in &self.offsets {
            if offset.unit.is_calendar() != local {
                args.push(if local { "'utc'" } else { "'localtime'" }.to_string());
                local = !local;
            }
            let (amount, unit) = match offset.unit {
                OffsetUnit::Minutes => (offset.amount, "minutes"),
                OffsetUnit::Hours => (offset.amount, "hours"),
                OffsetUnit::Days => (offset.amount, "days"),
                OffsetUnit::Weeks => (offset.amount.saturating_mul(7), "days"),
                OffsetUnit::Months => (offset.amount, "months"),
                OffsetUnit::Years => (offset.amount, "years"),
            };
            args.push(format!("'{amount:+} {unit}'"));
            // A month later than January 31st is the end of February, not March 3rd.
            if matches!(offset.unit, OffsetUnit::Months | OffsetUnit::Years) {
                args.push("'floor'".to_string());
            }
        }
        if local {
            args.push("'utc'".to_string());
        }
        // A subquery of its own, which SQLite evaluates once. Inline, `strftime` over the reference's
        // subquery would run again for every row compared against it.
        format!(
            "(SELECT CAST(strftime('%s', {}) AS INTEGER))",
            args.join(", ")
        )
    }
}

/// An SQL expression for the start of the local day `days` days after `date`, as Unix time.
pub(crate) fn local_day_start_sql(date: NaiveDate, days: i64) -> String {
    format!("(SELECT CAST(strftime('%s', '{date}', '{days:+} days', 'utc') AS INTEGER))")
}

/// An SQL expression for the start of the local month `months` months after the month starting on
/// `month`, as Unix time.
pub(crate) fn local_month_start_sql(month: NaiveDate, months: i64) -> String {
    format!("(SELECT CAST(strftime('%s', '{month}', '{months:+} months', 'utc') AS INTEGER))")
}

impl fmt::Display for RelativeDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.anchor {
            Anchor::Note(id, column) => write!(f, "note[{id}].{}", column.name())?,
            Anchor::Card(id, column) => write!(f, "card[{id}].{}", column.name())?,
            Anchor::Today => write!(f, "today")?,
            Anchor::Now => write!(f, "now")?,
        }
        for offset in &self.offsets {
            write!(f, "{:+}{}", offset.amount, offset.unit.suffix())?;
        }
        Ok(())
    }
}

/// Parses the anchor at the start of `s`, returning it and the rest of `s`.
fn parse_anchor(s: &str) -> Result<(Anchor, &str), Error> {
    for (prefix, is_note) in [("note[", true), ("card[", false)] {
        let Some(after_prefix) = s.strip_prefix(prefix) else {
            continue;
        };
        let (id, after_id) = after_prefix
            .split_once(']')
            .ok_or_else(|| miette!("Missing `]` after the id in `{s}`"))?;
        let after_dot = after_id.strip_prefix('.').ok_or_else(|| {
            miette!(
                "Expected `.` and a field after `{prefix}{id}]` in `{s}`, e.g. `{prefix}{id}].created_at`"
            )
        })?;
        let field_end = after_dot
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after_dot.len());
        let (field, rest) = after_dot.split_at(field_end);
        let column = match field {
            "created_at" => DateColumn::CreatedAt,
            "updated_at" => DateColumn::UpdatedAt,
            "due" if !is_note => DateColumn::Due,
            _ => {
                let fields = if is_note {
                    "`created_at` or `updated_at`"
                } else {
                    "`created_at`, `updated_at` or `due`"
                };
                return Err(miette!(
                    "`{field}` in `{s}` is not a date field of a {}. Expected {fields}",
                    if is_note { "note" } else { "card" }
                ));
            }
        };
        let anchor = if is_note {
            Anchor::Note(id.parse::<NoteId>().map_err(|e| miette!("{e}"))?, column)
        } else {
            let card_id = id
                .parse::<CardId>()
                .map_err(|_| miette!("`{id}` in `{s}` is not a card id: expected a number"))?;
            Anchor::Card(card_id, column)
        };
        return Ok((anchor, rest));
    }
    if let Some(rest) = s.strip_prefix("today") {
        return Ok((Anchor::Today, rest));
    }
    if let Some(rest) = s.strip_prefix("now") {
        return Ok((Anchor::Now, rest));
    }
    Err(miette!(
        "`{s}` is not a relative date. Expected `note[<id>].<field>`, `card[<id>].<field>`, `today` or `now`"
    ))
}

/// Parses offsets like `-5d+12h`.
fn parse_offsets(mut s: &str) -> Result<Vec<Offset>, Error> {
    let mut offsets = Vec::new();
    while !s.is_empty() {
        let sign = match s.as_bytes()[0] {
            b'+' => 1,
            b'-' => -1,
            _ => {
                return Err(miette!(
                    "Unexpected `{s}`. Offsets start with `+` or `-`, e.g. `-5d`"
                ));
            }
        };
        let after_sign = &s[1..];
        let digits_end = after_sign
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_sign.len());
        if digits_end == 0 {
            return Err(miette!("Expected a number after `{}`", &s[..1]));
        }
        let amount = after_sign[..digits_end]
            .parse::<i64>()
            .map_err(|_| miette!("The offset `{}` is too large", &s[..=digits_end]))?;
        let after_digits = &after_sign[digits_end..];
        let units = [
            ("mo", OffsetUnit::Months),
            ("m", OffsetUnit::Minutes),
            ("h", OffsetUnit::Hours),
            ("d", OffsetUnit::Days),
            ("w", OffsetUnit::Weeks),
            ("y", OffsetUnit::Years),
        ];
        let Some((suffix, unit)) = units
            .into_iter()
            .find(|(suffix, _)| after_digits.starts_with(suffix))
        else {
            return Err(miette!(
                "Expected a unit after `{}`: `m` (minutes), `h` (hours), `d` (days), `w` (weeks), `mo` (months) or `y` (years)",
                &s[..=digits_end]
            ));
        };
        offsets.push(Offset {
            amount: sign * amount,
            unit,
        });
        s = &after_digits[suffix.len()..];
    }
    Ok(offsets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_display() {
        for input in [
            "note[gir].created_at",
            "note[gir].updated_at-5d",
            "card[12].due+1w-12h",
            "card[12].created_at",
            "today",
            "today+3d",
            "now-30m",
            "today-1mo",
            "now+2y-3mo+1m",
        ] {
            assert_eq!(RelativeDate::parse(input).unwrap().to_string(), input);
        }
    }

    #[test]
    fn rejects_malformed_values() {
        for input in [
            "note[gir",
            "note[gir]",
            "note[gir]created_at",
            "note[gir].due",
            "note[gir].stability",
            "note[12].created_at",
            "card[gir].due",
            "card[12].data",
            "today-5",
            "today-d",
            "today-5x",
            "today-5M",
            "today5d",
            "today-99999999999999999999d",
            "yesterday",
        ] {
            assert!(
                RelativeDate::parse(input).is_err(),
                "Expected an error for `{input}`"
            );
        }
    }

    #[test]
    fn compiles_to_sql() {
        let sql = |input| RelativeDate::parse(input).unwrap().to_sql();
        assert_eq!(
            sql("note[gir].created_at"),
            "(SELECT created_at FROM note WHERE id = 76967537148)"
        );
        assert_eq!(
            sql("note[gir].created_at-5d"),
            "(SELECT CAST(strftime('%s', (SELECT created_at FROM note WHERE id = 76967537148), 'unixepoch', 'localtime', '-5 days', 'utc') AS INTEGER))"
        );
        assert_eq!(
            sql("card[3].due+12h"),
            "(SELECT CAST(strftime('%s', (SELECT due FROM card WHERE id = 3), 'unixepoch', '+12 hours') AS INTEGER))"
        );
        assert_eq!(
            sql("today"),
            "(SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', 'utc') AS INTEGER))"
        );
        // Calendar offsets stay in local time, exact ones switch back to UTC first.
        assert_eq!(
            sql("today-1w+2d-3h"),
            "(SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', '-7 days', '+2 days', 'utc', '-3 hours') AS INTEGER))"
        );
        assert_eq!(
            sql("now-30m"),
            "(SELECT CAST(strftime('%s', 'now', '-30 minutes') AS INTEGER))"
        );
        // Months and years are calendar offsets that stop at the end of a short month.
        assert_eq!(
            sql("today-1mo+1y"),
            "(SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', '-1 months', 'floor', '+1 years', 'floor', 'utc') AS INTEGER))"
        );
        assert!(
            RelativeDate::parse("today-1mo")
                .unwrap()
                .next_day()
                .is_some()
        );
    }

    #[sqlx::test]
    async fn months_stop_at_the_end_of_a_short_month(pool: sqlx::SqlitePool) {
        let date: String = sqlx::query_scalar(
            "SELECT date('2026-01-31', '+1 months', 'floor', '+1 years', 'floor')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(date, "2027-02-28");
    }

    #[test]
    fn word_anchors() {
        assert!(RelativeDate::starts_with_word_anchor("today"));
        assert!(RelativeDate::starts_with_word_anchor("today-5d"));
        assert!(RelativeDate::starts_with_word_anchor("now+1h"));
        assert!(!RelativeDate::starts_with_word_anchor("todays"));
        assert!(!RelativeDate::starts_with_word_anchor("nowhere"));
        assert!(!RelativeDate::starts_with_word_anchor("dog"));
    }
}
