//! The "markdown-language-tools" parser: the [Markdown parser](super::markdown), with tools for
//! learning languages. It is opt-in (`spares parser add --name markdown-language-tools`), and a
//! collection without it is unchanged. The manual's `language-tools.md` is the user's guide.
//!
//! - **Readings** ([`readings`]): `漢字[かんじ]` shows かんじ above 漢字, as HTML `<ruby>` and
//!   luatexja-ruby's `\ruby` in the PDF. `"readings": "back"` in a note's custom data leaves them
//!   out of its cards' fronts. Typed answers are the base text, and are also accepted typed as the
//!   readings. Text to speech reads the readings of languages whose readings are how they sound
//!   (Japanese kana), and the base text otherwise.
//! - **The note's language** ([`language`]): `"language": "ja"` in its custom data, or else the
//!   one its script shows. It chooses the PDF's CJK font, and the dictionaries and deinflection
//!   of a lookup.
//! - **Dictionaries** (`dictionary`, not in the browser's build): Yomitan dictionaries each user
//!   imports, for looking words up from a card in the web app (`src/languageTools/`).
//!
//! Everything else is the Markdown parser's: clozes, links, audio, images, image occlusions, CLI
//! blocks and TTS settings. Everything the parser adds is in this directory, behind the
//! `language-tools` feature, and uses `Parseable`'s methods, so removing the directory and the
//! lines naming it removes it.
//!
//! # Rendering
//!
//! PDFs are rendered with lualatex, whose luatexja typesets CJK text, with a font of TeX Live's
//! for the note's language (`CJKmainfont`, and its bold in `CJKoptions`, which headings need).
//! `<config>/markdown-language-tools/pdf-defaults.yaml`, a pandoc defaults file, can override them
//! (`metadata: {CJKmainfont: …, CJKoptions: [BoldFont=…]}`) and anything else, and
//! `web-defaults.yaml` next to it does so for the HTML. The Markdown parser's defaults files are
//! not used.
//!
//! A file's notes are between `<!--- spares: start markdown-language-tools --->` and
//! `<!--- spares: end markdown-language-tools --->`, rather than the Markdown parser's
//! `spares: start` and `spares: end`, so that importing a file finds which of the two it is for.
//!
//! A card's file starts with a `spares: language` comment when its note sets one, since
//! the card's file doesn't have the note's custom data.

use std::ffi::OsString;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use language::Language;
use serde_json::Value;

use super::markdown;
use super::markdown::MarkdownParser;
use crate::Error;
use crate::LibraryError;
use crate::config::get_config_dir;
use crate::helpers::get_or_compile_regex;
use crate::model::CustomData;
use crate::parsers::ClozeMatch;
use crate::parsers::ClozeReplacement;
use crate::parsers::ConstructFileDataType;
use crate::parsers::ConstructImageOcclusionType;
use crate::parsers::GenerateNoteFilesRequest;
use crate::parsers::NoteImportAction;
use crate::parsers::Parseable;
use crate::parsers::RenderDependencies;
use crate::parsers::RenderedFile;
use crate::parsers::WebRenderMode;
use crate::parsers::audio::AudioRef;
use crate::parsers::generate_files::CardSide;
use crate::parsers::image_occlusion::ImageOcclusionData;

#[cfg(not(target_family = "wasm"))]
pub mod dictionary;
pub mod language;
pub mod readings;

pub const PARSER_NAME: &str = "markdown-language-tools";

/// The custom data key that leaves readings out of the fronts of cards, with the value `back`.
pub const READINGS_KEY: &str = "readings";

#[derive(Clone, Copy, Debug, Default)]
pub struct MarkdownLanguageToolsParser {
    markdown: MarkdownParser,
}

impl MarkdownLanguageToolsParser {
    pub fn new() -> Self {
        Self {
            markdown: MarkdownParser::new(),
        }
    }

    /// The readings of `text`, outside its code, math and comments.
    fn readings(self, text: &str) -> Vec<readings::Reading> {
        readings::find_readings(text, &self.markdown.unspoken_ranges(text))
    }

    /// `text` with each reading replaced by its base text.
    fn base_text(self, text: &str) -> String {
        readings::replace_readings(text, &self.readings(text), |base, _| base.to_string())
    }

    /// `text` with each reading replaced by the reading, or `None` if it has none.
    fn reading_text(self, text: &str) -> Option<String> {
        let readings = self.readings(text);
        (!readings.is_empty())
            .then(|| readings::replace_readings(text, &readings, |_, reading| reading.to_string()))
    }

    fn config_dir() -> PathBuf {
        get_config_dir().join(PARSER_NAME)
    }

    /// The defaults file `name` in the parser's config directory, if it exists.
    fn defaults_file(name: &str) -> Option<PathBuf> {
        Some(Self::config_dir().join(name)).filter(|path| path.is_file())
    }
}

/// The comments a file's notes are between, which name the parser, so that importing a file
/// without saying its parser tells this parser's notes from the Markdown parser's, whose syntax
/// they share.
const BLOCK_START: &str = "spares: start markdown-language-tools";
const BLOCK_END: &str = "spares: end markdown-language-tools";

/// Whether a note's cards show its readings only on their backs.
fn readings_on_back(custom_data: &CustomData) -> bool {
    custom_data.get(READINGS_KEY).and_then(Value::as_str) == Some("back")
}

/// The comment a card's file starts with to tell its render the note's language.
fn language_comment(parser: &dyn Parseable, code: &str) -> String {
    parser.construct_comment(&format!("spares: language {code}"))
}

/// The language of a note's or card's file `contents`: from a card's `spares: language`
/// comment, a note's `custom-data` setting, or else its script.
fn file_language(contents: &str) -> Option<&'static Language> {
    let comment = get_or_compile_regex(r"<!--- spares: language ([A-Za-z_-]+) --->").unwrap();
    if let Ok(Some(captures)) = comment.captures(contents) {
        return Language::from_code(&captures[1]);
    }
    let setting = get_or_compile_regex(r"<!--- # custom-data: (\{[^\n]*\}) --->").unwrap();
    if let Ok(Some(captures)) = setting.captures(contents)
        && let Ok(custom_data) = serde_json::from_str::<CustomData>(&captures[1])
    {
        return Language::of_note(&custom_data, contents);
    }
    Language::detect(contents)
}

/// pandoc's arguments for a file in `language`: its CJK font for the PDF, its language for the
/// HTML.
fn pandoc_args(contents: &str, pdf: bool) -> Vec<OsString> {
    let language = file_language(contents);
    let mut args: Vec<OsString> = Vec::new();
    if pdf {
        args.push("--pdf-engine=lualatex".into());
        if let Some(font) = language::cjk_font(language, contents) {
            args.push(format!("--metadata=CJKmainfont={}", font.regular).into());
            args.push(format!("--metadata=CJKoptions=BoldFont={}", font.bold).into());
        }
    } else if let Some(language) = language {
        // Browsers choose the CJK glyphs of the page's language
        args.push(format!("--metadata=lang={}", language.code).into());
    }
    args
}

impl Parseable for MarkdownLanguageToolsParser {
    fn get_parser_name(&self) -> &'static str {
        PARSER_NAME
    }

    fn get_linked_notes(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        self.markdown.get_linked_notes(data)
    }

    fn get_embedded_keywords(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        self.markdown.get_embedded_keywords(data)
    }

    fn get_settings(&self, data: &str) -> Result<Vec<Range<usize>>, LibraryError> {
        self.markdown.get_settings(data)
    }

    fn start_end_regex(&self) -> Arc<fancy_regex::Regex> {
        let regex_string = format!(
            "(?s)(?<=\\A|\n){}(.*?)\n{}?",
            fancy_regex::escape(&self.construct_comment(BLOCK_START)),
            fancy_regex::escape(&self.construct_comment(BLOCK_END))
        );
        get_or_compile_regex(&regex_string).unwrap()
    }

    fn get_clozes(&self, data: &str) -> Result<Vec<ClozeMatch>, LibraryError> {
        self.markdown.get_clozes(data)
    }

    fn construct_cloze(&self, cloze_settings_string: &str) -> (String, String) {
        self.markdown.construct_cloze(cloze_settings_string)
    }

    /// The base text, without its readings.
    fn typed_answer_text(&self, text: &str) -> String {
        self.markdown.typed_answer_text(&self.base_text(text))
    }

    /// The answer typed as its readings, `たべる` for `食[た]べる`.
    fn typed_answer_alternatives(&self, text: &str) -> Vec<String> {
        self.reading_text(text)
            .map(|reading| self.markdown.typed_answer_text(&reading))
            .into_iter()
            .collect()
    }

    /// The Markdown parser's, with each reading as raw HTML `<ruby>` and LaTeX `\ruby`.
    fn note_render_source(&self, contents: String) -> String {
        let source = self.markdown.note_render_source(contents);
        let readings = self.readings(&source);
        if readings.is_empty() {
            return source;
        }
        readings::replace_readings(&source, &readings, readings::ruby)
    }

    fn audio_refs(&self, text: &str) -> Vec<AudioRef> {
        self.markdown.audio_refs(text)
    }

    fn unspoken_ranges(&self, text: &str) -> Vec<Range<usize>> {
        self.markdown.unspoken_ranges(text)
    }

    /// The readings of a reading that sounds as it is written (kana, as Japanese has), and the
    /// base text of the others (pinyin).
    fn spoken_text(&self, text: &str) -> String {
        let readings = self.readings(text);
        let text = readings::replace_readings(text, &readings, |base, reading| {
            let speak_reading = Language::detect(reading).is_some_and(|l| l.speak_reading);
            if speak_reading { reading } else { base }.to_string()
        });
        self.markdown.spoken_text(&text)
    }

    fn construct_setting(&self, data: &str) -> String {
        self.markdown.construct_setting(data)
    }

    fn construct_comment(&self, data: &str) -> String {
        self.markdown.construct_comment(data)
    }

    fn extract_comment<'a>(&self, data: &'a str) -> &'a str {
        self.markdown.extract_comment(data)
    }

    fn construct_file_data(
        &self,
        output_type: ConstructFileDataType,
        request: &GenerateNoteFilesRequest,
        note_import_action: &NoteImportAction,
        include_separator: bool,
    ) -> String {
        let data = markdown::construct_file_data(
            self,
            output_type,
            request,
            note_import_action,
            include_separator,
        );
        let ConstructFileDataType::Card(_, _, side) = output_type else {
            return data;
        };
        let data = if side == CardSide::Front && readings_on_back(&request.custom_data) {
            self.base_text(&data)
        } else {
            data
        };
        match request
            .custom_data
            .get(language::LANGUAGE_KEY)
            .and_then(Value::as_str)
        {
            Some(code) => format!("{}{data}", language_comment(self, code)),
            None => data,
        }
    }

    /// [`Parseable::construct_full_file_data`]'s, between this parser's own [`BLOCK_START`] and
    /// [`BLOCK_END`].
    fn construct_full_file_data(
        &self,
        requests: &[(ConstructFileDataType, &GenerateNoteFilesRequest)],
        note_import_action: &NoteImportAction,
    ) -> String {
        let data = |(i, (data_type, request)): (
            usize,
            &(ConstructFileDataType, &GenerateNoteFilesRequest),
        )| {
            let separator =
                matches!(data_type, ConstructFileDataType::Note) && i + 1 != requests.len();
            self.construct_file_data(*data_type, request, note_import_action, separator)
        };
        let body = requests.iter().enumerate().map(data).collect::<String>();
        match requests.first().map(|(data_type, _)| data_type) {
            Some(ConstructFileDataType::Note) => format!(
                "{}\n{body}\n{}",
                self.construct_comment(BLOCK_START),
                self.construct_comment(BLOCK_END)
            ),
            _ => body,
        }
    }

    fn construct_cloze_replacement(
        &self,
        cloze_replacement: &ClozeReplacement,
        side: CardSide,
        id: Option<&str>,
    ) -> String {
        self.markdown
            .construct_cloze_replacement(cloze_replacement, side, id)
    }

    fn construct_image_occlusion(
        &self,
        image_occlusion_data: &ImageOcclusionData,
        output_type: ConstructImageOcclusionType<'_>,
    ) -> String {
        self.markdown
            .construct_image_occlusion(image_occlusion_data, output_type)
    }

    fn construct_image(&self, file_path: &Path) -> String {
        self.markdown.construct_image(file_path)
    }

    fn construct_audio(&self, file_path: &Path) -> Option<String> {
        self.markdown.construct_audio(file_path)
    }

    fn construct_linked_note(&self, keyword: &str) -> Option<String> {
        self.markdown.construct_linked_note(keyword)
    }

    fn file_extension(&self) -> &'static str {
        self.markdown.file_extension()
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
    ) -> Result<RenderedFile, Error> {
        let contents =
            std::fs::read_to_string(output_text_filepath).map_err(|source| Error::Io {
                description: format!("Failed to read {}", output_text_filepath.display()),
                source,
            })?;
        markdown::render_pdf(
            self,
            output_text_filepath,
            output_rendered_filepath,
            Self::defaults_file("pdf-defaults.yaml").as_deref(),
            &pandoc_args(&contents, true),
        )
    }

    fn has_web_render(&self) -> bool {
        true
    }

    fn render_web_file(
        &self,
        output_text_filepath: &Path,
        output_rendered_filepath: &Path,
        output_web_filepath: &Path,
        mode: WebRenderMode,
        dependencies: &mut RenderDependencies,
    ) -> Result<(), String> {
        let contents = std::fs::read_to_string(output_text_filepath)
            .map_err(|e| format!("Failed to read {}: {e}", output_text_filepath.display()))?;
        markdown::render_html(
            self,
            output_text_filepath,
            output_rendered_filepath,
            output_web_filepath,
            mode,
            dependencies,
            Self::defaults_file("web-defaults.yaml").as_deref(),
            &pandoc_args(&contents, false),
        )
    }
}

#[cfg(test)]
mod tests;
