//! `spares card steps`: the learning and relearning steps that fit how quickly cards are
//! forgotten in them.

use std::fmt::Write as _;

use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schema::statistics::StepsGroupKind;
use spares_core::schema::statistics::StepsStatsRequest;
use spares_core::schema::statistics::StepsStatsResponse;

use crate::args::StepsArgs;
use crate::utils::ensure_ok;

pub(crate) async fn print_steps_stats(
    StepsArgs { preset, days }: StepsArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    let request = StepsStatsRequest {
        preset: preset.clone(),
        period_days: (days > 0).then_some(days),
    };
    let response = client
        .post(format!("{base_url}/api/statistics/steps"))
        .json(&request)
        .send()
        .await
        .map_err(|e| miette!("{}", e))?;
    let response: StepsStatsResponse = ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{}", e))?;
    print!("{}", format_steps_stats(&response, &request));
    Ok(())
}

fn group_label(kind: StepsGroupKind) -> &'static str {
    match kind {
        StepsGroupKind::Again => "Again",
        StepsGroupKind::Hard => "Hard",
        StepsGroupKind::Good => "Good",
        StepsGroupKind::AgainThenGood => "Again, then Good",
        StepsGroupKind::Relearning => "Lapse (Again)",
    }
}

/// A step at a useful precision: `45s`, `12m`, `2.5h` or `1.3d`.
#[expect(
    clippy::cast_precision_loss,
    reason = "steps are far below 2^52 seconds"
)]
fn format_step(seconds: i64) -> String {
    let seconds_f = seconds as f64;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", (seconds_f / 60.0).round())
    } else if seconds < 86_400 {
        format!("{:.1}h", seconds_f / 3600.0)
    } else {
        format!("{:.1}d", seconds_f / 86_400.0)
    }
}

fn format_steps(steps: &[i64]) -> String {
    if steps.is_empty() {
        "none".to_string()
    } else {
        steps
            .iter()
            .map(|step| format_step(*step))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn format_recommended(steps: &[Option<i64>]) -> String {
    steps
        .iter()
        .map(|step| step.map_or_else(|| "?".to_string(), format_step))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_steps_stats(response: &StepsStatsResponse, request: &StepsStatsRequest) -> String {
    let mut out = String::new();
    let cards = request.preset.as_ref().map_or_else(
        || "cards using no preset".to_string(),
        |preset| format!("preset `{preset}`"),
    );
    let period = request.period_days.map_or_else(
        || "all time".to_string(),
        |days| format!("last {days} days"),
    );
    writeln!(out, "Steps of the {cards}, {period}").unwrap();
    let retention = format!("{:.0}%", response.desired_retention * 100.0);
    writeln!(out, "Desired retention {retention}\n").unwrap();

    let header_falls = format!("Falls to {retention} after");
    writeln!(
        out,
        "{:<18} {:>7} {:>8} {:>10}  {header_falls}",
        "After", "Reviews", "Recalled", "Stability"
    )
    .unwrap();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a stability of at most 100 days"
    )]
    for group in &response.groups {
        let recalled = if group.reviews == 0 {
            "-".to_string()
        } else {
            format!(
                "{:.0}%",
                f64::from(group.recalled) / f64::from(group.reviews) * 100.0
            )
        };
        let stability = group.stability.map_or_else(
            || "-".to_string(),
            |days| format_step((days * 86_400.0).round() as i64),
        );
        let interval = group.interval.map_or_else(|| "-".to_string(), format_step);
        let few = if group.reviews > 0 && group.reviews < response.min_reviews {
            " (too few)"
        } else {
            ""
        };
        writeln!(
            out,
            "{:<18} {:>7} {:>8} {:>10}  {interval}{few}",
            group_label(group.kind),
            group.reviews,
            recalled,
            stability
        )
        .unwrap();
    }

    let recommended = &response.recommended;
    writeln!(out, "\n{:<18} {:<12} Recommended", "", "Now").unwrap();
    writeln!(
        out,
        "{:<18} {:<12} {}",
        "Learning steps",
        format_steps(&response.learning_steps),
        format_recommended(&[
            recommended.first_learning_step,
            recommended.second_learning_step
        ])
    )
    .unwrap();
    writeln!(
        out,
        "{:<18} {:<12} {}",
        "Relearning steps",
        format_steps(&response.relearning_steps),
        format_recommended(&[recommended.relearning_step])
    )
    .unwrap();
    writeln!(
        out,
        "\n? is a step without enough reviews to recommend one: {} in its groups. A step of a day or\nmore can go, letting the card graduate instead.",
        response.min_reviews
    )
    .unwrap();
    out
}

#[cfg(test)]
mod tests {
    use spares_core::schema::statistics::RecommendedSteps;
    use spares_core::schema::statistics::StepsGroup;

    use super::*;

    #[test]
    fn steps_stats_as_a_table() {
        let group = |kind, reviews, recalled, stability: Option<f64>, interval| StepsGroup {
            kind,
            reviews,
            recalled,
            stability,
            interval,
        };
        let response = StepsStatsResponse {
            presets: vec!["lang".to_string()],
            desired_retention: 0.9,
            min_reviews: 100,
            groups: vec![
                group(
                    StepsGroupKind::Again,
                    400,
                    300,
                    Some(180.0 / 86_400.0),
                    Some(180),
                ),
                group(StepsGroupKind::Hard, 20, 18, Some(0.01), Some(864)),
                group(StepsGroupKind::Good, 0, 0, None, None),
                group(
                    StepsGroupKind::AgainThenGood,
                    150,
                    140,
                    Some(0.5),
                    Some(43_200),
                ),
                group(StepsGroupKind::Relearning, 0, 0, None, None),
            ],
            learning_steps: vec![60, 600],
            relearning_steps: vec![600],
            recommended: RecommendedSteps {
                first_learning_step: Some(180),
                second_learning_step: Some(43_200),
                relearning_step: None,
            },
        };
        let request = StepsStatsRequest {
            preset: Some("lang".to_string()),
            period_days: Some(365),
        };
        let expected = "\
Steps of the preset `lang`, last 365 days
Desired retention 90%

After              Reviews Recalled  Stability  Falls to 90% after
Again                  400      75%         3m  3m
Hard                    20      90%        14m  14m (too few)
Good                     0        -          -  -
Again, then Good       150      93%      12.0h  12.0h
Lapse (Again)            0        -          -  -

                   Now          Recommended
Learning steps     1m 10m       3m 12.0h
Relearning steps   10m          ?

? is a step without enough reviews to recommend one: 100 in its groups. A step of a day or
more can go, letting the card graduate instead.
";
        assert_eq!(format_steps_stats(&response, &request), expected);
    }

    #[test]
    fn steps_at_a_useful_precision() {
        assert_eq!(format_step(45), "45s");
        assert_eq!(format_step(90), "2m");
        assert_eq!(format_step(5400), "1.5h");
        assert_eq!(format_step(2 * 86_400), "2.0d");
    }
}
