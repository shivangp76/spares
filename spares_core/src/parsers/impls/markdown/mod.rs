//! The "markdown" parser, which renders notes with pandoc.
//!
//! # On the web
//!
//! Rendering a Markdown note also writes an HTML file next to its PDF, which the web app shows
//! instead of the PDF. This needs pandoc 3.12.1 or later, on your machine and on the server
//! (`setup_vps.sh` installs it). If the HTML would lose content that only the PDF can show, such
//! as raw LaTeX like a `TikZ` picture or math pandoc cannot convert to `MathML`, the web app shows the
//! PDF instead, and rendering logs why (with `RUST_LOG=warn`).
//!
//! - Set `"render": "pdf"` in a note's custom data to always show its PDF, or `"render": "html"`
//!   to show its HTML even if content is lost.
//! - Raw LaTeX meant only for the PDF, written as a ```` ```{=latex} ```` block, is left out of the
//!   HTML without falling back to the PDF.
//! - Style the HTML with a pandoc defaults file at `markdown/web-defaults.yaml` in the config
//!   directory, e.g. with `css: [~/.config/spares/markdown/web.css]`. The PDF's `defaults_file` is
//!   not used for HTML. Math macros defined in its `header-includes` are unknown to the HTML, so
//!   notes using them show their PDF; define macros in the note itself to have them apply to both.
//! - Notes rendered before upgrading show their PDF until they are rendered again, e.g. with
//!   `spares note generate --render --force-render`.

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
use crate::config::get_config_dir;
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
use crate::parsers::WebRenderMode;
use crate::parsers::audio::AudioRef;
use crate::parsers::audio::find_audio_refs;
use crate::parsers::construct_card_data;
use crate::parsers::generate_files::CardSide;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::ImageOcclusionData;
use crate::parsers::image_occlusion::construct_image_occlusion_from_image;
use crate::parsers::render_command_output;
use crate::parsers::render_dependencies_filepath;
use crate::schema::note::LinkedNote;

mod cloze_parser;

/// Between a card's header and body. Explicit raw blocks, since `\hrulefill` cannot be converted
/// to `MathML`, so as math it would make every card's HTML lose content. HTML drops the LaTeX block
/// without counting it as lost (see [`is_content_loss`]).
const CARD_SEPARATOR: &str = "```{=latex}\n\\hrulefill\n```\n\n```{=html}\n<hr>\n```";

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
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

    fn typed_answer_text(&self, text: &str) -> String {
        // Links and images (`[text](url)`, `![alt](path)`) and spans (`[text]{.class}`) keep their
        // text. Then emphasis and code markers are removed.
        let links_regex = get_or_compile_regex(r"!?\[([^\]]*)\](?:\([^)]*\)|\{[^}]*\})").unwrap();
        let text = links_regex.replace_all(text, "$1");
        text.replace(['*', '_', '`'], "")
    }

    fn audio_refs(&self, text: &str) -> Vec<AudioRef> {
        // `[text](path)` or `[text](<path>)`, with an optional title, but not an image
        find_audio_refs(
            text,
            r#"(?<!!)\[[^\]]*\]\((?:<([^>\n]+)>|([^)\s]+))(?:\s+"[^"]*")?\)"#,
            r"(?s)<!--.*?-->|```.*?```|`[^`\n]*`|\$\$.*?\$\$|\$[^$\n]*\$",
        )
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
                    // The blank line lets pandoc read the link definitions, which cannot interrupt
                    // a paragraph.
                    linked_notes_string = format!("\nLinked Notes:\n\n{}", linked_notes_string);
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
                    note_id.letters()
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
                let mut lines = vec![format!(
                    "- note-id{} {}",
                    settings_key_value_delim,
                    note_id.letters()
                )];
                if !keywords_str.is_empty() {
                    lines.push(format!(
                        "- keywords{} {}",
                        settings_key_value_delim, keywords_str
                    ));
                }
                lines.extend(vec![
                    format!("- tags{} {}", settings_key_value_delim, tags_str),
                    String::new(),
                    CARD_SEPARATOR.to_string(),
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

    fn construct_image(&self, file_path: &Path) -> String {
        format!("![]({})", file_path.display())
    }

    fn construct_audio(&self, file_path: &Path) -> Option<String> {
        // Words, not an emoji, since pdflatex cannot typeset one
        Some(format!("[listen]({})", file_path.display()))
    }

    fn construct_linked_note(&self, keyword: &str) -> Option<String> {
        Some(format!("[{keyword}][li]"))
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

    fn has_web_render(&self) -> bool {
        true
    }

    fn render_web_file(
        &self,
        output_text_filepath: &Path,
        output_rendered_filepath: &Path,
        output_web_filepath: &Path,
        mode: WebRenderMode,
    ) -> Result<(), String> {
        let dependencies_filepath = render_dependencies_filepath(output_rendered_filepath);
        let mut log_filepath = dependencies_filepath.clone();
        log_filepath.set_extension("pandoc-web-log.json");
        let web_defaults_file = Some(web_defaults_filepath()).filter(|path| path.is_file());
        let page_title = output_rendered_filepath
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut command = Command::new("pandoc");
        command
            .arg(output_text_filepath)
            .arg("-o")
            .arg(output_web_filepath)
            .args(["-t", "html5", "--standalone", "--embed-resources"])
            .arg("--math-method=mathml")
            // Web clients style it like the app. The web defaults file can turn these back on.
            .arg("--metadata=document-css=false")
            .arg("--metadata")
            .arg(format!("pagetitle={page_title}"))
            .arg("--log")
            .arg(&log_filepath);
        // The PDF defaults file is never passed, since its options may not apply to HTML.
        if let Some(web_defaults_file) = &web_defaults_file {
            command.arg("--defaults").arg(web_defaults_file);
        }
        let result = render_web_command(
            &mut command,
            &log_filepath,
            web_defaults_file.as_deref(),
            &dependencies_filepath,
            mode,
        );
        let _ = remove_file(&log_filepath);
        result
    }
}

/// Optional pandoc defaults file for the HTML for web clients.
fn web_defaults_filepath() -> PathBuf {
    get_config_dir().join("markdown").join("web-defaults.yaml")
}

fn render_web_command(
    command: &mut Command,
    log_filepath: &Path,
    web_defaults_file: Option<&Path>,
    dependencies_filepath: &Path,
    mode: WebRenderMode,
) -> Result<(), String> {
    let output = render_command_output(command, "pandoc").map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "pandoc exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    let log = read_to_string(log_filepath)
        .map_err(|e| format!("Failed to read {}: {e}", log_filepath.display()))?;
    let messages: Vec<PandocLogMessage> =
        serde_json::from_str(&log).map_err(|e| format!("Failed to parse pandoc's log: {e}"))?;

    // The web defaults file and what it loads, such as stylesheets, are synced to the server like
    // the PDF's.
    let mut files = loaded_resources_of(&messages);
    files.extend(web_defaults_file.map(Path::to_path_buf));
    if let Err(e) = RenderDependencies::extend_file(dependencies_filepath, files) {
        warn!(
            "Failed to record dependencies in {}: {e}",
            dependencies_filepath.display()
        );
    }

    match mode {
        WebRenderMode::Auto => match messages.iter().find_map(content_loss) {
            Some(reason) => Err(reason),
            None => Ok(()),
        },
        WebRenderMode::Html | WebRenderMode::Pdf => Ok(()),
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum PandocLogMessage {
    LoadedResource {
        from: String,
    },
    /// `contents` is the pandoc AST element, e.g. `{"t":"RawBlock","c":["tex","..."]}`.
    BlockNotRendered {
        contents: serde_json::Value,
    },
    InlineNotRendered {
        contents: serde_json::Value,
    },
    CouldNotConvertTeXMath {
        contents: String,
    },
    #[serde(other)]
    Other,
}

/// Why the HTML lost content according to a log message of its render, if it did.
fn content_loss(message: &PandocLogMessage) -> Option<String> {
    match message {
        PandocLogMessage::BlockNotRendered { contents }
        | PandocLogMessage::InlineNotRendered { contents } => {
            is_content_loss(contents).then(|| match raw_contents(contents) {
                Some((_, text)) => format!("raw TeX: {}", shorten(text)),
                None => format!("not rendered: {}", shorten(&contents.to_string())),
            })
        }
        PandocLogMessage::CouldNotConvertTeXMath { contents } => {
            Some(format!("math: {}", shorten(contents)))
        }
        PandocLogMessage::LoadedResource { .. } | PandocLogMessage::Other => None,
    }
}

/// Whether an element the HTML writer did not render lost content. Explicit raw elements for
/// another format (e.g. ```` ```{=latex} ````) are only meant for that format. Implicit raw TeX
/// that only defines macros was already applied by pandoc.
fn is_content_loss(contents: &serde_json::Value) -> bool {
    match raw_contents(contents) {
        Some(("tex", text)) => !only_defines_macros(text),
        Some(_) => false,
        None => true,
    }
}

/// The format and text of a raw block or inline.
fn raw_contents(contents: &serde_json::Value) -> Option<(&str, &str)> {
    if !matches!(contents.get("t")?.as_str()?, "RawBlock" | "RawInline") {
        return None;
    }
    match contents.get("c")?.as_array()?.as_slice() {
        [format, text] => Some((format.as_str()?, text.as_str()?)),
        _ => None,
    }
}

/// Whether `tex` consists only of macro definitions, such as `\newcommand{\R}{\mathbb{R}}`.
/// Conservative: anything it does not recognize is not a definition.
fn only_defines_macros(tex: &str) -> bool {
    const DEFINITIONS: [&str; 6] = [
        "newcommand",
        "renewcommand",
        "providecommand",
        "def",
        "gdef",
        "DeclareMathOperator",
    ];
    let mut chars = tex.chars().peekable();
    let mut depth = 0_usize;
    let mut found_definition = false;
    // Whether the next control sequence at the top level is the name being defined
    let mut expects_name = false;
    while let Some(c) = chars.next() {
        match c {
            '%' => {
                // Comment until the end of the line
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '{' | '[' => {
                if depth == 0 {
                    expects_name = false;
                }
                depth += 1;
            }
            '}' | ']' => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return false,
            },
            '\\' => {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if !c.is_ascii_alphabetic() {
                        break;
                    }
                    name.push(c);
                    chars.next();
                }
                if name.is_empty() {
                    // A control symbol, such as `\{`
                    chars.next();
                }
                if depth > 0 {
                    continue;
                }
                if DEFINITIONS.contains(&name.as_str()) {
                    found_definition = true;
                    expects_name = true;
                } else if expects_name {
                    expects_name = false;
                } else {
                    return false;
                }
            }
            _ if depth > 0 => {}
            c if c.is_whitespace() || c == '*' || c == '#' || c.is_ascii_digit() => {}
            _ => return false,
        }
    }
    found_definition && depth == 0
}

fn shorten(text: &str) -> String {
    const MAX_CHARS: usize = 60;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= MAX_CHARS {
        text
    } else {
        let shortened = text.chars().take(MAX_CHARS).collect::<String>();
        format!("{shortened}…")
    }
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
    Ok(loaded_resources_of(&messages))
}

fn loaded_resources_of(messages: &[PandocLogMessage]) -> Vec<PathBuf> {
    messages
        .iter()
        .filter_map(|message| match message {
            PandocLogMessage::LoadedResource { from } => Some(PathBuf::from(from)),
            _ => None,
        })
        // Skips URLs.
        .filter(|path| path.is_absolute())
        .collect()
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
                linked_note_id: Some("aaa".parse().unwrap()),
                matched_keyword: Some("keyword 1".to_string()),
            },
            LinkedNote {
                searched_keyword: "keyword 1.5".to_string(),
                linked_note_id: Some("aaa".parse().unwrap()),
                matched_keyword: Some("keyword 1".to_string()),
            },
            LinkedNote {
                searched_keyword: "keyword 2".to_string(),
                linked_note_id: Some("aab".parse().unwrap()),
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
        let expected_linked_notes_string = "[li1]: /tmp/spares/data/notes/markdown/aaa.md \"keyword 1 -> keyword 1\"\n[li2]: /tmp/spares/data/notes/markdown/aaa.md \"keyword 1.5 -> keyword 1\"\n[li3]: /tmp/spares/data/notes/markdown/aab.md \"keyword 2 -> keyword 2\"";
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

    mod web_render {
        use std::fs::read_to_string;
        use std::fs::write;
        use std::path::Path;
        use std::path::PathBuf;

        use super::super::CARD_SEPARATOR;
        use super::super::MarkdownParser;
        use super::super::PandocLogMessage;
        use super::super::content_loss;
        use super::super::only_defines_macros;
        use crate::parsers::Parseable;
        use crate::parsers::RenderDependencies;
        use crate::parsers::WebRenderMode;
        use crate::parsers::render_dependencies_filepath;

        /// A 1x1 PNG.
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99,
            0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        fn test_dir(name: &str) -> PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("spares-web-render-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// Renders `markdown` to HTML in a new directory, returning the result and the HTML path.
        fn render(
            name: &str,
            markdown: &str,
            mode: WebRenderMode,
        ) -> (Result<(), String>, PathBuf) {
            let dir = test_dir(name);
            let text = dir.join("0001.md");
            write(&text, markdown).unwrap();
            let html = dir.join("0001.html");
            let result =
                MarkdownParser::new().render_web_file(&text, &dir.join("0001.pdf"), &html, mode);
            (result, html)
        }

        fn loss(markdown: &str) -> Option<String> {
            render("loss", markdown, WebRenderMode::Auto).0.err()
        }

        #[test]
        fn test_only_defines_macros() {
            for tex in [
                r"\newcommand{\R}{\mathbb{R}}",
                r"\newcommand{\norm}[1]{\left\lVert#1\right\rVert}",
                "\\renewcommand*{\\a}{b} % comment\n\\def\\c#1{d#1}",
                r"\DeclareMathOperator{\tr}{tr}",
                r"\def\e{\{}",
            ] {
                assert!(only_defines_macros(tex), "{tex}");
            }
            for tex in [
                r"\begin{tikzpicture}\draw (0,0);\end{tikzpicture}",
                r"\newcommand{\R}{\mathbb{R}} \hrulefill",
                r"\newcommand{\R}{\mathbb{R}} text",
                r"\textcolor{red}{x}",
                "",
                r"\newcommand{\R}{",
            ] {
                assert!(!only_defines_macros(tex), "{tex}");
            }
        }

        #[test]
        fn test_content_loss_from_log() {
            let log = r#"[
                {"type": "BlockNotRendered", "verbosity": "INFO", "contents": {"t": "RawBlock", "c": ["tex", "\\newcommand{\\R}{x}"]}},
                {"type": "BlockNotRendered", "verbosity": "INFO", "contents": {"t": "RawBlock", "c": ["latex", "\\hrulefill"]}},
                {"type": "NoLangSpecified", "verbosity": "INFO"},
                {"type": "LoadedResource", "verbosity": "INFO", "for": "a.png", "from": "/a.png"},
                {"type": "BlockNotRendered", "verbosity": "INFO", "contents": {"t": "RawBlock", "c": ["tex", "\\begin{tikzpicture}\\end{tikzpicture}"]}},
                {"type": "InlineNotRendered", "verbosity": "INFO", "contents": {"t": "RawInline", "c": ["tex", "\\textcolor{red}{hi}"]}},
                {"type": "CouldNotConvertTeXMath", "verbosity": "WARNING", "contents": "\\foo{x}", "message": "..."}
            ]"#;
            let messages: Vec<PandocLogMessage> = serde_json::from_str(log).unwrap();
            let reasons = messages.iter().filter_map(content_loss).collect::<Vec<_>>();
            assert_eq!(
                reasons,
                [
                    r"raw TeX: \begin{tikzpicture}\end{tikzpicture}",
                    r"raw TeX: \textcolor{red}{hi}",
                    r"math: \foo{x}",
                ]
            );
        }

        #[test]
        fn test_render_web_file_plain_note() {
            let dir = test_dir("plain");
            let image = dir.join("a.png");
            write(&image, PNG).unwrap();
            let text = dir.join("0001.md");
            write(
                &text,
                format!(
                    "# Title\n\n- item with $x^2$\n\n![a]({})\n",
                    image.display()
                ),
            )
            .unwrap();
            let rendered = dir.join("0001.pdf");
            let html = dir.join("0001.html");
            MarkdownParser::new()
                .render_web_file(&text, &rendered, &html, WebRenderMode::Auto)
                .unwrap();
            let html = read_to_string(html).unwrap();
            assert!(!html.contains("<script"));
            assert!(html.contains("data:image/png"));
            assert!(html.contains("<math"));
            // The image is recorded so sync copies it to the server
            let dependencies: RenderDependencies = serde_json::from_str(
                &read_to_string(render_dependencies_filepath(&rendered)).unwrap(),
            )
            .unwrap();
            assert!(dependencies.files.contains(&image));
        }

        #[test]
        fn test_render_web_file_card_separator() {
            let (result, html) = render(
                "separator",
                &format!("- note-id: 1\n\n{CARD_SEPARATOR}\n\nFront"),
                WebRenderMode::Auto,
            );
            result.unwrap();
            assert!(read_to_string(html).unwrap().contains("<hr"));
        }

        #[test]
        fn test_render_web_file_content_loss() {
            let tikz = "\\begin{tikzpicture}\n\\draw (0,0) -- (1,1);\n\\end{tikzpicture}\n";
            assert!(loss(tikz).unwrap().starts_with("raw TeX"));
            assert!(loss("Some \\textcolor{red}{text}.").is_some());
            assert!(loss("$\\foo{x}$").unwrap().starts_with("math"));
            assert_eq!(loss("\\newcommand{\\R}{\\mathbb{R}}\n\n$\\R$"), None);
            assert_eq!(loss("```{=latex}\n\\vspace{1em}\n```\n\nText"), None);
            assert!(loss("\\vspace{1em}\n\nText").is_some());
            // `html` keeps the HTML anyway
            let (result, _) = render("html-mode", tikz, WebRenderMode::Html);
            result.unwrap();
        }

        #[test]
        fn test_render_web_file_missing_image() {
            let (result, _) = render(
                "missing-image",
                "![a](/nonexistent/spares/a.png)",
                WebRenderMode::Html,
            );
            assert!(result.unwrap_err().starts_with("pandoc exited"));
        }

        #[test]
        fn test_extend_dependencies_file() {
            let dir = test_dir("dependencies");
            let path = dir.join("0001.deps.json");
            RenderDependencies {
                files: vec![PathBuf::from("/b")],
                packages: Vec::new(),
            }
            .write(&path)
            .unwrap();
            RenderDependencies::extend_file(&path, vec![PathBuf::from("/a"), PathBuf::from("/b")])
                .unwrap();
            let dependencies: RenderDependencies =
                serde_json::from_str(&read_to_string(&path).unwrap()).unwrap();
            assert_eq!(dependencies.files, [Path::new("/a"), Path::new("/b")]);
            // A missing file is created
            let new_path = dir.join("0002.deps.json");
            RenderDependencies::extend_file(&new_path, vec![PathBuf::from("/c")]).unwrap();
            assert!(new_path.exists());
        }
    }
}
