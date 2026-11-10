//! Local days and hours, in the server's time zone or one a client names.
//!
//! A day starts at [`SparesExternalConfig::day_starts_at`](crate::config::SparesExternalConfig::day_starts_at),
//! 4 AM by default, not at midnight, so a review at 1 AM counts for the day before. Hours are the
//! hours of the clock.
//!
//! Grouping stays in SQL: [`LocalClock`] turns a Unix timestamp column into local seconds with the
//! zone's UTC offsets written out as a `CASE`, one branch per daylight saving change in the
//! range, so SQLite can `GROUP BY` local days and hours without handing every row to Rust.

use std::fmt::Write;

use chrono::DateTime;
use chrono::Datelike;
use chrono::Duration;
use chrono::Local;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::Offset;
use chrono::TimeZone;
use chrono::Utc;
use chrono_tz::Tz;

use crate::Error;
use crate::LibraryError;

const SECONDS_PER_DAY: i64 = 86_400;
/// `NaiveDate::num_days_from_ce` of 1970-01-01.
const UNIX_EPOCH_DAYS_FROM_CE: i64 = 719_163;

/// The time zone days and hours are told apart in, and the hour days start at.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Zone {
    /// `None` for the server's own, as SQLite's `'localtime'` uses.
    tz: Option<Tz>,
    /// See [`SparesExternalConfig::day_starts_at`](crate::config::SparesExternalConfig::day_starts_at).
    day_starts_at: u8,
}

impl Zone {
    /// The server's zone.
    pub(crate) fn server(day_starts_at: u8) -> Self {
        Self {
            tz: None,
            day_starts_at,
        }
    }

    /// `tz` is an IANA name such as `Europe/Berlin`, or `None` for the server's zone.
    pub(crate) fn parse(tz: Option<&str>, day_starts_at: u8) -> Result<Self, Error> {
        let tz = tz
            .map(|name| {
                name.parse::<Tz>().map_err(|_| {
                    Error::Library(LibraryError::InvalidTimeZone(format!(
                        "Unknown time zone `{name}`. Expected an IANA name, e.g. `Europe/Berlin`."
                    )))
                })
            })
            .transpose()?;
        Ok(Self { tz, day_starts_at })
    }

    fn day_start(self) -> Duration {
        Duration::hours(i64::from(self.day_starts_at))
    }

    /// The UTC offset, in seconds, at `timestamp`.
    fn offset_at(self, timestamp: i64) -> i64 {
        let utc = DateTime::from_timestamp(timestamp, 0)
            .unwrap_or_default()
            .naive_utc();
        let offset = match self.tz {
            None => Local.offset_from_utc_datetime(&utc).fix(),
            Some(tz) => tz.offset_from_utc_datetime(&utc).fix(),
        };
        i64::from(offset.local_minus_utc())
    }

    /// The day `at` counts for: its local date, or the date before when it is earlier in the day
    /// than the hour days start at.
    pub(crate) fn date(self, at: DateTime<Utc>) -> NaiveDate {
        (at + Duration::seconds(self.offset_at(at.timestamp())) - self.day_start()).date_naive()
    }

    /// The first instant of `date`: the hour days start at, in local time, unless a daylight
    /// saving change skips that time, in which case it is the change.
    pub(crate) fn start_of(self, date: NaiveDate) -> DateTime<Utc> {
        let start = date.and_time(NaiveTime::MIN) + self.day_start();
        let earliest = match self.tz {
            None => Local
                .from_local_datetime(&start)
                .earliest()
                .map(|d| d.to_utc()),
            Some(tz) => tz
                .from_local_datetime(&start)
                .earliest()
                .map(|d| d.to_utc()),
        };
        earliest.unwrap_or_else(|| self.skipped_start(start))
    }

    /// The first and last second of the day `at` counts for.
    pub(crate) fn bounds(self, at: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let date = self.date(at);
        let next = date.succ_opt().unwrap_or(date);
        (
            self.start_of(date),
            self.start_of(next) - Duration::seconds(1),
        )
    }

    /// A day's start read with the offset in force before the change that skipped it, which lands
    /// on the change.
    fn skipped_start(self, start: NaiveDateTime) -> DateTime<Utc> {
        let as_utc = start.and_utc();
        let before = self.offset_at(as_utc.timestamp() - SECONDS_PER_DAY);
        as_utc - Duration::seconds(before)
    }

    /// A clock that reads the local time of timestamps from `from` to `to` exactly. Outside that
    /// range it carries on with the nearest offset.
    pub(crate) fn clock(self, from: i64, to: i64) -> LocalClock {
        let mut offset = self.offset_at(from);
        let mut segments = vec![(from, offset)];
        let mut t = from;
        // Daylight saving changes are months apart, so checking once a day finds each of them,
        // and a binary search then finds its second.
        while t < to {
            let next = t.saturating_add(SECONDS_PER_DAY).min(to);
            if self.offset_at(next) != offset {
                let (mut unchanged, mut changed) = (t, next);
                while changed - unchanged > 1 {
                    let mid = unchanged + (changed - unchanged) / 2;
                    if self.offset_at(mid) == offset {
                        unchanged = mid;
                    } else {
                        changed = mid;
                    }
                }
                offset = self.offset_at(changed);
                segments.push((changed, offset));
            }
            t = next;
        }
        LocalClock {
            segments,
            day_start: self.day_start().num_seconds(),
        }
    }
}

/// See [`Zone::clock`].
#[derive(Debug)]
pub(crate) struct LocalClock {
    /// The offset from each change on, the first from the start of the range.
    segments: Vec<(i64, i64)>,
    /// The seconds after local midnight that a day starts at.
    day_start: i64,
}

impl LocalClock {
    /// SQL for `column`, a Unix timestamp, in local seconds since 1970-01-01 00:00.
    pub(crate) fn seconds(&self, column: &str) -> String {
        let [(_, first), rest @ ..] = self.segments.as_slice() else {
            return column.to_string();
        };
        if rest.is_empty() {
            return format!("({column} + {first})");
        }
        let mut sql = format!("({column} + CASE");
        let mut offset = *first;
        for (start, next) in rest {
            let _ = write!(sql, " WHEN {column} < {start} THEN {offset}");
            offset = *next;
        }
        let _ = write!(sql, " ELSE {offset} END)");
        sql
    }

    /// SQL for the day `column` counts for (see [`Zone::date`]), as a day number (see
    /// [`day_number`]).
    pub(crate) fn day(&self, column: &str) -> String {
        if self.day_start == 0 {
            format!("({} / {SECONDS_PER_DAY})", self.seconds(column))
        } else {
            format!(
                "(({} - {}) / {SECONDS_PER_DAY})",
                self.seconds(column),
                self.day_start
            )
        }
    }

    /// SQL for the local hour of `column`, 0 to 23.
    pub(crate) fn hour(&self, column: &str) -> String {
        format!("({} % {SECONDS_PER_DAY} / 3600)", self.seconds(column))
    }
}

/// Days since 1970-01-01, as [`LocalClock::day`] gives them.
pub(crate) fn day_number(date: NaiveDate) -> i64 {
    i64::from(date.num_days_from_ce()) - UNIX_EPOCH_DAYS_FROM_CE
}

/// The inverse of [`day_number`].
pub(crate) fn date_of_day_number(day: i64) -> NaiveDate {
    i32::try_from(day + UNIX_EPOCH_DAYS_FROM_CE)
        .ok()
        .and_then(NaiveDate::from_num_days_from_ce_opt)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn berlin() -> Zone {
        Zone::parse(Some("Europe/Berlin"), 0).unwrap()
    }

    #[test]
    fn rejects_unknown_zones() {
        assert!(Zone::parse(Some("Mars/Olympus"), 4).is_err());
        assert!(Zone::parse(None, 4).unwrap().tz.is_none());
    }

    #[test]
    fn day_numbers_round_trip() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        assert_eq!(day_number(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()), 0);
        assert_eq!(date_of_day_number(day_number(date)), date);
    }

    #[test]
    fn clock_finds_each_daylight_saving_change() {
        // Berlin moved its clocks forward on 2026-03-29 at 01:00 UTC, and back on 2026-10-25 at
        // 01:00 UTC.
        let from = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp();
        let to = Utc
            .with_ymd_and_hms(2026, 12, 31, 0, 0, 0)
            .unwrap()
            .timestamp();
        let clock = berlin().clock(from, to);
        let spring = Utc
            .with_ymd_and_hms(2026, 3, 29, 1, 0, 0)
            .unwrap()
            .timestamp();
        let autumn = Utc
            .with_ymd_and_hms(2026, 10, 25, 1, 0, 0)
            .unwrap()
            .timestamp();
        assert_eq!(
            clock.segments,
            vec![(from, 3600), (spring, 7200), (autumn, 3600)]
        );
        assert_eq!(
            clock.seconds("t"),
            format!(
                "(t + CASE WHEN t < {spring} THEN 3600 WHEN t < {autumn} THEN 7200 ELSE 3600 END)"
            )
        );
    }

    #[test]
    fn a_zone_without_changes_is_one_offset() {
        let tokyo = Zone::parse(Some("Asia/Tokyo"), 0).unwrap();
        assert_eq!(
            tokyo.clock(0, 400 * SECONDS_PER_DAY).seconds("t"),
            "(t + 32400)"
        );
    }

    #[test]
    fn days_start_at_local_midnight() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        assert_eq!(
            berlin().start_of(date),
            Utc.with_ymd_and_hms(2026, 10, 15, 22, 0, 0).unwrap()
        );
        let late = Utc.with_ymd_and_hms(2026, 10, 15, 22, 30, 0).unwrap();
        assert_eq!(berlin().date(late), date);
    }

    #[test]
    fn a_skipped_midnight_starts_the_day_at_the_change() {
        // Santiago skipped from 00:00 to 01:00 on 2026-09-06, at 04:00 UTC.
        let santiago = Zone::parse(Some("America/Santiago"), 0).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
        assert_eq!(
            santiago.start_of(date),
            Utc.with_ymd_and_hms(2026, 9, 6, 4, 0, 0).unwrap()
        );
    }

    #[test]
    fn days_start_at_the_hour_set() {
        let zone = Zone::parse(Some("Europe/Berlin"), 4).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        // 04:00 in Berlin, at UTC+2
        let start = Utc.with_ymd_and_hms(2026, 10, 16, 2, 0, 0).unwrap();
        assert_eq!(zone.start_of(date), start);
        assert_eq!(zone.date(start), date);
        assert_eq!(
            zone.date(start - Duration::seconds(1)),
            date.pred_opt().unwrap()
        );
        assert_eq!(
            zone.bounds(start + Duration::hours(23)),
            (
                start,
                Utc.with_ymd_and_hms(2026, 10, 17, 1, 59, 59).unwrap()
            )
        );
    }

    #[test]
    fn a_day_lasts_until_the_next_start_across_daylight_saving() {
        // Berlin moved its clocks back from 03:00 to 02:00 on 2026-10-25, so that day lasts 25
        // hours from 04:00 to 04:00.
        let zone = Zone::parse(Some("Europe/Berlin"), 4).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let (start, end) = zone.bounds(Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap());
        assert_eq!(start, zone.start_of(date));
        assert_eq!(end + Duration::seconds(1) - start, Duration::hours(25));
        // 02:30 the second time, at UTC+1, still counts for the 24th
        assert_eq!(
            zone.date(Utc.with_ymd_and_hms(2026, 10, 25, 1, 30, 0).unwrap()),
            date
        );
    }

    #[test]
    fn a_skipped_start_starts_the_day_at_the_change() {
        // New York skipped from 02:00 to 03:00 on 2026-03-08, at 07:00 UTC.
        let zone = Zone::parse(Some("America/New_York"), 2).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let change = Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap();
        assert_eq!(zone.start_of(date), change);
        assert_eq!(zone.date(change), date);
        assert_eq!(
            zone.date(change - Duration::seconds(1)),
            date.pred_opt().unwrap()
        );
    }

    #[test]
    fn the_clock_counts_days_from_their_start() {
        let tokyo = Zone::parse(Some("Asia/Tokyo"), 4).unwrap();
        assert_eq!(
            tokyo.clock(0, SECONDS_PER_DAY).day("t"),
            format!("(((t + 32400) - 14400) / {SECONDS_PER_DAY})")
        );
        assert_eq!(
            tokyo.clock(0, SECONDS_PER_DAY).hour("t"),
            format!("((t + 32400) % {SECONDS_PER_DAY} / 3600)")
        );
    }
}
