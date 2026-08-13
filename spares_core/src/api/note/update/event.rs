use serde_json::Value;
use sqlx::sqlite::SqlitePool;

use super::super::create_note_keywords;
use super::super::create_note_tags;
use super::super::delete_empty_tags;
use crate::Error;
use crate::LibraryError;
use crate::api::execute_batched_query;
use crate::api::max_rows_for;
use crate::api::placeholders_2d;
use crate::api::tag::DEFAULT_TAG_AUTO_DELETE;
use crate::api::tag::create_tag;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::UpdateNotePayload;
use crate::api::undo::payloads::UpdateNotesPayload;
use crate::model::EventType;
use crate::model::Note;
use crate::model::NoteId;
use crate::model::TagId;
use crate::schema::tag::CreateTagRequest;

/// Apply an `UpdateNotes` event payload (used when undoing `UpdateNotes`).
/// For each note, applies the `after` field values from each transition.
#[expect(clippy::too_many_lines)]
pub async fn update_notes_event(
    db: &SqlitePool,
    payload: UpdateNotesPayload,
    log: bool,
) -> Result<(), Error> {
    let at = chrono::Utc::now();
    for note_payload in &payload.notes {
        let UpdateNotePayload {
            id,
            data,
            parser_id,
            keywords,
            tags,
            custom_data,
            cards,
        } = note_payload;

        // Build UPDATE note SQL for scalar fields if any changed
        let new_data: Option<&str> = data.as_ref().map(|t| t.after.as_str());
        let new_parser_id: Option<i64> = parser_id.as_ref().map(|t| t.after);
        let new_custom_data: Option<&Value> = custom_data.as_ref().map(|t| &t.after);

        if new_data.is_some() || new_parser_id.is_some() || new_custom_data.is_some() {
            // Fetch existing note to fill in unchanged fields
            let existing_note: Note = sqlx::query_as(r"SELECT * FROM note WHERE id = ?")
                .bind(id)
                .fetch_one(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let data_to_set = new_data.unwrap_or(existing_note.data.as_str());
            let parser_id_to_set = new_parser_id.unwrap_or(existing_note.parser_id);
            let custom_data_to_set = new_custom_data.unwrap_or(&existing_note.custom_data);
            sqlx::query(
                r"UPDATE note SET data = ?, parser_id = ?, custom_data = ?, updated_at = ? WHERE id = ?",
            )
            .bind(data_to_set)
            .bind(parser_id_to_set)
            .bind(custom_data_to_set)
            .bind(at.timestamp())
            .bind(id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        }

        // Update keywords
        if let Some(kw_transition) = keywords {
            sqlx::query(r"DELETE FROM note_keyword WHERE note_id = ?")
                .bind(id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            let kw_entries: Vec<(NoteId, Vec<(String, bool)>)> = vec![(
                *id,
                kw_transition
                    .after
                    .iter()
                    .map(|k| (k.clone(), false))
                    .collect(),
            )];
            create_note_keywords(db, &kw_entries).await?;
        }

        // Update tags
        if let Some(tags_transition) = tags {
            // Get tags with auto_delete before removing them
            let tag_ids_to_check: Vec<TagId> =
                sqlx::query_scalar(r"SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.auto_delete = 1")
                    .bind(id)
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;

            // Remove all note_tags
            sqlx::query(r"DELETE FROM note_tag WHERE note_id = ?")
                .bind(id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

            // Delete empty auto_delete tags
            delete_empty_tags(db, &tag_ids_to_check).await?;

            // Re-add tags from the `after` list (create if needed, do NOT log)
            let mut new_tag_ids: Vec<i64> = Vec::new();
            for tag_name in &tags_transition.after {
                let existing_tag_id: Option<i64> =
                    sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ? LIMIT 1")
                        .bind(tag_name)
                        .fetch_optional(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                let tag_id = if let Some(tid) = existing_tag_id {
                    tid
                } else {
                    let tag_response = create_tag(
                        db,
                        CreateTagRequest {
                            name: tag_name.clone(),
                            description: String::new(),
                            query: None,
                            auto_delete: DEFAULT_TAG_AUTO_DELETE,
                        },
                        false,
                    )
                    .await?;
                    tag_response.id
                };
                new_tag_ids.push(tag_id);
            }
            create_note_tags(
                db,
                &new_tag_ids
                    .into_iter()
                    .map(|tid| (*id, tid))
                    .collect::<Vec<_>>(),
            )
            .await?;
        }

        // Restore cards
        if let Some(cards_transition) = cards {
            // Delete all existing cards
            sqlx::query(r"DELETE FROM card WHERE note_id = ?")
                .bind(id)
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;

            // Re-insert cards from `after` with specific IDs
            let card_snapshots = &cards_transition.after;
            if !card_snapshots.is_empty() {
                execute_batched_query(db, card_snapshots, max_rows_for(12), async |db, chunk| {
                    let query_str = format!(
                        "INSERT INTO card (id, note_id, \"order\", back_type, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data) VALUES {}",
                        placeholders_2d(chunk.len(), 12)
                    );
                    let mut query = sqlx::query(&query_str);
                    for card in chunk {
                        query = query.bind(card.id);
                        query = query.bind(id);
                        query = query.bind(card.order);
                        query = query.bind(card.back_type);
                        query = query.bind(card.due.timestamp());
                        query = query.bind(card.due.timestamp());
                        query = query.bind(card.stability);
                        query = query.bind(card.difficulty);
                        query = query.bind(card.desired_retention);
                        query = query.bind(card.special_state);
                        query = query.bind(card.state);
                        query = query.bind(&card.custom_data);
                    }
                    query
                        .execute(db)
                        .await
                        .map_err(|e| Error::Sqlx { source: e })?;
                    Ok(())
                })
                .await?;
            }
        }
    }

    if log {
        insert_events(
            db,
            &[(
                EventType::UpdateNotes,
                serde_json::to_value(&payload).map_err(|e| {
                    Error::Library(LibraryError::InvalidConfig(format!(
                        "failed to serialize update notes payload: {e}"
                    )))
                })?,
            )],
            at,
            None,
        )
        .await?;
    }

    Ok(())
}
