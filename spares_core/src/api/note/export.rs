use std::collections::HashMap;

use itertools::Itertools;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::api::note::get_render_note_data;
use crate::api::note::render_note_data_to_generate_files_request;
use crate::parsers::ConstructFileDataType;
use crate::parsers::NoteImportAction;
use crate::parsers::Parseable;
use crate::parsers::TemplateType;
use crate::parsers::find_parser;
use crate::schema::note::ExportNotesRequest;
use crate::search::evaluator::Evaluator;

pub async fn export_notes(
    db: &SqlitePool,
    request: ExportNotesRequest,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<HashMap<String, String>, Error> {
    let evaluator = Evaluator::new(&request.query);
    let note_ids = evaluator.get_note_ids(db).await?;
    if note_ids.is_empty() {
        return Ok(HashMap::new());
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
    if grouped_parse_note_requests.is_empty() {
        return Ok(HashMap::new());
    }

    let mut result = HashMap::new();
    for (parser_name, generate_note_files_requests) in grouped_parse_note_requests {
        let requests_ref: Vec<_> = generate_note_files_requests
            .iter()
            .map(|r| (ConstructFileDataType::Note, r))
            .collect();
        let parser = find_parser(parser_name, all_parsers)?;
        let file_data =
            parser.construct_full_file_data(&requests_ref, &NoteImportAction::Update(0));

        // Get template
        let (export_template_contents, body_placeholder) = parser
            .get_template_data(TemplateType::Export)
            .map_err(|e| Error::Io {
                description: format!(
                    "Failed to read template for parser {}",
                    parser.get_parser_name()
                ),
                source: e,
            })?;
        let rendered = export_template_contents.replace(&body_placeholder, &file_data);
        result.insert(parser.file_extension().to_string(), rendered);
    }
    Ok(result)
}
