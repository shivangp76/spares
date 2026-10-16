use std::collections::HashMap;
use std::fmt::Write;

use chrono::Local;
use reqwest::Client;
use spares_core::model::CardId;
use spares_core::model::RatingId;
use spares_core::model::ReviewLogKind;
use spares_core::model::StateId;
use spares_core::schema::card::CardReviewEntry;
use spares_core::schema::review::Rating;

use crate::review::utils::format_duration;

const REVIEW_STATE: StateId = 2;

/// Ratings by scheduler name, since rating ids are relative to the scheduler that recorded them.
type RatingsByScheduler = HashMap<String, Vec<Rating>>;

pub(crate) async fn fetch_card_reviews(
    card_id: CardId,
    base_url: &str,
    client: &Client,
) -> Result<Vec<CardReviewEntry>, String> {
    let url = format!("{}/api/cards/{}/reviews", base_url, card_id);
    let response = client.get(url).send().await.map_err(|e| format!("{}", e))?;
    let response = crate::utils::ensure_ok(response)
        .await
        .map_err(|e| format!("{}", e))?;
    response.json().await.map_err(|e| format!("{}", e))
}

async fn fetch_ratings_by_scheduler(
    entries: &[CardReviewEntry],
    base_url: &str,
    client: &Client,
) -> Result<RatingsByScheduler, String> {
    let mut ratings_by_scheduler = RatingsByScheduler::new();
    for entry in entries {
        if ratings_by_scheduler.contains_key(&entry.scheduler_name) {
            continue;
        }
        let url = format!(
            "{}/api/scheduler/{}/ratings",
            base_url, entry.scheduler_name
        );
        let response = client.get(url).send().await.map_err(|e| format!("{}", e))?;
        let response = crate::utils::ensure_ok(response)
            .await
            .map_err(|e| format!("{}", e))?;
        let ratings: Vec<Rating> = response.json().await.map_err(|e| format!("{}", e))?;
        ratings_by_scheduler.insert(entry.scheduler_name.clone(), ratings);
    }
    Ok(ratings_by_scheduler)
}

/// Prints a card's review history and totals, like the frontend's Card Info.
pub(crate) async fn print_card_info(
    card_id: CardId,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let entries = fetch_card_reviews(card_id, base_url, client).await?;
    let ratings_by_scheduler = fetch_ratings_by_scheduler(&entries, base_url, client).await?;
    print!("{}", format_card_info(&entries, &ratings_by_scheduler));
    Ok(())
}

fn rating_label<'a>(
    entry: &CardReviewEntry,
    ratings_by_scheduler: &'a RatingsByScheduler,
) -> Option<&'a str> {
    ratings_by_scheduler
        .get(&entry.scheduler_name)?
        .iter()
        .find(|r| Some(r.id) == entry.rating)
        .map(|r| r.description.as_str())
}

pub(crate) fn state_label(state: StateId) -> String {
    match state {
        0 => "New".to_string(),
        1 => "Learning".to_string(),
        2 => "Review".to_string(),
        3 => "Relearning".to_string(),
        _ => state.to_string(),
    }
}

fn time_taken(entry: &CardReviewEntry) -> i64 {
    entry.recall_duration.unwrap_or(0) + entry.rate_duration.unwrap_or(0)
}

fn format_seconds(seconds: i64) -> String {
    format_duration(chrono::Duration::seconds(seconds))
}

/// A scheduling interval at the coarsest useful unit, like Anki's card info, e.g. `10m` or `3.2mo`.
#[expect(clippy::cast_precision_loss)]
fn format_interval(total_seconds: i64) -> String {
    const UNITS: [(i64, &str); 5] = [
        (365 * 86400, "y"),
        (30 * 86400, "mo"),
        (86400, "d"),
        (3600, "h"),
        (60, "m"),
    ];
    for (size, unit) in UNITS {
        if total_seconds >= size {
            if total_seconds % size == 0 || total_seconds / size >= 10 {
                return format!("{}{}", (total_seconds as f64 / size as f64).round(), unit);
            }
            return format!("{:.1}{}", total_seconds as f64 / size as f64, unit);
        }
    }
    format!("{}s", total_seconds)
}

fn dash_or<T>(value: Option<T>, f: impl FnOnce(T) -> String) -> String {
    value.map_or_else(|| "—".to_string(), f)
}

fn format_date(entry: &CardReviewEntry) -> String {
    entry
        .reviewed_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn format_card_info(
    entries: &[CardReviewEntry],
    ratings_by_scheduler: &RatingsByScheduler,
) -> String {
    if entries.is_empty() {
        return "This card has not been reviewed yet.\n".to_string();
    }
    let mut out = format_summary(entries, ratings_by_scheduler);
    out.push('\n');
    out.push_str(&format_table(entries, ratings_by_scheduler));
    out.push_str(
        "\nStability and difficulty are replayed with the scheduler's current parameters.\n",
    );
    out
}

fn format_summary(
    entries: &[CardReviewEntry],
    ratings_by_scheduler: &RatingsByScheduler,
) -> String {
    let reviews = entries
        .iter()
        .filter(|e| e.kind == ReviewLogKind::Review)
        .collect::<Vec<_>>();
    // Anki's meaning: forgetting a card that had graduated to review
    let lapses = reviews
        .iter()
        .filter(|e| {
            e.previous_state == REVIEW_STATE
                && rating_label(e, ratings_by_scheduler) == Some("Again")
        })
        .count();
    let total_time = reviews.iter().map(|e| time_taken(e)).sum::<i64>();

    let mut summary = Vec::new();
    if let (Some(first), Some(latest)) = (reviews.first(), reviews.last()) {
        summary.push(("First Review", format_date(first)));
        summary.push(("Latest Review", format_date(latest)));
    }
    summary.push(("Reviews", reviews.len().to_string()));
    summary.push(("Lapses", lapses.to_string()));
    if !reviews.is_empty() {
        #[expect(clippy::cast_possible_wrap)]
        let average = total_time / reviews.len() as i64;
        summary.push(("Average Time", format_seconds(average)));
    }
    summary.push(("Total Time", format_seconds(total_time)));
    // Ratings in filtered tags that do not reschedule, counted apart since they did not change
    // the card's schedule. Shown only for cards that have some.
    let crams = entries
        .iter()
        .filter(|e| e.kind == ReviewLogKind::Cram)
        .collect::<Vec<_>>();
    if !crams.is_empty() {
        summary.push(("Crams", crams.len().to_string()));
        let cram_time = crams.iter().map(|e| time_taken(e)).sum::<i64>();
        summary.push(("Cram Time", format_seconds(cram_time)));
    }

    let mut out = String::new();
    let label_width = summary.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    for (label, value) in summary {
        writeln!(out, "{:<label_width$}  {}", label, value).unwrap();
    }
    out
}

fn format_table(entries: &[CardReviewEntry], ratings_by_scheduler: &RatingsByScheduler) -> String {
    let header = [
        "Date",
        "Type",
        "Rating",
        "Interval",
        "Time",
        "Stability",
        "Difficulty",
    ];
    // Newest first, like Anki
    let rows = entries
        .iter()
        .rev()
        .map(|e| {
            let kind = if e.kind == ReviewLogKind::Forget {
                "Forget".to_string()
            } else if e.kind == ReviewLogKind::Cram {
                "Cram".to_string()
            } else if e.tag_id.is_some() {
                "Filtered".to_string()
            } else {
                state_label(e.previous_state)
            };
            let rating = dash_or(e.rating, |rating: RatingId| {
                rating_label(e, ratings_by_scheduler)
                    .map_or_else(|| rating.to_string(), ToString::to_string)
            });
            let time = if e.kind == ReviewLogKind::Forget {
                "—".to_string()
            } else {
                format_seconds(time_taken(e))
            };
            [
                format_date(e),
                kind,
                rating,
                dash_or(e.scheduled_time, format_interval),
                time,
                dash_or(e.stability, |s| format!("{:.2}", s)),
                dash_or(e.difficulty, |d| format!("{:.2}", d)),
            ]
        })
        .collect::<Vec<_>>();
    let widths = (0..header.len())
        .map(|i| {
            rows.iter()
                .map(|row| row[i].chars().count())
                .chain([header[i].len()])
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let mut out = String::new();
    let mut write_row = |cells: &[&str]| {
        let line = cells
            .iter()
            .zip(&widths)
            .map(|(cell, width)| {
                // Padded by hand since `{:<width$}` counts bytes of `—` rather than chars
                format!("{}{}", cell, " ".repeat(width - cell.chars().count()))
            })
            .collect::<Vec<_>>()
            .join("  ");
        writeln!(out, "{}", line.trim_end()).unwrap();
    };
    write_row(&header);
    for row in &rows {
        write_row(&row.each_ref().map(String::as_str));
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono::Utc;

    use super::*;

    fn entry(
        id: i64,
        kind: ReviewLogKind,
        rating: Option<RatingId>,
        previous_state: StateId,
    ) -> CardReviewEntry {
        let is_review = kind == ReviewLogKind::Review;
        let is_rating = is_review || kind == ReviewLogKind::Cram;
        CardReviewEntry {
            id,
            kind,
            reviewed_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap()
                + chrono::Duration::days(id),
            rating,
            scheduler_name: "fsrs".to_string(),
            previous_state,
            scheduled_time: is_review.then_some(86400 * 3),
            recall_duration: is_rating.then_some(5),
            rate_duration: is_rating.then_some(2),
            tag_id: (kind == ReviewLogKind::Cram).then_some(1),
            stability: is_review.then_some(3.0),
            difficulty: is_review.then_some(5.5),
        }
    }

    fn ratings() -> RatingsByScheduler {
        let ratings = [(1, "Again"), (3, "Good")]
            .into_iter()
            .map(|(id, description)| Rating {
                id,
                description: description.to_string(),
            })
            .collect();
        HashMap::from([("fsrs".to_string(), ratings)])
    }

    #[test]
    fn interval_uses_coarsest_unit() {
        assert_eq!(format_interval(30), "30s");
        assert_eq!(format_interval(600), "10m");
        assert_eq!(format_interval(86400), "1d");
        assert_eq!(format_interval(86400 * 3 / 2), "1.5d");
        assert_eq!(format_interval(86400 * 96), "3.2mo");
        assert_eq!(format_interval(86400 * 365 * 12 + 1), "12y");
    }

    #[test]
    fn unreviewed_card() {
        assert_eq!(
            format_card_info(&[], &ratings()),
            "This card has not been reviewed yet.\n"
        );
    }

    #[test]
    fn summary_and_rows_newest_first() {
        let entries = [
            entry(0, ReviewLogKind::Review, Some(3), 0),
            entry(1, ReviewLogKind::Review, Some(1), REVIEW_STATE),
            entry(2, ReviewLogKind::Forget, None, 3),
        ];
        let out = format_card_info(&entries, &ratings());
        assert!(out.contains("Reviews        2\n"), "{out}");
        assert!(out.contains("Lapses         1\n"), "{out}");
        assert!(out.contains("Average Time   7s\n"), "{out}");
        assert!(out.contains("Total Time     14s\n"), "{out}");
        let rows = out
            .lines()
            .skip_while(|l| !l.starts_with("Date"))
            .skip(1)
            .take_while(|l| !l.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].contains("Forget"), "{out}");
        assert!(
            rows[1].contains("Review") && rows[1].contains("Again"),
            "{out}"
        );
        assert!(rows[2].contains("New") && rows[2].contains("Good"), "{out}");
        assert!(rows[2].contains("3d") && rows[2].contains("5.50"), "{out}");
    }

    #[test]
    fn crams_are_listed_but_not_counted_as_reviews() {
        let entries = [
            entry(0, ReviewLogKind::Review, Some(3), 0),
            entry(1, ReviewLogKind::Cram, Some(1), REVIEW_STATE),
            entry(2, ReviewLogKind::Cram, Some(3), REVIEW_STATE),
        ];
        let out = format_card_info(&entries, &ratings());
        // A cram rated Again is not a lapse
        assert!(out.contains("Reviews        1\n"), "{out}");
        assert!(out.contains("Lapses         0\n"), "{out}");
        assert!(out.contains("Crams          2\n"), "{out}");
        assert!(out.contains("Cram Time      14s\n"), "{out}");
        let rows = out
            .lines()
            .skip_while(|l| !l.starts_with("Date"))
            .skip(1)
            .take_while(|l| !l.is_empty())
            .collect::<Vec<_>>();
        assert!(
            rows[0].contains("Cram") && rows[0].contains("Good"),
            "{out}"
        );
        assert!(
            rows[1].contains("Cram") && rows[1].contains("Again"),
            "{out}"
        );
    }

    #[test]
    fn unknown_rating_falls_back_to_id() {
        let entries = [entry(0, ReviewLogKind::Review, Some(9), 0)];
        let out = format_card_info(&entries, &RatingsByScheduler::new());
        assert!(
            out.lines().any(|l| l.contains("New") && l.contains(" 9 ")),
            "{out}"
        );
    }
}
