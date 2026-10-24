use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use clap::Args;
use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schedulers::get_default_scheduler_name;
use spares_core::schema::review::CardState;
use spares_core::schema::review::NotShownReason;
use spares_core::schema::review::QueueCard;
use spares_core::schema::review::QueuePositionRequest;
use spares_core::schema::review::QueuePositionResponse;

use super::FilterArgs;
use super::filter_args_to_filter;
use crate::card_info::fetch_states;
use crate::card_info::state_label;
use crate::utils::ensure_ok;

/// Where cards are in a review's queue, and why the others are not in it
///
/// Takes the same filter as `spares card review`, so a filter can be checked by changing only the
/// subcommand. For example, at which positions are the cards of notes matching
/// `data~re:"key.Uni"` in a review of a tag and the notes it links from:
///
/// `spares card queue -q 'tag="probability:grad:2" or linked_from_note=(tag="probability:grad:2")'
/// -m 'data~re:"key.Uni"'`
///
/// Learning cards held back by the learn ahead limit are listed after the queue, with when they
/// can be shown, then the matching cards the review would not show today, with why. Without
/// `--match`, the first cards of the queue are listed, as a preview of a session.
///
/// A position holds only until the next card is rated. A query with `limit` is looked up in the
/// snapshot its review builds, so start the review first.
#[derive(Args, Debug)]
pub(crate) struct QueueArgs {
    #[command(flatten)]
    pub(crate) filter_args: FilterArgs,
    /// The cards to locate, as a search query. Card filters such as `c.state=0` pick cards
    /// rather than whole notes.
    #[arg(short, long = "match")]
    pub(crate) match_query: Option<String>,
    /// The most cards of the queue to list without `--match`
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u32,
    /// Print the response as JSON, for scripts
    #[arg(long)]
    pub(crate) json: bool,
}

pub(crate) async fn print_queue_positions(
    QueueArgs {
        filter_args,
        match_query,
        limit,
        json,
    }: QueueArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    let filter = filter_args_to_filter(&filter_args, base_url, client)
        .await
        .map_err(|e| miette!("{e}"))?;
    let request = QueuePositionRequest {
        filter,
        limit: match_query.is_none().then_some(limit),
        match_query,
    };
    let response = client
        .post(format!("{base_url}/api/review/queue/position"))
        .json(&request)
        .send()
        .await
        .map_err(|e| miette!("{}", e))?;
    let response: QueuePositionResponse = ensure_ok(response)
        .await?
        .json()
        .await
        .map_err(|e| miette!("{}", e))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&response).unwrap());
        return Ok(());
    }
    // Schedulers that share a state id give it the same role, so the default's names do for
    // cards of any scheduler
    let states = fetch_states(get_default_scheduler_name(), base_url, client)
        .await
        .map_err(|e| miette!("{e}"))?;

    let cards = |n: u32| if n == 1 { "card" } else { "cards" };
    if request.match_query.is_some() {
        println!(
            "{} {} in this review. {} {}:",
            response.total,
            cards(response.total),
            response.matching,
            if response.matching == 1 {
                "matches"
            } else {
                "match"
            }
        );
    } else {
        println!(
            "{} {} in this review{}",
            response.total,
            cards(response.total),
            if response.positions.len() < response.total as usize {
                format!(". The first {}:", response.positions.len())
            } else {
                ":".to_string()
            }
        );
    }
    let width = terminal_width();
    if !response.positions.is_empty() {
        let rows = response
            .positions
            .iter()
            .map(|p| (p.position.to_string(), &p.card))
            .collect::<Vec<_>>();
        print_table("pos", &rows, &states, width);
    }
    if !response.held_back.is_empty() {
        println!("\nHeld back by the learn ahead limit, shown once nothing else is left:");
        let rows = response
            .held_back
            .iter()
            .map(|card| (format!("from {}", time(card.shown_from)), &card.card))
            .collect::<Vec<_>>();
        print_table("shown", &rows, &states, width);
    }
    if !response.not_shown.is_empty() {
        println!("\nNot shown today:");
        let rows = response
            .not_shown
            .iter()
            .map(|card| (reason(&card.reason), &card.card))
            .collect::<Vec<_>>();
        print_table("reason", &rows, &states, width);
        let more = response.not_shown_total as usize - response.not_shown.len();
        if more > 0 {
            println!("  and {more} more");
        }
    }
    if response.not_in_review > 0 {
        println!(
            "\n{} matching {} not in this review's filter.",
            response.not_in_review,
            if response.not_in_review == 1 {
                "card is"
            } else {
                "cards are"
            }
        );
    }
    println!("\nPositions change as you rate cards.");
    Ok(())
}

/// Prints `rows` as a table: the first column, headed `heading` and aligned right if it is
/// numbers, then each card's note, card order, state and due date, and the start of its note's text
/// cut to `width`.
fn print_table(heading: &str, rows: &[(String, &QueueCard)], states: &[CardState], width: usize) {
    let first_width = rows
        .iter()
        .map(|(first, _)| first.chars().count())
        .chain([heading.len()])
        .max()
        .unwrap_or(0);
    let note_width = rows
        .iter()
        .map(|(_, card)| card.note_id.letters().len())
        .chain([4])
        .max()
        .unwrap_or(4);
    let numbers = rows
        .iter()
        .all(|(first, _)| first.chars().all(|c| c.is_ascii_digit()));
    let pad = |first: &str| {
        if numbers {
            format!("{first:>first_width$}")
        } else {
            format!("{first:first_width$}")
        }
    };
    let header = format!(
        "  {}  {:note_width$}  card  {:10}  {:16}",
        pad(heading),
        "note",
        "state",
        "due"
    );
    println!("{}", header.trim_end());
    for (first, card) in rows {
        let line = format!(
            "  {}  {:note_width$}  {:>4}  {:10}  {:16}  ",
            pad(first),
            card.note_id.letters(),
            card.card_order,
            state_label(states, card.card_state),
            time(card.due),
        );
        let room = width.saturating_sub(line.chars().count());
        let text: String = card.note_preview.chars().take(room).collect();
        println!("{}", format!("{line}{text}").trim_end());
    }
}

fn reason(reason: &NotShownReason) -> String {
    let limit = |kind: &str, preset: &Option<String>| match preset {
        Some(preset) => format!("{kind} limit of {preset}"),
        None => format!("{kind} limit"),
    };
    match reason {
        NotShownReason::Suspended => "suspended".to_string(),
        NotShownReason::Buried => "buried".to_string(),
        NotShownReason::NotDue => "not due".to_string(),
        NotShownReason::WaitingOnLinkedNote => "waits on a linked note".to_string(),
        NotShownReason::NewLimit { preset } => limit("new", preset),
        NotShownReason::ReviewLimit { preset } => limit("review", preset),
    }
}

fn time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn terminal_width() -> usize {
    terminal_size::terminal_size().map_or(usize::MAX, |(width, _)| usize::from(width.0))
}
