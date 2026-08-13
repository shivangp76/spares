use std::collections::HashSet;

use itertools::Itertools;
use sqlx::sqlite::SqliteConnection;

use super::super::delete_empty_tags_on;
use crate::Error;
use crate::LibraryError;
use crate::TagErrorKind;
use crate::api::MAX_ROWS_IN_QUERY;
use crate::api::placeholders;
use crate::api::placeholders_2d;
use crate::api::tag::DEFAULT_TAG_AUTO_DELETE;
use crate::api::tag::create_tag_row;
use crate::api::undo::payloads::CreateTagPayload;
use crate::helpers::remove_ancestor_tags;
use crate::model::NoteId;
use crate::model::TagId;
use crate::schema::note::UpdateTags;

/// Adds `note_tag` rows for all of `tags_to_add`, creating any missing tags first.
/// Returns payloads for any newly created tags.
///
/// `existing_filtered_tag_names` must contain the `name` of every tag whose `query`
/// column is non-null. It is pre-fetched by the caller to avoid one query per note,
/// and is used to reject attempts to manually assign a filtered tag.
pub(super) async fn add_tags_to_note(
    conn: &mut SqliteConnection,
    note_id: NoteId,
    tags_to_add: &[String],
    existing_filtered_tag_names: &[String],
) -> Result<Vec<CreateTagPayload>, Error> {
    // Deduplicate so a repeated name never creates duplicate `tag` rows or events (there is
    // no unique constraint on `tag.name`).
    let tags_to_add: Vec<String> = tags_to_add.iter().unique().cloned().collect();
    if let Some(filtered_tag) = tags_to_add
        .iter()
        .find(|t| existing_filtered_tag_names.contains(t))
    {
        return Err(Error::Library(LibraryError::Tag(
            TagErrorKind::InvalidInput(format!(
                "Cannot manually add filtered tag `{}`. Filtered tags are dynamically assigned.",
                filtered_tag
            )),
        )));
    }
    let mut new_tag_payloads = Vec::new();
    let mut tags_info: Vec<(TagId, String)> = Vec::new();
    for chunk in tags_to_add.chunks(MAX_ROWS_IN_QUERY) {
        let query_str = format!(
            "SELECT id, name FROM tag WHERE name IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_as(query_str.as_str());
        for tag_name in chunk {
            query = query.bind(tag_name);
        }
        tags_info.extend(
            query
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?,
        );
    }
    let mut new_tag_ids: Vec<i64> = tags_info.iter().map(|(x, _)| *x).collect::<Vec<_>>();
    let existing_tag_names = tags_info
        .iter()
        .map(|x| x.1.clone())
        .collect::<HashSet<_>>();
    let new_tags = tags_to_add
        .iter()
        .filter(|tag_name| !existing_tag_names.contains(tag_name.as_str()))
        .collect::<Vec<_>>();
    // Create missing tags sequentially on the transaction connection (tags are rare, and the
    // connection can't be shared by concurrent tasks anyway).
    for tag in new_tags {
        let tag_id = create_tag_row(&mut *conn, tag).await?;
        new_tag_ids.push(tag_id);
        new_tag_payloads.push(CreateTagPayload {
            id: Some(tag_id),
            name: (*tag).clone(),
            description: String::new(),
            query: None,
            auto_delete: DEFAULT_TAG_AUTO_DELETE,
            note_ids: vec![],
            card_ids: vec![],
        });
    }
    for chunk in new_tag_ids.chunks(MAX_ROWS_IN_QUERY) {
        let query_str = format!(
            "INSERT INTO note_tag (note_id, tag_id) VALUES {}",
            placeholders_2d(chunk.len(), 2)
        );
        let mut query = sqlx::query(query_str.as_str());
        for tag_id in chunk {
            query = query.bind(note_id);
            query = query.bind(tag_id);
        }
        query
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    Ok(new_tag_payloads)
}

#[expect(clippy::too_many_lines)]
pub(super) async fn update_tags(
    conn: &mut SqliteConnection,
    tags: &UpdateTags,
    note_id: NoteId,
    existing_filtered_tag_names: &[String],
) -> Result<Vec<CreateTagPayload>, Error> {
    let mut new_tag_payloads: Vec<CreateTagPayload> = Vec::new();
    if matches!(tags, UpdateTags::None) {
        return Ok(new_tag_payloads);
    }

    let remove_all_tags = matches!(tags, UpdateTags::SetTags(_));
    let (tags_to_remove, tags_to_add) = match tags {
        UpdateTags::ModifyTags {
            tags_to_remove,
            tags_to_add,
        } => (tags_to_remove, tags_to_add),
        UpdateTags::SetTags(items) => (&None, &Some(items.clone())),
        UpdateTags::None => unreachable!("handled by early return earlier"),
    };

    if let Some(tags_to_remove) = tags_to_remove
        && let Some(filtered_tag) = tags_to_remove
            .iter()
            .find(|t| existing_filtered_tag_names.contains(t))
    {
        return Err(Error::Library(LibraryError::Tag(
            TagErrorKind::InvalidInput(format!(
                "Cannot manually remove filtered tag `{}`. Filtered tags are dynamically assigned.",
                filtered_tag
            )),
        )));
    }
    // Remove tags
    let mut tags_to_check = Vec::new();
    if remove_all_tags {
        // Get tags for the note that have `auto_delete` enabled
        let tag_ids: Vec<TagId> = sqlx::query_scalar(r"SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.auto_delete = 1")
                .bind(note_id)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        tags_to_check.extend(tag_ids);

        // Remove all tags
        let _delete_note_tag_result = sqlx::query(r"DELETE FROM note_tag WHERE note_id = ?")
            .bind(note_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    } else if let Some(tags_to_remove) = tags_to_remove
        && !tags_to_remove.is_empty()
    {
        // Get tags for the note that have `auto_delete` enabled
        let mut tags: Vec<TagId> = Vec::new();
        for chunk in tags_to_remove.chunks(MAX_ROWS_IN_QUERY) {
            let query_str = format!(
                "SELECT t.id FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ? AND t.name in ({}) AND t.auto_delete = 1",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_scalar::<_, TagId>(&query_str);
            query = query.bind(note_id);
            for tag_name in chunk {
                query = query.bind(tag_name);
            }
            tags.extend(
                query
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?,
            );
        }
        tags_to_check.extend(tags);

        for chunk in tags_to_remove.chunks(MAX_ROWS_IN_QUERY) {
            let query_str = format!(
                "DELETE FROM note_tag WHERE tag_id IN (SELECT id FROM tag WHERE name IN ({}))",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query(query_str.as_str());
            for tag_name in chunk {
                query = query.bind(tag_name);
            }
            query
                .execute(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        }
    }
    // Delete tags with no more notes
    delete_empty_tags_on(&mut *conn, &tags_to_check).await?;

    if let Some(tags_to_add) = tags_to_add {
        let mut tags_to_add = remove_ancestor_tags(tags_to_add);
        if !tags_to_add.is_empty() {
            let existing_note_tag_names: Vec<String> =
                sqlx::query_scalar(
                    "SELECT t.name FROM tag t JOIN note_tag nt ON t.id = nt.tag_id WHERE nt.note_id = ?",
                )
                .bind(note_id)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            tags_to_add.retain(|tag| {
                let prefix = format!("{}:", tag);
                !existing_note_tag_names
                    .iter()
                    .any(|existing| existing.starts_with(&prefix))
            });
        }
        new_tag_payloads.extend(
            add_tags_to_note(conn, note_id, &tags_to_add, existing_filtered_tag_names).await?,
        );
    }
    Ok(new_tag_payloads)
}
