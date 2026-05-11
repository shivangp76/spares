use std::collections::HashSet;
use std::path::Path;

use chrono::DateTime;
use chrono::Utc;
use itertools::Itertools;
use serde_json::Value;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::execute_batched_query;
use crate::api::fetch_batched_query;
use crate::api::note::fetch_note_snapshot;
use crate::api::parser::get_parser;
use crate::api::placeholders;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::CardSnapshot;
use crate::api::undo::payloads::DeleteNotesPayload;
use crate::api::undo::payloads::NoteSnapshot;
use crate::config::read_internal_config;
use crate::config::write_internal_config;
use crate::helpers::value_to_string_vec;
use crate::model::Card;
use crate::model::EventType;
use crate::model::Note;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::model::TagId;
use crate::parsers::Parseable;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::get_image_occlusion_card_filepath;
use crate::parsers::image_occlusion::get_image_occlusion_rendered_directory;
use crate::parsers::image_occlusion::parse_image_occlusion_data;
use crate::schema::FilterOptions;
use crate::schema::note::DeleteNotesRequest;
use crate::schema::note::LinkedNote;
use crate::schema::note::NoteResponse;
use crate::schema::note::NotesSelector;
use crate::search::evaluator::Evaluator;

fn delete_file(file_path: &Path) -> Result<(), Error> {
    if cfg!(test) {
        // Don't clutter the Trash with testing files
        std::fs::remove_file(file_path).map_err(|e| Error::Io {
            source: e,
            description: String::new(),
        })
    } else {
        trash::delete(file_path).map_err(Error::Trash)
    }
}

pub fn delete_note_files(
    parser: &dyn Parseable,
    note_id: NoteId,
    card_orders: &[usize],
    note_data: &str,
) -> Result<(), Error> {
    // NOTE: aux files are not recorded, so they cannot be deleted
    // Delete the following, if they exist:
    // - Note raw file
    // - Note rendered file
    // - All card raw files
    // - All card rendered files
    // - All image occlusion rendered files
    // Do NOT delete all image occlusion raw files, in case they are used elsewhere

    // Note raw path
    let mut note_raw_path =
        get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
    note_raw_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    note_raw_path.set_extension(parser.file_extension());
    if note_raw_path.exists() {
        delete_file(&note_raw_path)?;
    }

    // Note rendered path
    let mut note_rendered_path = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    note_rendered_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    if note_rendered_path.exists() {
        delete_file(&note_rendered_path)?;
    }

    let image_occlusion_clozes = parse_image_occlusion_data(note_data, parser, false, &mut 0)?;

    for current_card_order in card_orders {
        // Card front raw path
        let mut card_front_raw_path = get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(*current_card_order, CardSide::Front),
            None,
        );
        card_front_raw_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Front),
            note_id,
        ));
        card_front_raw_path.set_extension(parser.file_extension());
        if card_front_raw_path.exists() {
            delete_file(&card_front_raw_path)?;
        }

        // Card front rendered path
        let mut card_front_rendered_path =
            parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
        card_front_rendered_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Front),
            note_id,
        ));
        if card_front_rendered_path.exists() {
            delete_file(&card_front_rendered_path)?;
        }

        // Card back raw path
        let mut card_back_raw_path = get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(*current_card_order, CardSide::Back),
            None,
        );
        card_back_raw_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Back),
            note_id,
        ));
        card_back_raw_path.set_extension(parser.file_extension());
        if card_back_raw_path.exists() {
            delete_file(&card_back_raw_path)?;
        }

        // Card back rendered path
        let mut card_back_rendered_path =
            parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
        card_back_rendered_path.push(parser.get_output_filename(
            RenderOutputType::Card(*current_card_order, CardSide::Back),
            note_id,
        ));
        if card_back_rendered_path.exists() {
            delete_file(&card_back_rendered_path)?;
        }

        // Image occlusion rendered paths
        for (i, _image_occlusion_cloze) in image_occlusion_clozes.iter().enumerate() {
            for side in [CardSide::Front, CardSide::Back] {
                let mut output_rendered_filepath = get_image_occlusion_rendered_directory();
                output_rendered_filepath.push(parser.get_output_filename(
                    RenderOutputType::Card(*current_card_order, side),
                    note_id,
                ));
                let image_occlusion_order_in_card = i + 1;
                let image_occlusion_card_filepath = get_image_occlusion_card_filepath(
                    &output_rendered_filepath,
                    side,
                    image_occlusion_order_in_card,
                );
                if image_occlusion_card_filepath.exists() {
                    delete_file(&image_occlusion_card_filepath)?;
                }
            }
        }
    }
    Ok(())
}

pub async fn delete_notes(
    db: &SqlitePool,
    body: DeleteNotesRequest,
    all_parsers: &[fn() -> Box<dyn Parseable>],
    log: bool,
) -> Result<(), Error> {
    let DeleteNotesRequest { selector } = body;

    // Resolve selector to get note ids
    let note_ids = selector.to_note_ids(db).await?;
    if note_ids.is_empty() {
        return Ok(());
    }

    // Fetch note snapshots (includes cards, tags, keywords) for logging and file deletion
    let note_snapshots: Vec<NoteSnapshot> = {
        #[derive(sqlx::FromRow)]
        struct NoteRow {
            id: NoteId,
            parser_id: i64,
            data: String,
            created_at: i64,
            custom_data: Value,
        }
        let rows: Vec<NoteRow> = fetch_batched_query(db, &note_ids, async |db, chunk| {
            let query_str = format!(
                r"SELECT id, parser_id, data, created_at, custom_data FROM note WHERE id IN ({})",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_as(&query_str);
            for note_id in chunk {
                query = query.bind(note_id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;

        let mut snapshots = Vec::with_capacity(rows.len());
        for row in &rows {
            let created_at = DateTime::from_timestamp(row.created_at, 0).unwrap_or_default();
            let snapshot = fetch_note_snapshot(
                db,
                row.id,
                &row.data,
                created_at,
                row.parser_id,
                &row.custom_data,
            )
            .await?;
            snapshots.push(snapshot);
        }
        snapshots
    };

    // Delete notes from DB, clean up auto-delete tags, and log event
    let payload = DeleteNotesPayload {
        notes: note_snapshots,
    };
    delete_notes_from_db(db, &note_ids, &payload, log).await?;

    // Delete files for all notes (derive parser_id, data, and card orders from snapshots)
    let grouped_note_data = payload
        .notes
        .into_iter()
        .map(|s| (s.parser_id, (s.id, s.data, s.cards)))
        .into_group_map();
    for (parser_id, notes) in grouped_note_data {
        let parser_response = get_parser(db, parser_id).await?;
        let parser = find_parser(parser_response.name.as_str(), all_parsers)?;
        for (note_id, note_data, card_snapshots) in notes {
            let card_orders: Vec<usize> = card_snapshots.iter().map(|c| c.order as usize).collect();
            delete_note_files(parser.as_ref(), note_id, &card_orders, &note_data)?;
        }
    }

    // Update config
    let mut config = read_internal_config(db).await?;
    config.linked_notes_generated = false;
    write_internal_config(db, &config).await?;

    Ok(())
}

/// Shared core: delete notes from the DB, clean up auto-delete tags, and optionally log the event.
async fn delete_notes_from_db(
    db: &SqlitePool,
    note_ids: &[NoteId],
    payload: &DeleteNotesPayload,
    log: bool,
) -> Result<(), Error> {
    // Get all tags with `auto_delete` enabled for all notes (batched query)
    // NOTE: AUTOMATIC REBUILD: If `Automatic` rebuild is enabled in the future, then a check would be added to ensure `auto_delete` is false. In other words, `auto_delete` as true and rebuild as `Automatic` conflict since once the tag has 0 notes left, it will be deleted so that means notes are not automatically added to it anymore.
    let tags_rows: Vec<TagId> = fetch_batched_query(db, note_ids, async |db, chunk| {
        let query_str = format!(
            "SELECT DISTINCT t.id
            FROM tag t
            LEFT JOIN note_tag nt ON t.id = nt.tag_id
            LEFT JOIN card_tag ct ON t.id = ct.tag_id
            LEFT JOIN card c ON ct.card_id = c.id
            WHERE
                (nt.note_id IN ({}) OR c.note_id IN ({}))
                AND t.auto_delete = 1",
            placeholders(chunk.len()),
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_scalar(&query_str);
        for note_id in chunk {
            query = query.bind(note_id);
        }
        for note_id in chunk {
            query = query.bind(note_id);
        }
        query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })
    })
    .await?;

    // Delete notes
    execute_batched_query(db, note_ids, async |db, chunk| {
        let query_str = format!(
            "DELETE FROM note WHERE id IN ({})",
            placeholders(chunk.len()),
        );
        let mut query = sqlx::query(&query_str);
        for note_id in chunk {
            query = query.bind(note_id);
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await?;

    // Delete tags with no more notes
    let all_tag_ids = tags_rows
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    delete_empty_tags(db, &all_tag_ids).await?;

    if log {
        insert_events(
            db,
            &[(
                EventType::DeleteNotes,
                serde_json::to_value(payload).unwrap(),
            )],
            Utc::now(),
            None,
        )
        .await?;
    }

    Ok(())
}

/// Delete notes from the DB only (no file operations, no config update).
/// Used when applying a `DeleteNotes` undo event.
pub(crate) async fn delete_notes_event(
    db: &SqlitePool,
    payload: DeleteNotesPayload,
    log: bool,
) -> Result<(), Error> {
    let note_ids: Vec<NoteId> = payload.notes.iter().map(|n| n.id).collect();
    if note_ids.is_empty() {
        return Ok(());
    }

    delete_notes_from_db(db, &note_ids, &payload, log).await
}

pub async fn delete_empty_tags(db: &SqlitePool, tag_ids: &[TagId]) -> Result<(), Error> {
    execute_batched_query(db, tag_ids, async |db, chunk| {
        let query_str = format!(
            r"DELETE FROM tag
            WHERE id IN ({})
            AND auto_delete = 1
            AND NOT EXISTS (
                SELECT 1 FROM note_tag WHERE note_tag.tag_id = tag.id
            )
            AND NOT EXISTS (
                SELECT 1 FROM card_tag WHERE card_tag.tag_id = tag.id
            )",
            placeholders(chunk.len()),
        );
        let mut query = sqlx::query(&query_str);
        for tag_id in chunk {
            query = query.bind(tag_id);
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await
}
