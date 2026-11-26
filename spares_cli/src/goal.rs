//! `spares goal`: how the goals of `config.toml` are going.

use std::fmt::Write as _;

use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::api::goal::PACE_DAYS;
use spares_core::schema::goal::GoalMilestoneStatus;
use spares_core::schema::goal::GoalPartStatus;
use spares_core::schema::goal::GoalSearch;
use spares_core::schema::goal::GoalStatus;

use crate::args::GoalArgs;
use crate::utils::ensure_ok;

/// How the goals are going, from the host.
async fn statuses(base_url: &str, client: &Client) -> Result<Vec<GoalStatus>, Error> {
    let response = client
        .get(format!("{base_url}/api/goals/status"))
        .send()
        .await
        .map_err(|e| miette!("{e}"))?;
    ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{e}"))
}

/// `statuses` with only the goal named `name`, or an error if there is none.
fn only_goal(mut statuses: Vec<GoalStatus>, name: &str) -> Result<GoalStatus, Error> {
    let names = statuses
        .iter()
        .map(|status| status.name.clone())
        .collect::<Vec<_>>();
    statuses.retain(|status| status.name == name);
    statuses.pop().ok_or_else(|| no_goal(name, &names))
}

fn no_goal(name: &str, names: &[String]) -> Error {
    miette!(
        help = if names.is_empty() {
            "Goals are `[goals.<name>]` tables of config.toml".to_string()
        } else {
            format!("The goals are: {}", names.join(", "))
        },
        "No goal is named `{name}`"
    )
}

/// The search of the cards of goal `goal`, or of its part `part`, for `--goal` and `--part`.
pub(crate) async fn goal_query(
    goal: &str,
    part: Option<&str>,
    base_url: &str,
    client: &Client,
) -> Result<String, Error> {
    let response = client
        .get(format!("{base_url}/api/goals"))
        .send()
        .await
        .map_err(|e| miette!("{e}"))?;
    let searches: Vec<GoalSearch> = ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{e}"))?;
    let names = searches
        .iter()
        .map(|search| search.name.clone())
        .collect::<Vec<_>>();
    let search = searches
        .into_iter()
        .find(|search| search.name == goal)
        .ok_or_else(|| no_goal(goal, &names))?;
    let Some(part) = part else {
        return Ok(search.query);
    };
    let names = search
        .parts
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    search
        .parts
        .iter()
        .find(|(name, _)| name == part)
        .map(|(_, query)| query.clone())
        .ok_or_else(|| {
            miette!(
                help = format!("The parts of `{goal}` are: {}", names.join(", ")),
                "Goal `{goal}` has no part `{part}`"
            )
        })
}

pub(crate) async fn handle(args: GoalArgs, base_url: &str, client: &Client) -> Result<(), Error> {
    let mut statuses = statuses(base_url, client).await?;
    if let Some(name) = &args.name {
        statuses = vec![only_goal(statuses, name)?];
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&statuses).map_err(|e| miette!("{e}"))?
        );
        return Ok(());
    }
    if statuses.is_empty() {
        println!(
            "No goals. Add a `[goals.<name>]` table to config.toml: see `Goal` in spares_core::config."
        );
    }
    for (i, status) in statuses.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print!("{}", render(status));
    }
    Ok(())
}

/// `status` as text: a heading, a row per part, and the weeks.
fn render(status: &GoalStatus) -> String {
    let mut out = String::new();
    let retention = status.retention.unwrap_or(0.9);
    let _ = writeln!(
        out,
        "{}: {} ({} days), new cards learned by {} ({} days){}",
        status.name,
        status.date,
        status.days_to_date,
        status.learned_by,
        status.days_to_learned_by,
        status
            .retention
            .map_or_else(String::new, |retention| format!(", retention {retention}")),
    );
    if let Some(goal) = status.parts.first() {
        for milestone in &goal.milestones {
            let _ = writeln!(out, "{}", milestone_line(goal, milestone));
        }
        let last_week = goal.weeks.last().map_or(0.0, |week| week.minutes / 7.0);
        let _ = writeln!(
            out,
            "time: {} a day for the next week at these limits, plus the reviews of its new cards; {} a day the last 7 days",
            status.minutes_per_day.map_or_else(
                || "unknown".to_string(),
                |minutes| format!("~{}", duration(minutes))
            ),
            duration(last_week),
        );
    }
    // The pace each part needs for the next milestone, while one is to come
    let next = status.parts.first().and_then(|goal| {
        goal.milestones
            .iter()
            .position(|milestone| milestone.new_on_day.is_none())
    });
    let width = status
        .parts
        .iter()
        .map(|part| part.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(4);
    let next_header = next
        .and_then(|index| status.parts[0].milestones.get(index))
        .map_or_else(String::new, |milestone| {
            format!(
                "  {:<13}",
                format!("need by {}", milestone.date.format("%b %d"))
            )
        });
    let _ = writeln!(
        out,
        "{:<width$}  {:>6} {:>6} {:>6} {:>6} {:>5}  {:>9} {:>8}{next_header}  {:<12}  {:<13}  {:>6} {:>7}",
        "part",
        "cards",
        "new",
        "learn",
        "review",
        "susp",
        "learned14",
        "need/day",
        "at this pace",
        "projected",
        "recall",
        format!("<{retention}"),
    );
    for part in &status.parts {
        let _ = writeln!(out, "{}", row(part, width, next));
    }
    let held_back = status
        .presets
        .iter()
        .filter(|preset| preset.lost_to_links >= 0.005)
        .map(|preset| {
            format!(
                "{} {:.0}% of {}/day",
                preset.name,
                preset.lost_to_links * 100.0,
                preset.new_limit
            )
        })
        .collect::<Vec<_>>();
    if !held_back.is_empty() {
        let _ = writeln!(
            out,
            "Limits unused while new cards wait on linked notes, until {}: {}",
            status.learned_by,
            held_back.join(", ")
        );
    }
    out.push_str(&weeks_table(status, width));
    out
}

/// The weeks of `status`, a row per part, with names `width` wide.
fn weeks_table(status: &GoalStatus, width: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nWeeks (cards learned by the end of the week / reviews in it / minutes in it):"
    );
    if let Some(first) = status.parts.first() {
        let starts = first.weeks.iter().fold(String::new(), |mut row, week| {
            let _ = write!(row, "{:>14}", week.start.format("%b %d"));
            row
        });
        let _ = writeln!(out, "{:<width$}{starts}", "");
    }
    for part in &status.parts {
        let weeks = part.weeks.iter().fold(String::new(), |mut row, week| {
            let _ = write!(
                row,
                "{:>14}",
                format!("{}/{}/{:.0}", week.learned, week.reviews, week.minutes)
            );
            row
        });
        let _ = writeln!(out, "{:<width$}{weeks}", part.name);
    }
    out
}

/// A line on one of the goal's milestones, as `goal` (the whole goal's status) has it.
fn milestone_line(goal: &GoalPartStatus, milestone: &GoalMilestoneStatus) -> String {
    if let (Some(new_on_day), Some(met)) = (milestone.new_on_day, milestone.met) {
        return format!(
            "milestone {}: {}, {new_on_day} new on the day, at most {} wanted",
            milestone.date,
            if met { "met" } else { "missed" },
            milestone.new_left,
        );
    }
    #[expect(clippy::cast_precision_loss, reason = "a number of days")]
    let pace = f64::from(goal.learned_recently) / PACE_DAYS as f64;
    format!(
        "milestone {} ({} days): at most {} new left (now {}), need {:.1}/day, pace {pace:.1}/day{}",
        milestone.date,
        milestone.days_left,
        milestone.new_left,
        goal.new,
        milestone.new_per_day_needed.unwrap_or(0.0),
        if milestone.on_track == Some(false) {
            ": behind"
        } else {
            ""
        },
    ) + &milestone
        .projected_new_left
        .map_or_else(String::new, |[fewest, most]| {
            format!("; projected {} new left then", range(fewest, most))
        })
}

/// `minutes` as `2 h 10 min` or `25 min`.
fn duration(minutes: f64) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "minutes of study, rounded"
    )]
    let minutes = minutes.max(0.0).round() as u64;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// `low` alone if it is `high`, else `low-high`.
fn range(low: u32, high: u32) -> String {
    if low == high {
        low.to_string()
    } else {
        format!("{low}-{high}")
    }
}

/// When the projection has `part`'s new cards all learned.
fn projected(part: &GoalPartStatus) -> String {
    match (&part.projection, part.new) {
        (None, _) => "-".to_string(),
        (_, 0) => "done".to_string(),
        (Some(projection), _) => match (projection.learned_earliest, projection.learned_latest) {
            (Some(earliest), Some(latest)) if earliest == latest => earliest.to_string(),
            (Some(earliest), Some(latest)) => {
                format!("{}-{}", earliest.format("%b %d"), latest.format("%b %d"))
            }
            (Some(earliest), None) => format!("{} or never", earliest.format("%b %d")),
            (None, _) => "never".to_string(),
        },
    }
}

/// The pace `part` needs for the milestone at `next`, and `behind` if its recent pace falls short.
fn milestone_need(part: &GoalPartStatus, next: usize) -> String {
    part.milestones
        .get(next)
        .and_then(|milestone| {
            let need = milestone.new_per_day_needed?;
            let behind = if milestone.on_track == Some(false) {
                " behind"
            } else {
                ""
            };
            Some(format!("{need:.1}{behind}"))
        })
        .unwrap_or_else(|| "-".to_string())
}

fn row(part: &GoalPartStatus, width: usize, next: Option<usize>) -> String {
    let need = part
        .new_per_day_needed
        .map_or_else(|| "-".to_string(), |need| format!("{need:.1}"));
    let pace = match (part.new, part.projected_learned_by) {
        (0, _) => "done".to_string(),
        (_, Some(date)) => date.to_string(),
        (_, None) => "never".to_string(),
    };
    let recall = part.mean_recall_on_date.map_or_else(
        || "-".to_string(),
        |recall| format!("{:.0}%", recall * 100.0),
    );
    let next = next.map_or_else(String::new, |next| {
        format!("  {:<13}", milestone_need(part, next))
    });
    format!(
        "{:<width$}  {:>6} {:>6} {:>6} {:>6} {:>5}  {:>9} {:>8}{next}  {:<12}  {:<13}  {:>6} {:>7}",
        part.name,
        part.cards,
        part.new,
        part.learning,
        part.review,
        part.suspended,
        part.learned_recently,
        need,
        pace,
        projected(part),
        recall,
        part.below_retention_on_date,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_as_hours_and_minutes() {
        assert_eq!(duration(0.2), "0 min");
        assert_eq!(duration(25.4), "25 min");
        assert_eq!(duration(120.0), "2 h");
        assert_eq!(duration(130.0), "2 h 10 min");
    }
}
