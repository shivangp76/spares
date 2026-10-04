use std::collections::HashMap;
use std::fmt::Write;

use spares_core::model::StateId;
use spares_core::schema::review::StatisticsResponse;

use crate::card_info::state_label;
use crate::review::utils::format_duration;

/// Characters in the longest bar of the upcoming due chart.
const BAR_WIDTH: u32 = 40;

fn write_section(out: &mut String, title: &str, rows: &[(String, String)]) {
    writeln!(out, "--- {} ---", title).unwrap();
    let label_width = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    for (label, value) in rows {
        writeln!(out, "{:<label_width$}  {}", label, value).unwrap();
    }
    out.push('\n');
}

/// Counts by state, skipping empty states, with their total. `None` when every count is 0.
fn state_rows(counts: &HashMap<StateId, u32>) -> Option<Vec<(String, String)>> {
    let mut counts = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .collect::<Vec<_>>();
    if counts.is_empty() {
        return None;
    }
    counts.sort_by_key(|(state, _)| **state);
    let total = counts.iter().map(|(_, count)| **count).sum::<u32>();
    let mut rows = counts
        .into_iter()
        .map(|(state, count)| (state_label(*state), count.to_string()))
        .collect::<Vec<_>>();
    rows.push(("Total".to_string(), total.to_string()));
    Some(rows)
}

fn write_state_section(out: &mut String, title: &str, counts: &HashMap<StateId, u32>, empty: &str) {
    match state_rows(counts) {
        Some(rows) => write_section(out, title, &rows),
        None => writeln!(out, "--- {} ---\n{}\n", title, empty).unwrap(),
    }
}

/// The statistics as the frontend's statistics page shows them.
pub(crate) fn format_statistics(statistics: &StatisticsResponse) -> String {
    let mut out = String::new();
    write_section(
        &mut out,
        "Studied",
        &[
            (
                "Cards studied".to_string(),
                statistics.cards_studied_count.to_string(),
            ),
            (
                "Recall time".to_string(),
                format_duration(statistics.recall_duration),
            ),
            (
                "Rate time".to_string(),
                format_duration(statistics.rate_duration),
            ),
            (
                "Total time".to_string(),
                format_duration(statistics.recall_duration + statistics.rate_duration),
            ),
        ],
    );
    write_state_section(
        &mut out,
        "Due",
        &statistics.due_count_by_state,
        "Nothing due.",
    );
    write_section(
        &mut out,
        "Scheduling",
        &[
            (
                "Safe to advance".to_string(),
                statistics.advance_safe_count.to_string(),
            ),
            (
                "Safe to postpone".to_string(),
                statistics.postpone_safe_count.to_string(),
            ),
        ],
    );
    write_state_section(
        &mut out,
        "All cards by state",
        &statistics.card_count_by_state,
        "No cards.",
    );

    writeln!(out, "--- Upcoming due ---").unwrap();
    let mut due_by_date = statistics.due_count_by_date.iter().collect::<Vec<_>>();
    if due_by_date.is_empty() {
        writeln!(out, "No cards due after this date.").unwrap();
        return out;
    }
    due_by_date.sort_by_key(|(date, _)| **date);
    let max_due = due_by_date
        .iter()
        .map(|(_, count)| **count)
        .max()
        .unwrap_or(1)
        .max(1);
    let count_width = max_due.to_string().len();
    for (date, count) in due_by_date {
        // At least one character, so every day with cards due shows a bar
        let bar = (count * BAR_WIDTH).div_ceil(max_due);
        writeln!(
            out,
            "{}  {:>count_width$}  {}",
            date,
            count,
            "█".repeat(bar as usize)
        )
        .unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::NaiveDate;

    use super::*;

    fn statistics() -> StatisticsResponse {
        StatisticsResponse {
            cards_studied_count: 12,
            recall_duration: Duration::seconds(90),
            rate_duration: Duration::seconds(30),
            card_count_by_state: HashMap::from([(2, 30), (0, 10), (3, 0)]),
            due_count_by_state: HashMap::new(),
            due_count_by_date: HashMap::from([
                (NaiveDate::from_ymd_opt(2026, 10, 6).unwrap(), 1),
                (NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(), 20),
            ]),
            advance_safe_count: 4,
            postpone_safe_count: 0,
        }
    }

    #[test]
    fn sections_like_the_frontend() {
        let out = format_statistics(&statistics());
        let expected = "\
--- Studied ---
Cards studied  12
Recall time    1m 30s
Rate time      30s
Total time     2m 0s

--- Due ---
Nothing due.

--- Scheduling ---
Safe to advance   4
Safe to postpone  0

--- All cards by state ---
New     10
Review  30
Total   40

--- Upcoming due ---
2026-10-05  20  ████████████████████████████████████████
2026-10-06   1  ██
";
        assert_eq!(out, expected);
    }

    #[test]
    fn nothing_due_later() {
        let out = format_statistics(&StatisticsResponse {
            due_count_by_date: HashMap::new(),
            ..statistics()
        });
        assert!(
            out.ends_with("--- Upcoming due ---\nNo cards due after this date.\n"),
            "{out}"
        );
    }
}
