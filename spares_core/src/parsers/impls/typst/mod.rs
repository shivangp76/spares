use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use data_parser::TypstDataParser;
use indoc::indoc;

use crate::Error;
use crate::LibraryError;
use crate::ParserErrorKind;
use crate::config::get_cache_dir;
use crate::parsers::CliData;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::ClozeMatch;
use crate::parsers::ClozeReplacement;
use crate::parsers::ConstructFileDataType;
use crate::parsers::ConstructImageOcclusionType;
use crate::parsers::GenerateNoteFilesRequest;
use crate::parsers::NoteImportAction;
use crate::parsers::NotePart;
use crate::parsers::NoteSettingsKeys;
use crate::parsers::Parseable;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::RenderOutputType;
use crate::parsers::construct_card_data;
use crate::parsers::generate_files::CardSide;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::ImageOcclusionData;
use crate::parsers::image_occlusion::construct_image_occlusion_from_image;
use crate::schema::note::LinkedNote;

mod data_parser;
mod old_data_parser;

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
        // We cannot use regex here since then braces won't properly match up. For example, `#lin("test(a)") ( )` or `#lin[test[a]]` or `#lin[a [b]] a #test[]`.
        // Regex::new(r"(?s)#lin\(([^,\n]*)(?:, note_link: ([^\n\)]*))?\)").unwrap()
        let mut parser = TypstDataParser::new(data);
        Ok(parser.linked_notes)
    }

    fn get_embedded_keywords(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        let mut parser = TypstDataParser::new(data);
        Ok(parser.keywords)
    }

    fn get_settings(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        // Regex is not used here due to nested braces. For example, `#se[keywords: Test [data]] See [2]`.
        let mut parser = TypstDataParser::new(data);
        Ok(parser.settings)
    }

    fn note_settings_keys(&self) -> NoteSettingsKeys {
        NoteSettingsKeys {
            groupings_all: "\\*",
            ..Default::default()
        }
    }

    fn get_clozes(&self, data: &str) -> Result<Vec<ClozeMatch>, LibraryError> {
        // Note that a regex approach will not work for nested clozes.
        let mut parser = TypstDataParser::new(data);
        Ok(parser.clozes.into_iter().flatten().collect::<Vec<_>>())
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
                    // This header is added so that if the note ends with a bulleted list. In this
                    // case, the linked notes list will be merged with the list at the end of the
                    // note which is not desired.
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
                    "\n".to_string(),
                    self.construct_comment("spares: note start"),
                    note_data.clone(),
                    "\n".to_string(),
                    self.construct_comment("spares: note end"),
                    linked_notes_string,
                    "\n".to_string(),
                    // "\n".to_string(),
                    // self.construct_comment("spares: end"),
                ];
                if include_separator {
                    lines.push("#line(length: 100%)".to_string());
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
                    "#line(length: 100%)".to_string(),
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
        _side: CardSide,
        id: Option<&str>,
    ) -> String {
        match cloze_replacement {
            ClozeReplacement::Hidden(cloze_replacement) => match cloze_replacement {
                ClozeHiddenReplacement::ToAnswer { hint } => match (hint, id) {
                    (Some(hint), Some(id)) => {
                        format!("#cloze(hint: \"{hint}\", id: \"{id}\")")
                    }
                    (Some(hint), None) => {
                        format!("#cloze(hint: \"{hint}\")")
                    }
                    (None, Some(id)) => {
                        format!("#cloze(id: \"{id}\")")
                    }
                    (None, None) => "#cloze()".to_string(),
                },
                ClozeHiddenReplacement::NotToAnswer => "#cloze(to_answer: false)".to_string(),
            },
            ClozeReplacement::Reveal(data) => match id {
                Some(id) => format!("#cloze-reveal(id: \"{id}\")[{data}]"),
                None => format!("#cloze-reveal[{data}]"),
            },
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
        if cfg!(test) {
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
            .arg("--no-pdf-tags") // To reduce output filesize
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

fn get_linked_notes_string(
    parser: &dyn Parseable,
    note_data: &str,
    linked_notes_opt: Option<&Vec<LinkedNote>>,
) -> (String, String) {
    if let Some(linked_notes) = linked_notes_opt {
        let linked_notes_string = linked_notes
            .iter()
            .map(|linked_note_request| {
                let LinkedNote {
                    searched_keyword,
                    linked_note_id,
                    matched_keyword,
                } = linked_note_request;
                assert_eq!(linked_note_id.is_some(), matched_keyword.is_some());
                match (linked_note_id, matched_keyword) {
                    (None, None) => format!("+ {} $->$ (no match found)", searched_keyword),
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
                            "+ {} $->$ #link(\"{}\")[{}]",
                            searched_keyword,
                            note_raw_path.display(),
                            matched_keyword,
                        )
                    }
                    (None, Some(_)) | (Some(_), None) => unreachable!(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        (note_data.to_string(), linked_notes_string)

        // // Regex is not used here due to nested braces. For example, `#se[keywords: Test [data]] See [2]`.
        // // TODO: This doesn't match the paren version, only the bracket version.
        // let mut all_linked_notes = Vec::new();
        // let mut data_parser = TypstDataParser::new(note_data);
        // while let Some(linked_note) = data_parser.next_linked_note() {
        //     all_linked_notes.push(linked_note);
        // }
        // let _linked_notes = all_linked_notes.into_iter().collect::<Vec<_>>();
        //
        // // NOTE: Regex does not work here so if using this code, this will need to be fixed. See the comment in `get_linked_notes()` for an explanation.
        // let linked_notes_regex = get_linked_notes_regex();
        // let new_note_data = linked_notes_regex.replace_all(note_data, |caps: &Captures| {
        //     count += 1;
        //     format!("#lin({}, note_link: li{})", &caps[1], count)
        // });
        //
        // let items = linked_notes
        //     .iter()
        //     .enumerate()
        //     .map(|(i, linked_note_request)| {
        //         let LinkedNote {
        //             searched_keyword,
        //             linked_note_id,
        //             matched_keyword,
        //         } = linked_note_request;
        //         assert_eq!(linked_note_id.is_some(), matched_keyword.is_some());
        //         match (linked_note_id, matched_keyword) {
        //             (None, None) => format!("#let li{} = \"\"", i + 1),
        //             (Some(linked_note_id), Some(matched_keyword)) => {
        //                 let mut note_raw_path = get_output_raw_dir(
        //                     parser.get_parser_name(),
        //                     RenderOutputType::Note,
        //                     None,
        //                 );
        //                 note_raw_path.push(
        //                     parser.get_output_filename(RenderOutputType::Note, *linked_note_id),
        //                 );
        //                 note_raw_path.set_extension(parser.file_extension());
        //                 format!(
        //                     "#let li{} = {} // \"{} -> {}\"",
        //                     i + 1,
        //                     note_raw_path.display(),
        //                     searched_keyword,
        //                     matched_keyword,
        //                 )
        //             }
        //             (None, Some(_)) | (Some(_), None) => unreachable!(),
        //         }
        //     })
        //     .collect::<Vec<_>>()
        //     .join("\n");
        // // format!("{}\n\n{}", new_note_data, items)
        // format!("{}\n\n{}", items, new_note_data)
    } else {
        (note_data.to_string(), String::new())
    }
}

#[cfg(test)]
pub mod tests {
    use std::ops::Range;

    use super::get_linked_notes_string;
    use crate::parsers::BackReveal;
    use crate::parsers::BackType;
    use crate::parsers::CardData;
    use crate::parsers::ClozeGrouping;
    use crate::parsers::ClozeHiddenReplacement;
    use crate::parsers::ClozeMatch;
    use crate::parsers::FrontConceal;
    use crate::parsers::NotePart;
    use crate::parsers::Parseable;
    use crate::parsers::get_cards;
    use crate::parsers::impls::typst::TypstParser;
    use crate::schema::note::LinkedNote;

    #[test]
    fn test_typst_get_clozes() {
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let note_data = "Third #cl[Cloze here, linking to #lin([keyword 1]), #lin([keyword 1.5]), and #lin([keyword 2])][o:1]";
        let cloze_matches_res = parser.get_clozes(note_data);
        assert!(cloze_matches_res.is_ok());
        let cloze_matches = cloze_matches_res.unwrap();
        assert_eq!(cloze_matches.len(), 1);
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
        let note_data = "Third #cl[Cloze here, linking to #lin[keyword 1], #lin[keyword 1.5], and #lin[keyword 2]][o:1]";
        // let note_data = "Third #cl[Cloze here, linking to #lin([keyword 1]), #lin([keyword 1.5]), and #lin([keyword 2])][o:1]";
        let linked_notes_res = parser.get_linked_notes(note_data);
        assert!(linked_notes_res.is_ok());
        assert_eq!(linked_notes_res.unwrap().len(), 3);
    }

    #[test]
    fn test_typst_get_linked_notes_string() {
        let parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let original_note_data = "Third #cl[Cloze here, linking to #lin[keyword 1], #lin[keyword 1.5], and #lin[keyword 2]][o:1]";
        // let original_note_data = "Third #cl[Cloze here, linking to #lin([keyword 1]), #lin([keyword 1.5]), and #lin([keyword 2])][o:1]";
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
        let expected_new_note_data = "Third #cl[Cloze here, linking to #lin[keyword 1], #lin[keyword 1.5], and #lin[keyword 2]][o:1]";
        assert_eq!(new_note_data, expected_new_note_data);
        let expected_linked_notes_string = "+ keyword 1 $->$ #link(\"/tmp/spares/data/notes/typst/0001.typ\")[keyword 1]\n+ keyword 1.5 $->$ #link(\"/tmp/spares/data/notes/typst/0001.typ\")[keyword 1]\n+ keyword 2 $->$ #link(\"/tmp/spares/data/notes/typst/0002.typ\")[keyword 2]";
        assert_eq!(linked_notes_string, expected_linked_notes_string);
        // let expected_new_note_data = "#let li1 = /tmp/spares/data/notes/typst/0001.typ // \"keyword 1 -> keyword 1\"\n#let li2 = /tmp/spares/data/notes/typst/0001.typ // \"keyword 1.5 -> keyword 1\"\n#let li3 = /tmp/spares/data/notes/typst/0002.typ // \"keyword 2 -> keyword 2\"\n\nThird #cl[Cloze here, linking to #lin([keyword 1], note_link: li1), #lin([keyword 1.5], note_link: li2), and #lin([keyword 2], note_link: li3)][o:1]";
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
                    previous_order: None,
                    grouping: ClozeGrouping::Auto(1),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_emphasis: false,
                    back_type: BackType::NoteFilePath,
                    inherit: None,
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
                    previous_order: None,
                    grouping: ClozeGrouping::Auto(2),
                    is_suspended: None,
                    front_conceal: FrontConceal::OnlyGrouping,
                    back_reveal: BackReveal::FullNote,
                    back_emphasis: false,
                    back_type: BackType::NoteFilePath,
                    inherit: None,
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
                previous_order: None,
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_emphasis: false,
                back_type: BackType::NoteFilePath,
                inherit: None,
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
