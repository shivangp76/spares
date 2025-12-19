use crate::{
    Error, LibraryError, NoteErrorKind,
    api::note::{get_render_note_data, render_note_data_to_generate_files_request},
    parsers::{ConstructFileDataType, NoteImportAction, Parseable, TemplateType, find_parser},
    schema::note::ExportNotesRequest,
    search::evaluator::Evaluator,
};
use itertools::Itertools;
use sqlx::sqlite::SqlitePool;
use std::collections::HashMap;

pub async fn export_notes(
    db: &SqlitePool,
    request: ExportNotesRequest,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<String, Error> {
    let evaluator = Evaluator::new(&request.query);
    let note_ids = evaluator.get_note_ids(db).await?;
    if note_ids.is_empty() {
        return Ok(String::new());
    }

    let notes_data = get_render_note_data(db, Some(note_ids)).await?;

    // Group notes by parser
    let grouped_parse_note_requests = notes_data
        .iter()
        .map(|render_note_data| {
            (
                &render_note_data.parser_name,
                render_note_data_to_generate_files_request::<std::hash::RandomState>(
                    render_note_data,
                    None::<HashMap<_, _, _>>.as_ref(),
                ),
            )
        })
        .into_group_map();
    if grouped_parse_note_requests.keys().len() > 1 {
        return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
            description: "All notes must have the same parser to be exported.".to_string(),
        })));
    }
    if grouped_parse_note_requests.keys().len() == 0 {
        return Ok(String::new());
    }
    let (parser_name, generate_note_files_requests) =
        grouped_parse_note_requests.into_iter().next().unwrap();
    let requests_ref: Vec<_> = generate_note_files_requests
        .iter()
        .map(|r| (ConstructFileDataType::Note, r))
        .collect();
    let parser = find_parser(parser_name, all_parsers)?;
    let file_data = parser.construct_full_file_data(&requests_ref, &NoteImportAction::Update(0));

    // Get template
    let (export_template_contents, body_placeholder) = parser
        .get_template_data(TemplateType::Export)
        .map_err(|e| Error::Io {
            description: format!(
                "Failed to read template for parser {}",
                &parser.get_parser_name()
            ),
            source: e,
        })?;
    let result = export_template_contents.replace(&body_placeholder, &file_data);
    Ok(result)
}
