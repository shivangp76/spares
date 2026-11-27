//! `spares language-tools`: the dictionaries of the `markdown-language-tools` parser, on the host
//! (`/api/language-tools`).

use std::path::PathBuf;

use clap::Args;
use clap::Subcommand;
use miette::Error;
use miette::IntoDiagnostic;
use miette::miette;
use reqwest::Client;
use spares_core::parsers::language_tools::dictionary::DictionaryInfo;
use spares_core::parsers::language_tools::dictionary::UpdateDictionaryRequest;
use spares_core::parsers::language_tools::dictionary::lookup::LookupResponse;

use crate::utils::ensure_ok;

#[derive(Args, Debug)]
pub(crate) struct LanguageToolsArgs {
    #[command(subcommand)]
    pub(crate) command: LanguageToolsCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LanguageToolsCommands {
    /// The dictionaries words are looked up in, which each user imports on their host
    #[command(subcommand)]
    Dictionary(DictionaryCommands),
}

#[derive(Debug, Subcommand)]
pub(crate) enum DictionaryCommands {
    /// Import a Yomitan dictionary zip, such as Jitendex (Japanese) or CC-CEDICT (Chinese)
    ///
    /// A dictionary with the same title is replaced by a new revision.
    Import {
        file: PathBuf,
        /// The language of its words, such as `ja`, if its `index.json` doesn't say. Otherwise it
        /// is guessed from their script.
        #[arg(short, long)]
        language: Option<String>,
    },
    /// List the dictionaries, in the order their entries are shown
    List,
    /// Remove a dictionary
    Remove { id: i64 },
    /// Use a dictionary in lookups again
    Enable { id: i64 },
    /// Leave a dictionary out of lookups
    Disable { id: i64 },
    /// Look up the word a text starts with
    Lookup {
        text: String,
        /// The text's language, such as `ja`. Otherwise its script tells, or every dictionary is
        /// used.
        #[arg(short, long)]
        language: Option<String>,
    },
}

fn print_dictionary(dictionary: &DictionaryInfo) {
    println!(
        "{}\t{}\t{}\t{}\t{}",
        dictionary.id,
        if dictionary.enabled {
            "enabled"
        } else {
            "disabled"
        },
        dictionary.language.as_deref().unwrap_or("-"),
        dictionary.term_count,
        dictionary.title,
    );
}

pub(crate) async fn handle(
    args: LanguageToolsArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    let LanguageToolsCommands::Dictionary(command) = args.command;
    let dictionaries = format!("{base_url}/api/language-tools/dictionaries");
    match command {
        DictionaryCommands::Import { file, language } => {
            let bytes = std::fs::read(&file)
                .map_err(|e| miette!("Failed to read {}: {e}", file.display()))?;
            let mut request = client.post(&dictionaries).body(bytes);
            if let Some(language) = language {
                request = request.query(&[("language", language)]);
            }
            let response = request.send().await.into_diagnostic()?;
            let info: DictionaryInfo = ensure_ok(response).await?.json().await.into_diagnostic()?;
            print_dictionary(&info);
        }
        DictionaryCommands::List => {
            let response = client.get(&dictionaries).send().await.into_diagnostic()?;
            let list: Vec<DictionaryInfo> =
                ensure_ok(response).await?.json().await.into_diagnostic()?;
            for dictionary in &list {
                print_dictionary(dictionary);
            }
        }
        DictionaryCommands::Remove { id } => {
            let response = client
                .delete(format!("{dictionaries}/{id}"))
                .send()
                .await
                .into_diagnostic()?;
            ensure_ok(response).await?;
            println!("Done");
        }
        DictionaryCommands::Enable { id } | DictionaryCommands::Disable { id } => {
            let enabled = matches!(command, DictionaryCommands::Enable { .. });
            let request = UpdateDictionaryRequest {
                enabled: Some(enabled),
                language: None,
            };
            let response = client
                .patch(format!("{dictionaries}/{id}"))
                .json(&request)
                .send()
                .await
                .into_diagnostic()?;
            let info: DictionaryInfo = ensure_ok(response).await?.json().await.into_diagnostic()?;
            print_dictionary(&info);
        }
        DictionaryCommands::Lookup { text, language } => {
            let mut query = vec![("text", text)];
            query.extend(language.map(|l| ("language", l)));
            let response = client
                .get(format!("{base_url}/api/language-tools/lookup"))
                .query(&query)
                .send()
                .await
                .into_diagnostic()?;
            let response: LookupResponse =
                ensure_ok(response).await?.json().await.into_diagnostic()?;
            if response.entries.is_empty() {
                println!("No entries");
            }
            for entry in &response.entries {
                let reading = if entry.reading.is_empty() || entry.reading == entry.expression {
                    String::new()
                } else {
                    format!(" [{}]", entry.reading)
                };
                let inflections = if entry.inflections.is_empty() {
                    String::new()
                } else {
                    format!("  ({} « {})", entry.source, entry.inflections.join(" « "))
                };
                println!("{}{reading}  {}{inflections}", entry.expression, entry.tags);
                for (i, gloss) in entry.glosses.iter().enumerate() {
                    println!("  {}. {}", i + 1, gloss.replace('\n', "\n     "));
                }
                println!("  — {}", entry.dictionary);
            }
        }
    }
    Ok(())
}
