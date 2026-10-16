use std::collections::HashMap;
use std::fmt::Write;

use chrono::Datelike;
use chrono::Days;
use chrono::Local;
use chrono::NaiveDate;
use colored::Colorize;
use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schema::review::ReviewHistoryResponse;

use crate::utils::ensure_ok;

/// The most weeks the heatmap shows by default, like the frontend's initial view.
pub(crate) const DEFAULT_WEEKS: usize = 52;
/// Characters per week column.
const CELL_WIDTH: usize = 2;
/// The weekday labels' column, e.g. `Mo `.
const LABEL_WIDTH: usize = 3;
const WEEKDAY_LABELS: [&str; 7] = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
/// One glyph per level, so the heatmap still reads without color.
const LEVEL_GLYPHS: [&str; 5] = ["·", "░", "▒", "▓", "█"];

type Counts = HashMap<NaiveDate, u32>;

pub(crate) async fn print_history(
    weeks: Option<usize>,
    json: bool,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    let url = format!("{}/api/review/history", base_url);
    let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
    let response = ensure_ok(response).await?;
    let history: ReviewHistoryResponse = response.json().await.map_err(|e| miette!("{}", e))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&history).unwrap());
        return Ok(());
    }
    let weeks = weeks.unwrap_or_else(default_weeks).max(1);
    let today = Local::now().date_naive();
    print!(
        "{}",
        format_history(
            &history.review_count_by_date,
            &history.cram_count_by_date,
            today,
            weeks
        )
    );
    Ok(())
}

/// Reviews and crams added up per day. A day spent cramming for an exam is a day studied, so it
/// fills its cell and keeps the streak going like reviews do.
fn studied_counts(reviews: &Counts, crams: &Counts) -> Counts {
    let mut counts = reviews.clone();
    for (date, count) in crams {
        *counts.entry(*date).or_default() += count;
    }
    counts
}

/// As many weeks as fit in the terminal, up to [`DEFAULT_WEEKS`].
fn default_weeks() -> usize {
    terminal_size::terminal_size().map_or(DEFAULT_WEEKS, |(width, _)| {
        (usize::from(width.0).saturating_sub(LABEL_WIDTH) / CELL_WIDTH).clamp(1, DEFAULT_WEEKS)
    })
}

#[derive(Debug, PartialEq)]
struct Summary {
    daily_average: u32,
    days_learned_percent: u32,
    longest_streak: u32,
    current_streak: u32,
    total: u32,
}

fn previous_day(date: NaiveDate) -> NaiveDate {
    date - Days::new(1)
}

fn has_reviews(counts: &Counts, date: NaiveDate) -> bool {
    counts.get(&date).is_some_and(|count| *count > 0)
}

/// Statistics over the whole history, like Anki's Review Heatmap add-on shows. A port of the
/// frontend's `summarize` in `ReviewHeatmap.tsx`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn summarize(counts: &Counts, today: NaiveDate) -> Option<Summary> {
    let mut days = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(date, _)| *date)
        .collect::<Vec<_>>();
    days.sort_unstable();
    let first = *days.first()?;
    let total = days.iter().map(|date| counts[date]).sum::<u32>();
    let span = ((today - first).num_days() + 1).max(1) as f64;

    let mut longest_streak = 0;
    let mut streak = 0;
    let mut previous: Option<NaiveDate> = None;
    for &date in &days {
        streak = if previous.is_some_and(|p| (date - p).num_days() == 1) {
            streak + 1
        } else {
            1
        };
        longest_streak = longest_streak.max(streak);
        previous = Some(date);
    }

    // Not having reviewed yet today doesn't break the streak
    let mut day = if has_reviews(counts, today) {
        today
    } else {
        previous_day(today)
    };
    let mut current_streak = 0;
    while has_reviews(counts, day) {
        current_streak += 1;
        day = previous_day(day);
    }

    Some(Summary {
        daily_average: (f64::from(total) / span).round() as u32,
        days_learned_percent: (days.len() as f64 / span * 100.0).round() as u32,
        longest_streak,
        current_streak,
        total,
    })
}

/// Levels 1 to 4 split at half, once and one and a half times the average count of the days
/// reviewed in the past year, like the frontend.
fn thresholds(counts: &Counts, today: NaiveDate) -> [f64; 3] {
    let year_ago = today - Days::new(DEFAULT_WEEKS as u64 * 7);
    let active = counts
        .iter()
        .filter(|(date, count)| **count > 0 && **date > year_ago && **date <= today)
        .map(|(_, count)| *count)
        .collect::<Vec<_>>();
    #[expect(clippy::cast_precision_loss)]
    let average = f64::from(active.iter().sum::<u32>()) / active.len().max(1) as f64;
    [0.5, 1.0, 1.5].map(|f| f * average)
}

fn level(count: u32, thresholds: &[f64; 3]) -> usize {
    if count == 0 {
        return 0;
    }
    thresholds
        .iter()
        .position(|t| f64::from(count) <= *t)
        .map_or(4, |index| index + 1)
}

fn glyph(level: usize) -> String {
    let glyph = LEVEL_GLYPHS[level];
    if level == 0 {
        glyph.bright_black().to_string()
    } else {
        glyph.green().to_string()
    }
}

fn plural(n: u32, word: &str) -> String {
    format!("{} {}", n, plural_word(n, word))
}

fn plural_word(n: u32, word: &str) -> String {
    format!("{}{}", word, if n == 1 { "" } else { "s" })
}

/// The heatmap of `reviews` and `crams` together, ending at `today`, followed by its summary.
///
/// For example, a day with 30 reviews and 120 crams is shown as 150, and the total line reads
/// `150 reviews in total, 120 of them crams`.
fn format_history(reviews: &Counts, crams: &Counts, today: NaiveDate, weeks: usize) -> String {
    let counts = &studied_counts(reviews, crams);
    let weekday = |date: NaiveDate| date.weekday().num_days_from_sunday() as usize;
    // Columns are weeks starting on Sunday, ending with the week of today
    let this_week = today - Days::new(weekday(today) as u64);
    let start = this_week - Days::new((weeks as u64 - 1) * 7);
    let thresholds = thresholds(counts, today);

    let mut out = String::new();
    // A month's label starts at the week of its first day. January is labeled with its year.
    let mut month_row = " ".repeat(LABEL_WIDTH);
    for week in 0..weeks {
        let week_start = start + Days::new(week as u64 * 7);
        let first_of_month = (0..7)
            .map(|d| week_start + Days::new(d))
            .find(|date| date.day() == 1);
        let column = LABEL_WIDTH + week * CELL_WIDTH;
        if let Some(date) = first_of_month
            && month_row.len() <= column
        {
            month_row.push_str(&" ".repeat(column - month_row.len()));
            let label = if date.month() == 1 {
                date.format("%Y")
            } else {
                date.format("%b")
            };
            month_row.push_str(&label.to_string());
        }
    }
    writeln!(out, "{}", month_row.trim_end()).unwrap();
    for (day, label) in WEEKDAY_LABELS.iter().enumerate() {
        let mut row = format!("{:<LABEL_WIDTH$}", label);
        for week in 0..weeks {
            let date = start + Days::new((week * 7 + day) as u64);
            if date > today {
                break;
            }
            let count = counts.get(&date).copied().unwrap_or(0);
            row.push_str(&glyph(level(count, &thresholds)));
            row.push(' ');
        }
        writeln!(out, "{}", row.trim_end()).unwrap();
    }

    let Some(summary) = summarize(counts, today) else {
        out.push_str("\nNo reviews yet.\n");
        return out;
    };
    let legend = (0..LEVEL_GLYPHS.len())
        .map(glyph)
        .collect::<Vec<_>>()
        .join(" ");
    let cram_total = crams.values().sum::<u32>();
    let of_them_crams = if cram_total > 0 {
        format!(
            ", {} of them {}",
            cram_total,
            plural_word(cram_total, "cram")
        )
    } else {
        String::new()
    };
    writeln!(
        out,
        "\nLess {} More    {} in total{}\n",
        legend,
        plural(summary.total, "review"),
        of_them_crams
    )
    .unwrap();
    for (label, value) in [
        ("Daily average", summary.daily_average.to_string()),
        ("Days learned", format!("{}%", summary.days_learned_percent)),
        ("Longest streak", plural(summary.longest_streak, "day")),
        ("Current streak", plural(summary.current_streak, "day")),
    ] {
        writeln!(out, "{:<16}{}", label, value).unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn counts(days: &[(&str, u32)]) -> Counts {
        days.iter().map(|(d, c)| (date(d), *c)).collect()
    }

    #[test]
    fn summary_of_no_reviews_is_none() {
        assert_eq!(summarize(&Counts::new(), date("2026-10-04")), None);
        assert_eq!(
            summarize(&counts(&[("2026-10-01", 0)]), date("2026-10-04")),
            None
        );
    }

    #[test]
    fn summary_over_whole_history() {
        let counts = counts(&[
            ("2026-09-25", 10),
            ("2026-09-26", 10),
            ("2026-09-27", 10),
            ("2026-10-01", 5),
            ("2026-10-02", 3),
            ("2026-10-03", 2),
        ]);
        let summary = summarize(&counts, date("2026-10-04")).unwrap();
        assert_eq!(
            summary,
            Summary {
                // 40 reviews over the 10 days since the first one
                daily_average: 4,
                days_learned_percent: 60,
                longest_streak: 3,
                // Not having reviewed yet today doesn't break the streak
                current_streak: 3,
                total: 40,
            }
        );
    }

    #[test]
    fn current_streak_counts_today_and_breaks_on_a_gap() {
        let today = date("2026-10-04");
        let with_today = counts(&[("2026-10-03", 1), ("2026-10-04", 1)]);
        assert_eq!(summarize(&with_today, today).unwrap().current_streak, 2);
        let broken = counts(&[("2026-10-02", 1)]);
        assert_eq!(summarize(&broken, today).unwrap().current_streak, 0);
    }

    #[test]
    fn levels_split_around_the_average() {
        // The average of the days reviewed is 10
        let counts = counts(&[("2026-10-01", 5), ("2026-10-02", 15)]);
        let thresholds = thresholds(&counts, date("2026-10-04"));
        assert_eq!(thresholds, [5.0, 10.0, 15.0]);
        assert_eq!(level(0, &thresholds), 0);
        assert_eq!(level(5, &thresholds), 1);
        assert_eq!(level(6, &thresholds), 2);
        assert_eq!(level(15, &thresholds), 3);
        assert_eq!(level(16, &thresholds), 4);
    }

    #[test]
    fn grid_ends_at_today_with_month_labels() {
        colored::control::set_override(false);
        // A Sunday
        let today = date("2026-10-04");
        let out = format_history(&counts(&[("2026-10-04", 3)]), &Counts::new(), today, 6);
        let lines = out.lines().collect::<Vec<_>>();
        // 6 weeks back from the week of today starts on 2026-08-30, so September and October
        // start in the 1st and 5th columns
        assert_eq!(lines[0], "   Sep     Oct");
        // Today is the last cell of the Sunday row, at level 2 as it equals the average. The days
        // after it are in the future.
        assert_eq!(lines[1], "Su · · · · · ▒");
        assert_eq!(lines[2], "Mo · · · · ·");
        assert!(out.contains("3 reviews in total"), "{out}");
        assert!(out.contains("Current streak  1 day\n"), "{out}");
    }

    #[test]
    fn grid_without_reviews() {
        colored::control::set_override(false);
        let out = format_history(&Counts::new(), &Counts::new(), date("2026-10-04"), 2);
        assert!(out.ends_with("\nNo reviews yet.\n"), "{out}");
    }

    #[test]
    fn crams_fill_the_grid_and_the_streak() {
        colored::control::set_override(false);
        // A Sunday. Only crams yesterday, as the night before an exam.
        let today = date("2026-10-04");
        let reviews = counts(&[("2026-10-02", 2), ("2026-10-04", 1)]);
        let crams = counts(&[("2026-10-03", 120), ("2026-10-04", 3)]);
        let out = format_history(&reviews, &crams, today, 2);
        // Saturday is at the highest level, rather than empty
        assert!(out.contains("\nSa █\n"), "{out}");
        assert!(
            out.contains("126 reviews in total, 123 of them crams"),
            "{out}"
        );
        assert!(out.contains("Current streak  3 days\n"), "{out}");
    }
}
