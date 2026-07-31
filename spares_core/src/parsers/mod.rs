use std::fmt::Write as _;
use std::fs::read_to_string;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;

use fancy_regex::Regex;
use generate_files::CardSide;
use generate_files::GenerateNoteFilesRequest;
use generate_files::RenderOutputType;
use image_occlusion::ConstructImageOcclusionType;
use image_occlusion::ImageOcclusionData;
use image_occlusion::ImageOcclusionMatch;

use crate::Error;
use crate::LibraryError;
use crate::ParserErrorKind;
use crate::adapters::SrsAdapter;
use crate::config::get_cache_dir;
use crate::config::get_config_dir;
use crate::model::CustomData;
use crate::model::NoteId;

mod cards;
pub(crate) mod cli;
mod clozes;
pub mod generate_files;
mod helpers;
pub(crate) mod image_occlusion;
pub(crate) mod impls;
mod notes;
mod settings;

pub use cards::*;
pub use cli::CliBlockMatch;
pub use cli::CliData;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TemplateType {
    Note,
    Card,
    Export,
}

// These functions directly parse the data since each parser might have different regex capture groups, so that logic should be in the parser, not abstracted.
// There may be multiple parsers for the same file type. (Ex. Latex parser for math notes and a latex parser for chem notes). Thus, Parseable is a trait, not an enum.
pub trait Parseable: Send + Sync {
    /// This is used as a directory name, so only certain characters are valid. The preferred format is lower case with dashes.
    fn get_parser_name(&self) -> &'static str;

    fn get_notes_data(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        let start = self.construct_comment("spares: note start");
        let end = self.construct_comment("spares: note end");
        let regex_string = format!(
            "(?ms)(?<=\\A|\n){}(.*?)\n{}?",
            fancy_regex::escape(&start),
            fancy_regex::escape(&end)
        );
        let notes_regex = Regex::new(&regex_string).unwrap();
        let notes_data = notes_regex
            .captures_iter(data)
            .map(|c| c.unwrap())
            .filter(|c| c.get(1).is_some())
            .map(|c| c.get(1).map(|x| x.start()..x.end()).unwrap())
            .collect::<Vec<_>>();
        Ok(notes_data)
    }

    fn get_linked_notes(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError>;

    fn get_embedded_keywords(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError>;

    fn get_settings(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError>;

    fn note_settings_keys(&self) -> NoteSettingsKeys {
        NoteSettingsKeys::default()
    }

    fn start_end_regex(&self) -> Regex {
        let start = self.construct_comment("spares: start");
        let end = self.construct_comment("spares: end");
        let regex_string = format!(
            "(?s)(?<=\\A|\n){}(.*?)\n{}?",
            fancy_regex::escape(&start),
            fancy_regex::escape(&end)
        );
        Regex::new(&regex_string).unwrap()
    }

    // Nested clozes make it so "data" can NOT be split into disjoint segment of NotePart::Data and NotePart::Cloze. This is because what a cloze really represents is that you want to see everything else *besides* what is in the cloze.
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
        id: Option<&str>,
    ) -> String;

    fn construct_setting(&self, data: &str) -> String;

    fn construct_comment(&self, data: &str) -> String;

    fn extract_comment<'a>(&self, data: &'a str) -> &'a str;

    fn get_image_occlusions(&self, data: &str) -> Result<Vec<ImageOcclusionMatch>, LibraryError> {
        let start = self.construct_comment("spares: image occlusion start");
        let end = self.construct_comment("spares: image occlusion end");
        let regex_string = format!("(?s){}(.*?)\n{}?", start, end);
        let image_occlusion_regex = Regex::new(&regex_string).unwrap();
        let image_occlusions = image_occlusion_regex
            .find_iter(data)
            .map(|m| m.unwrap())
            .map(|m| m.start()..m.end())
            .zip(
                image_occlusion_regex
                    .captures_iter(data)
                    .map(|c| c.unwrap().get(1).map(|x| x.start()..x.end()).unwrap()),
            )
            .map(|(range, settings_range)| ImageOcclusionMatch {
                range,
                settings_range,
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

    /// Find all `spares: cli start…end` blocks in `data`. Default impl uses
    /// the parser's comment syntax, so this works for any host parser.
    /// The canonical implementation lives in [`cli::get_cli_blocks`]; this
    /// trait method cannot delegate there directly (object-safety), so it
    /// shares the same cache and unterminated-block detection helpers.
    fn get_cli_blocks(&self, data: &str) -> Result<Vec<CliBlockMatch>, LibraryError> {
        let start = self.construct_comment("spares: cli start");
        let end = self.construct_comment("spares: cli end");
        let regex_string = format!(
            "(?s){}(.*?)\n{}",
            fancy_regex::escape(&start),
            fancy_regex::escape(&end),
        );
        let cli_regex = cli::get_or_compile_cli_regex(&regex_string)?;
        let mut blocks = Vec::new();
        for captures in cli_regex.captures_iter(data) {
            let captures = captures.map_err(|e| {
                LibraryError::Note(crate::NoteErrorKind::Other {
                    description: e.to_string(),
                })
            })?;
            let full = captures.get(0).ok_or_else(|| {
                LibraryError::Note(crate::NoteErrorKind::Other {
                    description: "cli block regex produced no full match".to_string(),
                })
            })?;
            let body = captures.get(1).ok_or_else(|| {
                LibraryError::Note(crate::NoteErrorKind::Other {
                    description: "cli block regex produced no body capture".to_string(),
                })
            })?;
            blocks.push(CliBlockMatch {
                range: full.start()..full.end(),
                body_range: body.start()..body.end(),
            });
        }
        cli::check_unterminated_blocks(
            data,
            &start,
            &blocks.iter().map(|b| b.range.clone()).collect::<Vec<_>>(),
        )?;
        Ok(blocks)
    }

    /// Build the textual form of a CLI block in `parser`'s comment syntax.
    /// Re-emit a CLI block in this parser's comment syntax. Symmetric to
    /// [`Self::construct_image_occlusion`]; used by `add_order_to_note_data`
    /// to round-trip note data through card assembly.
    ///
    /// **Round-trip hazard:** if `exec` contains a line that, after
    /// `construct_comment`, equals the `spares: cli end` comment marker
    /// (e.g. `<!--- spares: cli end --->` in markdown), re-parsing the
    /// output will truncate the block at that point. This is a known
    /// limitation; multi-line `exec` values should avoid such lines.
    fn construct_cli_block(&self, cli_data: &CliData) -> String {
        // Build TOML `exec = "..."` with proper escaping for basic strings.
        // Avoids `toml_edit::ser` which can fail on control characters
        // (U+0000..=U+001F) that are illegal in TOML basic strings.
        fn escape_toml_basic(s: &str) -> String {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    // c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
                    c if c.is_control() => {
                        let _ = write!(out, "\\u{:04X}", c as u32);
                    }
                    c => out.push(c),
                }
            }
            out
        }
        let exec_line = format!(r#"exec = "{}""#, escape_toml_basic(&cli_data.exec));
        let mut out = String::new();
        out.push_str(&self.construct_comment("spares: cli start"));
        out.push_str(&self.construct_comment(&exec_line));
        out.push_str(&self.construct_comment("spares: cli end"));
        out
    }

    fn construct_file_data(
        &self,
        output_type: ConstructFileDataType,
        request: &GenerateNoteFilesRequest,
        note_import_action: &NoteImportAction,
        include_separator: bool,
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
                for (i, (data_type, request)) in requests.iter().enumerate() {
                    result.push(self.construct_file_data(
                        *data_type,
                        request,
                        note_import_action,
                        // If it's not the last one, then include separator.
                        i != requests.len() - 1,
                    ));
                }
                result.extend(["\n".to_string(), self.construct_comment("spares: end")]);
                result.into_iter().collect::<String>()
            }
            ConstructFileDataType::Card(..) => requests
                .iter()
                .map(|(data_type, request)| {
                    self.construct_file_data(*data_type, request, note_import_action, false)
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

    fn get_template_data(
        &self,
        template_type: TemplateType,
    ) -> Result<(String, String), std::io::Error> {
        let body_placeholder = self.construct_comment("spares: body");
        if cfg!(test) {
            // let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            // path.push("src/parsers/impls/templates/template.tex");
            return Ok((body_placeholder.clone(), body_placeholder));
        }
        match template_type {
            TemplateType::Note | TemplateType::Card => {
                let mut note_template_path: PathBuf = get_config_dir();
                let parser_name = self.get_parser_name();
                note_template_path.push(parser_name);
                note_template_path.push("templates");
                let file_extension = self.file_extension();
                let note_template_filename = format!("note_template.{}", file_extension);
                note_template_path.push(note_template_filename.as_str());
                let note_template_contents = read_to_string(&note_template_path)?;
                if template_type == TemplateType::Note {
                    return Ok((note_template_contents, body_placeholder));
                }
                let mut card_template_path = note_template_path.clone();
                let card_template_filename = format!("card_template.{}", file_extension);
                card_template_path.set_file_name(card_template_filename);
                let card_template_contents = if card_template_path.is_file() {
                    read_to_string(&card_template_path)?
                } else {
                    note_template_contents.clone()
                };
                Ok((card_template_contents, body_placeholder))
            }
            TemplateType::Export => {
                let mut export_template_path: PathBuf = get_config_dir();
                let parser_name = self.get_parser_name();
                export_template_path.push(parser_name);
                export_template_path.push("templates");
                let file_extension = self.file_extension();
                let export_template_filename = format!("export_template.{}", file_extension);
                export_template_path.push(export_template_filename.as_str());
                let export_template_contents = read_to_string(&export_template_path)?;
                Ok((export_template_contents, body_placeholder))
            }
        }
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

pub fn construct_card_data(
    parser: &dyn Parseable,
    card_order: usize,
    card_data: &CardData,
    side: CardSide,
    note_id: NoteId,
) -> String {
    let card_id = cloze_tag_str(note_id, card_order);
    let mut id_injected = false;
    let mut image_occlusion_order: usize = 1;
    card_data
        .data
        .iter()
        .map(|p| match p {
            NotePart::ClozeData(d, cloze_replacement) => {
                let replacement = ClozeReplacement::parse(side, cloze_replacement, d);
                let id_opt = if !id_injected
                    && matches!(cloze_replacement, ClozeHiddenReplacement::ToAnswer { .. })
                {
                    id_injected = true;
                    Some(card_id.as_str())
                } else {
                    None
                };
                parser.construct_cloze_replacement(&replacement, side, id_opt)
            }
            NotePart::SurroundingData(d) => d.clone(),
            NotePart::ImageOcclusion { data, .. } => {
                let image_occlusion = parser.construct_image_occlusion(
                    data,
                    ConstructImageOcclusionType::Card {
                        side,
                        note_id,
                        card_order,
                        image_occlusion_order,
                    },
                );
                image_occlusion_order += 1;
                image_occlusion
            }
            NotePart::Cli { .. } | NotePart::ClozeStart(_) | NotePart::ClozeEnd(_) => String::new(),
        })
        .collect::<String>()
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
    // Also run: `spares parser add --name="NAME"`
    let all_parsers: Vec<fn() -> Box<dyn Parseable>> = vec![
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
