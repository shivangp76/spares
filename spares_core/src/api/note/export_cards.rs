//! Exporting cards for other programs, with `spares note export --format csv|apkg`: a CSV of each
//! card's front and back, or an Anki package (`.apkg`), so trying spares is less of a commitment.
//!
//! ## What a card becomes
//!
//! Each spares card (a cloze group) is one row of the CSV, and one note of Anki's Cloze note type
//! in the package, with its clozes as `{{c1::...}}`. Its front is the note's text with the card's
//! clozes hidden as `[...]`, and its back their text, both in the note's markup. Its tags are its
//! note's and its own, with `:` written as Anki's `::` in a package.
//!
//! Anki shows HTML, so in a package Markdown becomes simple HTML (paragraphs, line breaks, and
//! `$...$` math as `MathJax`'s `\(...\)`), LaTeX is wrapped in Anki's `[latex]...[/latex]`, which
//! Anki renders with its own LaTeX, and Typst, which Anki can't render, is kept as its source.
//! The cards' rendered files are PDFs, which Anki can't show inline, so they aren't used.
//!
//! ## What is lost
//!
//! Links between notes and keyword links only mean something in spares, so they stay as text, and
//! the export says how many there were ("12 links left out"). So does anything spares-only in a
//! note's markup, such as image occlusions, which become their text.
//!
//! ## Review history
//!
//! With `with_history`, a package keeps each card's schedule: its state, due date and interval,
//! its FSRS memory (which Anki reads from a card's `data`), and its reviews as Anki's `revlog`, so
//! Anki shows the card when it is due. Without it, every card is new. The CSV has the due date and
//! interval as columns instead.
// The package is written only on a server, which most of the rest serves
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use std::collections::HashMap;
use std::fmt::Write as _;

use base64::Engine as _;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::Error;
#[cfg(target_family = "wasm")]
use crate::LibraryError;
use crate::api::note::get_render_note_data;
use crate::model::NoteId;
use crate::parsers::NotePart;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;
use crate::parsers::get_cards;
use crate::search::evaluator::Evaluator;

/// What to export the cards a query's notes have as.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardExportFormat {
    #[default]
    Csv,
    Apkg,
}

/// `POST /api/notes/export/cards`
#[derive(Debug, Deserialize, Serialize)]
pub struct ExportCardsRequest {
    /// The notes whose cards to export
    pub query: String,
    #[serde(default)]
    pub format: CardExportFormat,
    /// In a package, keep each card's schedule and reviews, so Anki carries them on. Off by
    /// default, which makes every card new there.
    #[serde(default)]
    pub with_history: bool,
}

/// The exported file, and what it left out.
#[derive(Debug, Deserialize, Serialize)]
pub struct ExportCardsResponse {
    /// The file, base64-encoded
    pub file: String,
    /// Such as `spares.csv`
    pub file_name: String,
    pub note_count: usize,
    pub card_count: usize,
    /// Links to other notes and keyword links in the notes, which only spares follows
    pub links_left_out: usize,
}

impl ExportCardsResponse {
    /// The file's bytes.
    pub fn bytes(&self) -> Result<Vec<u8>, base64::DecodeError> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(&self.file)
    }
}

/// A card to export, in plain parts.
#[derive(Clone, Debug, PartialEq)]
struct ExportCard {
    note_id: NoteId,
    order: u32,
    parser: String,
    /// The note's text with this card's clozes marked, as (text, `is_cloze`) pieces
    pieces: Vec<(String, bool)>,
    tags: Vec<String>,
    schedule: Option<Schedule>,
}

/// A card's schedule, from the database.
#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
struct Schedule {
    card_id: i64,
    state: i64,
    special_state: Option<i64>,
    due: i64,
    stability: f64,
    difficulty: f64,
    desired_retention: f64,
    last_review: Option<i64>,
}

impl ExportCard {
    fn front(&self) -> String {
        self.pieces
            .iter()
            .map(|(text, cloze)| if *cloze { "[...]" } else { text.as_str() })
            .collect()
    }

    fn back(&self) -> String {
        self.pieces
            .iter()
            .filter(|(_, cloze)| *cloze)
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Exports the cards of the notes `request.query` finds (see the module docs).
pub async fn export_cards(
    db: &SqlitePool,
    request: ExportCardsRequest,
    now: DateTime<Utc>,
) -> Result<ExportCardsResponse, Error> {
    let note_ids = Evaluator::new(&request.query).get_note_ids(db).await?;
    let notes = if note_ids.is_empty() {
        Vec::new()
    } else {
        get_render_note_data(db, Some(note_ids)).await?
    };
    let all_parsers = get_all_parsers();
    let schedules: Vec<(i64, u32, Schedule)> = sqlx::query_as::<_, (i64, u32, i64, i64, Option<i64>, i64, f64, f64, f64, Option<i64>)>(
        r#"SELECT c.note_id, c."order", c.id, c.state, c.special_state, c.due, c.stability,
                  c.difficulty, c.desired_retention,
                  (SELECT MAX(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = 0)
           FROM card c"#,
    )
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?
    .into_iter()
    .map(|(note, order, card_id, state, special_state, due, stability, difficulty, desired_retention, last_review)| {
        (note, order, Schedule { card_id, state, special_state, due, stability, difficulty, desired_retention, last_review })
    })
    .collect();
    let mut schedule_of: HashMap<(i64, u32), Schedule> = schedules
        .into_iter()
        .map(|(note, order, schedule)| ((note, order), schedule))
        .collect();

    let mut cards = Vec::new();
    let mut links_left_out = 0;
    for note in &notes {
        let parser = find_parser(&note.parser_name, &all_parsers)?;
        links_left_out += parser.get_linked_notes(&note.data)?.len();
        links_left_out += parser.get_embedded_keywords(&note.data)?.len();
        let note_tags: Vec<String> =
            serde_json::from_value(note.tags_value.clone()).unwrap_or_default();
        let card_tags: HashMap<String, Vec<String>> =
            serde_json::from_value(note.card_tags_value.clone()).unwrap_or_default();
        for (index, card) in get_cards(parser.as_ref(), None, &note.data, false, false)?
            .into_iter()
            .enumerate()
        {
            let order = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let mut tags = note_tags.clone();
            for tag in card_tags.get(&order.to_string()).into_iter().flatten() {
                if !tags.contains(tag) {
                    tags.push(tag.clone());
                }
            }
            cards.push(ExportCard {
                note_id: note.note_id,
                order,
                parser: note.parser_name.clone(),
                pieces: pieces(&card.data)
                    .into_iter()
                    .map(|(text, cloze)| {
                        if cloze {
                            (text, cloze)
                        } else {
                            (without_clozes(parser.as_ref(), &text), cloze)
                        }
                    })
                    .collect(),
                tags,
                schedule: schedule_of.remove(&(note.note_id.packed(), order)),
            });
        }
    }

    let (bytes, extension) = match request.format {
        CardExportFormat::Csv => (csv(&cards).into_bytes(), "csv"),
        CardExportFormat::Apkg => {
            #[cfg(not(target_family = "wasm"))]
            {
                let history = if request.with_history {
                    review_history(db, &cards).await?
                } else {
                    HashMap::new()
                };
                (
                    apkg::package(&cards, request.with_history, &history, now).await?,
                    "apkg",
                )
            }
            #[cfg(target_family = "wasm")]
            {
                let _ = now;
                return Err(Error::Library(LibraryError::InvalidConfig(
                    "An Anki package is made on the server".to_string(),
                )));
            }
        }
    };
    Ok(ExportCardsResponse {
        file: base64::engine::general_purpose::STANDARD.encode(bytes),
        file_name: format!("spares.{extension}"),
        note_count: notes.len(),
        card_count: cards.len(),
        links_left_out,
    })
}

/// A card's parts as text, with its own clozes marked.
fn pieces(parts: &[NotePart]) -> Vec<(String, bool)> {
    let mut pieces: Vec<(String, bool)> = Vec::new();
    let mut in_cloze = false;
    let push = |text: &str, cloze: bool, pieces: &mut Vec<(String, bool)>| match pieces.last_mut() {
        Some((last, last_cloze)) if *last_cloze == cloze => last.push_str(text),
        _ => pieces.push((text.to_string(), cloze)),
    };
    for part in parts {
        match part {
            NotePart::ClozeStart(_) => {
                in_cloze = true;
                // A cloze of its own, even right after another
                pieces.push((String::new(), true));
            }
            NotePart::ClozeEnd(_) => in_cloze = false,
            NotePart::ClozeData(text, _) => push(text, true, &mut pieces),
            NotePart::SurroundingData(text) => push(text, in_cloze, &mut pieces),
            NotePart::ImageOcclusion { .. } | NotePart::Cli { .. } => {}
        }
    }
    pieces.retain(|(text, cloze)| !*cloze || !text.is_empty());
    pieces
}

/// `text` with the clozes of other cards in it shown as their text, without their delimiters and
/// settings.
fn without_clozes(parser: &dyn crate::parsers::Parseable, text: &str) -> String {
    let Ok(clozes) = parser.get_clozes(text) else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for cloze in clozes {
        if cloze.start_match.start < at {
            continue;
        }
        out.push_str(&text[at..cloze.start_match.start]);
        out.push_str(&text[cloze.start_match.end..cloze.end_match.start]);
        at = cloze.end_match.end;
    }
    out.push_str(&text[at..]);
    out
}

/// A CSV of the cards: front, back, tags, then the due date and interval of those reviewed.
fn csv(cards: &[ExportCard]) -> String {
    let mut out = String::from("front,back,tags,due,interval_days\n");
    for card in cards {
        let (due, interval) = card
            .schedule
            .as_ref()
            .filter(|s| s.last_review.is_some())
            .map_or((String::new(), String::new()), |s| {
                let due = DateTime::from_timestamp(s.due, 0)
                    .map(|d| d.date_naive().to_string())
                    .unwrap_or_default();
                let interval = s
                    .last_review
                    .map(|last| {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "seconds are far below 2^52"
                        )]
                        let days = (s.due - last) as f64 / 86_400.0;
                        format!("{days:.1}")
                    })
                    .unwrap_or_default();
                (due, interval)
            });
        let fields = [
            card.front().trim().to_string(),
            card.back().trim().to_string(),
            card.tags.join(" "),
            due,
            interval,
        ];
        let line = fields
            .iter()
            .map(|f| csv_field(f))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// A field, quoted if it holds a comma, a quote or a newline.
fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// A review of a card, for Anki's `revlog`.
#[derive(Clone, Debug, sqlx::FromRow)]
struct Review {
    card_id: i64,
    reviewed_at: i64,
    rating: Option<i64>,
    scheduled_time: Option<i64>,
    previous_interval: Option<i64>,
    recall_duration: Option<i64>,
    rate_duration: Option<i64>,
    previous_state: i64,
}

#[cfg(not(target_family = "wasm"))]
async fn review_history(
    db: &SqlitePool,
    cards: &[ExportCard],
) -> Result<HashMap<i64, Vec<Review>>, Error> {
    let ids: std::collections::HashSet<i64> = cards
        .iter()
        .filter_map(|c| c.schedule.as_ref().map(|s| s.card_id))
        .collect();
    let reviews: Vec<Review> = sqlx::query_as(
        "SELECT card_id, reviewed_at, rating, scheduled_time, previous_interval, recall_duration,
                rate_duration, previous_state
         FROM review_log WHERE kind = 0 AND card_id IS NOT NULL ORDER BY reviewed_at, id",
    )
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let mut by_card: HashMap<i64, Vec<Review>> = HashMap::new();
    for review in reviews {
        if ids.contains(&review.card_id) {
            by_card.entry(review.card_id).or_default().push(review);
        }
    }
    Ok(by_card)
}

/// `text`, in `parser`'s markup, as the HTML of an Anki field (see the module docs), with its
/// cloze, if `cloze`, as `{{c1::...}}`.
fn anki_html(parser: &str, pieces: &[(String, bool)]) -> String {
    // Converted as one text, between markers of the private use area, so paragraphs and math
    // spanning a cloze are seen whole
    const START: char = '\u{e000}';
    const END: char = '\u{e001}';
    let mut source = String::new();
    for (piece, cloze) in pieces {
        if *cloze {
            // `}}` would end the cloze early
            let _ = write!(source, "{START}{}{END}", piece.replace("}}", "} }"));
        } else {
            source.push_str(piece);
        }
    }
    let converted = match parser {
        "markdown" => markdown_html(&source),
        "latex-note" => html_escape(&source),
        _ => html_escape(&source).replace('\n', "<br>"),
    };
    let text = converted.replace(START, "{{c1::").replace(END, "}}");
    match parser {
        "latex-note" => format!("[latex]{text}[/latex]"),
        "typst" => {
            format!("<div style=\"white-space: pre-wrap; font-family: monospace\">{text}</div>")
        }
        _ => text,
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Markdown as simple HTML: paragraphs, line breaks, and `$...$` or `$$...$$` math as `MathJax`'s
/// `\(...\)` and `\[...\]`, which Anki renders. Other markup is left as written.
fn markdown_html(text: &str) -> String {
    let escaped = html_escape(text);
    let mut out = String::new();
    let mut rest = escaped.as_str();
    while let Some(start) = rest.find('$') {
        let display = rest[start..].starts_with("$$");
        let delimiter = if display { "$$" } else { "$" };
        let after = &rest[start + delimiter.len()..];
        let Some(end) = after.find(delimiter) else {
            break;
        };
        out.push_str(&rest[..start]);
        let (open, close) = if display {
            ("\\[", "\\]")
        } else {
            ("\\(", "\\)")
        };
        let _ = write!(out, "{open}{}{close}", &after[..end]);
        rest = &after[end + delimiter.len()..];
    }
    out.push_str(rest);
    out.split("\n\n")
        .map(|paragraph| paragraph.trim().replace('\n', "<br>"))
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("<br><br>")
}

/// Anki's tags, `::` for a child tag, separated and surrounded by spaces.
fn anki_tags(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let tags: Vec<String> = tags
        .iter()
        .map(|t| t.replace(':', "::").replace(' ', "_"))
        .collect();
    format!(" {} ", tags.join(" "))
}

/// SHA-1, for Anki's note checksum: the first 8 hex digits of the SHA-1 of its sort field, as a
/// number.
#[expect(clippy::many_single_char_names, reason = "SHA-1's own names")]
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Anki's checksum of a note's sort field: its first 32 bits of SHA-1, of the field without HTML.
fn anki_checksum(field: &str) -> i64 {
    let mut plain = String::new();
    let mut in_tag = false;
    for c in field.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => plain.push(c),
            _ => {}
        }
    }
    let hash = sha1(plain.as_bytes());
    i64::from(u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]))
}

#[cfg(not(target_family = "wasm"))]
mod apkg {
    //! The legacy Anki package: a zip of `collection.anki2`, a SQLite collection of Anki's schema
    //! 11, and `media`, a JSON object of the media files, none here. Anki 2.1 and later import it.
    use std::collections::HashMap;
    use std::io::Write as _;

    use chrono::DateTime;
    use chrono::Utc;
    use serde_json::json;
    use sqlx::Connection as _;
    use sqlx::SqliteConnection;

    use super::ExportCard;
    use super::Review;
    use super::anki_checksum;
    use super::anki_html;
    use super::anki_tags;
    use crate::Error;
    use crate::LibraryError;
    use crate::schedulers::state_role;
    use crate::schema::review::StateRole;

    const MODEL_ID: i64 = 1_700_000_000_001;
    const DECK_ID: i64 = 1_700_000_000_002;
    const SCHEMA: &str = r"
        CREATE TABLE col (id integer primary key, crt integer not null, mod integer not null,
            scm integer not null, ver integer not null, dty integer not null, usn integer not null,
            ls integer not null, conf text not null, models text not null, decks text not null,
            dconf text not null, tags text not null);
        CREATE TABLE notes (id integer primary key, guid text not null, mid integer not null,
            mod integer not null, usn integer not null, tags text not null, flds text not null,
            sfld integer not null, csum integer not null, flags integer not null,
            data text not null);
        CREATE TABLE cards (id integer primary key, nid integer not null, did integer not null,
            ord integer not null, mod integer not null, usn integer not null,
            type integer not null, queue integer not null, due integer not null,
            ivl integer not null, factor integer not null, reps integer not null,
            lapses integer not null, left integer not null, odue integer not null,
            odid integer not null, flags integer not null, data text not null);
        CREATE TABLE revlog (id integer primary key, cid integer not null, usn integer not null,
            ease integer not null, ivl integer not null, lastIvl integer not null,
            factor integer not null, time integer not null, type integer not null);
        CREATE TABLE graves (usn integer not null, oid integer not null, type integer not null);
        CREATE INDEX ix_notes_usn on notes (usn);
        CREATE INDEX ix_cards_usn on cards (usn);
        CREATE INDEX ix_revlog_usn on revlog (usn);
        CREATE INDEX ix_cards_nid on cards (nid);
        CREATE INDEX ix_cards_sched on cards (did, queue, due);
        CREATE INDEX ix_revlog_cid on revlog (cid);
        CREATE INDEX ix_notes_csum on notes (csum);";

    fn sqlx_error(e: sqlx::Error) -> Error {
        Error::Sqlx { source: e }
    }

    /// The package of `cards`, with their schedules and `history` if `with_history`.
    pub(super) async fn package(
        cards: &[ExportCard],
        with_history: bool,
        history: &HashMap<i64, Vec<Review>>,
        now: DateTime<Utc>,
    ) -> Result<Vec<u8>, Error> {
        // A file of its own, removed once read
        let path = std::env::temp_dir().join(format!(
            "spares-apkg-{}-{}.anki2",
            std::process::id(),
            now.timestamp_nanos_opt().unwrap_or_default()
        ));
        let collection = write_collection(&path, cards, with_history, history, now).await;
        let _ = std::fs::remove_file(&path);
        let collection = collection?;
        zip_package(&collection)
    }

    /// Writes the collection of the package to `path`, and returns it.
    #[expect(
        clippy::too_many_lines,
        reason = "one row of each table per card, in order"
    )]
    async fn write_collection(
        path: &std::path::Path,
        cards: &[ExportCard],
        with_history: bool,
        history: &HashMap<i64, Vec<Review>>,
        now: DateTime<Utc>,
    ) -> Result<Vec<u8>, Error> {
        let mut db = SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .map_err(sqlx_error)?;
        sqlx::raw_sql(SCHEMA)
            .execute(&mut db)
            .await
            .map_err(sqlx_error)?;
        let now_s = now.timestamp();
        // The collection's day 0, which review cards' due days count from
        let crt = now_s - now_s.rem_euclid(86_400) - 30 * 86_400 * 365;
        sqlx::query(
            "INSERT INTO col VALUES (1, ?, ?, ?, 11, 0, 0, 0, ?, ?, ?, ?, '{}')",
        )
        .bind(crt)
        .bind(now_s * 1000)
        .bind(now_s * 1000)
        .bind(json!({"nextPos": cards.len() + 1, "curDeck": DECK_ID, "activeDecks": [DECK_ID], "schedVer": 2}).to_string())
        .bind(models(now_s).to_string())
        .bind(decks(now_s).to_string())
        .bind(dconf().to_string())
        .execute(&mut db)
        .await
        .map_err(sqlx_error)?;

        let mut revlog_ids = std::collections::HashSet::new();
        for (i, card) in cards.iter().enumerate() {
            let id = now_s * 1000 + i64::try_from(i).unwrap_or(0);
            let text = anki_html(&card.parser, &card.pieces);
            sqlx::query("INSERT INTO notes VALUES (?, ?, ?, ?, -1, ?, ?, ?, ?, 0, '')")
                .bind(id)
                .bind(format!("spares-{}-{}", card.note_id.letters(), card.order))
                .bind(MODEL_ID)
                .bind(now_s)
                .bind(anki_tags(&card.tags))
                .bind(format!("{text}\u{1f}"))
                .bind(&text)
                .bind(anki_checksum(&text))
                .execute(&mut db)
                .await
                .map_err(sqlx_error)?;
            let schedule = card.schedule.as_ref().filter(|_| with_history);
            let reviews = schedule
                .and_then(|s| history.get(&s.card_id))
                .map_or(&[][..], Vec::as_slice);
            let (kind, mut queue, due, ivl, data) = match schedule {
                Some(s) if s.last_review.is_some() => {
                    let role = state_role(u32::try_from(s.state).unwrap_or(0));
                    let ivl = s
                        .last_review
                        .map_or(0, |last| ((s.due - last) / 86_400).max(1));
                    let data =
                        json!({"s": s.stability, "d": s.difficulty, "dr": s.desired_retention})
                            .to_string();
                    if role == Some(StateRole::Review) {
                        (2, 2, (s.due - crt).div_euclid(86_400), ivl, data)
                    } else {
                        // In its steps: due at a time, in seconds
                        let kind = if role == Some(StateRole::Relearning) {
                            3
                        } else {
                            1
                        };
                        (kind, 1, s.due, 0, data)
                    }
                }
                _ => (0, 0, i64::try_from(i + 1).unwrap_or(0), 0, String::new()),
            };
            if schedule.is_some_and(|s| s.special_state == Some(1)) {
                queue = -1;
            }
            let lapses = reviews
                .iter()
                .filter(|r| r.rating == Some(1) && r.previous_state == 2)
                .count();
            sqlx::query("INSERT INTO cards VALUES (?, ?, ?, 0, ?, -1, ?, ?, ?, ?, 2500, ?, ?, 0, 0, 0, 0, ?)")
                .bind(id)
                .bind(id)
                .bind(DECK_ID)
                .bind(now_s)
                .bind(kind)
                .bind(queue)
                .bind(due)
                .bind(ivl)
                .bind(i64::try_from(reviews.len()).unwrap_or(0))
                .bind(i64::try_from(lapses).unwrap_or(0))
                .bind(data)
                .execute(&mut db)
                .await
                .map_err(sqlx_error)?;
            for review in reviews {
                // Ids are milliseconds, unique across the log
                let mut revlog_id = review.reviewed_at * 1000;
                while !revlog_ids.insert(revlog_id) {
                    revlog_id += 1;
                }
                // Days, or negative seconds under a day, as Anki keeps them
                let interval = |seconds: Option<i64>| {
                    seconds.map_or(0, |s| if s >= 86_400 { s / 86_400 } else { -s })
                };
                let review_type = match review.previous_state {
                    0 | 1 => 0,
                    2 => 1,
                    _ => 2,
                };
                let time = (review.recall_duration.unwrap_or(0)
                    + review.rate_duration.unwrap_or(0))
                    * 1000;
                sqlx::query("INSERT INTO revlog VALUES (?, ?, -1, ?, ?, ?, 2500, ?, ?)")
                    .bind(revlog_id)
                    .bind(id)
                    .bind(review.rating.unwrap_or(3).clamp(1, 4))
                    .bind(interval(review.scheduled_time))
                    .bind(interval(review.previous_interval))
                    .bind(time.min(60_000 * 60))
                    .bind(review_type)
                    .execute(&mut db)
                    .await
                    .map_err(sqlx_error)?;
            }
        }
        db.close().await.map_err(sqlx_error)?;
        std::fs::read(path).map_err(|e| Error::Io {
            description: "Failed to read the package's collection".to_string(),
            source: e,
        })
    }

    /// The package: the collection and an empty `media`, zipped.
    fn zip_package(collection: &[u8]) -> Result<Vec<u8>, Error> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let zip_error = |e: zip::result::ZipError| {
            Error::Library(LibraryError::InvalidConfig(format!(
                "Failed to write the package: {e}"
            )))
        };
        zip.start_file("collection.anki2", options)
            .map_err(zip_error)?;
        zip.write_all(collection).map_err(|e| Error::Io {
            description: "Failed to write the package".to_string(),
            source: e,
        })?;
        zip.start_file("media", options).map_err(zip_error)?;
        zip.write_all(b"{}").map_err(|e| Error::Io {
            description: "Failed to write the package".to_string(),
            source: e,
        })?;
        Ok(zip.finish().map_err(zip_error)?.into_inner())
    }

    fn models(now: i64) -> serde_json::Value {
        let field = |name: &str, ord: u32| {
            json!({"name": name, "ord": ord, "sticky": false, "rtl": false, "font": "Arial",
                   "size": 20, "media": []})
        };
        json!({ MODEL_ID.to_string(): {
            "id": MODEL_ID,
            "name": "Cloze (spares)",
            "type": 1,
            "mod": now,
            "usn": -1,
            "sortf": 0,
            "did": DECK_ID,
            "tmpls": [{
                "name": "Cloze", "ord": 0, "qfmt": "{{cloze:Text}}",
                "afmt": "{{cloze:Text}}<br>\n{{Back Extra}}", "did": null, "bqfmt": "", "bafmt": ""
            }],
            "flds": [field("Text", 0), field("Back Extra", 1)],
            "css": ".card { font-family: arial; font-size: 20px; text-align: left; color: black; background-color: white; }\n.cloze { font-weight: bold; color: blue; }\n.nightMode .cloze { color: lightblue; }",
            "latexPre": "\\documentclass[12pt]{article}\n\\special{papersize=3in,5in}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amssymb,amsmath}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0in}\n\\begin{document}\n",
            "latexPost": "\\end{document}",
            "latexsvg": false,
            "req": [[0, "any", [0]]],
            "tags": [],
            "vers": []
        }})
    }

    fn decks(now: i64) -> serde_json::Value {
        let deck = |id: i64, name: &str| {
            json!({"id": id, "name": name, "mod": now, "usn": -1, "lrnToday": [0, 0],
                   "revToday": [0, 0], "newToday": [0, 0], "timeToday": [0, 0],
                   "collapsed": false, "browserCollapsed": false, "desc": "", "dyn": 0,
                   "conf": 1, "extendNew": 0, "extendRev": 0})
        };
        json!({"1": deck(1, "Default"), DECK_ID.to_string(): deck(DECK_ID, "spares")})
    }

    fn dconf() -> serde_json::Value {
        json!({"1": {
            "id": 1, "name": "Default", "mod": 0, "usn": 0, "maxTaken": 60, "autoplay": true,
            "timer": 0, "replayq": true, "dyn": false,
            "new": {"bury": false, "delays": [1.0, 10.0], "initialFactor": 2500,
                    "ints": [1, 4, 0], "order": 1, "perDay": 20},
            "lapse": {"delays": [10.0], "leechAction": 1, "leechFails": 8, "minInt": 1,
                      "mult": 0.0},
            "rev": {"bury": false, "ease4": 1.3, "ivlFct": 1.0, "maxIvl": 36500,
                    "perDay": 200, "hardFactor": 1.2}
        }})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(pieces: &[(&str, bool)]) -> ExportCard {
        ExportCard {
            note_id: NoteId::UNSET,
            order: 1,
            parser: "markdown".to_string(),
            pieces: pieces.iter().map(|(t, c)| ((*t).to_string(), *c)).collect(),
            tags: vec!["geo:europe".to_string()],
            schedule: None,
        }
    }

    #[test]
    fn sha1_matches_the_standard() {
        let hex = |h: [u8; 20]| {
            h.iter().fold(String::new(), |mut s, b| {
                let _ = write!(s, "{b:02x}");
                s
            })
        };
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // Anki's checksum is its first 8 hex digits, of the field without HTML
        assert_eq!(anki_checksum("<b>abc</b>"), 0xa999_3e36);
    }

    #[test]
    fn a_card_is_its_front_and_back() {
        let card = card(&[
            ("The capital of France is ", false),
            ("Paris", true),
            (", of Italy Rome.", false),
        ]);
        assert_eq!(
            card.front(),
            "The capital of France is [...], of Italy Rome."
        );
        assert_eq!(card.back(), "Paris");
        assert_eq!(
            csv(&[card]),
            "front,back,tags,due,interval_days\n\"The capital of France is [...], of Italy Rome.\",Paris,geo:europe,,\n"
        );
    }

    #[test]
    fn anki_html_marks_the_cloze_in_each_markup() {
        let pieces = vec![
            ("Euler: $e^{i\\pi} + 1 = 0$, so <b>".to_string(), false),
            ("1 & 2".to_string(), true),
            ("\n\nNext".to_string(), false),
        ];
        assert_eq!(
            anki_html("markdown", &pieces),
            "Euler: \\(e^{i\\pi} + 1 = 0\\), so &lt;b&gt;{{c1::1 &amp; 2}}<br><br>Next"
        );
        assert!(anki_html("latex-note", &pieces).starts_with("[latex]Euler"));
        assert!(anki_html("typst", &pieces).contains("{{c1::1 &amp; 2}}"));
        assert_eq!(
            anki_tags(&["math:calculus".to_string(), "a b".to_string()]),
            " math::calculus a_b "
        );
    }

    #[sqlx::test]
    async fn every_card_of_the_notes_found_is_exported(pool: SqlitePool) {
        let parser = crate::api::parser::tests::create_parser_helper(&pool, "markdown").await;
        crate::api::note::create_notes(
            &pool,
            crate::schema::note::CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![crate::schema::note::CreateNoteRequest {
                    data: "{{Paris}} is in France and {{Berlin}} in Germany.".to_string(),
                    keywords: vec![],
                    tags: vec!["geo".to_string()],
                    is_suspended: false,
                    custom_data: serde_json::Map::new(),
                }],
            },
            Utc::now(),
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();
        let request = |format| ExportCardsRequest {
            query: "tag=geo".to_string(),
            format,
            with_history: true,
        };
        let exported = export_cards(&pool, request(CardExportFormat::Csv), Utc::now())
            .await
            .unwrap();
        assert_eq!((exported.note_count, exported.card_count), (1, 2));
        let file = base64::engine::general_purpose::STANDARD
            .decode(&exported.file)
            .unwrap();
        let csv = String::from_utf8(file).unwrap();
        assert!(
            csv.contains("[...] is in France and Berlin in Germany.,Paris,geo"),
            "{csv}"
        );
        assert!(
            csv.contains("Paris is in France and [...] in Germany.,Berlin,geo"),
            "{csv}"
        );

        let package = export_cards(&pool, request(CardExportFormat::Apkg), Utc::now())
            .await
            .unwrap();
        assert_eq!(package.file_name, "spares.apkg");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&package.file)
            .unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut collection = Vec::new();
        std::io::Read::read_to_end(
            &mut zip.by_name("collection.anki2").unwrap(),
            &mut collection,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.anki2");
        std::fs::write(&path, collection).unwrap();
        let anki = SqlitePool::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let fields: Vec<String> = sqlx::query_scalar("SELECT flds FROM notes ORDER BY id")
            .fetch_all(&anki)
            .await
            .unwrap();
        assert_eq!(
            fields,
            [
                "{{c1::Paris}} is in France and Berlin in Germany.\u{1f}",
                "Paris is in France and {{c1::Berlin}} in Germany.\u{1f}"
            ]
        );
        let cards: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cards WHERE type = 0")
            .fetch_one(&anki)
            .await
            .unwrap();
        assert_eq!(cards, 2, "new cards, never reviewed");
    }
}
