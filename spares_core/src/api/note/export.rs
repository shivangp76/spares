//! Exporting notes as files to import, with `spares note export`.
//!
//! An export names each note by its id, so importing it updates the notes in this collection,
//! as after editing them in bulk. An export to share
//! ([`ExportNotesRequest::share`](crate::schema::note::ExportNotesRequest::share)) is for someone
//! else's collection, so it leaves out what only means something in this one, and what is private:
//!
//! - The notes with [`SparesExternalConfig::private_tag_name`] or a tag under it are left out.
//! - Notes are written to be added, without their ids, so they are imported as new notes.
//! - Tags that say how a note is doing here rather than what it is about are left out:
//!   [`SparesExternalConfig::flagged_tag_name`], [`LeechConfig::tag_name`](crate::config::LeechConfig::tag_name),
//!   [`SYNC_CONFLICT_TAG`] and the `live:` tags of live notes.
//! - Custom data is left out, such as a live note's `live_sync_name` and an Anki note's id.
//! - Clozes' `id:` keys, which only live notes here use, are left out.
//!
//! Links to other notes are kept, since they are written as keywords, so the shared notes link to
//! each other again once imported.

use std::collections::HashMap;

use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::note::get_render_note_data;
use crate::api::note::render_note_data_to_generate_files_request;
use crate::config::SparesExternalConfig;
use crate::model::CustomData;
use crate::parsers::ConstructFileDataType;
use crate::parsers::NoteImportAction;
use crate::parsers::Parseable;
use crate::parsers::TemplateType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::remove_cloze_uid_from_note_data;
use crate::schema::note::ExportNotesRequest;
use crate::schema::note::ExportNotesResponse;
use crate::search::evaluator::Evaluator;
use crate::sync::client::SYNC_CONFLICT_TAG;

/// Whether `tag` is `name` or a tag under it, such as `private:health` under `private`.
fn is_tag_or_under(tag: &str, name: &str) -> bool {
    tag.strip_prefix(name)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
}

/// Whether `tag` says how a note is doing in this collection, rather than what it is about.
fn is_state_tag(tag: &str, config: &SparesExternalConfig) -> bool {
    tag == config.flagged_tag_name
        || tag == config.leech.tag_name
        || tag == SYNC_CONFLICT_TAG
        || tag.starts_with("live:")
}

/// `request` as it is shared: see the [module docs](self).
fn to_share(
    parser: &dyn Parseable,
    mut request: GenerateNoteFilesRequest,
    config: &SparesExternalConfig,
) -> Result<GenerateNoteFilesRequest, Error> {
    request.tags.retain(|tag| !is_state_tag(tag, config));
    request.custom_data = CustomData::default();
    request.note_data = remove_cloze_uid_from_note_data(parser, &request.note_data)?.0;
    Ok(request)
}

pub async fn export_notes(
    db: &SqlitePool,
    request: ExportNotesRequest,
    config: &SparesExternalConfig,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<ExportNotesResponse, Error> {
    let share = request.share;
    let evaluator = Evaluator::new(&request.query);
    let note_ids = evaluator.get_note_ids(db).await?;
    if note_ids.is_empty() {
        return Ok(ExportNotesResponse::default());
    }

    let notes_data = get_render_note_data(db, Some(note_ids)).await?;

    let mut private_notes_left_out = 0;
    let mut grouped_requests: HashMap<&str, Vec<GenerateNoteFilesRequest>> = HashMap::new();
    for render_note_data in &notes_data {
        let request = render_note_data_to_generate_files_request::<std::hash::RandomState>(
            render_note_data,
            None::<HashMap<_, _, _>>.as_ref(),
        );
        let request = if share {
            if request
                .tags
                .iter()
                .any(|tag| is_tag_or_under(tag, &config.private_tag_name))
            {
                private_notes_left_out += 1;
                continue;
            }
            let parser = find_parser(&render_note_data.parser_name, all_parsers)?;
            to_share(parser.as_ref(), request, config)?
        } else {
            request
        };
        grouped_requests
            .entry(&render_note_data.parser_name)
            .or_default()
            .push(request);
    }

    let mut files = HashMap::new();
    for (parser_name, generate_note_files_requests) in grouped_requests {
        let requests_ref: Vec<_> = generate_note_files_requests
            .iter()
            .map(|r| (ConstructFileDataType::Note, r))
            .collect();
        let parser = find_parser(parser_name, all_parsers)?;
        // Only whether the action is an update matters here, not its note id.
        let action = if share {
            NoteImportAction::Add
        } else {
            NoteImportAction::Update(requests_ref[0].1.note_id)
        };
        let file_data = parser.construct_full_file_data(&requests_ref, &action);

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
        files.insert(parser.file_extension().to_string(), rendered);
    }
    Ok(ExportNotesResponse {
        files,
        private_notes_left_out,
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::Map;
    use serde_json::Value;

    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;

    #[sqlx::test]
    async fn test_export_to_share(pool: SqlitePool) {
        let parser = create_parser_helper(&pool, "markdown").await;
        let note = |data: &str, tags: &[&str]| CreateNoteRequest {
            data: data.to_string(),
            keywords: vec!["France".to_string()],
            tags: tags.iter().map(ToString::to_string).collect(),
            is_suspended: false,
            custom_data: Map::from_iter([("anki-note-id".to_string(), Value::from("9"))]),
        };
        let requests = vec![
            note(
                "{{[id:abc123def456] Paris }} is the capital",
                &["geo", "flagged", "leech", "sync-conflict", "live:lecture"],
            ),
            note("{{ My doctor }} is Dr. Smith", &["geo", "private:health"]),
            note("{{ My password }} is secret", &["geo", "private"]),
        ];
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests,
        };
        create_notes(&pool, request, Utc::now(), &get_all_parsers(), false)
            .await
            .unwrap();
        let config = SparesExternalConfig::default();
        let export = |share| ExportNotesRequest {
            query: "tag=geo".to_string(),
            share,
        };

        let shared = export_notes(&pool, export(true), &config, &get_all_parsers())
            .await
            .unwrap();
        assert_eq!(shared.private_notes_left_out, 2);
        let file = &shared.files["md"];
        assert!(file.contains("Paris"), "{file}");
        assert!(
            file.contains("tags: geo -->") || file.contains("tags: geo --->"),
            "{file}"
        );
        for left_out in [
            "doctor",
            "password",
            "note-id",
            "abc123def456",
            "anki-note-id",
            "flagged",
            "live:",
        ] {
            assert!(!file.contains(left_out), "{left_out} in {file}");
        }

        // Without sharing, the export is the notes as they are, to import back here.
        let own = export_notes(&pool, export(false), &config, &get_all_parsers())
            .await
            .unwrap();
        assert_eq!(own.private_notes_left_out, 0);
        let file = &own.files["md"];
        for kept in [
            "doctor",
            "password",
            "note-id",
            "abc123def456",
            "anki-note-id",
            "flagged",
        ] {
            assert!(file.contains(kept), "{kept} not in {file}");
        }
    }

    #[test]
    fn test_is_tag_or_under() {
        assert!(is_tag_or_under("private", "private"));
        assert!(is_tag_or_under("private:health", "private"));
        assert!(!is_tag_or_under("privateer", "private"));
        assert!(!is_tag_or_under("math:private", "private"));
    }
}
