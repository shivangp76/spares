use crate::{
    Error, LibraryError, ParserErrorKind,
    config::get_cache_dir,
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
use data_parser::TypstDataParser;
use fancy_regex::{Captures, Regex};
use indoc::indoc;
use std::{
    ops::Range,
    path::{Path, PathBuf},
    process::Command,
};

mod data_parser;

/// See <https://typst.app/>
///
/// Note that clozes must pass arguments using markup syntax, not code syntax. For example,
/// `#cl[a][g:1]` is valid, while `#cl([a], [g:1])` is not.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypstParser {}

impl TypstParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl Parseable for TypstParser {
    fn get_parser_name(&self) -> &'static str {
        "typst"
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
        // Regex is not used here due to nested braces. For example, `#se[keywords: Test [data]] See [2]`.
        let mut all_settings = Vec::new();
        let mut parser = TypstDataParser::new(data);
        while let Some(setting) = parser.next_setting() {
            all_settings.push(RegexMatch {
                match_range: setting.clone(),
                capture_range: setting,
            });
        }
        Ok(all_settings.into_iter().collect::<Vec<_>>())
    }

    fn get_clozes(&self, data: &str) -> Result<Vec<ClozeMatch>, LibraryError> {
        // Note that a regex approach will not work for nested clozes.
        let mut all_clozes = Vec::new();
        let mut cloze_parser = TypstDataParser::new(data);
        while let Some(cloze) = cloze_parser.next_cloze() {
            all_clozes.push(cloze.clone());
        }
        Ok(all_clozes.into_iter().flatten().collect::<Vec<_>>())
    }

    fn construct_cloze(&self, cloze_settings_string: &str, _data: &str) -> (String, String) {
        let cloze_settings_string_with_delim = if cloze_settings_string.is_empty() {
            cloze_settings_string.to_string()
        } else {
            format!("[{}]", cloze_settings_string)
        };
        let cloze_start = "#cl[".to_string();
        let cloze_end = format!("]{}", cloze_settings_string_with_delim);
        (cloze_start, cloze_end)
    }

    // fn cloze_settings_side(&self) -> ClozeSettingsSide {
    //     ClozeSettingsSide::End
    // }

    fn construct_setting(&self, data: &str) -> String {
        format!("#se[{}]\n", data)
    }

    fn construct_comment(&self, data: &str) -> String {
        // Add trailing newline (POSIX convention)
        format!("// {data}\n")
    }

    fn extract_comment<'a>(&self, data: &'a str) -> &'a str {
        data.strip_prefix("//").map_or(data, |x| x.trim())
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
                    "\n".to_string(),
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
                    "#line(length: 100%)".to_string(),
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
        _side: CardSide,
    ) -> String {
        match cloze_replacement {
            ClozeReplacement::Hidden(cloze_replacement) => match cloze_replacement {
                ClozeHiddenReplacement::ToAnswer { hint } => {
                    if let Some(hint) = hint {
                        format!("#cloze(hint: \"{}\")", hint)
                    } else {
                        "#cloze()".to_string()
                    }
                }
                ClozeHiddenReplacement::NotToAnswer => "#cloze(to_answer: false)".to_string(),
            },
            ClozeReplacement::Reveal(data) => format!("#block(fill: aqua)[{}]", data),
        }
    }

    fn construct_image_occlusion(
        &self,
        image_occlusion_data: &ImageOcclusionData,
        output_type: ConstructImageOcclusionType,
    ) -> String {
        fn construct_image(file_path: &Path, caption: &str) -> String {
            format!(
                indoc! { r#"#figure(
                  std.image("{}", width: 80%),
                  caption: [{}],
                )
                "#},
                file_path.display(),
                caption,
            )
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
        std::env::var("TYPST_OUT_DIR")
            .ok()
            .map(PathBuf::from)
            .filter(|dir| dir.exists())
            .unwrap_or_else(get_cache_dir)
    }

    fn file_extension(&self) -> &'static str {
        "typ"
    }

    fn render_file(
        &self,
        _aux_dir: &Path,
        output_text_filepath: &Path,
        _output_rendered_dir: &Path,
        output_rendered_filepath: &Path,
    ) -> Result<std::process::Output, Error> {
        let typst_root_dir = std::env::var("TYPST_ROOT").map_err(|_| {
            Error::Library(LibraryError::Parser(ParserErrorKind::NotFound(
                "TYPST_ROOT environment variable is not set".to_string(),
            )))
        })?;
        let output = Command::new("typst")
            .arg("compile")
            .arg("--root")
            .arg(typst_root_dir)
            .arg(output_text_filepath)
            .arg(output_rendered_filepath)
            // .current_dir(output_text_filepath.parent().unwrap())
            .output()
            .map_err(|e| Error::Io {
                description: "Failed to run typst command".to_string(),
                source: e,
            })?;
        Ok(output)
    }
}

// TODO: This doesn't properly match braces.
// Ex. `#lin("test(a)") ( )`
fn get_linked_notes_regex() -> Regex {
    Regex::new(r"(?s)#lin\(([^,\n]*)(?:, note_link: ([^\n\)]*))?\)").unwrap()
}

fn get_linked_notes_string(
    parser: &dyn Parseable,
    note_data: &str,
    linked_notes_opt: Option<&Vec<LinkedNote>>,
) -> String {
    if let Some(linked_notes) = linked_notes_opt {
        // Order all linked notes in `note_data` sequentially
        let mut count = 0;

        // Regex is not used here due to nested braces. For example, `#se[keywords: Test [data]] See [2]`.
        // TODO: This doesn't match the paren version, only the bracket version.
        let mut all_linked_notes = Vec::new();
        let mut data_parser = TypstDataParser::new(note_data);
        while let Some(linked_note) = data_parser.next_linked_note() {
            all_linked_notes.push(linked_note);
        }
        let _linked_notes = all_linked_notes.into_iter().collect::<Vec<_>>();

        let linked_notes_regex = get_linked_notes_regex();
        let new_note_data = linked_notes_regex.replace_all(note_data, |caps: &Captures| {
            count += 1;
            format!("#lin({}, note_link: li{})", &caps[1], count)
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
                    (None, None) => format!("#let li{} = \"\"", i + 1),
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
                            "#let li{} = {} // \"{} -> {}\"",
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
        // format!("{}\n\n{}", new_note_data, items)
        format!("{}\n\n{}", items, new_note_data)
    } else {
        note_data.to_string()
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        parsers::{
            BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, ClozeMatch,
            FrontConceal, NotePart, Parseable, get_cards, impls::typst::TypstParser,
        },
        schema::note::LinkedNote,
    };

    use super::get_linked_notes_string;
    use std::ops::Range;

    #[test]
    fn test_typst_get_clozes() {
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let note_data = "Third #cl[Cloze here, linking to #lin([keyword 1]), #lin([keyword 1.5]), and #lin([keyword 2])][o:1]";
        let cloze_matches_res = parser.get_clozes(note_data);
        assert!(cloze_matches_res.is_ok());
        let cloze_matches = cloze_matches_res.unwrap();
        assert_eq!(cloze_matches.len(), 1);
        dbg!(&note_data[cloze_matches[0].start_match.clone()]);
        dbg!(&note_data[cloze_matches[0].end_match.clone()]);
        dbg!(&note_data[cloze_matches[0].settings_match.clone()]);
        assert_eq!(
            cloze_matches[0],
            ClozeMatch {
                start_match: 6..10,
                end_match: 94..100,
                settings_match: 96..99,
            }
        );
    }

    #[test]
    fn test_typst_linked_notes() {
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let note_data = "Third #cl[Cloze here, linking to #lin([keyword 1]), #lin([keyword 1.5]), and #lin([keyword 2])][o:1]";
        let linked_notes_res = parser.get_linked_notes(note_data);
        assert!(linked_notes_res.is_ok());
        assert_eq!(linked_notes_res.unwrap().len(), 3);
    }

    #[test]
    fn test_typst_get_linked_notes_string() {
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let original_note_data = "Third #cl[Cloze here, linking to #lin([keyword 1]), #lin([keyword 1.5]), and #lin([keyword 2])][o:1]";
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
        let expected_new_note_data = "#let li1 = /tmp/spares/data/notes/typst/0001.typ // \"keyword 1 -> keyword 1\"\n#let li2 = /tmp/spares/data/notes/typst/0001.typ // \"keyword 1.5 -> keyword 1\"\n#let li3 = /tmp/spares/data/notes/typst/0002.typ // \"keyword 2 -> keyword 2\"\n\nThird #cl[Cloze here, linking to #lin([keyword 1], note_link: li1), #lin([keyword 1.5], note_link: li2), and #lin([keyword 2], note_link: li3)][o:1]";
        assert_eq!(new_note_data, expected_new_note_data);
    }

    #[test]
    fn test_typst_clozes_escaped_bracket() {
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let note_data = "Test #cl[math \\] $ [3] $]";
        let clozes_res = parser.get_clozes(note_data);
        assert!(clozes_res.is_ok());
        let clozes = clozes_res.unwrap();
        assert_eq!(
            clozes,
            vec![ClozeMatch {
                start_match: 5..9,
                end_match: 24..25,
                settings_match: Range::default(),
            }]
        );
    }

    #[test]
    fn test_typst_get_cards_1() {
        let data = "#cl[\n- Test #cl[amps][h:Test]\n- Words\n]";
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let cards_res = get_cards(parser.as_ref(), None, data, true, false);
        assert!(cards_res.is_ok());
        if let Ok(cards) = cards_res {
            let expected = vec![
                CardData {
                    order: Some(1),
                    grouping: ClozeGrouping::Auto(1),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_type: BackType::FullNote,
                    data: vec![
                        NotePart::ClozeStart("#cl[".to_string()),
                        NotePart::ClozeData(
                            "\n- Test #cl[amps][h:Test;o:2]\n- Words\n".to_string(),
                            ClozeHiddenReplacement::ToAnswer { hint: None },
                        ),
                        NotePart::ClozeEnd("][o:1]".to_string()),
                    ],
                },
                CardData {
                    order: Some(2),
                    grouping: ClozeGrouping::Auto(2),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_type: BackType::FullNote,
                    data: vec![
                        NotePart::SurroundingData("#cl[\n- Test ".to_string()),
                        NotePart::ClozeStart("#cl[".to_string()),
                        NotePart::ClozeData(
                            "amps".to_string(),
                            ClozeHiddenReplacement::ToAnswer {
                                hint: Some("Test".to_string()),
                            },
                        ),
                        NotePart::ClozeEnd("][h:Test;o:2]".to_string()),
                        NotePart::SurroundingData("\n- Words\n][o:1]".to_string()),
                    ],
                },
            ];
            assert_eq!(cards, expected);
        }
    }

    #[test]
    fn test_typst_get_cards_2() {
        let data = "[#cl[Test]]";
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let cards_res = get_cards(parser.as_ref(), None, data, true, false);
        assert!(cards_res.is_ok());
        if let Ok(cards) = cards_res {
            let expected = vec![CardData {
                order: Some(1),
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
                data: vec![
                    NotePart::SurroundingData("[".to_string()),
                    NotePart::ClozeStart("#cl[".to_string()),
                    NotePart::ClozeData(
                        "Test".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("][o:1]".to_string()),
                    NotePart::SurroundingData("]".to_string()),
                ],
            }];
            assert_eq!(cards, expected);
        }
    }
}
