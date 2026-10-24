use std::collections::HashMap;
use std::fmt::Write;

use chrono::Local;
use reqwest::Client;
use spares_core::model::CardId;
use spares_core::model::RatingId;
use spares_core::model::RescheduleData;
use spares_core::model::ReviewLogKind;
use spares_core::model::StateId;
use spares_core::schema::card::CardReviewEntry;
use spares_core::schema::review::CardState;
use spares_core::schema::review::Rating;

use crate::review::utils::format_duration;

/// Ratings by scheduler name, since rating ids are relative to the scheduler that recorded them.
type RatingsByScheduler = HashMap<String, Vec<Rating>>;

/// States by scheduler name, since state ids are relative to the scheduler too.
type StatesByScheduler = HashMap<String, Vec<CardState>>;

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

/// `what` of a scheduler, like `ratings` or `states`, from `/api/scheduler/{name}/{what}`.
async fn fetch_of_scheduler<T: serde::de::DeserializeOwned>(
    scheduler_name: &str,
    what: &str,
    base_url: &str,
    client: &Client,
) -> Result<T, String> {
    let url = format!("{base_url}/api/scheduler/{scheduler_name}/{what}");
    let response = client.get(url).send().await.map_err(|e| format!("{}", e))?;
    let response = crate::utils::ensure_ok(response)
        .await
        .map_err(|e| format!("{}", e))?;
    response.json().await.map_err(|e| format!("{}", e))
}

/// The states `scheduler_name` puts cards in, for [`state_label`].
pub(crate) async fn fetch_states(
    scheduler_name: &str,
    base_url: &str,
    client: &Client,
) -> Result<Vec<CardState>, String> {
    fetch_of_scheduler(scheduler_name, "states", base_url, client).await
}

async fn fetch_by_scheduler(
    entries: &[CardReviewEntry],
    base_url: &str,
    client: &Client,
) -> Result<(RatingsByScheduler, StatesByScheduler), String> {
    let mut ratings_by_scheduler = RatingsByScheduler::new();
    let mut states_by_scheduler = StatesByScheduler::new();
    for entry in entries {
        let name = &entry.scheduler_name;
        if ratings_by_scheduler.contains_key(name) {
            continue;
        }
        let ratings = fetch_of_scheduler(name, "ratings", base_url, client).await?;
        ratings_by_scheduler.insert(name.clone(), ratings);
        states_by_scheduler.insert(name.clone(), fetch_states(name, base_url, client).await?);
    }
    Ok((ratings_by_scheduler, states_by_scheduler))
}

/// Prints a card's review history and totals, like the frontend's Card Info.
pub(crate) async fn print_card_info(
    card_id: CardId,
    base_url: &str,
    client: &Client,
) -> Result<(), String> {
    let entries = fetch_card_reviews(card_id, base_url, client).await?;
    let (ratings_by_scheduler, states_by_scheduler) =
        fetch_by_scheduler(&entries, base_url, client).await?;
    print!(
        "{}",
        format_card_info(&entries, &ratings_by_scheduler, &states_by_scheduler)
    );
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

/// The name of `state` among `states`, or its id if none has it.
pub(crate) fn state_label(states: &[CardState], state: StateId) -> String {
    states
        .iter()
        .find(|s| s.id == state)
        .map_or_else(|| state.to_string(), |s| s.name.clone())
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
    states_by_scheduler: &StatesByScheduler,
) -> String {
    if entries.is_empty() {
        return "This card has not been reviewed yet.\n".to_string();
    }
    let mut out = format_summary(entries);
    out.push('\n');
    out.push_str(&format_table(
        entries,
        ratings_by_scheduler,
        states_by_scheduler,
    ));
    out.push_str(
        "\nStability and difficulty are replayed with the scheduler's current parameters.\n",
    );
    out
}

fn format_summary(entries: &[CardReviewEntry]) -> String {
    let reviews = entries
        .iter()
        .filter(|e| e.kind == ReviewLogKind::Review)
        .collect::<Vec<_>>();
    let lapses = reviews.iter().filter(|e| e.lapse).count();
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

/// A move of a card's due date, with how far it moved, like `Postpone +3d`.
fn move_label(reschedule: RescheduleData) -> String {
    let seconds = (reschedule.due - reschedule.previous_due).num_seconds();
    if seconds >= 0 {
        format!("Postpone +{}", format_interval(seconds))
    } else {
        format!("Advance -{}", format_interval(-seconds))
    }
}

fn format_table(
    entries: &[CardReviewEntry],
    ratings_by_scheduler: &RatingsByScheduler,
    states_by_scheduler: &StatesByScheduler,
) -> String {
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
            } else if let Some(reschedule) = e.reschedule {
                move_label(reschedule)
            } else if e.kind == ReviewLogKind::Cram {
                "Cram".to_string()
            } else if e.tag_id.is_some() {
                "Filtered".to_string()
            } else {
                let states = states_by_scheduler.get(&e.scheduler_name);
                state_label(states.map_or(&[], Vec::as_slice), e.previous_state)
            };
            let rating = dash_or(e.rating, |rating: RatingId| {
                rating_label(e, ratings_by_scheduler)
                    .map_or_else(|| rating.to_string(), ToString::to_string)
            });
            let time = if matches!(e.kind, ReviewLogKind::Forget | ReviewLogKind::Reschedule) {
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

    const REVIEW_STATE: StateId = 2;

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
            // As the server marks them under FSRS: Again in the review state
            lapse: is_review && rating == Some(1) && previous_state == REVIEW_STATE,
            reschedule: None,
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

    fn states() -> StatesByScheduler {
        let fsrs = spares_core::schedulers::get_scheduler_from_string("fsrs").unwrap();
        HashMap::from([("fsrs".to_string(), fsrs.get_states())])
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
            format_card_info(&[], &ratings(), &states()),
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
        let out = format_card_info(&entries, &ratings(), &states());
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
        let out = format_card_info(&entries, &ratings(), &states());
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
    fn moves_are_listed_with_how_far_they_moved() {
        let moved = |id, days: i64| {
            let mut e = entry(id, ReviewLogKind::Reschedule, None, REVIEW_STATE);
            e.reschedule = Some(RescheduleData {
                previous_due: e.reviewed_at,
                due: e.reviewed_at + chrono::Duration::days(days),
            });
            e
        };
        let entries = [
            entry(0, ReviewLogKind::Review, Some(3), 0),
            moved(1, 3),
            moved(2, -2),
        ];
        let out = format_card_info(&entries, &ratings(), &states());
        assert!(out.contains("Reviews        1\n"), "{out}");
        let rows = out
            .lines()
            .skip_while(|l| !l.starts_with("Date"))
            .skip(1)
            .take_while(|l| !l.is_empty())
            .collect::<Vec<_>>();
        assert!(rows[0].contains("Advance -2d"), "{out}");
        assert!(rows[1].contains("Postpone +3d"), "{out}");
    }

    #[test]
    fn unknown_rating_falls_back_to_id() {
        let entries = [entry(0, ReviewLogKind::Review, Some(9), 0)];
        let out = format_card_info(&entries, &RatingsByScheduler::new(), &states());
        assert!(
            out.lines().any(|l| l.contains("New") && l.contains(" 9 ")),
            "{out}"
        );
    }

    #[test]
    fn states_are_named_by_the_scheduler_that_reviewed() {
        let mut mastered = entry(1, ReviewLogKind::Review, Some(3), 4);
        mastered.scheduler_name = "other".to_string();
        let mut states = states();
        states.insert(
            "other".to_string(),
            vec![CardState {
                id: 4,
                name: "Mastered".to_string(),
                role: spares_core::schema::review::StateRole::Review,
            }],
        );
        let entries = [entry(0, ReviewLogKind::Review, Some(3), 0), mastered];
        let out = format_card_info(&entries, &ratings(), &states);
        assert!(out.contains("Mastered"), "{out}");
        // Without the scheduler's states, the id is shown
        let out = format_card_info(&entries, &ratings(), &StatesByScheduler::new());
        assert!(!out.contains("Mastered") && !out.contains("New"), "{out}");
    }
}
