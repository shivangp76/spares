use super::generate_files::CardSide;
use super::get_cards_main;
use crate::parsers::image_occlusion::{ConstructImageOcclusionType, ImageOcclusionData};
use crate::parsers::{
    CardData, NoteImportAction, NoteSettings, Parseable, SrsAdapter, parse_note_settings,
    validate_cards,
};
use crate::{CardErrorKind, Error, LibraryError, NoteErrorKind};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct NoteRawData {
    pub metadata: Option<Range<usize>>,
    /// Note start and end index
    pub data: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClozeHiddenReplacement {
    ToAnswer { hint: Option<String> },
    NotToAnswer,
}

impl Default for ClozeHiddenReplacement {
    fn default() -> Self {
        Self::ToAnswer { hint: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClozeReplacement<'a> {
    Hidden(&'a ClozeHiddenReplacement),
    Reveal(String),
}

impl<'a> ClozeReplacement<'a> {
    pub fn parse(
        side: CardSide,
        cloze_replacement: &'a ClozeHiddenReplacement,
        data: &str,
    ) -> Self {
        match side {
            CardSide::Front => Self::Hidden(cloze_replacement),
            CardSide::Back => match cloze_replacement {
                ClozeHiddenReplacement::ToAnswer { hint: _ } => Self::Reveal(data.to_string()),
                ClozeHiddenReplacement::NotToAnswer => Self::Hidden(cloze_replacement),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NotePart {
    SurroundingData(String),
    ClozeData(String, ClozeHiddenReplacement),
    ClozeStart(String),
    ClozeEnd(String),
    // This is not moved inside of `ClozeData` since the image contains both `SurroundingData` and `ClozeData`.
    ImageOcclusion {
        /// Allows the card file to be generated.
        ///
        /// 0 indexed
        // NOTE: Clozes that are not a part of the grouping are not included here, even if
        // `FrontConceal::AllGroupings` or `BackReveal::OnlyAnswered`.
        cloze_indices: Vec<(usize, ClozeHiddenReplacement)>,
        data: Arc<ImageOcclusionData>,
    },
}

// This doesn't process cards, but it does validate them. `get_cards()` needs to be called on each element of the returned vector.
pub fn get_notes(
    parser: &dyn Parseable,
    to_parser_opt: Option<&dyn Parseable>,
    data: &str,
    adapter: &dyn SrsAdapter,
    move_files: bool,
) -> Result<Vec<(NoteSettings, Option<String>)>, Error> {
    // Remove comments
    // let data = if let Some(comment_regex) = parser.comment_regex() {
    //     comment_regex.replace_all(data, "").to_string()
    // } else {
    //     data.to_string()
    // };
    let data = data.to_string();

    let notes_raw_data = parser.get_notes_data(data.as_str())?;
    let mut settings_iter = parser.get_settings(data.as_str())?.into_iter().peekable();

    // Interweave settings and notes
    let mut notes: Vec<(NoteSettings, Option<String>)> = Vec::new();
    let mut global_settings: NoteSettings = NoteSettings::default();
    for note_c in notes_raw_data {
        // Parse local settings
        let mut local_settings = global_settings.clone();
        // Remove global warnings from local warnings
        local_settings.errors_and_warnings.clear();
        let mut note_settings_capture_ranges = Vec::new();
        while let Some(setting_match) = settings_iter.peek() {
            if setting_match.capture_range.end >= note_c.data.start {
                break;
            }
            note_settings_capture_ranges.push(setting_match.capture_range.clone());
            settings_iter.next();
        }
        parse_note_settings(
            parser,
            &data,
            &note_settings_capture_ranges,
            &mut global_settings,
            &mut local_settings,
            adapter,
            &note_c.data,
        );

        // Complete note
        let note_data = complete_note(
            parser,
            to_parser_opt,
            &data,
            &note_c,
            &mut local_settings,
            move_files,
        )
        .map_err(|e| {
            local_settings.errors_and_warnings.push(e);
        })
        .ok();
        notes.push((local_settings, note_data));
    }
    Ok(notes)
}

fn complete_note(
    parser: &dyn Parseable,
    to_parser_opt: Option<&dyn Parseable>,
    full_data: &str,
    note_c: &NoteRawData,
    local_settings: &mut NoteSettings,
    move_files: bool,
) -> Result<String, LibraryError> {
    // if note_c.metadata.is_some() && local_settings.keywords.is_empty() {
    //     local_settings
    //         .warnings
    //         .push("Note metadata was provided, but no keyword.".to_string());
    // }
    let data = &full_data[note_c.data.clone()];
    if data.contains("TODO") {
        local_settings.errors_and_warnings.push(LibraryError::Note(
            NoteErrorKind::SettingsWarning {
                description: "The field `data` contains TODO.".to_string(),
                src: full_data.to_string(),
                at: note_c.data.clone().into(),
            },
        ));
    }

    // Strip whitespace
    let note_data = data.trim().to_string();

    // Convert note to different parser, if requested, _and_
    // Parse cards so they can be validated
    // Add the ordering
    // This should be done at the card level, not cloze level, so `self.get_cards()` needs to be called, so cards can be identified. This requires `self.get_cards()` to also return the text surrounding the cloze, so the full note can be reconstructed.
    let add_order = local_settings.action == NoteImportAction::Add;
    let cards: Vec<CardData> = get_cards_main(
        parser,
        to_parser_opt,
        note_data.as_str(),
        add_order,
        move_files,
        (local_settings.front_conceal, local_settings.back_reveal),
    )?;
    validate_cards(&cards)?;
    local_settings.cards_count = Some(cards.len());
    // let card = cards
    //     .first()
    //     .ok_or(LibraryError::Card(CardErrorKind::NotFound {
    //         src: note_data,
    //     }))?;
    let output_parser = to_parser_opt.unwrap_or(parser);
    let new_data = if let Some(first_card) = cards.first() {
        first_card
            .data
            .clone()
            .into_iter()
            .map(|p| match p {
                NotePart::ClozeStart(text)
                | NotePart::ClozeEnd(text)
                | NotePart::SurroundingData(text)
                | NotePart::ClozeData(text, _) => text,
                NotePart::ImageOcclusion { data, .. } => output_parser
                    .construct_image_occlusion(&data, ConstructImageOcclusionType::Note),
            })
            .collect::<String>()
    } else {
        local_settings
            .errors_and_warnings
            .push(LibraryError::Card(CardErrorKind::NotFound {
                src: note_data.clone(),
            }));
        note_data
    };

    // Get linked notes
    let linked_notes = output_parser.get_linked_notes(new_data.as_str());
    match linked_notes {
        Ok(ln) => local_settings
            .linked_notes
            .extend(ln.into_iter().map(|range| new_data[range].to_string())),
        Err(e) => local_settings.errors_and_warnings.push(e),
    }
    Ok(new_data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::impls::markdown::MarkdownParser;
    use crate::parsers::impls::typst::TypstParser;
    use crate::{
        adapters::get_adapter_from_string,
        parsers::impls::latex::{LatexParserExerciseSolution, LatexParserNote},
    };
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_get_notes_basic_1() {
        let parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! { r"\begin{note}
            a
            \begin{cl}
            b
            \end{cl}
            \end{note}

            "};
        let notes_res = get_notes(parser.as_ref(), None, data, adapter.as_ref(), false);
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert!(note.1.is_some());
            if let Some(ref note_data) = note.1 {
                let expected_note_data = indoc! {r"a
                    \begin{cl}[o:1]
                    b
                    \end{cl}"};
                assert_eq!(*note_data, expected_note_data);
            }
        }
    }

    #[test]
    fn test_get_notes_basic_2() {
        let parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! {r"\begin{note}
            a
            \begin{cl}
            b
            \end{cl}
            c
            \end{note}"};
        let notes_res = get_notes(parser.as_ref(), None, data, adapter.as_ref(), false);
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert!(note.1.is_some());
            if let Some(ref note_data) = note.1 {
                let expected_note_data = indoc! {r"a
                    \begin{cl}[o:1]
                    b
                    \end{cl}
                    c"};
                assert_eq!(*note_data, expected_note_data);
            }
        }
    }

    #[test]
    fn test_get_notes_convert_parser_1() {
        let from_parser: Box<dyn Parseable> = Box::new(LatexParserExerciseSolution::new());
        let to_parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! {r"\begin{exercise}
            a
            \end{exercise}
            \begin{solution}
            b
            \end{solution}"};
        let notes_res = get_notes(
            from_parser.as_ref(),
            Some(to_parser.as_ref()),
            data,
            adapter.as_ref(),
            false,
        );
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert!(note.1.is_some());
            if let Some(ref note_data) = note.1 {
                let expected_note_data = indoc! {r"a
                    \begin{cl}[o:1]
                    b
                    \end{cl}"};
                assert_eq!(*note_data, expected_note_data);
            }
        }
    }

    #[test]
    fn test_get_notes_convert_parser_2() {
        let from_parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let to_parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! {r"<!--- spares: note start --->
            a{{b{{c}}d}}e
            <!--- spares: note end --->"};
        let notes_res = get_notes(
            from_parser.as_ref(),
            Some(to_parser.as_ref()),
            data,
            adapter.as_ref(),
            false,
        );
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert!(note.1.is_some());
            if let Some(ref note_data) = note.1 {
                let expected_note_data =
                    indoc! {r"a\begin{cl}[o:1]b\begin{cl}[o:2]c\end{cl}d\end{cl}e"};
                assert_eq!(*note_data, expected_note_data);
            }
        }
    }

    #[test]
    fn test_get_notes_convert_parser_3() {
        let from_parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let to_parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! {
            r"\begin{note}
             Test \begin{cl}[g:1;o:1] A \end{cl} and \begin{cl}[g:1] Be \end{cl} because \begin{cl}[o:2] C \end{cl}.
             \end{note}"
        };
        let notes_res = get_notes(
            from_parser.as_ref(),
            Some(to_parser.as_ref()),
            data,
            adapter.as_ref(),
            false,
        );
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert!(note.1.is_some());
            if let Some(ref note_data) = note.1 {
                let expected_note_data = indoc! {
                    r"Test #cl[ A ][g:1;o:1] and #cl[ Be ][g:1] because #cl[ C ][o:2]."
                };
                assert_eq!(*note_data, expected_note_data);
            }
        }
    }

    #[test]
    fn test_get_notes_convert_parser_4() {
        let from_parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let to_parser: Box<dyn Parseable> = Box::new(TypstParser::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let data = indoc! {
            r"\begin{note}
            A \begin{cl}[o:1] B \begin{cl}[o:2]C\end{cl} \end{cl}
            \end{note}"
        };
        let notes_res = get_notes(
            from_parser.as_ref(),
            Some(to_parser.as_ref()),
            data,
            adapter.as_ref(),
            false,
        );
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert!(note.1.is_some());
            if let Some(ref note_data) = note.1 {
                let expected_note_data = indoc! {
                    r"A #cl[ B #cl[C][o:2] ][o:1]"
                };
                assert_eq!(*note_data, expected_note_data);
            }
        }
    }

    #[test]
    #[cfg(feature = "testing")]
    fn test_get_notes_convert_parser_advanced() {
        // This tests:
        // - Image occlusion
        // - Multiple image occlusions to make sure the offset is correct and the surrounding data is properly parser
        // - Reimporting image occlusions (image occlusions that contain the rendered image so the user can preview it)

        use crate::parsers::{
            BackReveal, FrontConceal,
            image_occlusion::get_image_occlusion_directory,
            impls::{latex::LatexParserNote, markdown::MarkdownParser},
        };
        let from_parser: Box<dyn Parseable> = Box::new(MarkdownParser::new());
        let to_parser: Box<dyn Parseable> = Box::new(LatexParserNote::new());
        let adapter = get_adapter_from_string("spares").unwrap();
        let seed = "convert-parser";
        // - Image 1: 2 clozes, each with default settings
        // - Image 2: 2 clozes, grouped together
        let image_1_file_stem = format!("test-{}-1", seed);
        let image_2_file_stem = format!("test-{}-2", seed);

        // let temp_dir = std::env::temp_dir();
        let temp_dir = get_image_occlusion_directory();
        let mut original_image_filepath_1 = temp_dir.clone();
        original_image_filepath_1.push(format!("{}.svg", image_1_file_stem));
        let text = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 124 124" fill="none"><rect width="124" height="124" rx="24" fill="#F97316"/></svg>"##;
        std::fs::write(&original_image_filepath_1, text).unwrap();
        // class="layer" is for svgedit
        let clozes_filedata_1 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="350">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="#FFEBA2" height="75" width="123.21429" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" />
            <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" />
          </g>
        </svg>"## };
        let mut clozes_filepath_1 = temp_dir.clone();
        clozes_filepath_1.push(format!("{}_clozes.svg", image_1_file_stem));
        std::fs::write(&clozes_filepath_1, clozes_filedata_1).unwrap();

        let mut original_image_filepath_2 = temp_dir.clone();
        original_image_filepath_2.push(format!("{}.svg", image_2_file_stem));
        let text = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 124 124" fill="none"><rect width="124" height="124" rx="24" fill="#F97316"/></svg>"##;
        std::fs::write(&original_image_filepath_2, text).unwrap();
        let clozes_filedata_2 = indoc! { r##"<?xml version="1.0" encoding="UTF-8"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="350">
          <g class="layer" id="markup-group">
            <title>Markup</title>
          </g>
          <g class="layer" id="clozes-group">
            <title>Clozes</title>
            <rect fill="#FFEBA2" height="75" width="123.21429" data-cloze-settings="g:1" stroke="#2D2D2D" y="65.17857" id="svg_1" x="53.67857" />
            <ellipse fill="#FFEBA2" stroke="#2D2D2D" stroke-dasharray="null" stroke-linejoin="null" stroke-linecap="null" cx="346.52633" cy="78.94737" id="svg_2" rx="46.31579" ry="46.31579" data-cloze-settings="g:1;hide:" />
          </g>
        </svg>"## };
        let mut clozes_filepath_2 = temp_dir.clone();
        clozes_filepath_2.push(format!("{}_clozes.svg", image_2_file_stem));
        std::fs::write(&clozes_filepath_2, clozes_filedata_2).unwrap();

        let note_data_body = format!(
            indoc! { "a
        <!--- spares: image occlusion start --->
        <!--- original_image_filepath = \"{}\" --->
        <!--- clozes_filepath = \"{}\" --->
        <!--- front_conceal = \"{:?}\" --->
        <!--- back_reveal = \"{:?}\" --->
        <!--- spares: image occlusion end --->
        b
        <!--- spares: image occlusion start --->
        <!--- original_image_filepath = \"{}\" --->
        <!--- clozes_filepath = \"{}\" --->
        <!--- front_conceal = \"{:?}\" --->
        <!--- back_reveal = \"{:?}\" --->
        [Image Occlusion](/some/random/image/path)
        <!--- spares: image occlusion end --->
        c" },
            original_image_filepath_1.display(),
            clozes_filepath_1.display(),
            FrontConceal::AllGroupings,
            BackReveal::OnlyAnswered,
            original_image_filepath_2.display(),
            clozes_filepath_2.display(),
            FrontConceal::AllGroupings,
            BackReveal::OnlyAnswered,
        );
        let note_data = format!(
            indoc! {"
            <!--- spares: note start --->
            {}
            <!--- spares: note end --->"
            },
            note_data_body
        );
        let notes_res = get_notes(
            from_parser.as_ref(),
            Some(to_parser.as_ref()),
            note_data.as_str(),
            adapter.as_ref(),
            false,
        );
        assert!(notes_res.is_ok());
        if let Ok(notes) = notes_res {
            assert_eq!(notes.len(), 1);
            let note = notes.first().unwrap();
            assert!(note.1.is_some());
            if let Some(ref note_data) = note.1 {
                let expected_note_data = indoc! {r#"
                a
                % spares: image occlusion start
                % original_image_filepath = "/tmp/spares/data/image_occlusions/test-convert-parser-1.svg"
                % clozes_filepath = "/tmp/spares/data/image_occlusions/test-convert-parser-1_clozes.svg"
                % front_conceal = "AllGroupings"
                % back_reveal = "OnlyAnswered"
                \begin{figure}
                  \includesvg[width=0.5\textwidth,height=0.8\textheight,keepaspectratio]/tmp/spares/data/image_occlusions/test-convert-parser-1.svg
                \end{figure}
                % spares: image occlusion end
                b
                % spares: image occlusion start
                % original_image_filepath = "/tmp/spares/data/image_occlusions/test-convert-parser-2.svg"
                % clozes_filepath = "/tmp/spares/data/image_occlusions/test-convert-parser-2_clozes.svg"
                % front_conceal = "AllGroupings"
                % back_reveal = "OnlyAnswered"
                \begin{figure}
                  \includesvg[width=0.5\textwidth,height=0.8\textheight,keepaspectratio]/tmp/spares/data/image_occlusions/test-convert-parser-2.svg
                \end{figure}
                % spares: image occlusion end
                c"#};
                assert_eq!(*note_data, expected_note_data);
            }
        }
    }
}
