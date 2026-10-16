use std::collections::HashMap;
use std::fmt::Write;

use chrono::Duration;
use chrono::NaiveDate;
use clap::ValueEnum;
use colored::Color;
use miette::Error;
use miette::miette;
use reqwest::Client;
use serde::Serialize;
use spares_core::model::StateId;
use spares_core::schema::review::ReviewHistoryRequest;
use spares_core::schema::review::ReviewHistoryResponse;
use spares_core::schema::review::StatisticsRequest;
use spares_core::schema::review::StatisticsResponse;
use spares_core::schema::statistics::AnswerGroup;
use spares_core::schema::statistics::Histogram;
use spares_core::schema::statistics::PassFail;
use spares_core::schema::statistics::StatisticsGraphsRequest;
use spares_core::schema::statistics::StatisticsGraphsResponse;

use crate::args::StatisticsArgs;
use crate::card_info::state_label;
use crate::chart::bar;
use crate::chart::column_chart;
use crate::chart::format_value;
use crate::chart::legend;
use crate::history::format_history;
use crate::history::weeks_fitting;
use crate::review::utils::format_duration;
use crate::utils::ensure_ok;
use crate::utils::local_timezone;

/// Rows of a column chart.
const CHART_HEIGHT: usize = 8;
/// Rows of a histogram.
const HISTOGRAM_HEIGHT: usize = 6;
/// Characters in the longest bar of the hourly breakdown.
const HOUR_BAR_WIDTH: usize = 30;
/// The width to draw at when the terminal's cannot be told, e.g. in a pipe.
const DEFAULT_WIDTH: usize = 80;

const LEARNING: Color = Color::Yellow;
const RELEARNING: Color = Color::Red;
const YOUNG: Color = Color::BrightGreen;
const MATURE: Color = Color::Green;
const CRAM: Color = Color::Blue;
const REVIEW: Color = Color::Green;
const HISTOGRAM: Color = Color::Cyan;

/// The sections of `spares card statistics`, in the order the frontend's statistics page shows
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub(crate) enum Section {
    /// Today's reviews, time and % correct, and crams apart
    Today,
    /// The cards left to study today, by state
    Due,
    /// Cards due per day ahead
    Forecast,
    /// Reviews per day over the period
    Reviews,
    /// Of the reviews of cards in review, how many were passed
    Retention,
    /// The review heatmap
    Calendar,
    /// Cards by state
    Cards,
    /// How often each rating was chosen
    Buttons,
    /// Reviews and % correct by hour of the day
    Hourly,
    /// Cards added per day over the period
    Added,
    /// Histograms of interval, stability, difficulty and retrievability
    Distributions,
    /// Cards safe to advance or postpone
    Scheduling,
}

impl Section {
    fn needs_statistics(self) -> bool {
        matches!(self, Self::Today | Self::Due | Self::Scheduling)
    }

    fn needs_graphs(self) -> bool {
        !matches!(self, Self::Due | Self::Scheduling | Self::Calendar)
    }
}

/// `--period`: a number of days or `all`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Period(pub(crate) Option<u32>);

impl std::str::FromStr for Period {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("all") {
            return Ok(Self(None));
        }
        match s.parse::<u32>() {
            Ok(days) if days > 0 => Ok(Self(Some(days))),
            _ => Err(format!("expected a number of days or `all`, not `{s}`")),
        }
    }
}

/// What the statistics endpoints returned, for the sections asked for.
#[derive(Default, Serialize)]
pub(crate) struct Fetched {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) statistics: Option<StatisticsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) graphs: Option<StatisticsGraphsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) history: Option<ReviewHistoryResponse>,
}

pub(crate) async fn print_statistics(
    args: StatisticsArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    let StatisticsArgs {
        scheduler_name,
        date,
        query,
        period,
        forecast_days,
        sections,
        json,
    } = args;
    let mut sections = if sections.is_empty() {
        Section::value_variants().to_vec()
    } else {
        sections
    };
    sections.sort_unstable();
    sections.dedup();
    let tz = local_timezone();
    let query = query.filter(|query| !query.trim().is_empty());

    let statistics = async {
        if !sections.iter().any(|s| s.needs_statistics()) {
            return Ok(None);
        }
        let request = StatisticsRequest {
            scheduler_name: scheduler_name.clone(),
            date,
            tz: tz.clone(),
        };
        post(
            client,
            &format!("{base_url}/api/review/statistics"),
            &request,
        )
        .await
        .map(Some)
    };
    let graphs = async {
        if !sections.iter().any(|s| s.needs_graphs()) {
            return Ok(None);
        }
        let request = StatisticsGraphsRequest {
            scheduler_name: scheduler_name.clone(),
            query: query.clone(),
            period_days: period.0,
            forecast_days,
            date,
            tz: tz.clone(),
        };
        post(
            client,
            &format!("{base_url}/api/statistics/graphs"),
            &request,
        )
        .await
        .map(Some)
    };
    let history = async {
        if !sections.contains(&Section::Calendar) {
            return Ok(None);
        }
        let request = ReviewHistoryRequest {
            query: query.clone(),
            tz: tz.clone(),
        };
        let response = client
            .get(format!("{base_url}/api/review/history"))
            .query(&request)
            .send()
            .await
            .map_err(|e| miette!("{}", e))?;
        let response = ensure_ok(response).await?;
        response
            .json::<ReviewHistoryResponse>()
            .await
            .map(Some)
            .map_err(|e| miette!("{}", e))
    };
    let (statistics, graphs, history) = tokio::try_join!(statistics, graphs, history)?;
    let fetched = Fetched {
        statistics,
        graphs,
        history,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&fetched).unwrap());
    } else {
        print!(
            "{}",
            format_statistics(&fetched, &sections, terminal_width())
        );
    }
    Ok(())
}

async fn post<Q: Serialize, R: serde::de::DeserializeOwned>(
    client: &Client,
    url: &str,
    request: &Q,
) -> Result<R, Error> {
    let response = client
        .post(url)
        .json(request)
        .send()
        .await
        .map_err(|e| miette!("{}", e))?;
    let response = ensure_ok(response).await?;
    response.json().await.map_err(|e| miette!("{}", e))
}

fn terminal_width() -> usize {
    terminal_size::terminal_size().map_or(DEFAULT_WIDTH, |(width, _)| usize::from(width.0))
}

fn write_section(out: &mut String, title: &str, rows: &[(String, String)]) {
    writeln!(out, "--- {} ---", title).unwrap();
    let label_width = rows
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    for (label, value) in rows {
        let pad = label_width - label.chars().count();
        writeln!(out, "{}{}  {}", label, " ".repeat(pad), value).unwrap();
    }
    out.push('\n');
}

fn write_title(out: &mut String, title: &str) {
    writeln!(out, "--- {} ---", title).unwrap();
}

/// A table with a header row, the first column left-aligned and the rest right-aligned.
fn write_table(out: &mut String, header: &[String], rows: &[Vec<String>]) {
    let columns = header.len();
    let widths = (0..columns)
        .map(|i| {
            std::iter::once(header)
                .chain(rows.iter().map(Vec::as_slice))
                .filter_map(|row| row.get(i))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            let pad = " ".repeat(widths[i] - cell.chars().count());
            if i == 0 {
                line.push_str(cell);
                line.push_str(&pad);
            } else {
                line.push_str("  ");
                line.push_str(&pad);
                line.push_str(cell);
            }
        }
        writeln!(out, "{}", line.trim_end()).unwrap();
    }
    out.push('\n');
}

/// `80%`, or `-` when there is nothing to divide.
fn percent(part: u32, whole: u32) -> String {
    if whole == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", f64::from(part) / f64::from(whole) * 100.0)
    }
}

/// `80% (12 of 15)`, or `-`.
fn percent_of(part: u32, whole: u32) -> String {
    if whole == 0 {
        "-".to_string()
    } else {
        format!("{} ({part} of {whole})", percent(part, whole))
    }
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

/// The statistics as the frontend's statistics page shows them, `width` characters wide.
pub(crate) fn format_statistics(fetched: &Fetched, sections: &[Section], width: usize) -> String {
    let mut out = String::new();
    let statistics = fetched.statistics.as_ref();
    let graphs = fetched.graphs.as_ref();
    for section in sections {
        match section {
            Section::Today => write_today(&mut out, statistics, graphs),
            Section::Due => {
                if let Some(statistics) = statistics {
                    write_state_section(
                        &mut out,
                        "Due",
                        &statistics.due_count_by_state,
                        "Nothing due.",
                    );
                }
            }
            Section::Forecast => {
                if let Some(graphs) = graphs {
                    write_forecast(&mut out, graphs, width);
                }
            }
            Section::Reviews => {
                if let Some(graphs) = graphs {
                    write_reviews(&mut out, graphs, width);
                }
            }
            Section::Retention => {
                if let Some(graphs) = graphs {
                    write_retention(&mut out, graphs);
                }
            }
            Section::Calendar => {
                if let Some(history) = &fetched.history {
                    write_title(&mut out, "Calendar");
                    let today =
                        graphs.map_or_else(|| chrono::Local::now().date_naive(), |g| g.date);
                    out.push_str(&format_history(
                        &history.review_count_by_date,
                        &history.cram_count_by_date,
                        today,
                        weeks_fitting(width),
                    ));
                    out.push('\n');
                }
            }
            Section::Cards => {
                if let Some(graphs) = graphs {
                    write_card_counts(&mut out, graphs);
                }
            }
            Section::Buttons => {
                if let Some(graphs) = graphs {
                    write_answer_buttons(&mut out, graphs);
                }
            }
            Section::Hourly => {
                if let Some(graphs) = graphs {
                    write_hourly(&mut out, graphs);
                }
            }
            Section::Added => {
                if let Some(graphs) = graphs {
                    write_added(&mut out, graphs, width);
                }
            }
            Section::Distributions => {
                if let Some(graphs) = graphs {
                    write_distributions(&mut out, graphs, width);
                }
            }
            Section::Scheduling => {
                if let Some(statistics) = statistics {
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
                }
            }
        }
    }
    out
}

fn write_today(
    out: &mut String,
    statistics: Option<&StatisticsResponse>,
    graphs: Option<&StatisticsGraphsResponse>,
) {
    let mut rows = Vec::new();
    if let Some(statistics) = statistics {
        rows.extend([
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
        ]);
    }
    if let Some(today) = graphs.map(|graphs| &graphs.today) {
        rows.push(("Reviews".to_string(), today.reviews.to_string()));
        if today.reviews > 0 {
            rows.extend([
                (
                    "Correct".to_string(),
                    percent_of(today.passed, today.reviews),
                ),
                (
                    "Time per card".to_string(),
                    format_duration(Duration::seconds(today.seconds / i64::from(today.reviews))),
                ),
                (
                    "Learning".to_string(),
                    format!("{}  (relearning {})", today.learning, today.relearning),
                ),
                (
                    "Review".to_string(),
                    format!(
                        "{}  (young {}, mature {})",
                        today.young + today.mature,
                        today.young,
                        today.mature
                    ),
                ),
            ]);
        }
    }
    // Ratings in filtered tags that do not reschedule, apart from the reviews above. Shown only
    // on days with some, e.g. "Crams  120" after going twice through a 60 card `exam-cram` tag.
    let crams = statistics
        .map(|s| (s.cram_count, s.cram_duration))
        .or_else(|| graphs.map(|g| (g.today.crams, Duration::seconds(g.today.cram_seconds))));
    if let Some((cram_count, cram_duration)) = crams.filter(|(count, _)| *count > 0) {
        rows.push(("Crams".to_string(), cram_count.to_string()));
        rows.push(("Cram time".to_string(), format_duration(cram_duration)));
        if let Some(today) = graphs.map(|graphs| &graphs.today) {
            rows.push((
                "Cram correct".to_string(),
                percent_of(today.crams_passed, today.crams),
            ));
        }
    }
    let title = match graphs {
        Some(graphs) => format!("Today ({})", graphs.date),
        None => "Today".to_string(),
    };
    write_section(out, &title, &rows);
}

fn date_label(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn write_forecast(out: &mut String, graphs: &StatisticsGraphsResponse, width: usize) {
    let forecast = &graphs.forecast;
    write_title(out, &format!("Forecast (next {} days)", forecast.len()));
    let learning = forecast.iter().map(|day| day.learning).sum::<u32>();
    let review = forecast.iter().map(|day| day.review).sum::<u32>();
    if learning + review == 0 {
        writeln!(out, "Nothing due.\n").unwrap();
        return;
    }
    let columns = forecast
        .iter()
        .map(|day| vec![f64::from(day.learning), f64::from(day.review)])
        .collect::<Vec<_>>();
    let (first, last) = (
        forecast
            .first()
            .map(|day| date_label(day.date))
            .unwrap_or_default(),
        forecast
            .last()
            .map(|day| date_label(day.date))
            .unwrap_or_default(),
    );
    out.push_str(&column_chart(
        &columns,
        &[LEARNING, REVIEW],
        CHART_HEIGHT,
        chart_width(width),
        1,
        (&first, &last),
    ));
    writeln!(
        out,
        "{}\n",
        legend(&[("learning", LEARNING), ("review", REVIEW)])
    )
    .unwrap();
    let today = forecast.first().map_or(0, |day| day.learning + day.review);
    #[expect(clippy::cast_precision_loss, reason = "a few thousand days")]
    let average = f64::from(learning + review) / forecast.len().max(1) as f64;
    write_section_rows(
        out,
        &[
            (
                "Total".to_string(),
                format!(
                    "{}  (learning {learning}, review {review})",
                    learning + review
                ),
            ),
            ("Due today".to_string(), today.to_string()),
            ("Daily average".to_string(), format!("{average:.1}")),
            (
                "Due tomorrow".to_string(),
                forecast
                    .get(1)
                    .map_or(0, |day| day.learning + day.review)
                    .to_string(),
            ),
        ],
    );
}

/// The rows of [`write_section`] without its title.
fn write_section_rows(out: &mut String, rows: &[(String, String)]) {
    let label_width = rows
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    for (label, value) in rows {
        let pad = label_width - label.chars().count();
        writeln!(out, "{}{}  {}", label, " ".repeat(pad), value).unwrap();
    }
    out.push('\n');
}

/// The chart's width in a terminal `width` wide, less its axis.
fn chart_width(width: usize) -> usize {
    width.saturating_sub(8).max(10)
}

/// Every day of the period, with the days without reviews.
fn period_days(graphs: &StatisticsGraphsResponse) -> Vec<NaiveDate> {
    graphs
        .period_start
        .iter_days()
        .take_while(|day| *day <= graphs.date)
        .collect()
}

fn write_reviews(out: &mut String, graphs: &StatisticsGraphsResponse, width: usize) {
    let summary = &graphs.reviews_summary;
    write_title(
        out,
        &format!("Reviews ({} to {})", graphs.period_start, graphs.date),
    );
    if summary.reviews + summary.crams == 0 {
        writeln!(out, "No reviews in this period.\n").unwrap();
        return;
    }
    let by_date = graphs
        .reviews
        .iter()
        .map(|day| (day.date, day))
        .collect::<HashMap<_, _>>();
    let days = period_days(graphs);
    let columns = days
        .iter()
        .map(|date| {
            by_date.get(date).map_or_else(
                || vec![0.0; 5],
                |day| {
                    [
                        day.learning,
                        day.relearning,
                        day.young,
                        day.mature,
                        day.cram,
                    ]
                    .map(f64::from)
                    .to_vec()
                },
            )
        })
        .collect::<Vec<_>>();
    let labels = (date_label(graphs.period_start), date_label(graphs.date));
    out.push_str(&column_chart(
        &columns,
        &[LEARNING, RELEARNING, YOUNG, MATURE, CRAM],
        CHART_HEIGHT,
        chart_width(width),
        1,
        (&labels.0, &labels.1),
    ));
    writeln!(
        out,
        "{}\n",
        legend(&[
            ("learning", LEARNING),
            ("relearning", RELEARNING),
            ("young", YOUNG),
            ("mature", MATURE),
            ("cram", CRAM),
        ])
    )
    .unwrap();
    // Minutes per day, as the frontend's time toggle shows
    #[expect(clippy::cast_precision_loss, reason = "seconds in a day")]
    let minutes = days
        .iter()
        .map(|date| {
            vec![
                by_date
                    .get(date)
                    .map_or(0.0, |day| day.seconds as f64 / 60.0),
            ]
        })
        .collect::<Vec<_>>();
    writeln!(out, "Minutes per day").unwrap();
    out.push_str(&column_chart(
        &minutes,
        &[CRAM],
        CHART_HEIGHT / 2,
        chart_width(width),
        1,
        (&labels.0, &labels.1),
    ));
    out.push('\n');
    let crams = if summary.crams > 0 {
        format!(" and {} crams", summary.crams)
    } else {
        String::new()
    };
    write_section_rows(
        out,
        &[
            (
                "Total".to_string(),
                format!("{} reviews{crams}", summary.reviews),
            ),
            (
                "Days studied".to_string(),
                format!(
                    "{} of {} ({})",
                    summary.days_studied,
                    summary.days,
                    percent(summary.days_studied, summary.days)
                ),
            ),
            (
                "Daily average".to_string(),
                format!("{:.1}", summary.daily_average),
            ),
            (
                "Time".to_string(),
                format_duration(Duration::seconds(summary.seconds)),
            ),
        ],
    );
}

fn pass_fail_cells(pass_fail: PassFail) -> [String; 3] {
    [
        pass_fail.passed.to_string(),
        pass_fail.failed.to_string(),
        percent(pass_fail.passed, pass_fail.passed + pass_fail.failed),
    ]
}

fn write_retention(out: &mut String, graphs: &StatisticsGraphsResponse) {
    write_title(out, "True retention");
    writeln!(
        out,
        "Reviews of cards in review. Mature: interval of {} days or more.",
        graphs.mature_threshold_days
    )
    .unwrap();
    let header = ["", "Young", "", "", "Mature", "", "", "Total", "", ""].map(str::to_string);
    let sub_header = [
        "", "Pass", "Fail", "Rate", "Pass", "Fail", "Rate", "Pass", "Fail", "Rate",
    ]
    .map(str::to_string)
    .to_vec();
    let mut rows = vec![sub_header];
    for row in &graphs.true_retention {
        let total = PassFail {
            passed: row.young.passed + row.mature.passed,
            failed: row.young.failed + row.mature.failed,
        };
        let mut cells = vec![row.label.clone()];
        for pass_fail in [row.young, row.mature, total] {
            cells.extend(pass_fail_cells(pass_fail));
        }
        rows.push(cells);
    }
    // The group names sit over their Pass column
    write_table(out, &header, &rows);
}

fn write_card_counts(out: &mut String, graphs: &StatisticsGraphsResponse) {
    let counts = &graphs.card_counts;
    if counts.total == 0 {
        writeln!(out, "--- Card counts ---\nNo cards.\n").unwrap();
        return;
    }
    let mut states = counts.by_state.iter().collect::<Vec<_>>();
    states.sort_by_key(|(state, _)| **state);
    let mut rows = Vec::new();
    for (state, count) in states {
        if state_label(*state) == "Review" {
            rows.push(("Young".to_string(), counts.young.to_string()));
            rows.push((
                format!("Mature (≥ {} days)", graphs.mature_threshold_days),
                counts.mature.to_string(),
            ));
        } else if *count > 0 {
            rows.push((state_label(*state), count.to_string()));
        }
    }
    rows.push(("Suspended".to_string(), counts.suspended.to_string()));
    rows.push(("Buried".to_string(), counts.buried.to_string()));
    rows.push(("Total".to_string(), counts.total.to_string()));
    write_section(out, "Card counts", &rows);
}

fn write_answer_buttons(out: &mut String, graphs: &StatisticsGraphsResponse) {
    let buttons = &graphs.answer_buttons;
    write_title(out, "Answer buttons");
    let mut header = vec![String::new()];
    header.extend(
        buttons
            .ratings
            .iter()
            .map(|rating| rating.description.clone()),
    );
    header.push("Correct".to_string());
    let row = |label: &str, group: &AnswerGroup| {
        let mut cells = vec![label.to_string()];
        cells.extend(group.counts.iter().map(|(_, count)| {
            if group.total == 0 {
                count.to_string()
            } else {
                format!("{count} ({})", percent(*count, group.total))
            }
        }));
        cells.push(percent_of(group.passed, group.total));
        cells
    };
    let rows = [
        row("Learning", &buttons.learning),
        row("Young", &buttons.young),
        row("Mature", &buttons.mature),
    ];
    write_table(out, &header, &rows);
}

fn write_hourly(out: &mut String, graphs: &StatisticsGraphsResponse) {
    write_title(out, "Hourly breakdown");
    let hours = &graphs.hourly;
    let active = hours
        .iter()
        .enumerate()
        .filter(|(_, hour)| hour.reviews + hour.crams > 0)
        .map(|(hour, _)| hour)
        .collect::<Vec<_>>();
    let (Some(first), Some(last)) = (active.first(), active.last()) else {
        writeln!(out, "No reviews in this period.\n").unwrap();
        return;
    };
    let max = hours.iter().map(|hour| hour.reviews).max().unwrap_or(0);
    let count_width = max.to_string().len();
    let any_crams = hours.iter().any(|hour| hour.crams > 0);
    // Crams are left out of the bars and the review % correct, and listed apart
    for (index, hour) in hours.iter().enumerate().take(last + 1).skip(*first) {
        let mut line = format!(
            "{index:02}  {:>count_width$}  {:>4}  {:<HOUR_BAR_WIDTH$}",
            hour.reviews,
            percent(hour.passed, hour.reviews),
            bar(f64::from(hour.reviews), f64::from(max), HOUR_BAR_WIDTH),
        );
        if any_crams && hour.crams > 0 {
            write!(
                line,
                "  crams {} ({})",
                hour.crams,
                percent(hour.crams_passed, hour.crams)
            )
            .unwrap();
        }
        writeln!(out, "{}", line.trim_end()).unwrap();
    }
    out.push('\n');
}

fn write_added(out: &mut String, graphs: &StatisticsGraphsResponse, width: usize) {
    write_title(
        out,
        &format!("Added ({} to {})", graphs.period_start, graphs.date),
    );
    let total = graphs.added.iter().map(|day| day.cards).sum::<u32>();
    if total == 0 {
        writeln!(out, "No cards added in this period.\n").unwrap();
        return;
    }
    let by_date = graphs
        .added
        .iter()
        .map(|day| (day.date, day.cards))
        .collect::<HashMap<_, _>>();
    let days = period_days(graphs);
    let columns = days
        .iter()
        .map(|date| vec![f64::from(by_date.get(date).copied().unwrap_or(0))])
        .collect::<Vec<_>>();
    out.push_str(&column_chart(
        &columns,
        &[HISTOGRAM],
        CHART_HEIGHT / 2,
        chart_width(width),
        1,
        (&date_label(graphs.period_start), &date_label(graphs.date)),
    ));
    #[expect(clippy::cast_precision_loss, reason = "a few thousand days")]
    let average = f64::from(total) / days.len().max(1) as f64;
    writeln!(out, "\nTotal {total} cards, {average:.1} a day\n").unwrap();
}

fn write_histogram(out: &mut String, title: &str, histogram: &Histogram, width: usize, unit: &str) {
    writeln!(out, "{title}").unwrap();
    let (Some(first), Some(last)) = (histogram.edges.first(), histogram.edges.last()) else {
        writeln!(out, "No cards.\n").unwrap();
        return;
    };
    let columns = histogram
        .counts
        .iter()
        .map(|count| vec![f64::from(*count)])
        .collect::<Vec<_>>();
    let column_width = (chart_width(width) / columns.len().max(1)).clamp(1, 3);
    out.push_str(&column_chart(
        &columns,
        &[HISTOGRAM],
        HISTOGRAM_HEIGHT,
        chart_width(width),
        column_width,
        (
            &format_value(*first),
            &format!("{}{unit}", format_value(*last)),
        ),
    ));
    if let Some(mean) = histogram.mean {
        writeln!(out, "Average {}{unit}", format_mean(mean)).unwrap();
    }
    out.push('\n');
}

fn format_mean(mean: f64) -> String {
    if mean.abs() >= 100.0 {
        format!("{mean:.0}")
    } else {
        format!("{mean:.2}")
    }
}

fn write_distributions(out: &mut String, graphs: &StatisticsGraphsResponse, width: usize) {
    let distributions = &graphs.distributions;
    write_title(out, "Distributions");
    write_histogram(out, "Interval", &distributions.interval, width, " days");
    write_histogram(out, "Stability", &distributions.stability, width, " days");
    write_histogram(out, "Difficulty", &distributions.difficulty, width, "");
    if let Some(retrievability) = &distributions.retrievability {
        write_histogram(out, "Retrievability", retrievability, width, "");
        if let Some(knowledge) = distributions.total_knowledge {
            writeln!(out, "Estimated total knowledge {knowledge:.0} cards\n").unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::NaiveDate;
    use spares_core::schema::review::Rating;
    use spares_core::schema::statistics::AddedDay;
    use spares_core::schema::statistics::AnswerButtons;
    use spares_core::schema::statistics::CardCounts;
    use spares_core::schema::statistics::Distributions;
    use spares_core::schema::statistics::ForecastDay;
    use spares_core::schema::statistics::HourStats;
    use spares_core::schema::statistics::RetentionRow;
    use spares_core::schema::statistics::ReviewDay;
    use spares_core::schema::statistics::ReviewsSummary;
    use spares_core::schema::statistics::TodayStats;

    use super::*;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn statistics() -> StatisticsResponse {
        StatisticsResponse {
            cards_studied_count: 12,
            recall_duration: Duration::seconds(90),
            rate_duration: Duration::seconds(30),
            cram_count: 0,
            cram_duration: Duration::zero(),
            card_count_by_state: HashMap::from([(2, 30), (0, 10), (3, 0)]),
            due_count_by_state: HashMap::new(),
            due_count_by_date: HashMap::from([(date(6), 1), (date(5), 20)]),
            advance_safe_count: 4,
            postpone_safe_count: 0,
        }
    }

    fn pass_fail(passed: u32, failed: u32) -> PassFail {
        PassFail { passed, failed }
    }

    fn group(counts: [u32; 4]) -> AnswerGroup {
        AnswerGroup {
            counts: (1..=4).zip(counts).collect(),
            passed: counts[1..].iter().sum(),
            total: counts.iter().sum(),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "a response with every section filled in"
    )]
    fn graphs() -> StatisticsGraphsResponse {
        let mut hourly = [HourStats::default(); 24];
        hourly[8] = HourStats {
            reviews: 10,
            passed: 9,
            crams: 0,
            crams_passed: 0,
        };
        hourly[9] = HourStats {
            reviews: 5,
            passed: 4,
            crams: 4,
            crams_passed: 2,
        };
        StatisticsGraphsResponse {
            mature_threshold_days: 21,
            date: date(4),
            period_start: date(1),
            today: TodayStats {
                reviews: 15,
                passed: 12,
                seconds: 120,
                learning: 3,
                relearning: 1,
                young: 6,
                mature: 5,
                crams: 0,
                crams_passed: 0,
                cram_seconds: 0,
            },
            forecast: (4..=7)
                .map(|day| ForecastDay {
                    date: date(day),
                    learning: u32::from(day == 4) * 2,
                    review: (day - 3) * 2,
                })
                .collect(),
            reviews: vec![
                ReviewDay {
                    date: date(1),
                    learning: 2,
                    relearning: 0,
                    young: 4,
                    mature: 2,
                    cram: 0,
                    seconds: 600,
                },
                ReviewDay {
                    date: date(4),
                    learning: 4,
                    relearning: 1,
                    young: 6,
                    mature: 5,
                    cram: 0,
                    seconds: 120,
                },
            ],
            reviews_summary: ReviewsSummary {
                reviews: 24,
                crams: 0,
                seconds: 720,
                days_studied: 2,
                days: 4,
                daily_average: 6.0,
            },
            true_retention: vec![
                RetentionRow {
                    label: "Today".to_string(),
                    days: Some(1),
                    young: pass_fail(5, 1),
                    mature: pass_fail(5, 0),
                },
                RetentionRow {
                    label: "All time".to_string(),
                    days: None,
                    young: pass_fail(90, 10),
                    mature: pass_fail(0, 0),
                },
            ],
            answer_buttons: AnswerButtons {
                ratings: ["Again", "Hard", "Good", "Easy"]
                    .into_iter()
                    .zip(1..)
                    .map(|(description, id)| Rating {
                        id,
                        description: description.to_string(),
                    })
                    .collect(),
                learning: group([1, 0, 3, 0]),
                young: group([1, 1, 7, 1]),
                mature: group([0, 0, 0, 0]),
            },
            hourly,
            card_counts: CardCounts {
                by_state: HashMap::from([(0, 10), (2, 30)]),
                young: 20,
                mature: 10,
                suspended: 1,
                buried: 0,
                total: 41,
            },
            added: vec![AddedDay {
                date: date(2),
                cards: 3,
            }],
            distributions: Distributions {
                interval: Histogram {
                    edges: vec![0.0, 10.0, 20.0, 30.0],
                    counts: vec![1, 3, 2],
                    mean: Some(14.5),
                },
                stability: Histogram::default(),
                difficulty: Histogram::default(),
                retrievability: None,
                total_knowledge: None,
            },
        }
    }

    fn fetched() -> Fetched {
        Fetched {
            statistics: Some(statistics()),
            graphs: Some(graphs()),
            history: None,
        }
    }

    /// Every section but the calendar, which `history` tests, at a terminal width of 40.
    #[test]
    #[expect(clippy::too_many_lines, reason = "the whole output, as one snapshot")]
    fn sections_like_the_frontend() {
        colored::control::set_override(false);
        let sections = Section::value_variants()
            .iter()
            .copied()
            .filter(|section| *section != Section::Calendar)
            .collect::<Vec<_>>();
        let out = format_statistics(&fetched(), &sections, 40);
        let expected = "\
--- Today (2026-10-04) ---
Cards studied  12
Recall time    1m 30s
Rate time      30s
Total time     2m 0s
Reviews        15
Correct        80% (12 of 15)
Time per card  8s
Learning       3  (relearning 1)
Review         11  (young 6, mature 5)

--- Due ---
Nothing due.

--- Forecast (next 4 days) ---
8 ┤   █
  │   █
  │  ██
  │  ██
  │████
  │████
  │████
  │████
0 └────
   2026-10-04 2026-10-07
█ learning  █ review

Total          22  (learning 2, review 20)
Due today      4
Daily average  5.5
Due tomorrow   4

--- Reviews (2026-10-01 to 2026-10-04) ---
16 ┤   █
   │   █
   │   █
   │   █
   │█  █
   │█  █
   │█  █
   │█  █
 0 └────
    2026-10-01 2026-10-04
█ learning  █ relearning  █ young  █ mature  █ cram

Minutes per day
10 ┤█
   │█
   │█
   │█  ▆
 0 └────
    2026-10-01 2026-10-04

Total          24 reviews
Days studied   2 of 4 (50%)
Daily average  6.0
Time           12m 0s

--- True retention ---
Reviews of cards in review. Mature: interval of 21 days or more.
          Young              Mature              Total
           Pass  Fail  Rate    Pass  Fail  Rate   Pass  Fail  Rate
Today         5     1   83%       5     0  100%     10     1   91%
All time     90    10   90%       0     0     -     90    10   90%

--- Card counts ---
New                 10
Young               20
Mature (≥ 21 days)  10
Suspended           1
Buried              0
Total               41

--- Answer buttons ---
            Again     Hard     Good     Easy        Correct
Learning  1 (25%)   0 (0%)  3 (75%)   0 (0%)   75% (3 of 4)
Young     1 (10%)  1 (10%)  7 (70%)  1 (10%)  90% (9 of 10)
Mature          0        0        0        0              -

--- Hourly breakdown ---
08  10   90%  ██████████████████████████████
09   5   80%  ███████████████                 crams 4 (50%)

--- Added (2026-10-01 to 2026-10-04) ---
3 ┤ █
  │ █
  │ █
  │ █
0 └────
   2026-10-01 2026-10-04

Total 3 cards, 0.8 a day

--- Distributions ---
Interval
3 ┤   ███
  │   ███
  │   ██████
  │   ██████
  │█████████
  │█████████
0 └─────────
   0 30 days
Average 14.50 days

Stability
No cards.

Difficulty
No cards.

--- Scheduling ---
Safe to advance   4
Safe to postpone  0

";
        assert_eq!(out, expected);
    }

    #[test]
    fn crams_are_shown_apart() {
        colored::control::set_override(false);
        let mut fetched = fetched();
        fetched.statistics = Some(StatisticsResponse {
            cards_studied_count: 0,
            recall_duration: Duration::zero(),
            rate_duration: Duration::zero(),
            cram_count: 120,
            cram_duration: Duration::minutes(50),
            ..statistics()
        });
        let graphs = fetched.graphs.as_mut().unwrap();
        graphs.today = TodayStats {
            crams: 120,
            crams_passed: 90,
            cram_seconds: 3000,
            ..TodayStats::default()
        };
        let out = format_statistics(&fetched, &[Section::Today], 80);
        assert_eq!(
            out,
            "\
--- Today (2026-10-04) ---
Cards studied  0
Recall time    0s
Rate time      0s
Total time     0s
Reviews        0
Crams          120
Cram time      50m 0s
Cram correct   75% (90 of 120)

"
        );
        // The hourly breakdown lists them apart from the reviews' bars and % correct
        let out = format_statistics(&fetched, &[Section::Hourly], 80);
        assert!(out.contains("  crams 4 (50%)\n"), "{out}");
    }

    #[test]
    fn empty_sections_say_so() {
        colored::control::set_override(false);
        let mut graphs = graphs();
        graphs.forecast.iter_mut().for_each(|day| {
            day.learning = 0;
            day.review = 0;
        });
        graphs.reviews.clear();
        graphs.reviews_summary = ReviewsSummary::default();
        graphs.hourly = [HourStats::default(); 24];
        graphs.added.clear();
        let fetched = Fetched {
            graphs: Some(graphs),
            ..Fetched::default()
        };
        let out = format_statistics(
            &fetched,
            &[
                Section::Forecast,
                Section::Reviews,
                Section::Hourly,
                Section::Added,
            ],
            80,
        );
        assert_eq!(
            out,
            "\
--- Forecast (next 4 days) ---
Nothing due.

--- Reviews (2026-10-01 to 2026-10-04) ---
No reviews in this period.

--- Hourly breakdown ---
No reviews in this period.

--- Added (2026-10-01 to 2026-10-04) ---
No cards added in this period.

"
        );
    }

    #[test]
    fn periods_parse() {
        assert_eq!("30".parse::<Period>(), Ok(Period(Some(30))));
        assert_eq!("all".parse::<Period>(), Ok(Period(None)));
        assert!("0".parse::<Period>().is_err());
        assert!("month".parse::<Period>().is_err());
    }

    #[test]
    fn only_the_endpoints_a_section_needs() {
        assert!(Section::Due.needs_statistics() && !Section::Due.needs_graphs());
        assert!(Section::Today.needs_statistics() && Section::Today.needs_graphs());
        assert!(!Section::Calendar.needs_statistics() && !Section::Calendar.needs_graphs());
        assert!(!Section::Forecast.needs_statistics());
    }
}
