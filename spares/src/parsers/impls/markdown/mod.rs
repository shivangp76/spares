use crate::{
    Error, LibraryError,
    config::{get_cache_dir, read_external_config},
    parsers::{
        ClozeHiddenReplacement, ClozeMatch, ClozeReplacement, ConstructFileDataType,
        ConstructImageOcclusionType, GenerateNoteFilesRequest, NoteImportAction, NotePart,
        NoteSettingsKeys, Parseable, RegexMatch, RenderOutputDirectoryType, RenderOutputType,
        generate_files::CardSide,
        get_output_raw_dir,
        image_occlusion::{ImageOcclusionData, construct_image_occlusion_from_image},
    },
    schema::note::LinkedNote,
};
use cloze_parser::ClozeParser;
use fancy_regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    process::Command,
};

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
            .filter_map(|c| c.unwrap().get(1).map(|x| (x.start()..x.end())))
            .collect::<Vec<_>>();
        Ok(linked_notes_data)
    }

    fn get_settings(&self, data: &str) -> Result<Vec<RegexMatch>, LibraryError> {
        // let settings_regex = Regex::new(r"(?m)<!--- # (.*) --->").unwrap();
        let settings_regex = Regex::new(r"(?s)<!--- # ([^\n]*) --->").unwrap();
        let settings_data = settings_regex
            .find_iter(data)
            .map(|m| m.unwrap())
            .map(|m| (m.start()..m.end()))
            .zip(
                settings_regex
                    .captures_iter(data)
                    .map(|c| c.unwrap().get(1).map(|x| (x.start()..x.end())).unwrap()),
            )
            .map(|(match_range, capture_range)| RegexMatch {
                match_range,
                capture_range,
            })
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
        // let (cloze_start_regex, settings_capture_group_index) =
        //     (Regex::new(r"(?s)(\{\{)(?:(\[)([^\n\]]*)(\]))?").unwrap(), 3);
        // let cloze_end_regex = Regex::new(r"(?s)\}\}").unwrap();
        // get_matched_clozes(
        //     data,
        //     &cloze_start_regex,
        //     settings_capture_group_index,
        //     &cloze_end_regex,
        //     &ClozeSettingsSide::Start,
        // )
    }

    fn construct_cloze(&self, cloze_settings_string: &str, _data: &str) -> (String, String) {
        let cloze_settings_string_with_delim = if cloze_settings_string.is_empty() {
            cloze_settings_string.to_string()
        } else {
            format!("[{}]", cloze_settings_string)
        };
        let cloze_start = format!("{{{{{}", cloze_settings_string_with_delim);
        let cloze_end = "}}".to_string();
        (cloze_start, cloze_end)
    }

    // fn cloze_settings_side(&self) -> ClozeSettingsSide {
    //     ClozeSettingsSide::Start
    // }

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
                let note_data =
                    get_linked_notes_string(self, note_data.as_str(), linked_notes.as_ref());
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
                let lines = [
                    // self.construct_comment("spares: start"),
                    // "\n".to_string(),
                    "\n".to_string(),
                    action_string,
                    self.construct_setting(&note_id_string),
                    self.construct_setting(&keywords_string),
                    self.construct_setting(&tags_string),
                    custom_data_str,
                    self.construct_comment("spares: note start"),
                    note_data.to_string(),
                    "\n".to_string(),
                    self.construct_comment("spares: note end"),
                    "\n".to_string(),
                    // "\n".to_string(),
                    // self.construct_comment("spares: end"),
                ];
                let note_file_data = lines.into_iter().collect::<String>();
                note_file_data
            }
            ConstructFileDataType::Card(card_order, card_data, side) => {
                let mut image_occlusion_order: usize = 1;
                let card_data = card_data
                    .data
                    .iter()
                    .map(|p| match p {
                        NotePart::ClozeData(d, cloze_replacement) => self
                            .construct_cloze_replacement(
                                &ClozeReplacement::parse(side, cloze_replacement, d),
                                side,
                            ),
                        NotePart::SurroundingData(d) => d.to_string(),
                        NotePart::ImageOcclusion { data, .. } => {
                            let image_occlusion = self.construct_image_occlusion(
                                data,
                                ConstructImageOcclusionType::Card {
                                    side,
                                    note_id: *note_id,
                                    card_order,
                                    image_occlusion_order,
                                },
                            );
                            image_occlusion_order += 1;
                            image_occlusion
                        }
                        NotePart::ClozeStart(_) | NotePart::ClozeEnd(_) => String::new(),
                    })
                    .collect::<String>();
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
                    card_data.to_string(),
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
    ) -> String {
        match cloze_replacement {
            ClozeReplacement::Hidden(cloze_replacement) => match cloze_replacement {
                ClozeHiddenReplacement::ToAnswer { hint } => {
                    if let Some(hint) = hint {
                        // format!("[_____({})]", hint)
                        format!("[_____({})]{{.mark}}", hint)
                    } else {
                        // "[_____]".to_string()
                        "[_____]{.mark}".to_string()
                    }
                }
                ClozeHiddenReplacement::NotToAnswer => match side {
                    CardSide::Front => "[_____(no answer)]{.mark}".to_string(),
                    CardSide::Back => "[_____]{.mark}".to_string(),
                },
            },
            ClozeReplacement::Reveal(data) => format!("[{}]{{.mark}}", data),
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
        if cfg!(feature = "testing") {
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

    fn render_file(
        &self,
        _aux_dir: &Path,
        output_text_filepath: &Path,
        _output_rendered_dir: &Path,
        output_rendered_filepath: &Path,
    ) -> Result<std::process::Output, Error> {
        // Output is rendered as a pdf. This is because some formats, like png, do not support text selection. Other formats, such as svg, do not have popular viewers on all platforms.
        let mut base_command = Command::new("pandoc");
        let mut command = base_command
            .arg("-o")
            .arg(output_rendered_filepath)
            .arg(output_text_filepath);
        let config = read_external_config().unwrap();
        if let Some(defaults_file) = config.parser.markdown.defaults_file {
            command = command.arg("--defaults").arg(defaults_file);
        }
        command.output().map_err(|e| Error::Io {
            description: "Failed to run pandoc command".to_string(),
            source: e,
        })
    }
}

/// <https://pandoc.org/MANUAL.html?pandocs-markdown#reference-links>
fn get_linked_notes_regex() -> Regex {
    Regex::new(r"(?m)\[([^\]]*)\]\[li([^\]]*)?\]").unwrap()
}

fn get_linked_notes_string(
    parser: &dyn Parseable,
    note_data: &str,
    linked_notes_opt: Option<&Vec<LinkedNote>>,
) -> String {
    if let Some(linked_notes) = linked_notes_opt {
        // Order all linked notes in `note_data` sequentially
        let mut count = 0;
        let linked_notes_regex = get_linked_notes_regex();
        let new_note_data = linked_notes_regex.replace_all(note_data, |caps: &Captures| {
            count += 1;
            format!("[{}][li{}]", &caps[1], count)
        });

        let items = linked_notes
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
        format!("{}\n\n{}", new_note_data, items)
    } else {
        note_data.to_string()
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        parsers::{ClozeMatch, Parseable, impls::markdown::MarkdownParser},
        schema::note::LinkedNote,
    };

    use super::get_linked_notes_string;

    #[test]
    fn test_markdown_linked_notes() {
        let parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let note_data = "Third {{[o:1] Cloze here, linking to [keyword 1][li], [keyword 1.5][li], and [keyword 2][li] }}";
        let linked_notes_res = parser.get_linked_notes(note_data);
        assert!(linked_notes_res.is_ok());
        dbg!(&linked_notes_res);
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
        let new_note_data = get_linked_notes_string(
            parser.as_ref(),
            original_note_data,
            linked_notes_opt.as_ref(),
        );
        let expected_new_note_data = "Third {{[o:1] Cloze here, linking to [keyword 1][li1], [keyword 1.5][li2], and [keyword 2][li3] }}\n\n[li1]: /tmp/spares/data/notes/markdown/0001.md \"keyword 1 -> keyword 1\"\n[li2]: /tmp/spares/data/notes/markdown/0001.md \"keyword 1.5 -> keyword 1\"\n[li3]: /tmp/spares/data/notes/markdown/0002.md \"keyword 2 -> keyword 2\"";
        assert_eq!(new_note_data, expected_new_note_data);
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
}
