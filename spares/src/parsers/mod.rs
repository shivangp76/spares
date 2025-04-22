use crate::adapters::SrsAdapter;
use crate::config::{get_cache_dir, get_config_dir};
use crate::model::{CustomData, NoteId};
use crate::{Error, LibraryError, ParserErrorKind};
use fancy_regex::Regex;
use generate_files::{CardSide, GenerateNoteFilesRequest, RenderOutputType};
use image_occlusion::{ConstructImageOcclusionType, ImageOcclusionData};
use std::fs::read_to_string;
use std::ops::Range;
use std::path::{Path, PathBuf};

mod cards;
mod clozes;
pub mod generate_files;
mod helpers;
pub mod image_occlusion;
pub mod impls;
mod notes;
mod settings;
pub use cards::*;
pub use clozes::*;
pub use helpers::*;
pub use notes::*;
pub use settings::*;

#[derive(Clone, Copy, Debug)]
pub enum ConstructFileDataType<'a> {
    Note,
    /// Card order
    Card(usize, &'a CardData, CardSide),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderOutputDirectoryType {
    Note,
    Card,
}

#[derive(Clone, Debug)]
pub struct TemplateData {
    pub template_contents: String,
    pub body_placeholder: String,
}

// These functions directly parse the data since each parser might have different regex capture groups, so that logic should be in the parser, not abstracted.
// There may be multiple parsers for the same file type. (Ex. Latex parser for math notes and a latex parser for chem notes). Thus, Parseable is a trait, not an enum.
pub trait Parseable: Send + Sync {
    /// This is used as a directory name, so only certain characters are valid. The preferred format is lower case with dashes.
    fn get_parser_name(&self) -> &'static str;

    fn get_notes_data(&self, data: &str) -> Result<Vec<NoteRawData>, LibraryError> {
        let start = self.construct_comment("spares: note start");
        let end = self.construct_comment("spares: note end");
        let regex_string = format!(
            "(?ms){}(.*?)\n{}?",
            fancy_regex::escape(&start),
            fancy_regex::escape(&end)
        );
        let notes_regex = Regex::new(&regex_string).unwrap();
        let notes_data = notes_regex
            .captures_iter(data)
            .map(|c| c.unwrap())
            .filter(|c| c.get(1).is_some())
            .map(|c| NoteRawData {
                metadata: None,
                data: c.get(1).map(|x| (x.start()..x.end())).unwrap(),
            })
            .collect::<Vec<_>>();
        Ok(notes_data)
    }

    // Nested clozes make it so "data" can NOT be split into disjoint segment of NotePart::Data and NotePart::Cloze. This is because what a cloze really represents is that you want to see everything else *besides* what is in the cloze.
    fn get_linked_notes(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError>;

    fn get_settings(&self, data: &str) -> Result<Vec<RegexMatch>, LibraryError>;

    fn note_settings_keys(&self) -> NoteSettingsKeys {
        NoteSettingsKeys::default()
    }

    fn start_end_regex(&self) -> Regex {
        let start = self.construct_comment("spares: start");
        let end = self.construct_comment("spares: end");
        let regex_string = format!(
            "(?s){}(.*?)\n{}?",
            fancy_regex::escape(&start),
            fancy_regex::escape(&end)
        );
        Regex::new(&regex_string).unwrap()
    }

    fn get_clozes(&self, data: &str) -> Result<Vec<ClozeMatch>, LibraryError>;

    fn cloze_settings_keys(&self) -> ClozeSettingsKeys {
        ClozeSettingsKeys::default()
    }

    // By returning a prefix and suffix, we allow the `cloze_settings_string` to modify the
    // string both before and after the cloze's data. Typically, this is not needed since the
    // settings string is attached to either the start or end of the cloze. However, when
    // converting between parsers, the other delimiter will also likely change length. For example,
    // if converting from markdown's `{{[o:1]` and `}}` to latex's `\\begin{note}[o:1]` and `\\end{note}`,
    // the length of the delimiter increases even though both have their settings strings attached
    // to the starting delimiter.
    fn construct_cloze(&self, cloze_settings_string: &str, data: &str) -> (String, String);

    // fn cloze_settings_side(&self) -> ClozeSettingsSide;

    fn construct_cloze_replacement(
        &self,
        cloze_replacement: &ClozeReplacement,
        side: CardSide,
    ) -> String;

    fn construct_setting(&self, data: &str) -> String;

    fn construct_comment(&self, data: &str) -> String;

    fn extract_comment<'a>(&self, data: &'a str) -> &'a str;

    fn get_image_occlusions(&self, data: &str) -> Result<Vec<RegexMatch>, LibraryError> {
        let start = self.construct_comment("spares: image occlusion start");
        let end = self.construct_comment("spares: image occlusion end");
        let regex_string = format!("(?s){}(.*?)\n{}?", start, end);
        let image_occlusion_regex = Regex::new(&regex_string).unwrap();
        let image_occlusions = image_occlusion_regex
            .find_iter(data)
            .map(|m| m.unwrap())
            .map(|m| (m.start()..m.end()))
            .zip(
                image_occlusion_regex
                    .captures_iter(data)
                    .map(|c| c.unwrap().get(1).map(|x| (x.start()..x.end())).unwrap()),
            )
            .map(|(match_range, capture_range)| RegexMatch {
                match_range,
                capture_range,
            })
            .collect::<Vec<_>>();
        Ok(image_occlusions)
    }

    // The original image filepath and clozes filepath can be passed back to `svgedit` to modify the card. Upon reimporting this card, the image occlusion file will be parsed again and the card file paths will be updated.
    fn construct_image_occlusion(
        &self,
        image_occlusion_data: &ImageOcclusionData,
        output_type: ConstructImageOcclusionType,
    ) -> String;

    fn construct_file_data(
        &self,
        output_type: ConstructFileDataType,
        request: &GenerateNoteFilesRequest,
        note_import_action: &NoteImportAction,
    ) -> String;

    fn construct_full_file_data(
        &self,
        requests: &[(ConstructFileDataType, &GenerateNoteFilesRequest)],
        note_import_action: &NoteImportAction,
    ) -> String {
        assert!(!requests.is_empty());
        let first_request_construct_data_type = requests.first().unwrap().0;
        assert!(requests.iter().map(|x| x.0).all(|x| {
            match first_request_construct_data_type {
                ConstructFileDataType::Note => matches!(x, ConstructFileDataType::Note),
                ConstructFileDataType::Card(_, _, side) => match side {
                    CardSide::Front => {
                        matches!(x, ConstructFileDataType::Card(_, _, CardSide::Front))
                    }
                    CardSide::Back => {
                        matches!(x, ConstructFileDataType::Card(_, _, CardSide::Back))
                    }
                },
            }
        }));

        match requests.first().unwrap().0 {
            ConstructFileDataType::Note => {
                let mut result = vec![self.construct_comment("spares: start"), "\n".to_string()];
                for (data_type, request) in requests {
                    result.push(self.construct_file_data(*data_type, request, note_import_action));
                }
                result.extend(["\n".to_string(), self.construct_comment("spares: end")]);
                result.into_iter().collect::<String>()
            }
            ConstructFileDataType::Card(..) => requests
                .iter()
                .map(|(data_type, request)| {
                    self.construct_file_data(*data_type, request, note_import_action)
                })
                .collect::<String>(),
        }
    }

    fn render_file(
        &self,
        aux_dir: &Path,
        output_text_filepath: &Path,
        output_rendered_dir: &Path,
        output_rendered_filepath: &Path,
    ) -> Result<std::process::Output, Error>;

    // fn comment_regex(&self) -> Option<Regex> {
    //     None
    // }

    fn file_extension(&self) -> &'static str;

    fn template_contents(&self) -> Result<TemplateData, std::io::Error> {
        let body_placeholder = self.construct_comment("spares: note body");
        if cfg!(feature = "testing") {
            // let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            // path.push("src/parsers/impls/templates/template.tex");
            return Ok(TemplateData {
                template_contents: body_placeholder.clone(),
                body_placeholder,
            });
        }
        let mut template_path: PathBuf = get_config_dir();
        let parser_name = self.get_parser_name();
        template_path.push(parser_name);
        template_path.push("templates");
        let file_extension = self.file_extension();
        let template_filename = format!("template.{}", file_extension);
        template_path.push(template_filename.as_str());

        let template_contents = read_to_string(&template_path)?;
        Ok(TemplateData {
            template_contents,
            body_placeholder,
        })
    }

    // This can be overridden for a specific parser, so it is in the trait.
    fn get_output_rendered_dir(&self, _output_type: RenderOutputDirectoryType) -> PathBuf {
        get_cache_dir()
    }

    // This can be overridden for a specific parser, so it is in the trait.
    fn get_aux_dir(&self, output_type: RenderOutputType, _note_id: NoteId) -> PathBuf {
        let directory_output_type = match output_type {
            RenderOutputType::Note => RenderOutputDirectoryType::Note,
            RenderOutputType::Card(..) => RenderOutputDirectoryType::Card,
        };
        self.get_output_rendered_dir(directory_output_type)
    }

    // This is separated from the get_.*_dir functions since for syncing notes, cards are rendering in /tmp, where the file name is needed, but not the rest of the filepath.
    fn get_output_filename(&self, output_type: RenderOutputType, note_id: NoteId) -> String {
        match output_type {
            RenderOutputType::Note => {
                format!("{:0>4}.pdf", note_id)
            }
            RenderOutputType::Card(card_order, side) => match side {
                CardSide::Front => {
                    format!("{:0>4}-{:0>1}-front.pdf", note_id, card_order)
                }
                CardSide::Back => {
                    format!("{:0>4}-{:0>1}-back.pdf", note_id, card_order)
                }
            },
        }
    }
}

pub fn validate_parser(parser: &dyn Parseable) -> Option<String> {
    // Ensure that the parser name only contains lowercase and dashes to make sure it is safe to use as a directory name.
    if parser
        .get_parser_name()
        .chars()
        .any(|c| !(c.is_ascii_lowercase() || c == '-'))
    {
        return Some("Invalid characters returned from `parser.get_parser_name()`. Only lowercase letters and dashes are allowed.".to_string());
    }
    None
}

pub fn get_all_parsers() -> Vec<fn() -> Box<dyn Parseable>> {
    // NOTE: Add parser here
    // Also run: `spares_cli add parser --name="NAME"`
    let all_parsers: Vec<fn() -> Box<dyn Parseable>> = vec![
        || Box::new(impls::latex::LatexParserExerciseSolution::new()),
        || Box::new(impls::latex::LatexParserNote::new()),
        || Box::new(impls::markdown::MarkdownParser::new()),
        || Box::new(impls::typst::TypstParser::new()),
    ];
    all_parsers
}

pub fn find_parser(
    parser_str: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Box<dyn Parseable>, Error> {
    let matching_parsers = all_parsers
        .iter()
        .filter(|p| parser_str == p().get_parser_name())
        .collect::<Vec<_>>();
    if matching_parsers.is_empty() {
        return Err(Error::Library(LibraryError::Parser(
            ParserErrorKind::NotFound(parser_str.to_string()),
        )));
    }
    if matching_parsers.len() > 1 {
        return Err(Error::Library(LibraryError::Parser(
            ParserErrorKind::NotFound(parser_str.to_string()),
        )));
    }
    Ok(matching_parsers[0]())
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn test_parsers_validation() {
        let all_parsers = get_all_parsers();
        assert!(!all_parsers.is_empty());
        let mut all_parser_names = Vec::new();
        for parser_fn in all_parsers {
            let parser = parser_fn();
            all_parser_names.push(parser.get_parser_name());
            assert!(validate_parser(parser.as_ref()).is_none());
        }
        assert_eq!(
            all_parser_names.len(),
            all_parser_names.iter().unique().count()
        );
    }
}
