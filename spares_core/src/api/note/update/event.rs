use serde_json::Value;
use sqlx::sqlite::SqlitePool;

use super::super::create_note_keywords;
use super::super::create_note_tags;
use super::super::delete_empty_tags;
use crate::Error;
use crate::LibraryError;
use crate::api::execute_batched_query;
use crate::api::max_rows_for;
use crate::api::placeholders;
use crate::api::placeholders_2d;
use crate::api::tag::DEFAULT_TAG_AUTO_DELETE;
use crate::api::tag::create_tag;
use crate::api::tag::restore_tags;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::CardSnapshot;
use crate::api::undo::payloads::UpdateNotePayload;
use crate::api::undo::payloads::UpdateNotesPayload;
use crate::model::CardId;
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
    restore_tags(db, &payload.restored_tags).await?;
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
                            reschedule: true,
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

            // Delete empty auto_delete tags, after re-adding, so a tag the note keeps stays
            delete_empty_tags(db, &tag_ids_to_check).await?;
        }

        // Restore cards
        if let Some(cards_transition) = cards {
            restore_note_cards(db, *id, &cards_transition.after, at).await?;
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

/// Gives note `note_id` exactly the cards `snapshots`, as they were. Cards are updated in place
/// rather than deleted and inserted again, so they keep their reviews and filtered tags, and sync
/// sees them change rather than being deleted. A card another note has now (e.g. one moved
/// between live notes) is moved back.
async fn restore_note_cards(
    db: &SqlitePool,
    note_id: NoteId,
    snapshots: &[CardSnapshot],
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), Error> {
    let keep: Vec<CardId> = snapshots.iter().map(|card| card.id).collect();
    let query_str = format!(
        "DELETE FROM card WHERE note_id = ? AND id NOT IN ({})",
        placeholders(keep.len())
    );
    let mut query = sqlx::query(&query_str).bind(note_id);
    for card_id in &keep {
        query = query.bind(card_id);
    }
    query
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    execute_batched_query(db, snapshots, max_rows_for(12), async |db, chunk| {
        let query_str = format!(
            "INSERT INTO card (id, note_id, \"order\", back_type, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data) VALUES {}
            ON CONFLICT (id) DO UPDATE SET note_id = excluded.note_id, \"order\" = excluded.\"order\",
                back_type = excluded.back_type, updated_at = excluded.updated_at, due = excluded.due,
                stability = excluded.stability, difficulty = excluded.difficulty,
                desired_retention = excluded.desired_retention, special_state = excluded.special_state,
                state = excluded.state, custom_data = excluded.custom_data",
            placeholders_2d(chunk.len(), 12)
        );
        let mut query = sqlx::query(&query_str);
        for card in chunk {
            query = query.bind(card.id);
            query = query.bind(note_id);
            query = query.bind(card.order);
            query = query.bind(card.back_type);
            query = query.bind(at.timestamp());
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
    .await
}
