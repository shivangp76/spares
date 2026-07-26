use std::collections::HashMap;
use std::fs::read_to_string;
use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use clap::Args;
use colored::Colorize;
use indicatif::ProgressBar;
use indicatif::ProgressIterator;
use spares_core::Error;
use spares_core::LibraryError;
use spares_core::ParserErrorKind;
use spares_core::adapters::SrsAdapter;
use spares_core::config::read_external_config;
use spares_core::parsers::NoteSettings;
use spares_core::parsers::Parseable;
use spares_core::parsers::get_all_parsers;
use spares_core::parsers::get_notes;

#[derive(Args, Debug)]
pub(crate) struct ImportArgs {
    // NOTE: To import to spares-local-files, refer to `spares generate`
    #[arg(short, long, default_value = "spares")]
    pub(crate) adapter: String,

    /// If this is not specified, then spares will attempt to automatically determine the parser.
    #[arg(short, long, required = false)]
    pub(crate) parser: Option<String>,

    /// Parser to convert notes to before importing
    #[arg(short, long, required = false)]
    pub(crate) to_parser: Option<String>,

    #[arg(short, long, default_value_t = false)]
    pub(crate) dry_run: bool,

    /// Input file(s)
    #[arg(required = true, value_delimiter = ' ', num_args = 1..)]
    pub(crate) files: Vec<PathBuf>,
}

fn print_notes(notes: &[(NoteSettings, Option<String>)], quiet: bool, dry_run: bool) {
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
                println!("{}", "Will Suspend Cards".purple());
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
        if dry_run {
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

/// If `parser.is_none()`, then this function will attempt to automatically determine the parser.
pub(crate) async fn import_from_files<P>(
    adapter: &mut dyn SrsAdapter,
    parser_opt: Option<&dyn Parseable>,
    to_parser_opt: Option<&dyn Parseable>,
    file_paths: &[P],
    dry_run: bool,
    quiet: bool,
) -> Result<(), Error>
where
    P: AsRef<Path>,
{
    if dry_run {
        println!("{}\n", "DRY RUN".black().on_bright_yellow());
    }

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
    let mut parser_to_notes: HashMap<&str, (&dyn Parseable, Vec<_>)> = HashMap::new();

    let external_config = read_external_config().ok();
    let count = file_paths.len();
    let progress_bar = if quiet {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(u64::try_from(count).unwrap())
    };
    for file_path in file_paths.iter().progress_with(progress_bar) {
        let file_contents = read_to_string(file_path).map_err(|e| Error::Io {
            description: format!("Failed to read {}", file_path.as_ref().display()),
            source: e,
        })?;

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
                let notes = get_notes(
                    *parser,
                    to_parser_opt,
                    block,
                    adapter,
                    !dry_run,
                    external_config.as_ref().map(|c| &c.overlapper),
                )?;
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
        if let Some(max_parser) = max_parser {
            let parser_notes_ref = parser_to_notes
                .entry(max_parser.get_parser_name())
                .or_insert((max_parser, vec![]));
            parser_notes_ref.1.extend(max_parser_all_notes);
        } else {
            if parser_opt.is_none() {
                return Err(Error::Library(LibraryError::Parser(
                    ParserErrorKind::FailedToGuess(String::new()),
                )));
            }
            // No notes to process
            return Ok(());
        }
    }

    for (_parser_name, (parser, notes)) in parser_to_notes {
        print_notes(&notes, quiet, dry_run);

        adapter
            .process_data(notes, parser, dry_run, quiet, Utc::now())
            .await?;
    }

    Ok(())
}
