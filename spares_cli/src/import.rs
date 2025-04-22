use chrono::Utc;
use clap::Args;
use colored::Colorize;
use indicatif::ProgressIterator;
use spares::adapters::SrsAdapter;
use spares::parsers::{NoteSettings, Parseable, get_all_parsers, get_notes};
use spares::{Error, LibraryError, ParserErrorKind};
use std::fs::read_to_string;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct ImportArgs {
    // NOTE: To import to spares-local-files, refer to `spares_cli generate`
    #[arg(short, long, default_value = "spares")]
    pub adapter: String,

    /// If this is not specified, then spares will attempt to automatically determine the parser.
    #[arg(short, long, required = false)]
    pub parser: Option<String>,

    /// Parser to convert to notes to before importing
    #[arg(short, long, required = false)]
    pub to_parser: Option<String>,

    #[arg(short, long, default_value_t = false)]
    pub run: bool,

    /// Input file(s)
    #[arg(required = true, value_delimiter = ' ', num_args = 1..)]
    pub files: Vec<PathBuf>,
}

fn print_notes(notes: &[(NoteSettings, Option<String>)], quiet: bool, run: bool) {
    let warnings = notes
        .iter()
        .enumerate()
        .filter(|(_, (s, _))| !s.errors_and_warnings.is_empty())
        .map(|(i, (s, _))| (i, s.errors_and_warnings.clone()))
        .collect::<Vec<_>>();
    let notes_len = notes.len();
    let mut total_card_count = 0;
    for (i, (local_settings, note_data_res)) in notes.iter().enumerate() {
        if note_data_res.is_none() {
            continue;
        }
        if !quiet {
            let note_data = note_data_res.as_ref().unwrap();
            let card_count = local_settings.cards_count.unwrap();
            total_card_count += card_count;
            println!("Note {} (Card count: {})", i + 1, card_count);
            println!("Action:       {:?}", local_settings.action);
            println!("Tags:         {:?}", local_settings.tags);
            println!("Keywords:     {:?}", local_settings.keywords);
            println!("Linked Notes: {:?}", local_settings.linked_notes);
            if local_settings.is_suspended {
                println!("{}", "Will Suspend Cards".purple(),);
            }
            if !local_settings.custom_data.is_empty() {
                println!("Custom Data: ");
                for (key, value) in &local_settings.custom_data {
                    println!(
                        "- {}: {}",
                        key.black().on_bright_green(),
                        value.to_string().black().on_bright_green()
                    );
                }
            }
            if !local_settings.errors_and_warnings.is_empty() {
                println!("{}", "Warnings: ".black().on_bright_yellow());
                for warning in &local_settings.errors_and_warnings {
                    println!("- {:?}", warning.to_string());
                }
            }
            println!("Data: {}", note_data.green());
            println!();
        }
    }

    if !quiet {
        println!("SUMMARY");
        if !run {
            println!("{}\n", "DRY RUN".black().on_bright_yellow());
        }
        println!("Note Count: {}", notes_len);
        println!("Card Count: {}", total_card_count);
        if !warnings.is_empty() {
            println!("Warnings:");
            for (note_index, note_warnings) in &warnings {
                println!(
                    "- {} {}:",
                    "Note".black().on_yellow(),
                    (note_index + 1).to_string().black().on_yellow()
                );
                for warning in note_warnings {
                    println!("  - {:?}", miette::Report::new(warning.clone()));
                }
            }
        }
    }
}

pub async fn import_from_files(
    adapter: &mut dyn SrsAdapter,
    parser: Option<&dyn Parseable>,
    to_parser_opt: Option<&dyn Parseable>,
    files: Vec<PathBuf>,
    run: bool,
    quiet: bool,
) -> Result<(), Error> {
    let count = files.len();
    for file in files
        .into_iter()
        .progress_count(u64::try_from(count).unwrap())
    {
        import_from_file(adapter, parser, to_parser_opt, &file, run, quiet).await?;
    }
    Ok(())
}

/// If `parser.is_none()`, then this function will attempt to automatically determine the parser.
pub async fn import_from_file(
    adapter: &mut dyn SrsAdapter,
    parser_opt: Option<&dyn Parseable>,
    to_parser_opt: Option<&dyn Parseable>,
    file_path: &PathBuf,
    run: bool,
    quiet: bool,
) -> Result<(), Error> {
    if !run {
        println!("{}\n", "DRY RUN".black().on_bright_yellow());
    }

    let file_contents = read_to_string(file_path).map_err(|e| Error::Io {
        description: format!("Failed to read {}", &file_path.display()),
        source: e,
    })?;

    let all_parsers = get_all_parsers()
        .into_iter()
        .map(|x| x())
        .collect::<Vec<_>>();
    assert!(!all_parsers.is_empty(), "not possible by validation test");
    let parsers_to_try = if let Some(parser) = parser_opt {
        vec![parser]
    } else {
        all_parsers.iter().map(|x| x.as_ref()).collect::<Vec<_>>()
    };

    let mut max_parser: Option<&dyn Parseable> = None;
    let mut max_parser_all_notes = Vec::new();
    let mut max_notes_count = 0;
    for parser in &parsers_to_try {
        let mut all_notes = Vec::new();
        let blocks = parser
            .start_end_regex()
            .captures_iter(file_contents.as_str())
            .map(|c| c.unwrap().get(1).unwrap().as_str())
            .collect::<Vec<_>>();
        for block in blocks {
            let notes = get_notes(*parser, to_parser_opt, block, adapter, run)?;
            all_notes.extend(notes);
        }
        if !all_notes.is_empty() {
            if max_notes_count > 0 {
                return Err(Error::Library(LibraryError::Parser(
                    ParserErrorKind::FailedToGuess(
                        "More than one parser parsed some notes from the file.".to_string(),
                    ),
                )));
            }
            max_notes_count = all_notes.len();
            max_parser = Some(*parser);
            max_parser_all_notes = all_notes;
        }
    }
    if parsers_to_try.len() > 1 && max_notes_count == 0 {
        return Err(Error::Library(LibraryError::Parser(
            ParserErrorKind::FailedToGuess("All parsers parsed 0 notes.".to_string()),
        )));
    }
    if max_parser.is_none() && parser_opt.is_none() {
        return Err(Error::Library(LibraryError::Parser(
            ParserErrorKind::FailedToGuess(String::new()),
        )));
    }
    let max_parser_final = max_parser.unwrap_or_else(|| parser_opt.unwrap());

    let notes = max_parser_all_notes;
    let parser = to_parser_opt.unwrap_or(max_parser_final);

    print_notes(&notes, quiet, run);

    adapter
        .process_data(notes, parser, run, quiet, Utc::now())
        .await?;

    Ok(())
}
