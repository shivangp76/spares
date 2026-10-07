use std::fs::read_to_string;
use std::fs::remove_file;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::LazyLock;

use cloze_parser::ClozeParser;
use fancy_regex::Captures;
use log::warn;
use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::LibraryError;
use crate::config::get_cache_dir;
use crate::config::is_test_mode;
use crate::config::read_external_config;
use crate::helpers::get_or_compile_regex;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::ClozeMatch;
use crate::parsers::ClozeReplacement;
use crate::parsers::ConstructFileDataType;
use crate::parsers::ConstructImageOcclusionType;
use crate::parsers::GenerateNoteFilesRequest;
use crate::parsers::NoteImportAction;
use crate::parsers::NoteSettingsKeys;
use crate::parsers::Parseable;
use crate::parsers::RenderDependencies;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::RenderOutputType;
use crate::parsers::construct_card_data;
use crate::parsers::generate_files::CardSide;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::ImageOcclusionData;
use crate::parsers::image_occlusion::construct_image_occlusion_from_image;
use crate::parsers::render_command_output;
use crate::parsers::render_dependencies_filepath;
use crate::schema::note::LinkedNote;

mod cloze_parser;

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct MarkdownParserConfig {
    pub defaults_file: Option<String>,
}

/// Inspired by <https://github.com/st3v3nmw/obsidian-spaced-repetition>.
///
/// Adheres to the [CommonMark Spec](https://commonmark.org/help/).
///
/// See <https://allefeld.github.io/nerd-notes/Markdown/A%20writer's%20guide%20to%20Pandoc's%20Markdown.html>
#[derive(Clone, Copy, Debug, Default)]
pub struct MarkdownParser {}

impl MarkdownParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl Parseable for MarkdownParser {
    fn get_parser_name(&self) -> &'static str {
        "markdown"
    }

    fn get_linked_notes(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        let linked_notes_regex = get_linked_notes_regex();
        let linked_notes_data = linked_notes_regex
            .captures_iter(data)
            .filter_map(|c| c.unwrap().get(1).map(|x| x.start()..x.end()))
            .collect::<Vec<_>>();
        Ok(linked_notes_data)
    }

    fn get_embedded_keywords(&self, _data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        Ok(vec![])
    }

    fn get_settings(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        // let settings_regex = Regex::new(r"(?m)<!--- # (.*) --->").unwrap();
        let settings_regex = get_or_compile_regex(r"(?s)<!--- # ([^\n]*) --->").unwrap();
        let settings_data = settings_regex
            .captures_iter(data)
            .map(|c| c.unwrap().get(1).map(|x| x.start()..x.end()).unwrap())
            .collect::<Vec<_>>();
        Ok(settings_data)
    }

    fn get_clozes(&self, data: &str) -> Result<Vec<ClozeMatch>, LibraryError> {
        let mut all_clozes = Vec::new();
        let mut cloze_parser = ClozeParser::new(data);
        while let Some(cloze) = cloze_parser.next_cloze() {
            all_clozes.push(cloze.clone());
        }
        Ok(all_clozes.into_iter().flatten().collect::<Vec<_>>())
    }

    fn construct_cloze(&self, cloze_settings_string: &str) -> (String, String) {
        let cloze_settings_string_with_delim = if cloze_settings_string.is_empty() {
            cloze_settings_string.to_string()
        } else {
            format!("[{}]", cloze_settings_string)
        };
        let cloze_start = format!("{{{{{}", cloze_settings_string_with_delim);
        let cloze_end = "}}".to_string();
        (cloze_start, cloze_end)
    }

    fn construct_setting(&self, data: &str) -> String {
        format!("<!--- # {data} --->\n")
    }

    fn construct_comment(&self, data: &str) -> String {
        // Add trailing newline (POSIX convention)
        format!("<!--- {data} --->\n")
    }

    fn extract_comment<'a>(&self, data: &'a str) -> &'a str {
        data.strip_prefix("<!---")
            .and_then(|x| x.strip_suffix("--->"))
            .map_or(data, |x| x.trim())
    }

    #[allow(clippy::let_and_return, reason = "Make note vs card data explicit")]
    #[allow(clippy::too_many_lines, reason = "File data is long")]
    fn construct_file_data(
        &self,
        output_type: ConstructFileDataType,
        request: &GenerateNoteFilesRequest,
        note_import_action: &NoteImportAction,
        include_separator: bool,
    ) -> String {
        let GenerateNoteFilesRequest {
            note_id,
            note_data,
            keywords,
            linked_notes,
            custom_data,
            tags,
        } = request;
        let keywords_str = keywords.join(", ");
        let tags_str = tags.join(", ");
        let NoteSettingsKeys {
            action: action_key,
            action_add: action_add_key,
            settings_key_value_delim,
            custom_data: custom_data_key,
            note_id: note_id_key,
            ..
        } = self.note_settings_keys();
        match output_type {
            ConstructFileDataType::Note => {
                let (note_data, mut linked_notes_string) =
                    get_linked_notes_string(self, note_data.as_str(), linked_notes.as_ref());
                if !linked_notes_string.is_empty() {
                    linked_notes_string = format!("\nLinked Notes:\n{}", linked_notes_string);
                }
                let custom_data_str = if custom_data.is_empty() {
                    String::new()
                } else {
                    let custom_data_str_content = serde_json::to_string(custom_data).unwrap();
                    let custom_data_string = format!(
                        "{}{} {}",
                        custom_data_key.get_write(),
                        settings_key_value_delim,
                        custom_data_str_content.as_str(),
                    );
                    self.construct_setting(custom_data_string.as_str())
                    // let delim_with_space = format!("{} ", settings_delim);
                    // let custom_data_str_content = custom_data
                    //     .iter()
                    //     // `v.as_str()` removes the extra quotes around the value. See <https://stackoverflow.com/questions/72345657/how-do-i-get-the-string-value-of-a-json-value-without-quotes>.
                    //     .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                    //     .map(|(k, v)| format!("{}{} {}", k, settings_key_value_delim, v))
                    //     .collect::<Vec<_>>()
                    //     .join(delim_with_space.as_str());
                    // format!(
                    //     "{}{}",
                    //     self.construct_comment("Custom data"),
                    //     self.construct_setting(custom_data_str_content.as_str()),
                    // )
                };
                let action_string = if matches!(note_import_action, NoteImportAction::Update(_)) {
                    String::new()
                } else {
                    let action_value = match note_import_action {
                        NoteImportAction::Add => action_add_key,
                        NoteImportAction::Update(_) | NoteImportAction::Delete(_) => unreachable!(),
                    };
                    self.construct_setting(&format!(
                        "{}{} {}",
                        action_key.get_write(),
                        settings_key_value_delim,
                        action_value.get_write(),
                    ))
                };
                let note_id_string = format!(
                    "{}{} {}",
                    note_id_key.get_write(),
                    settings_key_value_delim,
                    note_id
                );
                let keywords_string =
                    format!("keywords{} {}", settings_key_value_delim, keywords_str);
                let tags_string = format!("tags{} {}", settings_key_value_delim, tags_str);
                let mut lines = vec![
                    // self.construct_comment("spares: start"),
                    // "\n".to_string(),
                    "\n".to_string(),
                    action_string,
                    self.construct_setting(&note_id_string),
                    self.construct_setting(&keywords_string),
                    self.construct_setting(&tags_string),
                    custom_data_str,
                    self.construct_comment("spares: note start"),
                    note_data,
                    "\n".to_string(),
                    self.construct_comment("spares: note end"),
                    linked_notes_string,
                    "\n".to_string(),
                    // "\n".to_string(),
                    // self.construct_comment("spares: end"),
                ];
                if include_separator {
                    lines.push("---".to_string());
                }
                let note_file_data = lines.into_iter().collect::<String>();
                note_file_data
            }
            ConstructFileDataType::Card(card_order, card_data, side) => {
                let card_data_str =
                    construct_card_data(self, card_order, card_data, side, *note_id);
                let mut lines = vec![format!("- note-id{} {}", settings_key_value_delim, note_id)];
                if !keywords_str.is_empty() {
                    lines.push(format!(
                        "- keywords{} {}",
                        settings_key_value_delim, keywords_str
                    ));
                }
                lines.extend(vec![
                    format!("- tags{} {}", settings_key_value_delim, tags_str),
                    String::new(),
                    "$\\hrulefill$".to_string(),
                    String::new(),
                    card_data_str,
                ]);
                let card_file_data = lines.join("\n");
                card_file_data
            }
        }
    }

    fn construct_cloze_replacement(
        &self,
        cloze_replacement: &ClozeReplacement,
        side: CardSide,
        id: Option<&str>,
    ) -> String {
        match cloze_replacement {
            ClozeReplacement::Hidden(cloze_replacement) => match cloze_replacement {
                ClozeHiddenReplacement::ToAnswer { hint } => match (hint, id) {
                    (Some(hint), Some(id)) => {
                        format!("[{id} _____({hint})]{{.mark}}")
                    }
                    (Some(hint), None) => {
                        format!("[_____({hint})]{{.mark}}")
                    }
                    (None, Some(id)) => {
                        format!("[{id} _____]{{.mark}}")
                    }
                    (None, None) => "[_____]{.mark}".to_string(),
                },
                ClozeHiddenReplacement::NotToAnswer => match side {
                    CardSide::Front => "[_____(no answer)]{.mark}".to_string(),
                    CardSide::Back => "[_____]{.mark}".to_string(),
                },
            },
            ClozeReplacement::Reveal(data) => match id {
                Some(id) => format!("[{id} {data}]{{.mark}}"),
                None => format!("[{data}]{{.mark}}"),
            },
        }
    }

    fn construct_image_occlusion(
        &self,
        image_occlusion_data: &ImageOcclusionData,
        output_type: ConstructImageOcclusionType,
    ) -> String {
        fn construct_image(file_path: &Path, caption: &str) -> String {
            format!("![{}]({})\n", caption, file_path.display())
        }
        construct_image_occlusion_from_image(
            self,
            construct_image,
            image_occlusion_data,
            output_type,
        )
    }

    fn get_output_rendered_dir(&self, _output_type: RenderOutputDirectoryType) -> PathBuf {
        if is_test_mode() {
            return get_cache_dir();
        }
        std::env::var("MARKDOWN_OUT_DIR")
            .ok()
            .map(PathBuf::from)
            .filter(|dir| dir.exists())
            .unwrap_or_else(get_cache_dir)
    }

    fn file_extension(&self) -> &'static str {
        "md"
    }

    fn default_template(&self) -> &'static str {
        include_str!("template.md")
    }

    fn render_file(
        &self,
        _aux_dir: &Path,
        output_text_filepath: &Path,
        _output_rendered_dir: &Path,
        output_rendered_filepath: &Path,
    ) -> Result<std::process::Output, Error> {
        // Output is rendered as a pdf. This is because some formats, like png, do not support text selection. Other formats, such as svg, do not have popular viewers on all platforms.
        let dependencies_filepath = render_dependencies_filepath(output_rendered_filepath);
        // Pandoc logs the resources it loads, which are the dependencies besides the defaults file.
        let mut log_filepath = dependencies_filepath.clone();
        log_filepath.set_extension("pandoc-log.json");
        let mut base_command = Command::new("pandoc");
        let mut command = base_command
            .arg("-o")
            .arg(output_rendered_filepath)
            .arg(output_text_filepath)
            .arg("--log")
            .arg(&log_filepath);
        let config = read_external_config().unwrap();
        if let Some(defaults_file) = &config.parser.markdown.defaults_file {
            command = command.arg("--defaults").arg(defaults_file);
        }
        let output = render_command_output(command, "pandoc");
        if output.is_ok()
            && let Err(e) = record_dependencies(
                &log_filepath,
                config
                    .parser
                    .markdown
                    .defaults_file
                    .as_deref()
                    .map(Path::new),
                &dependencies_filepath,
            )
        {
            warn!(
                "Failed to record dependencies of {}: {e}",
                output_text_filepath.display()
            );
        }
        let _ = remove_file(&log_filepath);
        output
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum PandocLogMessage {
    LoadedResource {
        from: String,
    },
    #[serde(other)]
    Other,
}

/// Writes the [`RenderDependencies`] of a rendered file from the resources in pandoc's log and the
/// defaults file. Files that are only referenced by the defaults file, such as a template, are not
/// logged, so they are not included.
fn record_dependencies(
    log_filepath: &Path,
    defaults_file: Option<&Path>,
    dependencies_filepath: &Path,
) -> Result<(), String> {
    let log = read_to_string(log_filepath)
        .map_err(|e| format!("Failed to read {}: {e}", log_filepath.display()))?;
    let mut files = loaded_resources(&log)?;
    if let Some(defaults_file) = defaults_file {
        files.extend(find_defaults_file(defaults_file));
    }
    RenderDependencies {
        files,
        packages: Vec::new(),
    }
    .write(dependencies_filepath)
}

/// Absolute paths of the local files pandoc loaded according to its JSON log.
fn loaded_resources(log: &str) -> Result<Vec<PathBuf>, String> {
    let messages: Vec<PandocLogMessage> = serde_json::from_str(log).map_err(|e| e.to_string())?;
    Ok(messages
        .into_iter()
        .filter_map(|message| match message {
            PandocLogMessage::LoadedResource { from } => Some(PathBuf::from(from)),
            PandocLogMessage::Other => None,
        })
        // Skips URLs.
        .filter(|path| path.is_absolute())
        .collect())
}

/// The path of a defaults file given to `pandoc --defaults`. Like pandoc, looks relative to the
/// working directory, then in the `defaults` directory of pandoc's user data directory.
fn find_defaults_file(defaults_file: &Path) -> Option<PathBuf> {
    let defaults_file = if defaults_file.extension().is_none() {
        defaults_file.with_extension("yaml")
    } else {
        defaults_file.to_path_buf()
    };
    if defaults_file.is_file() {
        return std::path::absolute(defaults_file).ok();
    }
    let path = pandoc_user_data_dir()?.join("defaults").join(defaults_file);
    path.is_file().then_some(path)
}

fn pandoc_user_data_dir() -> Option<&'static Path> {
    static DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
        let output = Command::new("pandoc").arg("--version").output().ok()?;
        String::from_utf8(output.stdout)
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("User data directory: "))
            .map(|dir| PathBuf::from(dir.trim()))
    });
    DIR.as_deref()
}

/// <https://pandoc.org/MANUAL.html?pandocs-markdown#reference-links>
fn get_linked_notes_regex() -> Arc<fancy_regex::Regex> {
    get_or_compile_regex(r"(?m)\[([^\]]*)\]\[li([^\]]*)?\]").unwrap()
}

fn get_linked_notes_string(
    parser: &dyn Parseable,
    note_data: &str,
    linked_notes_opt: Option<&Vec<LinkedNote>>,
) -> (String, String) {
    if let Some(linked_notes) = linked_notes_opt {
        // Order all linked notes in `note_data` sequentially
        let mut count = 0;
        let linked_notes_regex = get_linked_notes_regex();
        let new_note_data =
            linked_notes_regex.replace_all(note_data, |caps: &Captures<'_, str>| {
                count += 1;
                format!("[{}][li{}]", &caps[1], count)
            });

        let linked_notes_string = linked_notes
            .iter()
            .enumerate()
            .map(|(i, linked_note_request)| {
                let LinkedNote {
                    searched_keyword,
                    linked_note_id,
                    matched_keyword,
                } = linked_note_request;
                assert_eq!(linked_note_id.is_some(), matched_keyword.is_some());
                match (linked_note_id, matched_keyword) {
                    (None, None) => format!("[li{}]: -", i + 1),
                    (Some(linked_note_id), Some(matched_keyword)) => {
                        let mut note_raw_path = get_output_raw_dir(
                            parser.get_parser_name(),
                            RenderOutputType::Note,
                            None,
                        );
                        note_raw_path.push(
                            parser.get_output_filename(RenderOutputType::Note, *linked_note_id),
                        );
                        note_raw_path.set_extension(parser.file_extension());
                        format!(
                            "[li{}]: {} \"{} -> {}\"",
                            i + 1,
                            note_raw_path.display(),
                            searched_keyword,
                            matched_keyword,
                        )
                    }
                    (None, Some(_)) | (Some(_), None) => unreachable!(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        (new_note_data.to_string(), linked_notes_string)
    } else {
        (note_data.to_string(), String::new())
    }
}

#[cfg(test)]
pub mod tests {
    use std::path::PathBuf;

    use super::get_linked_notes_string;
    use super::loaded_resources;
    use crate::parsers::ClozeMatch;
    use crate::parsers::Parseable;
    use crate::parsers::impls::markdown::MarkdownParser;
    use crate::schema::note::LinkedNote;

    #[test]
    fn test_markdown_linked_notes() {
        let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let note_data = "Third {{[o:1] Cloze here, linking to [keyword 1][li], [keyword 1.5][li], and [keyword 2][li] }}";
        let linked_notes_res = parser.get_linked_notes(note_data);
        assert!(linked_notes_res.is_ok());
        assert_eq!(linked_notes_res.unwrap().len(), 3);
    }

    #[test]
    fn test_markdown_get_linked_notes_string() {
        let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let original_note_data = "Third {{[o:1] Cloze here, linking to [keyword 1][li], [keyword 1.5][li], and [keyword 2][li] }}";
        let linked_notes_opt = Some(vec![
            LinkedNote {
                searched_keyword: "keyword 1".to_string(),
                linked_note_id: Some(1),
                matched_keyword: Some("keyword 1".to_string()),
            },
            LinkedNote {
                searched_keyword: "keyword 1.5".to_string(),
                linked_note_id: Some(1),
                matched_keyword: Some("keyword 1".to_string()),
            },
            LinkedNote {
                searched_keyword: "keyword 2".to_string(),
                linked_note_id: Some(2),
                matched_keyword: Some("keyword 2".to_string()),
            },
        ]);
        let (new_note_data, linked_notes_string) = get_linked_notes_string(
            parser.as_ref(),
            original_note_data,
            linked_notes_opt.as_ref(),
        );
        let expected_new_note_data = "Third {{[o:1] Cloze here, linking to [keyword 1][li1], [keyword 1.5][li2], and [keyword 2][li3] }}";
        assert_eq!(new_note_data, expected_new_note_data);
        let expected_linked_notes_string = "[li1]: /tmp/spares/data/notes/markdown/0001.md \"keyword 1 -> keyword 1\"\n[li2]: /tmp/spares/data/notes/markdown/0001.md \"keyword 1.5 -> keyword 1\"\n[li3]: /tmp/spares/data/notes/markdown/0002.md \"keyword 2 -> keyword 2\"";
        assert_eq!(linked_notes_string, expected_linked_notes_string);
    }

    #[test]
    fn test_markdown_clozes_math_mode() {
        let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let note_data = "Test {{[o:1] $3^{2^{2}}$}}";
        let clozes_res = parser.get_clozes(note_data);
        assert!(clozes_res.is_ok());
        let clozes = clozes_res.unwrap();
        assert_eq!(
            clozes,
            vec![ClozeMatch {
                start_match: 5..12,
                end_match: 24..26,
                settings_match: 8..11,
            }]
        );
    }

    #[test]
    fn test_loaded_resources() {
        let log = r#"[
            {"type": "LoadedResource", "verbosity": "INFO", "for": "header.tex", "from": "/home/a/.config/pandoc/defaults/header.tex"},
            {"type": "LoadedResource", "verbosity": "INFO", "for": "https://example.com/a.png", "from": "https://example.com/a.png"},
            {"type": "Extracting", "verbosity": "INFO", "path": "/tmp/media/a.png"},
            {"type": "MakePDFInfo", "verbosity": "INFO", "contents": "/tmp/media", "description": "Temp dir:"}
        ]"#;
        assert_eq!(
            loaded_resources(log).unwrap(),
            [PathBuf::from("/home/a/.config/pandoc/defaults/header.tex")]
        );
    }
}
