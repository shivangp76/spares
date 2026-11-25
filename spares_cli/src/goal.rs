//! `spares goal`: how the goals of `config.toml` are going.

use std::fmt::Write as _;

use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schema::goal::GoalPartStatus;
use spares_core::schema::goal::GoalStatus;

use crate::args::GoalArgs;
use crate::utils::ensure_ok;

pub(crate) async fn handle(args: GoalArgs, base_url: &str, client: &Client) -> Result<(), Error> {
    let response = client
        .get(format!("{base_url}/api/goals/status"))
        .send()
        .await
        .map_err(|e| miette!("{e}"))?;
    let mut statuses: Vec<GoalStatus> = ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{e}"))?;
    if let Some(name) = &args.name {
        statuses.retain(|status| &status.name == name);
        if statuses.is_empty() {
            return Err(miette!(
                help = "Goals are `[goals.<name>]` tables of config.toml",
                "No goal is named `{name}`"
            ));
        }
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
    let width = status
        .parts
        .iter()
        .map(|part| part.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(4);
    let _ = writeln!(
        out,
        "{:<width$}  {:>6} {:>6} {:>6} {:>6} {:>5}  {:>9} {:>8}  {:<12}  {:>6} {:>7}",
        "part",
        "cards",
        "new",
        "learn",
        "review",
        "susp",
        "learned14",
        "need/day",
        "at this pace",
        "recall",
        format!("<{retention}"),
    );
    for part in &status.parts {
        let _ = writeln!(out, "{}", row(part, width));
    }
    let _ = writeln!(
        out,
        "\nWeeks (cards learned by the end of the week / reviews in it):"
    );
    if let Some(first) = status.parts.first() {
        let starts = first.weeks.iter().fold(String::new(), |mut row, week| {
            let _ = write!(row, "{:>11}", week.start.format("%b %d"));
            row
        });
        let _ = writeln!(out, "{:<width$}{starts}", "");
    }
    for part in &status.parts {
        let weeks = part.weeks.iter().fold(String::new(), |mut row, week| {
            let _ = write!(row, "{:>11}", format!("{}/{}", week.learned, week.reviews));
            row
        });
        let _ = writeln!(out, "{:<width$}{weeks}", part.name);
    }
    out
}

fn row(part: &GoalPartStatus, width: usize) -> String {
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
    format!(
        "{:<width$}  {:>6} {:>6} {:>6} {:>6} {:>5}  {:>9} {:>8}  {:<12}  {:>6} {:>7}",
        part.name,
        part.cards,
        part.new,
        part.learning,
        part.review,
        part.suspended,
        part.learned_recently,
        need,
        pace,
        recall,
        part.below_retention_on_date,
    )
}
