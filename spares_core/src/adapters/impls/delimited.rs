//! Importing a table of cards: CSV or TSV from a file, a table pasted from Google Sheets or Excel,
//! or lines like Quizlet's export, `term - definition`.
//!
//! ## Reading the table
//!
//! The delimiter is a tab, a comma, a semicolon, or a dash between spaces (` - `, ` – `, ` — `)
//! or an em dash. Unless given, it is the one that splits the most records into the same number
//! of at least two fields, tabs first, then commas, semicolons and dashes. Tabs, commas and
//! semicolons follow CSV's quoting (RFC 4180), which spreadsheets use when they copy or export a
//! cell holding a delimiter, a quote or a newline: such a field is quoted, `"`, with each quote in
//! it doubled. A dash splits each line once, at its first dash, so the definition may have dashes
//! of its own, and has no quoting.
//!
//! The first record is a header unless said otherwise when one of its fields names a column, as
//! `front`, `back`, `term`, `definition`, `question`, `answer` or `tags` do.
//!
//! ## Making notes
//!
//! Each record becomes a note of its own whose back is a cloze, so its one card shows the front and
//! asks for the back: in Markdown `Front\n\n{{Back}}`, with the parser's own cloze for Typst and
//! LaTeX ([`Parseable::construct_cloze`]). Cells are plain text unless said otherwise, so the
//! markup's special characters in them (`*`, `#`, `$`, ...) are escaped and shown as typed.
//! Records with an empty front or back are left out, as are blank ones. A tags column is split at
//! spaces and commas, with Anki's `::` for a child tag read as spares' `:`.
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::api::note::create_notes;
use crate::api::note::share_bundle::parser_id;
use crate::parsers::Parseable;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;
use crate::schema::note::CreateNoteRequest;
use crate::schema::note::CreateNotesRequest;

/// What separates a record's fields.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delimiter {
    Tab,
    Comma,
    Semicolon,
    /// ` - `, ` – `, ` — ` or `—`, once per line
    Dash,
}

impl Delimiter {
    /// In the order detection prefers them
    const ALL: [Self; 4] = [Self::Tab, Self::Comma, Self::Semicolon, Self::Dash];

    fn char(self) -> Option<char> {
        match self {
            Self::Tab => Some('\t'),
            Self::Comma => Some(','),
            Self::Semicolon => Some(';'),
            Self::Dash => None,
        }
    }
}

/// A table read from text: its records, after the header if it has one.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Table {
    pub delimiter: Delimiter,
    pub header: Option<Vec<String>>,
    pub records: Vec<Vec<String>>,
}

impl Table {
    /// The most fields any record (or the header) has.
    pub fn column_count(&self) -> usize {
        self.header
            .iter()
            .chain(&self.records)
            .map(Vec::len)
            .max()
            .unwrap_or(0)
    }
}

/// Names of columns that say the first record is a header.
const HEADER_NAMES: [&str; 14] = [
    "front",
    "back",
    "term",
    "definition",
    "question",
    "answer",
    "tags",
    "tag",
    "word",
    "meaning",
    "prompt",
    "text",
    "extra",
    "notes",
];

/// Reads `text` as a table, with `delimiter` and `has_header` detected when not given (see the
/// module docs).
pub fn parse(text: &str, delimiter: Option<Delimiter>, has_header: Option<bool>) -> Table {
    // A BOM, which Excel starts its CSV with, isn't part of the first field
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = delimiter.unwrap_or_else(|| detect(text));
    let mut records = records(text, delimiter);
    let has_header = has_header.unwrap_or_else(|| {
        records.first().is_some_and(|first| {
            first
                .iter()
                .any(|field| HEADER_NAMES.contains(&field.trim().to_lowercase().as_str()))
        })
    });
    let header = (has_header && !records.is_empty()).then(|| records.remove(0));
    Table {
        delimiter,
        header,
        records,
    }
}

/// The delimiter that splits the most records into the same number of at least two fields.
fn detect(text: &str) -> Delimiter {
    let mut best = (Delimiter::Tab, 0);
    for delimiter in Delimiter::ALL {
        let records = records(text, delimiter);
        let mut counts = std::collections::HashMap::<usize, usize>::new();
        for record in &records {
            if record.len() >= 2 {
                *counts.entry(record.len()).or_default() += 1;
            }
        }
        let score = counts.values().copied().max().unwrap_or(0);
        // Earlier delimiters win ties
        if score > best.1 {
            best = (delimiter, score);
        }
    }
    best.0
}

/// The records of `text`, leaving out blank ones.
fn records(text: &str, delimiter: Delimiter) -> Vec<Vec<String>> {
    let records = match delimiter.char() {
        Some(c) => quoted_records(text, c),
        None => text.lines().map(split_at_dash).collect(),
    };
    records
        .into_iter()
        .filter(|record| record.iter().any(|field| !field.trim().is_empty()))
        .collect()
}

/// A line split at its first dash, or the whole line as one field.
fn split_at_dash(line: &str) -> Vec<String> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let at = [" - ", " – ", " — ", "—"]
        .iter()
        .filter_map(|dash| line.find(dash).map(|at| (at, dash.len())))
        .min();
    match at {
        Some((at, len)) => vec![
            line[..at].trim().to_string(),
            line[at + len..].trim().to_string(),
        ],
        None => vec![line.trim().to_string()],
    }
}

/// CSV's records with `delimiter` (RFC 4180): fields may be quoted, to hold the delimiter, a
/// newline or a doubled quote. A quote in a field that doesn't start with one is kept as it is,
/// as is a field whose closing quote is missing, up to the end.
fn quoted_records(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    // Whether the field so far is quoted, so a quote ends it rather than being part of it
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !quoted => {
                in_quotes = true;
                quoted = true;
            }
            c if c == delimiter => {
                record.push(std::mem::take(&mut field));
                quoted = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                quoted = false;
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() || quoted {
        record.push(field);
        records.push(record);
    }
    records
}

/// Which of a table's columns (from 0) are what.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Columns {
    pub front: usize,
    pub back: usize,
    #[serde(default)]
    pub tags: Option<usize>,
}

impl Columns {
    /// The columns a header names, else the first two as front and back. The header's names may
    /// be `front`/`back`, `term`/`definition`, `question`/`answer` or `word`/`meaning`.
    pub fn guess(table: &Table) -> Self {
        let find = |names: &[&str]| {
            table.header.as_ref().and_then(|header| {
                header
                    .iter()
                    .position(|name| names.contains(&name.trim().to_lowercase().as_str()))
            })
        };
        Self {
            front: find(&["front", "term", "question", "word", "prompt"]).unwrap_or(0),
            back: find(&["back", "definition", "answer", "meaning"]).unwrap_or(1),
            tags: find(&["tags", "tag"]),
        }
    }
}

/// A note made from a record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TableNote {
    /// The record's number in the table, from 1, after the header
    pub record: usize,
    pub front: String,
    pub back: String,
    pub tags: Vec<String>,
    /// The note's text
    pub data: String,
}

/// The notes `table`'s records make, and the numbers (from 1) of those left out for an empty
/// front or back. `escape` says the cells are plain text, whose special characters in `parser`'s
/// markup are escaped. Every note gets `tag` too, if given.
pub fn notes(
    table: &Table,
    columns: Columns,
    parser: &dyn Parseable,
    escape: bool,
    tag: Option<&str>,
) -> Result<(Vec<TableNote>, Vec<usize>), Error> {
    let width = table.column_count();
    for (name, column) in [
        ("front", Some(columns.front)),
        ("back", Some(columns.back)),
        ("tags", columns.tags),
    ] {
        if let Some(column) = column
            && column >= width
        {
            return Err(Error::Library(LibraryError::InvalidConfig(format!(
                "There is no column {} for the {name}: the table has {width}.",
                column + 1
            ))));
        }
    }
    let (start, end) = parser.construct_cloze("");
    let mut notes = Vec::new();
    let mut skipped = Vec::new();
    for (index, record) in table.records.iter().enumerate() {
        let cell = |column: usize| record.get(column).map_or("", |field| field.trim());
        let (front, back) = (cell(columns.front), cell(columns.back));
        if front.is_empty() || back.is_empty() {
            skipped.push(index + 1);
            continue;
        }
        let mut tags: Vec<String> = columns
            .tags
            .map(|column| tags_of(cell(column)))
            .unwrap_or_default();
        if let Some(tag) = tag.map(str::trim).filter(|tag| !tag.is_empty())
            && !tags.iter().any(|t| t == tag)
        {
            tags.push(tag.to_string());
        }
        let text = |cell: &str| {
            if escape {
                escape_for(parser.get_parser_name(), cell)
            } else {
                cell.to_string()
            }
        };
        notes.push(TableNote {
            record: index + 1,
            front: front.to_string(),
            back: back.to_string(),
            tags,
            data: format!("{}\n\n{start}{}{end}\n", text(front), text(back)),
        });
    }
    Ok((notes, skipped))
}

/// The tags in a tags cell, split at spaces and commas, with Anki's `::` read as `:`.
fn tags_of(cell: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in cell.split(|c: char| c.is_whitespace() || c == ',') {
        let tag = tag.replace("::", ":");
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// `text`, plain text, written so `parser`'s markup shows it as it is. Newlines are kept.
fn escape_for(parser: &str, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    match parser {
        "markdown" => {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                // What would start a heading, a list or a quote
                let rest = match line.chars().next() {
                    Some(c @ ('#' | '-' | '+' | '>')) => {
                        out.push('\\');
                        out.push(c);
                        &line[1..]
                    }
                    _ => line,
                };
                // A numbered list item, e.g. `1. `
                let digits = rest.chars().take_while(char::is_ascii_digit).count();
                let rest = if digits > 0 && rest[digits..].starts_with(". ") {
                    out.push_str(&rest[..digits]);
                    out.push('\\');
                    &rest[digits..]
                } else {
                    rest
                };
                for c in rest.chars() {
                    if "\\`*_{}[]<>$|~^".contains(c) {
                        out.push('\\');
                    }
                    out.push(c);
                }
            }
        }
        "typst" => {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    // A lone newline is a space in Typst's markup: a line break keeps it
                    out.push_str(" \\\n");
                }
                for (j, c) in line.chars().enumerate() {
                    if "\\#[]$*_`<>@~/".contains(c) || (j == 0 && "=-+".contains(c)) {
                        out.push('\\');
                    }
                    out.push(c);
                }
            }
        }
        "latex-note" => {
            for c in text.chars() {
                match c {
                    '\\' => out.push_str("\\textbackslash{}"),
                    '~' => out.push_str("\\textasciitilde{}"),
                    '^' => out.push_str("\\textasciicircum{}"),
                    '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                        out.push('\\');
                        out.push(c);
                    }
                    '\n' => out.push_str("\\\\\n"),
                    c => out.push(c),
                }
            }
        }
        _ => out.push_str(text),
    }
    out
}

/// `POST /api/notes/import/table`: a table to import as notes, or to preview with `dry_run`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ImportTableRequest {
    pub text: String,
    /// Detected when not given
    #[serde(default)]
    pub delimiter: Option<Delimiter>,
    /// Detected when not given
    #[serde(default)]
    pub has_header: Option<bool>,
    /// Guessed from the header, else the first two columns, when not given
    #[serde(default)]
    pub columns: Option<Columns>,
    /// The parser of the notes, i.e. their markup: `markdown`, `typst` or `latex-note`
    pub parser: String,
    /// Whether the cells are plain text, whose markup characters are escaped. On unless the
    /// cells are written in the parser's markup already.
    #[serde(default = "yes")]
    pub escape: bool,
    /// A tag for every note imported
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

fn yes() -> bool {
    true
}

/// What a table import read and did, or, for a dry run, would do.
#[derive(Debug, Deserialize, Serialize)]
pub struct ImportTableResponse {
    pub delimiter: Delimiter,
    pub header: Option<Vec<String>>,
    pub column_count: usize,
    /// The columns used, guessed if the request had none
    pub columns: Columns,
    pub note_count: usize,
    /// The first notes, for a preview
    pub preview: Vec<TableNote>,
    /// The records left out for an empty front or back, by number from 1 after the header
    pub skipped_records: Vec<usize>,
    /// The event that undoes the import, unless a dry run
    pub event_id: Option<i64>,
}

/// How many notes a response previews.
const PREVIEW_NOTES: usize = 5;

/// Imports `request`'s table as new notes, one per record (see the module docs), as one event that
/// undo deletes them with. With `dry_run`, only says what it would import.
pub async fn import_table(
    db: &SqlitePool,
    request: ImportTableRequest,
    at: DateTime<Utc>,
) -> Result<ImportTableResponse, Error> {
    let all_parsers = get_all_parsers();
    let parser = find_parser(&request.parser, &all_parsers)?;
    let table = parse(&request.text, request.delimiter, request.has_header);
    let columns = request.columns.unwrap_or_else(|| Columns::guess(&table));
    let (notes, skipped_records) = notes(
        &table,
        columns,
        parser.as_ref(),
        request.escape,
        request.tag.as_deref(),
    )?;
    let mut response = ImportTableResponse {
        delimiter: table.delimiter,
        column_count: table.column_count(),
        header: table.header,
        columns,
        note_count: notes.len(),
        preview: notes.iter().take(PREVIEW_NOTES).cloned().collect(),
        skipped_records,
        event_id: None,
    };
    if request.dry_run || notes.is_empty() {
        return Ok(response);
    }
    let parser_id = parser_id(db, parser.get_parser_name(), &mut Vec::new()).await?;
    let created = create_notes(
        db,
        CreateNotesRequest {
            parser_id,
            requests: notes
                .into_iter()
                .map(|note| CreateNoteRequest {
                    data: note.data,
                    keywords: Vec::new(),
                    tags: note.tags,
                    is_suspended: false,
                    custom_data: Map::new(),
                })
                .collect(),
        },
        at,
        &all_parsers,
        true,
    )
    .await?;
    response.event_id = created.event_id;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser(name: &str) -> Box<dyn Parseable> {
        find_parser(name, &get_all_parsers()).unwrap()
    }

    fn fields(table: &Table) -> Vec<Vec<&str>> {
        table
            .records
            .iter()
            .map(|r| r.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn quoted_fields_hold_commas_quotes_and_newlines() {
        let csv = "front,back\n\"Paris, France\",\"The capital\nof France\"\n\"He said \"\"hi\"\"\",greeting\r\n";
        let table = parse(csv, None, None);
        assert_eq!(table.delimiter, Delimiter::Comma);
        assert_eq!(table.header, Some(vec!["front".into(), "back".into()]));
        assert_eq!(
            fields(&table),
            [
                vec!["Paris, France", "The capital\nof France"],
                vec!["He said \"hi\"", "greeting"]
            ]
        );
    }

    #[test]
    fn a_table_pasted_from_a_spreadsheet_is_tab_separated() {
        // Sheets quotes a cell with a newline, and leaves the rest alone
        let pasted = "hola\thello\n\"buenos\ndías\"\tgood morning\nadiós\tbye, goodbye\n";
        let table = parse(pasted, None, None);
        assert_eq!(table.delimiter, Delimiter::Tab);
        assert_eq!(table.header, None, "no field names a column");
        assert_eq!(
            fields(&table),
            [
                vec!["hola", "hello"],
                vec!["buenos\ndías", "good morning"],
                vec!["adiós", "bye, goodbye"]
            ]
        );
    }

    #[test]
    fn lines_split_at_their_first_dash() {
        let text = "mitochondria - the powerhouse of the cell - famously\nwell-known — familiar\nosmosis—diffusion of water\n";
        let table = parse(text, None, None);
        assert_eq!(table.delimiter, Delimiter::Dash);
        assert_eq!(
            fields(&table),
            [
                vec!["mitochondria", "the powerhouse of the cell - famously"],
                vec!["well-known", "familiar"],
                vec!["osmosis", "diffusion of water"]
            ]
        );
    }

    #[test]
    fn semicolons_from_a_european_excel() {
        let table = parse(
            "\u{feff}Frage;Antwort\nHund;dog\nKatze;cat, kitty\n",
            None,
            Some(true),
        );
        assert_eq!(table.delimiter, Delimiter::Semicolon);
        assert_eq!(table.header, Some(vec!["Frage".into(), "Antwort".into()]));
        assert_eq!(
            fields(&table),
            [vec!["Hund", "dog"], vec!["Katze", "cat, kitty"]]
        );
    }

    #[test]
    fn a_missing_closing_quote_keeps_the_rest_and_blank_lines_are_left_out() {
        let table = parse("a,b\n\n,\nc,\"d\n", Some(Delimiter::Comma), Some(false));
        assert_eq!(fields(&table), [vec!["a", "b"], vec!["c", "d\n"]]);
    }

    #[test]
    fn a_header_says_which_columns_are_which() {
        let table = parse(
            "tags,definition,term\nbio,the powerhouse,mitochondria\n",
            None,
            None,
        );
        let columns = Columns::guess(&table);
        assert_eq!((columns.front, columns.back, columns.tags), (2, 1, Some(0)));
        let table = parse("a,b\n", None, None);
        let columns = Columns::guess(&table);
        assert_eq!((columns.front, columns.back, columns.tags), (0, 1, None));
    }

    #[test]
    fn each_record_is_a_note_whose_back_is_its_one_cloze() {
        let table = parse(
            "front,back,tags\nWhat is 2 * 3?,6,math::arithmetic quiz\nEmpty back,,\n,No front,\nLast,one,\n",
            None,
            None,
        );
        let markdown = parser("markdown");
        let (notes, skipped) = super::notes(
            &table,
            Columns::guess(&table),
            markdown.as_ref(),
            true,
            Some("imported"),
        )
        .unwrap();
        assert_eq!(skipped, [2, 3], "an empty back or front is left out");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].data, "What is 2 \\* 3?\n\n{{6}}\n");
        assert_eq!(notes[0].tags, ["math:arithmetic", "quiz", "imported"]);
        assert_eq!(notes[1].tags, ["imported"]);
        for note in &notes {
            assert_eq!(
                markdown.get_clozes(&note.data).unwrap().len(),
                1,
                "{}",
                note.data
            );
        }
    }

    #[test]
    fn every_markup_gets_its_own_cloze_with_its_special_characters_escaped() {
        let table = parse(
            "question,answer\n\"Cost of #1 item, in $\",\"$5 [sale] {50%}_off\"\n\"line one\nline two\",x^2 \\ y~z\n",
            None,
            None,
        );
        for name in ["markdown", "typst", "latex-note"] {
            let parser = parser(name);
            let (notes, skipped) =
                super::notes(&table, Columns::guess(&table), parser.as_ref(), true, None).unwrap();
            assert_eq!(skipped, Vec::<usize>::new());
            for note in &notes {
                let clozes = parser.get_clozes(&note.data).unwrap();
                assert_eq!(clozes.len(), 1, "{name}: {}", note.data);
            }
        }
        let typst = parser("typst");
        let (notes, _) =
            super::notes(&table, Columns::guess(&table), typst.as_ref(), true, None).unwrap();
        assert_eq!(
            notes[0].data,
            "Cost of \\#1 item, in \\$\n\n#cl[\\$5 \\[sale\\] {50%}\\_off]\n"
        );
        let latex = parser("latex-note");
        let (notes, _) =
            super::notes(&table, Columns::guess(&table), latex.as_ref(), true, None).unwrap();
        assert!(
            notes[0]
                .data
                .contains("\\begin{cl}\\$5 [sale] \\{50\\%\\}\\_off\\end{cl}"),
            "{}",
            notes[0].data
        );
    }

    #[test]
    fn cells_already_in_the_markup_are_kept_as_they_are() {
        let table = parse("front\tback\n*Bold* term\t$x^2$\n", None, None);
        let (notes, _) = super::notes(
            &table,
            Columns::guess(&table),
            parser("markdown").as_ref(),
            false,
            None,
        )
        .unwrap();
        assert_eq!(notes[0].data, "*Bold* term\n\n{{$x^2$}}\n");
    }

    #[sqlx::test]
    async fn importing_a_table_adds_a_note_per_record_that_undo_deletes(pool: SqlitePool) {
        let request = |dry_run| ImportTableRequest {
            text: "Term\tDefinition\nperro\tdog\ngato\tcat\nvacío\t\n".to_string(),
            delimiter: None,
            has_header: None,
            columns: None,
            parser: "markdown".to_string(),
            escape: true,
            tag: Some("spanish".to_string()),
            dry_run,
        };
        let count = || async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM card")
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        let preview = import_table(&pool, request(true), Utc::now())
            .await
            .unwrap();
        assert_eq!(preview.delimiter, Delimiter::Tab);
        assert_eq!(
            preview.header,
            Some(vec!["Term".into(), "Definition".into()])
        );
        assert_eq!(preview.note_count, 2);
        assert_eq!(preview.skipped_records, [3]);
        assert_eq!(preview.preview[0].data, "perro\n\n{{dog}}\n");
        assert_eq!(count().await, 0, "a dry run adds nothing");

        let imported = import_table(&pool, request(false), Utc::now())
            .await
            .unwrap();
        assert_eq!(count().await, 2, "a card per note");
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT t.name FROM note_tag nt JOIN tag t ON t.id = nt.tag_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(tags, ["spanish"]);
        crate::api::undo::undo_event(
            &pool,
            crate::schema::undo::UndoEventRequest {
                event_id: imported.event_id,
                undo_group: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(count().await, 0, "undone");
    }

    #[test]
    fn a_column_past_the_table_is_an_error() {
        let table = parse("a,b\n", None, None);
        let columns = Columns {
            front: 0,
            back: 2,
            tags: None,
        };
        assert!(notes(&table, columns, parser("markdown").as_ref(), true, None).is_err());
    }
}
