//! Date values that are resolved when a query runs: another item's timestamp (`note[gir].created_at`,
//! `card[12].due`), or `today` and `now`, each optionally shifted by offsets like `-5d`, `+1w-12h` or
//! `-1mo`.
//!
//! They compile to SQL expressions rather than constants, so the database resolves them in the same
//! statement as the rest of the query.

use std::fmt;

use chrono::DateTime;
use chrono::Duration;
use chrono::Months;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::Utc;
use miette::Error;
use miette::miette;

use crate::api::statistics::local_time::Zone;
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
    /// The start of the current day, which starts at
    /// [`SparesExternalConfig::day_starts_at`](crate::config::SparesExternalConfig::day_starts_at)
    /// in local time, in the zone of
    /// [`SparesExternalConfig::time_zone`](crate::config::SparesExternalConfig::time_zone).
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

    /// An SQL expression for the date as Unix time, matching how dates are stored. `today` is the
    /// start of the day that is under way, with days starting `day_starts_at` hours after local
    /// midnight, in `zone`.
    ///
    /// In the server's zone, SQLite's `'localtime'` does the local arithmetic. SQLite knows no
    /// other zone, so in another one `today` and `now` are worked out here, and the local time of
    /// another item's date is read with the zone's offsets written out (see
    /// [`Zone::clock`]).
    pub(crate) fn to_sql(&self, zone: Zone) -> String {
        if zone.is_server() {
            self.to_server_sql(zone.day_starts_at())
        } else {
            self.to_zone_sql(zone, Utc::now())
        }
    }

    /// [`Self::to_sql`] in a zone that is not the server's, at `now`.
    fn to_zone_sql(&self, zone: Zone, now: DateTime<Utc>) -> String {
        let reference = match self.anchor {
            Anchor::Today => {
                let start = zone.date(now).and_time(NaiveTime::MIN)
                    + Duration::hours(i64::from(zone.day_starts_at()));
                return self
                    .resolve(zone, Time::Local(start))
                    .timestamp()
                    .to_string();
            }
            Anchor::Now => return self.resolve(zone, Time::Utc(now)).timestamp().to_string(),
            Anchor::Note(id, column) => {
                format!(
                    "(SELECT {} FROM note WHERE id = {})",
                    column.name(),
                    id.packed()
                )
            }
            Anchor::Card(id, column) => {
                format!("(SELECT {} FROM card WHERE id = {id})", column.name())
            }
        };
        if self.offsets.is_empty() {
            return reference;
        }
        // Exact for the dates within 40 years of now; further away the nearest offset, which can
        // be an hour off.
        let years = Duration::days(40 * 365).num_seconds();
        let clock = zone.clock(now.timestamp() - years, now.timestamp() + years);
        // Each conversion reads its input once, as `x`, rather than once per offset in the zone.
        let convert = |sql: &str, convert: &dyn Fn(&str) -> String| {
            format!("(SELECT {} FROM (SELECT {sql} AS x))", convert("x"))
        };
        let mut sql = reference;
        let mut local = false;
        for offset in &self.offsets {
            if offset.unit.is_calendar() != local {
                sql = if local {
                    convert(&sql, &|x| clock.utc_seconds(x))
                } else {
                    convert(&sql, &|x| clock.seconds(x))
                };
                local = !local;
            }
            let (modifier, floor) = offset.sql_modifier();
            let floor = if floor { ", 'floor'" } else { "" };
            sql = format!("CAST(strftime('%s', {sql}, 'unixepoch', {modifier}{floor}) AS INTEGER)");
        }
        if local {
            sql = convert(&sql, &|x| clock.utc_seconds(x));
        }
        format!("(SELECT {sql})")
    }

    /// The date, from `start`, in `zone`.
    fn resolve(&self, zone: Zone, start: Time) -> DateTime<Utc> {
        let mut time = start;
        for offset in &self.offsets {
            time = if offset.unit.is_calendar() {
                let local = match time {
                    Time::Local(local) => local,
                    Time::Utc(at) => zone.to_local(at),
                };
                Time::Local(offset.add_to_local(local))
            } else {
                let at = match time {
                    Time::Local(local) => zone.to_utc(local),
                    Time::Utc(at) => at,
                };
                let amount = match offset.unit {
                    OffsetUnit::Hours => Duration::try_hours(offset.amount),
                    _ => Duration::try_minutes(offset.amount),
                };
                Time::Utc(amount.and_then(|a| at.checked_add_signed(a)).unwrap_or(at))
            };
        }
        match time {
            Time::Local(local) => zone.to_utc(local),
            Time::Utc(at) => at,
        }
    }

    /// [`Self::to_sql`] in the server's zone, which SQLite's `'localtime'` reads.
    fn to_server_sql(&self, day_starts_at: u8) -> String {
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
            Anchor::Today => {
                let mut args = vec!["'now'".to_string(), "'localtime'".to_string()];
                if day_starts_at == 0 {
                    args.push("'start of day'".to_string());
                } else {
                    // Before the day's start, the day under way began on the date before.
                    args.extend([
                        format!("'-{day_starts_at} hours'"),
                        "'start of day'".to_string(),
                        format!("'+{day_starts_at} hours'"),
                    ]);
                }
                (args, true)
            }
            Anchor::Now => (vec!["'now'".to_string()], false),
        };
        for offset in &self.offsets {
            if offset.unit.is_calendar() != local {
                args.push(if local { "'utc'" } else { "'localtime'" }.to_string());
                local = !local;
            }
            let (modifier, floor) = offset.sql_modifier();
            args.push(modifier);
            if floor {
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

/// A time before or after an offset of [`RelativeDate::resolve`]: calendar offsets move local
/// times, exact ones instants.
#[derive(Clone, Copy)]
enum Time {
    Local(NaiveDateTime),
    Utc(DateTime<Utc>),
}

impl Offset {
    /// The `strftime` modifier that applies the offset, and whether `'floor'` must follow it: a
    /// month later than January 31st is the end of February, not March 3rd.
    fn sql_modifier(self) -> (String, bool) {
        let (amount, unit) = match self.unit {
            OffsetUnit::Minutes => (self.amount, "minutes"),
            OffsetUnit::Hours => (self.amount, "hours"),
            OffsetUnit::Days => (self.amount, "days"),
            OffsetUnit::Weeks => (self.amount.saturating_mul(7), "days"),
            OffsetUnit::Months => (self.amount, "months"),
            OffsetUnit::Years => (self.amount, "years"),
        };
        (
            format!("'{amount:+} {unit}'"),
            matches!(self.unit, OffsetUnit::Months | OffsetUnit::Years),
        )
    }

    /// `local` moved by this calendar offset, as SQLite's modifier and `'floor'` move it.
    fn add_to_local(self, local: NaiveDateTime) -> NaiveDateTime {
        let months = |months: i64| {
            let magnitude = Months::new(u32::try_from(months.unsigned_abs()).unwrap_or(u32::MAX));
            if months >= 0 {
                local.checked_add_months(magnitude)
            } else {
                local.checked_sub_months(magnitude)
            }
        };
        let days =
            |days: i64| Duration::try_days(days).and_then(|days| local.checked_add_signed(days));
        match self.unit {
            OffsetUnit::Days => days(self.amount),
            OffsetUnit::Weeks => days(self.amount.saturating_mul(7)),
            OffsetUnit::Months => months(self.amount),
            OffsetUnit::Years => months(self.amount.saturating_mul(12)),
            OffsetUnit::Minutes | OffsetUnit::Hours => Some(local),
        }
        .unwrap_or(local)
    }
}

/// An SQL expression for the start of the day `days` days after `date`, as Unix time, with days
/// starting as `zone` says.
pub(crate) fn local_day_start_sql(date: NaiveDate, days: i64, zone: Zone) -> String {
    if !zone.is_server() {
        let date = date
            .checked_add_signed(Duration::days(days))
            .unwrap_or(date);
        return zone.start_of(date).timestamp().to_string();
    }
    format!(
        "(SELECT CAST(strftime('%s', '{date}', '{days:+} days'{}, 'utc') AS INTEGER))",
        hours_sql(zone.day_starts_at())
    )
}

/// The `strftime` argument that moves midnight to the start of a day, if it is not midnight.
fn hours_sql(day_starts_at: u8) -> String {
    if day_starts_at == 0 {
        String::new()
    } else {
        format!(", '+{day_starts_at} hours'")
    }
}

/// An SQL expression for the start of the month `months` months after the month starting on
/// `month`, as Unix time: the start of its first day (see [`local_day_start_sql`]).
pub(crate) fn local_month_start_sql(month: NaiveDate, months: i64, zone: Zone) -> String {
    if !zone.is_server() {
        let magnitude = Months::new(u32::try_from(months.unsigned_abs()).unwrap_or(u32::MAX));
        let date = if months >= 0 {
            month.checked_add_months(magnitude)
        } else {
            month.checked_sub_months(magnitude)
        };
        return zone.start_of(date.unwrap_or(month)).timestamp().to_string();
    }
    format!(
        "(SELECT CAST(strftime('%s', '{month}', '{months:+} months'{}, 'utc') AS INTEGER))",
        hours_sql(zone.day_starts_at())
    )
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
    fn today_starts_at_the_hour_days_start() {
        let sql = |input| {
            RelativeDate::parse(input)
                .unwrap()
                .to_sql(Zone::parse(None, 4).unwrap())
        };
        assert_eq!(
            sql("today"),
            "(SELECT CAST(strftime('%s', 'now', 'localtime', '-4 hours', 'start of day', '+4 hours', 'utc') AS INTEGER))"
        );
        assert_eq!(
            sql("today-1d+2h"),
            "(SELECT CAST(strftime('%s', 'now', 'localtime', '-4 hours', 'start of day', '+4 hours', '-1 days', 'utc', '+2 hours') AS INTEGER))"
        );
        assert_eq!(
            local_day_start_sql(
                NaiveDate::from_ymd_opt(2026, 10, 17).unwrap(),
                1,
                Zone::parse(None, 4).unwrap()
            ),
            "(SELECT CAST(strftime('%s', '2026-10-17', '+1 days', '+4 hours', 'utc') AS INTEGER))"
        );
        assert_eq!(
            local_month_start_sql(
                NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                1,
                Zone::parse(None, 0).unwrap()
            ),
            "(SELECT CAST(strftime('%s', '2026-10-01', '+1 months', 'utc') AS INTEGER))"
        );
    }

    #[test]
    fn compiles_to_sql() {
        let sql = |input| {
            RelativeDate::parse(input)
                .unwrap()
                .to_sql(Zone::parse(None, 0).unwrap())
        };
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

    /// In a named zone, `today` is the start of that zone's day, whatever zone the server is in.
    #[test]
    fn today_and_now_in_a_named_zone() {
        use chrono::TimeZone;

        let zone = Zone::parse(Some("America/New_York"), 4).unwrap();
        let sql = |input, now| {
            RelativeDate::parse(input)
                .unwrap()
                .to_zone_sql(zone, now)
                .parse::<i64>()
                .unwrap()
        };
        let utc = |d, h, m| {
            Utc.with_ymd_and_hms(2026, 11, d, h, m, 0)
                .unwrap()
                .timestamp()
        };
        // 03:59 and 04:01 in New York (UTC-5) on the 10th: before 4 AM it is still the 9th.
        let before = Utc.with_ymd_and_hms(2026, 11, 10, 8, 59, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 11, 10, 9, 1, 0).unwrap();
        assert_eq!(sql("today", before), utc(9, 9, 0));
        assert_eq!(sql("today", after), utc(10, 9, 0));
        assert_eq!(sql("today+1d", before), utc(10, 9, 0));
        assert_eq!(sql("today-1d+2h", after), utc(9, 11, 0));
        assert_eq!(sql("now-30m", after), utc(10, 8, 31));
        // Two weeks before the 10th is before the change from daylight saving time on the 1st,
        // so 4 AM is an hour earlier in UTC.
        assert_eq!(sql("today-2w", after), utc(3, 9, 0) - 7 * 86_400 - 3600);
        assert_eq!(
            sql("today-1mo", after),
            Utc.with_ymd_and_hms(2026, 10, 10, 8, 0, 0)
                .unwrap()
                .timestamp()
        );
        assert_eq!(
            local_day_start_sql(NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(), 1, zone),
            utc(1, 9, 0).to_string()
        );
        assert_eq!(
            local_month_start_sql(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), 1, zone),
            utc(1, 9, 0).to_string()
        );
    }

    /// Another item's date moves by the calendar of the named zone: a day before a time just after
    /// the change from daylight saving time is 25 hours before it.
    #[sqlx::test]
    async fn another_items_date_in_a_named_zone(pool: sqlx::SqlitePool) {
        use chrono::TimeZone;

        let zone = Zone::parse(Some("America/New_York"), 4).unwrap();
        // 2026-11-01 12:00 in New York, EST.
        let due = Utc
            .with_ymd_and_hms(2026, 11, 1, 17, 0, 0)
            .unwrap()
            .timestamp();
        // A temporary table of its own, on one connection, in place of the real `card` table.
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("CREATE TEMP TABLE card (id INTEGER PRIMARY KEY, due INTEGER)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO temp.card VALUES (3, ?)")
            .bind(due)
            .execute(&mut *conn)
            .await
            .unwrap();
        let mut value = async |input: &str| -> i64 {
            let sql = RelativeDate::parse(input)
                .unwrap()
                .to_zone_sql(zone, Utc::now());
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {sql}")))
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        assert_eq!(value("card[3].due").await, due);
        assert_eq!(value("card[3].due-1d").await, due - 25 * 3600);
        assert_eq!(value("card[3].due-1d+2h").await, due - 23 * 3600);
        assert_eq!(value("card[3].due+1mo").await, due + 30 * 86_400);
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
