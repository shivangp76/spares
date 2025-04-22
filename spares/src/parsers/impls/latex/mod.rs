use crate::config::get_cache_dir;
use crate::helpers::{find_pair, is_monotonic_increasing};
use crate::model::NoteId;
use crate::parsers::generate_files::CardSide;
use crate::parsers::image_occlusion::{
    ConstructImageOcclusionType, ImageOcclusionData, construct_image_occlusion_from_image,
};
use crate::parsers::{
    ClozeHiddenReplacement, ClozeMatch, ClozeReplacement, ClozeSettingsSide, ConstructFileDataType,
    GenerateNoteFilesRequest, NoteImportAction, NotePart, NoteRawData, NoteSettingsKeys, Parseable,
    RegexMatch, RenderOutputDirectoryType, RenderOutputType, get_matched_clozes,
    get_output_raw_dir,
};
use crate::schema::note::LinkedNote;
use crate::{DelimiterErrorKind, Error, LibraryError};
use fancy_regex::Regex;
use std::ffi::OsString;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Clone, Copy, Debug, Default)]
pub struct LatexParserNote {}

impl LatexParserNote {
    pub fn new() -> Self {
        Self {}
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LatexParserExerciseSolution {}

impl LatexParserExerciseSolution {
    pub fn new() -> Self {
        Self {}
    }
}

impl Parseable for LatexParserExerciseSolution {
    fn get_parser_name(&self) -> &'static str {
        "latex-exercise-solution"
    }

    fn get_notes_data(&self, data: &str) -> Result<Vec<NoteRawData>, LibraryError> {
        // NOTE: Workaround: Including \end{solution} inside the match is a workaround for the cloze environment currently not being used
        let notes_regex = Regex::new(
            r"(?ms)^\\begin\{exercise\}(?:\[([^\n]*?)\])?(?:\[([^\n]*?)\])?\n(.*?\n\\end\{solution})",
        )
        .unwrap();
        let notes_data = notes_regex
            .captures_iter(data)
            .map(|c| c.unwrap())
            .filter(|c| c.get(3).is_some())
            .map(|c| NoteRawData {
                metadata: c.get(1).map(|x| (x.start()..x.end())),
                // keywords: c.get(2).map(|x| x.as_str()),
                data: c.get(3).map(|x| (x.start()..x.end())).unwrap(),
            })
            .collect::<Vec<_>>();
        Ok(notes_data)
    }

    fn get_linked_notes(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        get_linked_notes(data)
    }

    fn get_settings(&self, data: &str) -> Result<Vec<RegexMatch>, LibraryError> {
        get_settings(data)
    }

    fn get_clozes(&self, data: &str) -> Result<Vec<ClozeMatch>, LibraryError> {
        // Due to the possibility of nested clozes, regex start and end matches cannot be interweaved. A stack is needed to ensure clozes are matched up correctly.
        let (cloze_start_regex, settings_capture_group_index) = (
            // Remove ending newline
            Regex::new(r"\\end\{exercise\}\n\\begin\{solution\}(?:\[([^\n\]]*)\])?").unwrap(),
            // Regex::new(r"\\end\{exercise\}\n\\begin\{solution\}(?:\[([^\n]*)\])?\n").unwrap(),
            1,
        );
        let cloze_end_regex = Regex::new(r"\\end\{solution}").unwrap();
        get_matched_clozes(
            data,
            &cloze_start_regex,
            settings_capture_group_index,
            &cloze_end_regex,
            &ClozeSettingsSide::Start,
        )
    }

    fn construct_cloze(&self, cloze_settings_string: &str, _data: &str) -> (String, String) {
        let cloze_settings_string_with_delim = if cloze_settings_string.is_empty() {
            cloze_settings_string.to_string()
        } else {
            format!("[{}]", cloze_settings_string)
        };
        let cloze_start = format!(
            "\\end{{exercise}}\n\\begin{{solution}}{}",
            cloze_settings_string_with_delim
        );
        let cloze_end = r"\end{solution}".to_string();
        (cloze_start, cloze_end)
    }

    // fn cloze_settings_side(&self) -> ClozeSettingsSide {
    //     ClozeSettingsSide::Start
    // }

    fn construct_setting(&self, data: &str) -> String {
        construct_setting(data)
    }

    fn construct_comment(&self, data: &str) -> String {
        construct_comment(data)
    }

    fn extract_comment<'a>(&self, data: &'a str) -> &'a str {
        extract_comment(data)
    }

    fn construct_file_data(
        &self,
        output_type: ConstructFileDataType,
        data: &GenerateNoteFilesRequest,
        note_import_action: &NoteImportAction,
    ) -> String {
        construct_file_data(self, output_type, data, note_import_action)
    }

    fn file_extension(&self) -> &'static str {
        file_extension(self)
    }

    fn construct_cloze_replacement(
        &self,
        cloze_replacement: &ClozeReplacement,
        side: CardSide,
    ) -> String {
        construct_cloze_replacement(cloze_replacement, side)
    }

    fn construct_image_occlusion(
        &self,
        image_occlusion_data: &ImageOcclusionData,
        output_type: ConstructImageOcclusionType,
    ) -> String {
        construct_image_occlusion(self, image_occlusion_data, output_type)
    }

    fn get_output_rendered_dir(&self, output_type: RenderOutputDirectoryType) -> PathBuf {
        get_output_rendered_dir(self, output_type)
    }

    fn get_aux_dir(&self, output_type: RenderOutputType, note_id: NoteId) -> PathBuf {
        get_aux_dir(self, output_type, note_id)
    }

    fn render_file(
        &self,
        aux_dir: &Path,
        output_text_filepath: &Path,
        output_rendered_dir: &Path,
        output_rendered_filepath: &Path,
    ) -> Result<std::process::Output, Error> {
        render_file(
            aux_dir,
            output_text_filepath,
            output_rendered_dir,
            output_rendered_filepath,
        )
    }
}

impl Parseable for LatexParserNote {
    fn get_parser_name(&self) -> &'static str {
        "latex-note"
    }

    fn get_notes_data(&self, data: &str) -> Result<Vec<NoteRawData>, LibraryError> {
        let notes_regex = Regex::new(
            r"(?ms)^\\begin\{note\}(?:\[([^\n]*?)\])?(?:\[([^\n]*?)\])?\n(.*?)\n\\end\{note}",
        )
        .unwrap();
        let notes_data = notes_regex
            .captures_iter(data)
            .map(|c| c.unwrap())
            .filter(|c| c.get(3).is_some())
            .map(|c| NoteRawData {
                metadata: c.get(1).map(|x| (x.start()..x.end())),
                // keywords: c.get(2).map(|x| x.as_str()),
                data: c.get(3).map(|x| (x.start()..x.end())).unwrap(),
            })
            .collect::<Vec<_>>();
        Ok(notes_data)
    }

    fn get_linked_notes(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        get_linked_notes(data)
    }

    fn get_settings(&self, data: &str) -> Result<Vec<RegexMatch>, LibraryError> {
        get_settings(data)
    }

    // <https://tex.stackexchange.com/questions/8373/why-does-latex-make-a-distinction-between-commands-and-environments>
    fn get_clozes(&self, data: &str) -> Result<Vec<ClozeMatch>, LibraryError> {
        let (cloze_start_regex, settings_capture_group_index) = (
            // Removed final newline in Regex. No more newlines
            Regex::new(r"(?s)(\\begin\{cl\})(?:(\[)([^\n\]]*)(\]))?").unwrap(),
            // Removed initial newline in Regex. The optional newline is only after the cloze start now.
            // Regex::new(r"(?s)(\\begin\{cl\})(?:(\[)([^\n]*)(\]))?((?:\n)?)").unwrap(),
            // Regex::new(r"(?s)((?:\n)?\\begin\{cl\})(?:(\[)([^\n]*)(\]))?((?:\n)?)").unwrap(),
            3,
        );
        let cloze_end_regex =
        // Removed final newline in Regex. No more newlines
        Regex::new(r"(?s)\\end\{cl\}").unwrap();
        // Removed ending newline in Regex. The optional newline is only before the cloze end now.
        // Regex::new(r"(?s)(?:\n)?\\end\{cl\}(?:\n)?").unwrap()
        // Regex::new(r"(?s)\\end\{cl\}(?:\n)?").unwrap()
        get_matched_clozes(
            data,
            &cloze_start_regex,
            settings_capture_group_index,
            &cloze_end_regex,
            &ClozeSettingsSide::Start,
        )
    }

    fn construct_cloze(&self, cloze_settings_string: &str, _data: &str) -> (String, String) {
        let cloze_settings_string_with_delim = if cloze_settings_string.is_empty() {
            cloze_settings_string.to_string()
        } else {
            format!("[{}]", cloze_settings_string)
        };
        let cloze_start = format!("\\begin{{cl}}{}", cloze_settings_string_with_delim);
        let cloze_end = r"\end{cl}".to_string();
        (cloze_start, cloze_end)
    }

    // fn cloze_settings_side(&self) -> ClozeSettingsSide {
    //     ClozeSettingsSide::Start
    // }

    fn construct_setting(&self, data: &str) -> String {
        construct_setting(data)
    }

    fn construct_comment(&self, data: &str) -> String {
        construct_comment(data)
    }

    fn extract_comment<'a>(&self, data: &'a str) -> &'a str {
        extract_comment(data)
    }

    fn construct_file_data(
        &self,
        output_type: ConstructFileDataType,
        data: &GenerateNoteFilesRequest,
        note_import_action: &NoteImportAction,
    ) -> String {
        construct_file_data(self, output_type, data, note_import_action)
    }

    fn file_extension(&self) -> &'static str {
        file_extension(self)
    }

    fn construct_cloze_replacement(
        &self,
        cloze_replacement: &ClozeReplacement,
        side: CardSide,
    ) -> String {
        construct_cloze_replacement(cloze_replacement, side)
    }

    fn construct_image_occlusion(
        &self,
        image_occlusion_data: &ImageOcclusionData,
        output_type: ConstructImageOcclusionType,
    ) -> String {
        construct_image_occlusion(self, image_occlusion_data, output_type)
    }

    fn get_output_rendered_dir(&self, output_type: RenderOutputDirectoryType) -> PathBuf {
        get_output_rendered_dir(self, output_type)
    }

    fn get_aux_dir(&self, output_type: RenderOutputType, note_id: NoteId) -> PathBuf {
        get_aux_dir(self, output_type, note_id)
    }

    fn render_file(
        &self,
        aux_dir: &Path,
        output_text_filepath: &Path,
        output_rendered_dir: &Path,
        output_rendered_filepath: &Path,
    ) -> Result<std::process::Output, Error> {
        render_file(
            aux_dir,
            output_text_filepath,
            output_rendered_dir,
            output_rendered_filepath,
        )
    }
}

// Generic latex functions
// NOTE: This does NOT handle nested commands since generally LaTeX commands are not nested, only LaTeX environments might be.
fn get_latex_command(
    data: &str,
    start_regex: &Regex,
) -> Result<Vec<RegexMatch>, DelimiterErrorKind> {
    let start_matches = start_regex
        .find_iter(data)
        .map(|m| m.unwrap())
        .map(|m| (m.start()..m.end()))
        .collect::<Vec<_>>();
    let end_matches = start_matches
        .iter()
        .map(|start_range| {
            let e = start_range.end;
            let end = find_pair(&data[e..], '{', '}')?;
            Ok(e + end..e + end + 1)
        })
        .collect::<Result<Vec<_>, DelimiterErrorKind>>()?;
    assert_eq!(start_matches.len(), end_matches.len());
    let all_matches: Vec<(Range<usize>, Range<usize>)> = start_matches
        .into_iter()
        .zip(end_matches)
        .collect::<Vec<_>>();
    let flattened_matches: Vec<usize> = all_matches
        .iter()
        .flat_map(|(start_range, end_range)| {
            [
                start_range.start,
                start_range.end,
                end_range.start,
                end_range.end,
            ]
        })
        .collect::<Vec<_>>();
    if !is_monotonic_increasing(&flattened_matches) {
        return Err(DelimiterErrorKind::UnequalMatches {
            src: data.to_string(),
        });
    }
    let result = all_matches
        .into_iter()
        .map(|(start_range, end_range)| RegexMatch {
            match_range: (start_range.start..end_range.end),
            capture_range: (start_range.end..end_range.start),
        })
        .collect::<Vec<_>>();
    Ok(result)
}

fn get_linked_notes(data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
    let linked_notes_start_regex = Regex::new(r"\\li{").unwrap();
    let linked_notes = get_latex_command(data, &linked_notes_start_regex)
        .map_err(LibraryError::Delimiter)?
        .into_iter()
        .map(|settings_match| settings_match.capture_range)
        .collect::<Vec<_>>();
    Ok(linked_notes)
}

fn get_settings(data: &str) -> Result<Vec<RegexMatch>, LibraryError> {
    let settings_start_regex = Regex::new(r"\\se{").unwrap();
    let settings_str =
        get_latex_command(data, &settings_start_regex).map_err(LibraryError::Delimiter)?;
    Ok(settings_str)
}

fn construct_setting(data: &str) -> String {
    format!("\\se{{{data}}}\n")
}

fn construct_comment(data: &str) -> String {
    // Add trailing newline (POSIX convention)
    format!("% {data}\n")
}

fn extract_comment(data: &str) -> &str {
    data.strip_prefix("%").map_or(data, |x| x.trim())
}

// #[allow(clippy::unnecessary_wraps, reason = "Match trait function signature")]
// fn comment_regex() -> Option<Regex> {
//     // Keep comments
//     None
//     // Some(Regex::new(r"(?m)(?<!\\)%.*$").unwrap())
// }

#[allow(clippy::let_and_return, reason = "Make note vs card data explicit")]
#[allow(clippy::too_many_lines, reason = "File data is long")]
fn construct_file_data(
    parser: &impl Parseable,
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
    } = parser.note_settings_keys();
    let note_id_string = format!(
        "{}{} {}",
        note_id_key.get_write(),
        settings_key_value_delim,
        note_id
    );
    let keywords_string = format!("keywords{} {}", settings_key_value_delim, keywords_str);
    let tags_string = format!("tags{} {}", settings_key_value_delim, tags_str);
    match output_type {
        ConstructFileDataType::Note => {
            let linked_notes_str = get_linked_notes_string(parser, linked_notes.as_ref());
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
                parser.construct_setting(custom_data_string.as_str())
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
                //     parser.construct_comment("Custom data"),
                //     parser.construct_setting(custom_data_str_content.as_str()),
                // )
            };
            let action_string = if matches!(note_import_action, NoteImportAction::Update(_)) {
                String::new()
            } else {
                let action_value = match note_import_action {
                    NoteImportAction::Add => action_add_key,
                    NoteImportAction::Update(_) | NoteImportAction::Delete(_) => unreachable!(),
                };
                parser.construct_setting(&format!(
                    "{}{} {}",
                    action_key.get_write(),
                    settings_key_value_delim,
                    action_value.get_write(),
                ))
            };
            let lines = [
                // parser.construct_setting("spares: start"),
                // "\n".to_string(),
                "\n".to_string(),
                action_string,
                parser.construct_setting(&note_id_string),
                parser.construct_setting(&keywords_string),
                parser.construct_setting(&tags_string),
                custom_data_str,
                "\\begin{note}".to_string(),
                "\n".to_string(),
                note_data.to_string(),
                "\n".to_string(),
                "\\end{note}".to_string(),
                linked_notes_str,
                "\n".to_string(),
                "\n".to_string(),
                // "\n".to_string(),
                // parser.construct_setting("spares: end"),
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
                    NotePart::ClozeData(d, cloze_replacement) => parser
                        .construct_cloze_replacement(
                            &ClozeReplacement::parse(side, cloze_replacement, d),
                            side,
                        ),
                    NotePart::SurroundingData(d) => d.to_string(),
                    NotePart::ImageOcclusion { data, .. } => {
                        let image_occlusion = parser.construct_image_occlusion(
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
            let lines = [
                // parser.construct_setting("spares: start"),
                "\n".to_string(),
                "\n".to_string(),
                // parser.construct_setting(action_string.as_str()),
                parser.construct_setting(&note_id_string),
                parser.construct_setting(&keywords_string),
                parser.construct_setting(&tags_string),
                // custom_data_str,
                "\\begin{mdframed}".to_string(),
                "\n".to_string(),
                card_data.to_string(),
                "\n".to_string(),
                "\\end{mdframed}".to_string(),
                // linked_notes_str,
                "\n".to_string(),
                "\n".to_string(),
                // "\n".to_string(),
                // parser.construct_setting("spares: end"),
            ];
            let card_file_data = lines.into_iter().collect::<String>();
            card_file_data
        }
    }
}

fn get_linked_notes_string(
    parser: &impl Parseable,
    linked_notes: Option<&Vec<LinkedNote>>,
) -> String {
    if let Some(linked_notes) = linked_notes {
        let items = linked_notes
            .iter()
            .map(|linked_note_request| {
                let LinkedNote {
                    searched_keyword,
                    linked_note_id,
                    matched_keyword,
                } = linked_note_request;
                assert_eq!(linked_note_id.is_some(), matched_keyword.is_some());
                match (linked_note_id, matched_keyword) {
                    (None, None) => "  \\item{{-}}".to_string(),
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
                            "  \\item{{\\lhref{{{}}}{{{} $\\rightarrow$ {}}}}}",
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
        format!(
            "\n\nLinked Notes:\n\\begin{{enumerate}}[label=\\arabic*.]\n{}\n\\end{{enumerate}}",
            items
        )
    } else {
        String::new()
    }
}

// fn start_end_regex() -> Regex {
//     Regex::new(r"(?s)% spares: start\n(.*)\n% spares: end").unwrap()
//     // Regex::new(r"(?s)\\se\{spares: start}\n(.*)\n\\se\{spares: end}").unwrap()
// }

fn file_extension(_: &impl Parseable) -> &'static str {
    "tex"
}

fn construct_cloze_replacement(cloze_replacement: &ClozeReplacement, _side: CardSide) -> String {
    match cloze_replacement {
        ClozeReplacement::Hidden(cloze_replacement) => match cloze_replacement {
            ClozeHiddenReplacement::ToAnswer { hint } => {
                if let Some(hint) = hint {
                    format!("\\hl{{\\_\\_\\_\\_\\_ ({})}}", hint)
                } else {
                    "\\hl{\\_\\_\\_\\_\\_}".to_string()
                }
            }
            ClozeHiddenReplacement::NotToAnswer => {
                "{\\sethlcolor{{green}}\\hl{\\_\\_\\_\\_\\_}}".to_string()
            }
        },
        ClozeReplacement::Reveal(data) => format!("{{\\sethlcolor{{blue}}\\hl{{{}}}}}", data),
    }
}

fn construct_image_occlusion(
    parser: &impl Parseable,
    image_occlusion_data: &ImageOcclusionData,
    output_type: ConstructImageOcclusionType,
) -> String {
    fn construct_image(file_path: &Path, _caption: &str) -> String {
        [
            r"\begin{figure}".to_string(),
            format!(
                "  \\includesvg[width=0.5\\textwidth,height=0.8\\textheight,keepaspectratio]{}",
                file_path.display()
            ),
            r"\end{figure}".to_string(),
            String::new(),
        ]
        .join("\n")
    }
    construct_image_occlusion_from_image(parser, construct_image, image_occlusion_data, output_type)
}

fn get_output_rendered_dir(_: &impl Parseable, _output_type: RenderOutputDirectoryType) -> PathBuf {
    if cfg!(feature = "testing") {
        return get_cache_dir();
    }
    std::env::var("LATEX_OUT_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|dir| dir.exists())
        .unwrap_or_else(get_cache_dir)
}

fn get_aux_dir(parser: &impl Parseable, output_type: RenderOutputType, note_id: NoteId) -> PathBuf {
    // NOTE: `$out_dir` can be a directory with a large number of files. However, $aux_dir must be a directory with a relatively small number of files. Otherwise, latexmk will take significantly longer to run (sometimex 5x). Thus, we create a subdirectory for each file to hold the auxiliary files. Note that this path must match that configured in `vimtex`. See `Workflows` in the Spares Manual for more details.
    let directory_output_type = match output_type {
        RenderOutputType::Note => RenderOutputDirectoryType::Note,
        RenderOutputType::Card(..) => RenderOutputDirectoryType::Card,
    };
    let mut aux_dir = parser.get_output_rendered_dir(directory_output_type);
    aux_dir.push("aux");
    aux_dir.push(format!("{:0>4}", note_id));
    aux_dir
}

fn render_file(
    aux_dir: &Path,
    output_text_filepath: &Path,
    output_rendered_dir: &Path,
    _output_rendered_filepath: &Path,
) -> Result<Output, Error> {
    // This is currently the same `pdflatex` command that `latexmk` ends up running.
    // let output = Command::new("pdflatex")
    //     .arg("-shell-escape") // for svg package
    //     .arg("-file-line-error")
    //     .arg("-synctex=1")
    //     .arg("-recorder")
    //     .arg("-interaction=nonstopmode")
    //     .arg("-output-directory")
    //     .arg(&output_rendered_dir)
    //     .arg(&output_text_filepath)
    //     .output()
    //     .map_err(|e| SrsError::Io(e, "Failed to run latex command".to_string()))?;

    // Command is from <https://github.com/lervag/vimtex/blob/8ca74380935beb4ed5d213bb55b2380cc1a83bd6/doc/vimtex.txt#L1019>.
    let mut auxdir_arg: OsString = "-auxdir=".to_owned().into();
    auxdir_arg.push(aux_dir);
    let mut outdir_arg: OsString = "-outdir=".to_owned().into();
    outdir_arg.push(output_rendered_dir);
    // Example for testing:
    // `cd $XDG_DATA_HOME/spares/notes/latex-note/ && time latexmk -verbose -file-line-error -synctex=1 -interaction=nonstopmode -pdf -auxdir=$XDG_CACHE_HOME/vimtex/aux/2051 $XDG_DATA_HOME/spares/notes/latex-note/2051.tex`
    let output = Command::new("latexmk")
        .arg("-verbose")
        .arg("-file-line-error")
        .arg("-synctex=1")
        .arg("-interaction=nonstopmode")
        .arg("-pdf")
        .arg(&auxdir_arg)
        .arg(&outdir_arg)
        .arg(output_text_filepath)
        // Current directory is changed to ensure first vimtex compilation is fast.
        .current_dir(output_text_filepath.parent().unwrap())
        .output()
        .map_err(|e| Error::Io {
            description: "Failed to run latex command".to_string(),
            source: e,
        })?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use crate::{
        adapters::get_adapter_from_string,
        parsers::{
            BackReveal, BackType, CardData, ClozeGrouping, ClozeHiddenReplacement, FrontConceal,
            NoteImportAction, NotePart, Parseable, get_cards, get_notes,
            impls::latex::LatexParserNote,
        },
    };
    use indoc::indoc;
    use serde_json::Number;

    #[test]
    fn test_get_cards_basic_1_latex() {
        let data = indoc! { r"a
            \begin{cl}
            b
            \end{cl}
            c"};
        let parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
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
                    NotePart::SurroundingData("a\n".to_string()),
                    NotePart::ClozeStart("\\begin{cl}[o:1]".to_string()),
                    NotePart::ClozeData(
                        "\nb\n".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("\\end{cl}".to_string()),
                    NotePart::SurroundingData("\nc".to_string()),
                ],
            }];
            assert_eq!(cards, expected);
        }
        let cards_res = get_cards(parser.as_ref(), None, data, false, false);
        assert!(cards_res.is_ok());
        if let Ok(cards) = cards_res {
            let expected = vec![CardData {
                order: None,
                grouping: ClozeGrouping::Auto(1),
                is_suspended: None,
                front_conceal: FrontConceal::OnlyGrouping,
                back_reveal: BackReveal::FullNote,
                back_type: BackType::FullNote,
                data: vec![
                    NotePart::SurroundingData("a\n".to_string()),
                    NotePart::ClozeStart("\\begin{cl}".to_string()),
                    NotePart::ClozeData(
                        "\nb\n".to_string(),
                        ClozeHiddenReplacement::ToAnswer { hint: None },
                    ),
                    NotePart::ClozeEnd("\\end{cl}".to_string()),
                    NotePart::SurroundingData("\nc".to_string()),
                ],
            }];
            assert_eq!(cards, expected);
        }
    }

    #[test]
    fn test_get_notes_custom_data_1() {
        let parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! {r#"
            \se{custom-data: {"num_days_to_simulate":25}}
            \begin{note}
            a
            \begin{cl}
            b
            \end{cl}
            \end{note}
            "#};
        let notes_res = get_notes(parser.as_ref(), None, data, adapter.as_ref(), false);
        assert!(notes_res.is_ok());
        let notes = notes_res.unwrap();
        assert_eq!(notes.len(), 1);
        let (note_settings, _note_data) = &notes[0];
        let value = note_settings.custom_data.get("num_days_to_simulate");
        assert_eq!(value, Some(&serde_json::Value::Number(Number::from(25))));
    }

    #[test]
    fn test_get_notes_custom_data_2() {
        let parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! { r#"
            \se{action: update}
            \se{note-id: 10}
            \se{anki-note-id: 99}
            \begin{note}
            a
            \begin{cl}
            b
            \end{cl}
            \end{note}
            "#};
        let notes_res = get_notes(parser.as_ref(), None, data, adapter.as_ref(), false);
        assert!(notes_res.is_ok());
        let notes = notes_res.unwrap();
        assert_eq!(notes.len(), 1);
        let (note_settings, _note_data) = &notes[0];

        assert_eq!(note_settings.action, NoteImportAction::Update(10));

        let value = note_settings.custom_data.get("note-id");
        assert_eq!(value, None);

        let value = note_settings.custom_data.get("anki-note-id");
        assert_eq!(value, Some(&serde_json::Value::String("99".to_string())));
    }
}
