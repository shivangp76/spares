use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::LibraryError;
use crate::NoteErrorKind;
use crate::adapters::impls::spares::SparesAdapter;
use crate::adapters::impls::spares::SparesRequestProcessor;
use crate::api::note::RenderNoteData;
use crate::config::read_external_config;
use crate::helpers::value_to_string_vec;
use crate::model::CustomData;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::parsers::NoteImportAction;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;
use crate::parsers::TemplateType;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::generate_files::construct_note_file_contents;
use crate::parsers::get_file_notes;
use crate::schema::note::LinkedNote;

/// What importing a note file sets on the note. Two files describing the same note are equal here
/// even if they differ in formatting, the hash line or the linked-notes section.
#[derive(Debug, PartialEq)]
pub struct NoteFields {
    pub data: String,
    /// Extra keywords, without those embedded in the data
    pub keywords: BTreeSet<String>,
    pub tags: BTreeSet<String>,
    pub custom_data: CustomData,
}

/// A note file that holds exactly one valid note updating `note_id`.
#[derive(Debug)]
pub struct ParsedNoteFile {
    pub note_id: NoteId,
    pub settings: NoteSettings,
    pub data: String,
}

impl ParsedNoteFile {
    pub fn fields(&self, parser: &dyn Parseable) -> NoteFields {
        let embedded: BTreeSet<String> = parser
            .get_embedded_keywords(&self.data)
            .map(|ranges| {
                ranges
                    .into_iter()
                    .map(|range| self.data[range].to_string())
                    .collect()
            })
            .unwrap_or_default();
        NoteFields {
            data: self.data.clone(),
            keywords: self
                .settings
                .keywords
                .iter()
                .filter(|k| !embedded.contains(*k))
                .cloned()
                .collect(),
            tags: self.settings.tags.iter().cloned().collect(),
            custom_data: self.settings.custom_data.clone(),
        }
    }
}

impl NoteFields {
    /// The note as the parser writes it, without linked notes. Link references in the data are
    /// numbered when a file shows linked notes (`[li1]` in markdown) and not in the database
    /// (`[li]`), so they are renumbered the same way for both.
    fn normalized(&self, parser: &dyn Parseable, note_id: NoteId) -> Option<String> {
        let (template, placeholder) = parser.get_template_data(TemplateType::Note).ok()?;
        let request = GenerateNoteFilesRequest {
            note_id,
            keywords: self.keywords.iter().cloned().collect(),
            tags: self.tags.iter().cloned().collect(),
            linked_notes: Some(Vec::new()),
            note_data: self.data.clone(),
            custom_data: self.custom_data.clone(),
        };
        Some(construct_note_file_contents(
            parser,
            &template,
            &placeholder,
            &request,
        ))
    }
}

/// Whether `a` and `b` describe the same note: importing either would store the same thing.
pub fn same_note(parser: &dyn Parseable, note_id: NoteId, a: &NoteFields, b: &NoteFields) -> bool {
    a == b
        || a.normalized(parser, note_id)
            .is_some_and(|a| Some(a) == b.normalized(parser, note_id))
}

pub fn db_note_fields(row: &RenderNoteData) -> NoteFields {
    NoteFields {
        data: row.data.clone(),
        keywords: value_to_string_vec(&row.keywords_value)
            .into_iter()
            .collect(),
        tags: value_to_string_vec(&row.tags_value).into_iter().collect(),
        custom_data: row.custom_data.as_object().cloned().unwrap_or_default(),
    }
}

/// Parses `contents` as the file of note `expected_id`. Errors list every problem, and include
/// anything the import would silently skip or misinterpret: no note or several notes, missing note
/// data, a note that would be created (`action: add`) or deleted (`action: delete`), a note id
/// that differs from the file name, and parse errors. Warnings (e.g. `TODO` in the data) are
/// allowed.
pub fn parse_note_file(
    parser: &dyn Parseable,
    contents: &str,
    expected_id: NoteId,
    move_files: bool,
) -> Result<ParsedNoteFile, Vec<String>> {
    let adapter = SparesAdapter::new(SparesRequestProcessor::Server);
    let config = read_external_config().ok();
    let notes = get_file_notes(
        parser,
        None,
        contents,
        &adapter,
        move_files,
        config.as_ref().map(|c| &c.overlapper),
    )
    .map_err(|e| vec![format!("failed to parse: {e}")])?;
    let count = notes.len();
    let mut notes = notes.into_iter();
    let (settings, data) = match (notes.next(), count) {
        (Some(note), 1) => note,
        (_, 0) => {
            return Err(vec![
                "holds no note (are its start and end markers intact?)".into(),
            ]);
        }
        (_, n) => {
            return Err(vec![format!(
                "holds {n} notes; a note file must hold exactly one. Create notes with `spares note add` or `spares import`"
            )]);
        }
    };
    let mut errors: Vec<String> = settings
        .errors_and_warnings
        .iter()
        .filter(|e| !matches!(e, LibraryError::Note(NoteErrorKind::SettingsWarning { .. })))
        .map(ToString::to_string)
        .collect();
    match settings.action {
        NoteImportAction::Update(id) if id == expected_id => {}
        NoteImportAction::Update(id) => errors.push(format!(
            "names note {id}, but the file belongs to note {expected_id}"
        )),
        NoteImportAction::Add => errors.push(
            "would create a new note; create notes with `spares note add` or `spares import`"
                .into(),
        ),
        NoteImportAction::Delete(_) => {
            errors.push("would delete the note; delete notes with `spares note delete`".into());
        }
    }
    let Some(data) = data else {
        if errors.is_empty() {
            errors.push("has no note data".into());
        }
        return Err(errors);
    };
    if errors.is_empty() {
        Ok(ParsedNoteFile {
            note_id: expected_id,
            settings,
            data,
        })
    } else {
        Err(errors)
    }
}

/// The note's fields as a file would set them, or `None` if the file does not parse as that note.
pub fn file_note_fields(
    parser: &dyn Parseable,
    contents: &str,
    note_id: NoteId,
) -> Option<NoteFields> {
    parse_note_file(parser, contents, note_id, false)
        .ok()
        .map(|parsed| parsed.fields(parser))
}

/// The raw file the server writes for a note: without the hash line, with the linked-notes section
/// from `linked_notes_map`.
pub fn construct_db_note_file(
    parser: &dyn Parseable,
    row: &RenderNoteData,
    linked_notes_map: &HashMap<NoteId, Vec<NoteLink>>,
) -> Result<String, crate::Error> {
    let (template, placeholder) =
        parser
            .get_template_data(TemplateType::Note)
            .map_err(|e| crate::Error::Io {
                description: format!(
                    "Failed to read template for parser {}",
                    parser.get_parser_name()
                ),
                source: e,
            })?;
    let request = generate_request(row, Some(linked_notes_map));
    Ok(construct_note_file_contents(
        parser,
        &template,
        &placeholder,
        &request,
    ))
}

pub fn generate_request(
    row: &RenderNoteData,
    linked_notes_map: Option<&HashMap<NoteId, Vec<NoteLink>>>,
) -> GenerateNoteFilesRequest {
    crate::api::note::render_note_data_to_generate_files_request(row, linked_notes_map)
}

/// Linked notes of `note_id` as written in its file, from its `note_link` rows.
pub fn linked_notes_from_map(
    linked_notes_map: &HashMap<NoteId, Vec<NoteLink>>,
    note_id: NoteId,
) -> Option<Vec<LinkedNote>> {
    linked_notes_map.get(&note_id).map(|links| {
        links
            .iter()
            .map(|link| LinkedNote {
                searched_keyword: link.searched_keyword.clone(),
                linked_note_id: link.linked_note_id,
                matched_keyword: link.matched_keyword.clone(),
            })
            .collect()
    })
}

/// In the notes repository, fills in the linked-notes section from the notes' current `note_link`
/// rows, so files written by creating or updating notes match what a render writes. Without VCS the
/// section is left out, as before, since it is only filled in by rendering.
pub async fn fill_linked_notes(
    db: &sqlx::SqlitePool,
    requests: &mut [GenerateNoteFilesRequest],
) -> Result<(), crate::Error> {
    if !super::enabled() || requests.is_empty() {
        return Ok(());
    }
    let ids: Vec<NoteId> = requests.iter().map(|r| r.note_id).collect();
    let links = crate::api::note::get_note_links_map(db, &ids).await?;
    for request in requests {
        request.linked_notes = linked_notes_from_map(&links, request.note_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use serde_json::Map;
    use serde_json::json;
    use sqlx::SqlitePool;

    use super::construct_db_note_file;
    use super::db_note_fields;
    use super::file_note_fields;
    use super::parse_note_file;
    use super::same_note;
    use crate::api::note::create_notes;
    use crate::api::note::get_note_links_map;
    use crate::api::note::get_render_note_data;
    use crate::api::parser::create_parser;
    use crate::parsers::find_parser;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::parser::CreateParserRequest;

    /// The push base check relies on a file spares writes parsing back to the database's note.
    #[sqlx::test]
    async fn test_note_file_round_trip(pool: SqlitePool) {
        let cases = [
            (
                "markdown",
                "Linked to [kw a][li] {{ Cloze }} and {{[g:1] Grouped }}",
            ),
            (
                "typst",
                "Linked to #lin([kw a]) #cl[Cloze] and #cl[Grouped][g:1]",
            ),
        ];
        for (parser_name, data) in cases {
            let parser_row = create_parser(
                &pool,
                CreateParserRequest {
                    name: parser_name.to_string(),
                },
                false,
            )
            .await
            .unwrap();
            let mut custom_data = Map::new();
            custom_data.insert("source".to_string(), json!("book"));
            custom_data.insert("page".to_string(), json!(12));
            create_notes(
                &pool,
                CreateNotesRequest {
                    parser_id: parser_row.id,
                    requests: vec![CreateNoteRequest {
                        data: data.to_string(),
                        keywords: vec!["kw b".to_string(), "kw a".to_string()],
                        tags: vec!["b tag".to_string(), "a tag".to_string()],
                        is_suspended: false,
                        custom_data,
                    }],
                },
                Utc::now(),
                &get_all_parsers(),
                false,
            )
            .await
            .unwrap();
        }
        let rows = get_render_note_data(&pool, None).await.unwrap();
        assert_eq!(rows.len(), 2);
        let ids: Vec<i64> = rows.iter().map(|r| r.note_id).collect();
        let links = get_note_links_map(&pool, &ids).await.unwrap();
        for row in &rows {
            let parser = find_parser(&row.parser_name, &get_all_parsers()).unwrap();
            for links in [&links, &HashMap::new()] {
                let file = construct_db_note_file(parser.as_ref(), row, links).unwrap();
                let parsed = parse_note_file(parser.as_ref(), &file, row.note_id, false)
                    .unwrap_or_else(|e| panic!("{}: {e:?}\n{file}", row.parser_name));
                assert_eq!(parsed.note_id, row.note_id);
                let fields = file_note_fields(parser.as_ref(), &file, row.note_id).unwrap();
                assert!(
                    same_note(parser.as_ref(), row.note_id, &fields, &db_note_fields(row)),
                    "{}:\n{file}\n{fields:?}",
                    row.parser_name
                );
            }
            // A changed note is not the same note.
            let mut changed = db_note_fields(row);
            changed.data.push_str(" more");
            assert!(!same_note(
                parser.as_ref(),
                row.note_id,
                &changed,
                &db_note_fields(row)
            ));
            let mut changed = db_note_fields(row);
            changed.tags.insert("new tag".to_string());
            assert!(!same_note(
                parser.as_ref(),
                row.note_id,
                &changed,
                &db_note_fields(row)
            ));
        }
    }
}
