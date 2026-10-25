use chrono::Utc;
use serde_json::to_value;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::TagErrorKind;
use crate::api::MAX_ROWS_IN_QUERY;
use crate::api::execute_batched_query;
use crate::api::undo::insert_events;
use crate::api::undo::join_event_group;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::DeleteTagPayload;
use crate::api::undo::payloads::DeletedTag;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateTagPayload;
use crate::ids::IdTable;
use crate::ids::next_id;
use crate::ids::next_id_in;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::NoteId;
use crate::model::Tag;
use crate::model::TagId;
use crate::notes_vcs;
use crate::schema::FilterOptions;
use crate::schema::tag::CreateTagRequest;
use crate::schema::tag::TagResponse;
use crate::schema::tag::TagSelector;
use crate::schema::tag::UpdateTagRequest;

mod query;
pub use query::*;

const TAG_DEFAULT_LIMIT: usize = 500;
pub const DEFAULT_TAG_AUTO_DELETE: bool = true;

pub async fn create_tag(
    db: &SqlitePool,
    body: CreateTagRequest,
    log: bool,
) -> Result<TagResponse, Error> {
    let now = Utc::now();
    let payload = CreateTagPayload {
        id: None,
        name: body.name,
        description: body.description,
        query: body.query,
        auto_delete: body.auto_delete,
        reschedule: body.reschedule,
        created_at: now,
        updated_at: now,
        note_ids: vec![],
        card_ids: vec![],
        review_log_ids: vec![],
    };
    create_tag_event(db, payload, log).await
}

/// Inserts a new tag row and returns it. The caller must have already verified that a tag
/// with this name does not exist (and deduplicated names), e.g. `update_tags` pre-filters
/// existing tags. Unlike [`create_tag`], it only supports the `query = None` case and runs on a
/// supplied connection so it can participate in a surrounding transaction.
pub(crate) async fn create_tag_row(
    conn: &mut sqlx::sqlite::SqliteConnection,
    name: &str,
) -> Result<Tag, Error> {
    let id = next_id(&mut *conn, IdTable::Tag).await?;
    sqlx::query_as(
        r"INSERT INTO tag (id, name, description, query, auto_delete) VALUES (?, ?, '', NULL, ?) RETURNING *",
    )
    .bind(id)
    .bind(name)
    .bind(DEFAULT_TAG_AUTO_DELETE)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx { source: e })
}

pub async fn create_tag_event(
    db: &SqlitePool,
    payload: CreateTagPayload,
    log: bool,
) -> Result<TagResponse, Error> {
    // First, check if a tag with the same name already exists
    // This is enforced manually instead of setting the primary key of the table to `tag.name` so this restriction can be removed in the future, if desired.
    let existing_tag: Option<i64> = sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ?")
        .bind(&payload.name)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    if existing_tag.is_some() {
        return Err(Error::Library(LibraryError::Tag(
            TagErrorKind::InvalidInput("A tag with this name already exists.".to_string()),
        )));
    }

    if let Some(ref query) = payload.query {
        verify_filtered_tag_query(db, query.as_str()).await?;
    }

    // Undoing a `DeleteTag` supplies the id, along with the original timestamps, so the tag comes
    // back exactly as it was.
    let id = match payload.id {
        Some(id) => id,
        None => next_id_in(db, IdTable::Tag).await?,
    };
    let tag: Tag = sqlx::query_as(
        r"INSERT INTO tag (id, name, description, query, auto_delete, reschedule, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING *",
    )
    .bind(id)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&payload.query)
    .bind(payload.auto_delete)
    .bind(payload.reschedule)
    .bind(payload.created_at.timestamp())
    .bind(payload.updated_at.timestamp())
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    // When undo restores a tag, its saved `card_ids` are the exact set to bring back. Re-running
    // the query would add whichever cards it matches today.
    if let Some(ref query) = payload.query
        && payload.id.is_none()
    {
        // Execute query and add tag to all notes that match query
        tag_cards_from_query(db, query, tag.id).await?;
    }

    // Restore note_tag associations (for undo of DeleteTag)
    for note_id in &payload.note_ids {
        sqlx::query(r"INSERT OR IGNORE INTO note_tag (note_id, tag_id) VALUES (?, ?)")
            .bind(note_id)
            .bind(tag.id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    // Restore card_tag associations (for undo of DeleteTag)
    for card_id in &payload.card_ids {
        sqlx::query(r"INSERT OR IGNORE INTO card_tag (card_id, tag_id) VALUES (?, ?)")
            .bind(card_id)
            .bind(tag.id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    restore_review_tags(db, tag.id, &payload.review_log_ids).await?;

    // Log event
    if log {
        let log_payload = CreateTagPayload {
            id: Some(tag.id),
            name: tag.name.clone(),
            description: tag.description.clone(),
            query: tag.query.clone(),
            auto_delete: tag.auto_delete,
            reschedule: tag.reschedule,
            created_at: tag.created_at,
            updated_at: tag.updated_at,
            note_ids: payload.note_ids.clone(),
            card_ids: payload.card_ids.clone(),
            review_log_ids: payload.review_log_ids.clone(),
        };
        let _event_id = insert_events(
            db,
            &[(EventType::CreateTag, to_value(&log_payload).unwrap())],
            Utc::now(),
            None,
        )
        .await?;
    }

    Ok(TagResponse::new(&tag))
}

/// Recreates `tags`, which an undone action deleted, as they were, without their notes or cards and
/// without running their queries, and gives their reviews back their tag. A tag whose name another
/// tag has taken since is skipped: notes link to tags by name, so the restored notes link to that
/// one instead.
pub(crate) async fn restore_tags(db: &SqlitePool, tags: &[DeletedTag]) -> Result<(), Error> {
    for DeletedTag {
        tag,
        review_log_ids,
    } in tags
    {
        sqlx::query(
            r"INSERT INTO tag (id, name, description, query, auto_delete, reschedule, created_at, updated_at)
            SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
            WHERE NOT EXISTS (SELECT 1 FROM tag WHERE id = ?1 OR name = ?2)",
        )
        .bind(tag.id)
        .bind(&tag.name)
        .bind(&tag.description)
        .bind(&tag.query)
        .bind(tag.auto_delete)
        .bind(tag.reschedule)
        .bind(tag.created_at.timestamp())
        .bind(tag.updated_at.timestamp())
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        restore_review_tags(db, tag.id, review_log_ids).await?;
    }
    Ok(())
}

/// The reviews done under `tag_id`, whose `tag_id` deleting the tag will clear
pub(crate) async fn review_ids_with_tag(db: &SqlitePool, tag_id: TagId) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(r"SELECT id FROM review_log WHERE tag_id = ? ORDER BY id")
        .bind(tag_id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

/// Gives `review_log_ids` back the tag `tag_id`, which deleting it cleared, if the tag exists
pub(crate) async fn restore_review_tags(
    db: &SqlitePool,
    tag_id: TagId,
    review_log_ids: &[i64],
) -> Result<(), Error> {
    execute_batched_query(db, review_log_ids, MAX_ROWS_IN_QUERY, async |db, chunk| {
        let query_str = format!(
            "UPDATE review_log SET tag_id = ?1 WHERE id IN ({}) AND tag_id IS NULL
            AND EXISTS (SELECT 1 FROM tag WHERE id = ?1)",
            (0..chunk.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut query = sqlx::query(&query_str).bind(tag_id);
        for id in chunk {
            query = query.bind(id);
        }
        query
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        Ok(())
    })
    .await
}

pub async fn get_tag(db: &SqlitePool, id: i64) -> Result<TagResponse, Error> {
    let tag: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(TagResponse::new(&tag))
}

pub async fn get_tag_by_name(db: &SqlitePool, name: &str) -> Result<TagResponse, Error> {
    let tag: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE name = ?")
        .bind(name)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(TagResponse::new(&tag))
}

async fn tag_selector_to_id(db: &SqlitePool, tag_selector: TagSelector) -> Result<TagId, Error> {
    match tag_selector {
        TagSelector::Id(id) => Ok(id),
        TagSelector::Name(name) => {
            let tag_id: TagId = sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ?")
                .bind(name)
                .fetch_one(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            Ok(tag_id)
        }
    }
}

pub async fn update_tag_event(
    db: &SqlitePool,
    payload: UpdateTagPayload,
    id: TagId,
    log: bool,
) -> Result<TagResponse, Error> {
    let body = UpdateTagRequest {
        tag_to_modify: TagSelector::Id(id),
        name: payload.name.map(|t| t.after),
        description: payload.description.map(|t| t.after),
        query: payload.query.map(|t| t.after),
        auto_delete: payload.auto_delete.map(|t| t.after),
        reschedule: payload.reschedule.map(|t| t.after),
    };
    // Each tag a subtree rename changed has its own event, so replaying one changes only its tag.
    update_single_tag(db, body, id, log).await
}

/// Merges `source_tag` into `target_id`, returning the target and the id of the event logged.
async fn merge_tag_into(
    db: &SqlitePool,
    source_id: TagId,
    source_tag: &Tag,
    target_id: TagId,
    log: bool,
) -> Result<(TagResponse, Option<i64>), Error> {
    // Create new note tag associations using previous note tag associations
    sqlx::query(
        r"INSERT OR IGNORE INTO note_tag (note_id, tag_id) SELECT note_id, ? FROM note_tag WHERE tag_id = ?",
    )
    .bind(target_id)
    .bind(source_id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    sqlx::query(
        r"INSERT OR IGNORE INTO card_tag (card_id, tag_id) SELECT card_id, ? FROM card_tag WHERE tag_id = ?",
    )
    .bind(target_id)
    .bind(source_id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    let note_ids: Vec<NoteId> =
        sqlx::query_scalar(r"SELECT note_id FROM note_tag WHERE tag_id = ?")
            .bind(source_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let card_ids: Vec<CardId> =
        sqlx::query_scalar(r"SELECT card_id FROM card_tag WHERE tag_id = ?")
            .bind(source_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    // Delete old tag (and respective note tag associations)
    let delete_payload = DeleteTagPayload {
        id: Some(source_id),
        name: source_tag.name.clone(),
        description: source_tag.description.clone(),
        query: source_tag.query.clone(),
        auto_delete: source_tag.auto_delete,
        reschedule: source_tag.reschedule,
        created_at: source_tag.created_at,
        updated_at: source_tag.updated_at,
        note_ids,
        card_ids,
        review_log_ids: review_ids_with_tag(db, source_id).await?,
    };
    let event_id = delete_tag_logged(db, delete_payload, log).await?;

    Ok((get_tag(db, target_id).await?, event_id))
}

/// Notes with tag `id`, whose files list the tag.
async fn notes_with_tag(db: &SqlitePool, id: TagId) -> Result<Vec<NoteId>, Error> {
    sqlx::query_scalar(r"SELECT note_id FROM note_tag WHERE tag_id = ?")
        .bind(id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

/// Updates a tag. Renaming it renames the tags under it too, as [`rename_tags`] does, so renaming
/// `math` to `maths` also renames `math:calculus` to `maths:calculus`.
pub async fn update_tag(
    db: &SqlitePool,
    body: UpdateTagRequest,
    log: bool,
) -> Result<TagResponse, Error> {
    let id = tag_selector_to_id(db, body.tag_to_modify.clone()).await?;
    if let Some(ref name) = body.name {
        let existing_name: String = sqlx::query_scalar(r"SELECT name FROM tag WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if *name != existing_name {
            let to = name.clone();
            let renamed = rename_tags_with(db, &existing_name, &to, Some(body), log).await?;
            // The tag itself is the shortest name, so it is renamed first.
            return Ok(renamed.into_iter().next().unwrap());
        }
    }
    update_single_tag(db, body, id, log).await
}

/// Updates only tag `id`, leaving the tags under it as they are.
async fn update_single_tag(
    db: &SqlitePool,
    body: UpdateTagRequest,
    id: TagId,
    log: bool,
) -> Result<TagResponse, Error> {
    // Renaming, merging or making a tag filtered changes the files of its notes, which list it.
    let affected = if body.name.is_some() || body.query.is_some() {
        notes_with_tag(db, id).await?
    } else {
        Vec::new()
    };
    let result = update_tag_inner(db, body, id, log).await;
    notes_vcs::reconcile_after(db, affected, "retag").await;
    result.map(|(tag, _event_id)| tag)
}

/// Renames the tag `from` and the tags under it, like `from:calculus`, to `to` and
/// `to:calculus`. A tag whose new name is taken is merged into the tag with that name. `from`
/// needs no tag of its own, so `math` can be renamed while only `math:calculus` exists, and
/// moving a tag to another parent is renaming it, like `math:calculus` to `analysis:calculus`.
///
/// Undo reverts the whole rename at once. Returns the tags after the rename, shortest name first.
pub async fn rename_tags(
    db: &SqlitePool,
    from: &str,
    to: &str,
    log: bool,
) -> Result<Vec<TagResponse>, Error> {
    rename_tags_with(db, from, to, None, log).await
}

/// [`rename_tags`], applying `root_changes`, the other changes of an [`UpdateTagRequest`], to the
/// tag named `from`.
async fn rename_tags_with(
    db: &SqlitePool,
    from: &str,
    to: &str,
    mut root_changes: Option<UpdateTagRequest>,
    log: bool,
) -> Result<Vec<TagResponse>, Error> {
    let invalid =
        |message: String| Error::Library(LibraryError::Tag(TagErrorKind::InvalidInput(message)));
    if to.trim().is_empty() || to.starts_with(':') || to.ends_with(':') {
        return Err(invalid(format!("`{to}` is not a valid tag name.")));
    }
    if to.starts_with(&format!("{from}:")) {
        return Err(invalid(format!(
            "Cannot move `{from}` into `{to}`, which is under it."
        )));
    }
    // `;` follows `:`, so these are the names that start with `from:`.
    let tags: Vec<Tag> = sqlx::query_as(
        r"SELECT * FROM tag WHERE name = ?1 OR (name > ?1 || ':' AND name < ?1 || ';')
        ORDER BY length(name), id",
    )
    .bind(from)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    if tags.is_empty() {
        return Err(invalid(format!(
            "No tag is named `{from}` or starts with `{from}:`."
        )));
    }
    if to == from {
        return Ok(tags.iter().map(TagResponse::new).collect());
    }

    let mut affected = Vec::new();
    for tag in &tags {
        affected.extend(notes_with_tag(db, tag.id).await?);
    }
    let result = rename_tags_inner(db, from, to, &tags, &mut root_changes, log).await;
    notes_vcs::reconcile_after(db, affected, "retag").await;
    result
}

async fn rename_tags_inner(
    db: &SqlitePool,
    from: &str,
    to: &str,
    tags: &[Tag],
    root_changes: &mut Option<UpdateTagRequest>,
    log: bool,
) -> Result<Vec<TagResponse>, Error> {
    let mut renamed = Vec::with_capacity(tags.len());
    let mut event_ids = Vec::new();
    for tag in tags {
        let name = format!("{to}{}", &tag.name[from.len()..]);
        let body = match root_changes.take_if(|_| tag.name == from) {
            Some(changes) => UpdateTagRequest {
                tag_to_modify: TagSelector::Id(tag.id),
                name: Some(name),
                ..changes
            },
            None => UpdateTagRequest {
                tag_to_modify: TagSelector::Id(tag.id),
                name: Some(name),
                description: None,
                query: None,
                auto_delete: None,
                reschedule: None,
            },
        };
        let (response, event_id) = update_tag_inner(db, body, tag.id, log).await?;
        renamed.push(response);
        event_ids.extend(event_id);
    }
    if let Some((&leader, others)) = event_ids.split_first()
        && !others.is_empty()
    {
        for &other in others {
            join_event_group(db, leader, other).await?;
        }
    }
    Ok(renamed)
}

/// Updates tag `id`, returning it and the id of the event logged.
async fn update_tag_inner(
    db: &SqlitePool,
    body: UpdateTagRequest,
    id: TagId,
    log: bool,
) -> Result<(TagResponse, Option<i64>), Error> {
    let existing_tag: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    // Update (if empty, use old value)
    let new_name = body
        .name
        .clone()
        .unwrap_or_else(|| existing_tag.name.clone());
    let new_description = body
        .description
        .clone()
        .unwrap_or_else(|| existing_tag.description.clone());
    let new_query = body
        .query
        .clone()
        .unwrap_or_else(|| existing_tag.query.clone());
    let new_auto_delete = body.auto_delete.unwrap_or(existing_tag.auto_delete);
    let new_reschedule = body.reschedule.unwrap_or(existing_tag.reschedule);
    if let Some(ref name) = body.name {
        let tag_with_name: Option<i64> = sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ?")
            .bind(name)
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if let Some(target_id) = tag_with_name
            && target_id != id
        {
            return merge_tag_into(db, id, &existing_tag, target_id, log).await;
        }
    }

    if let Some(Some(ref query)) = body.query {
        verify_filtered_tag_query(db, query.as_str()).await?;

        // Delete existing card tags with this tag
        let _delete_card_tag_result = sqlx::query(r"DELETE FROM card_tag WHERE tag_id = ?")
            .bind(existing_tag.id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Execute query and add tag to all notes that match query
        tag_cards_from_query(db, query.as_str(), existing_tag.id).await?;
    }

    let updated_tag: Tag = sqlx::query_as(
        r"UPDATE tag SET name = ?, description = ?, query = ?, auto_delete = ?, reschedule = ?, updated_at = strftime('%s', 'now') WHERE id = ? RETURNING *",
    )
    .bind(&new_name)
    .bind(&new_description)
    .bind(&new_query)
    .bind(new_auto_delete)
    .bind(new_reschedule)
    .bind(id)
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let tag_response = TagResponse::new(&updated_tag);

    // Log event
    if log {
        let payload = UpdateTagPayload {
            id,
            name: body.name.map(|_| Transition {
                before: existing_tag.name.clone(),
                after: updated_tag.name.clone(),
            }),
            description: body.description.map(|_| Transition {
                before: existing_tag.description.clone(),
                after: updated_tag.description.clone(),
            }),
            query: body.query.map(|_| Transition {
                before: existing_tag.query.clone(),
                after: updated_tag.query.clone(),
            }),
            auto_delete: body.auto_delete.map(|_| Transition {
                before: existing_tag.auto_delete,
                after: updated_tag.auto_delete,
            }),
            reschedule: body.reschedule.map(|_| Transition {
                before: existing_tag.reschedule,
                after: updated_tag.reschedule,
            }),
        };
        let event_ids = insert_events(
            db,
            &[(EventType::UpdateTag, to_value(&payload).unwrap())],
            Utc::now(),
            None,
        )
        .await?;
        return Ok((tag_response, event_ids.first().copied()));
    }

    Ok((tag_response, None))
}

pub async fn delete_tag(db: &SqlitePool, id: i64, log: bool) -> Result<(), Error> {
    let affected = notes_with_tag(db, id).await?;
    let result = delete_tag_inner(db, id, log).await;
    notes_vcs::reconcile_after(db, affected, "untag").await;
    result
}

async fn delete_tag_inner(db: &SqlitePool, id: i64, log: bool) -> Result<(), Error> {
    let tag: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    let note_ids: Vec<NoteId> =
        sqlx::query_scalar(r"SELECT note_id FROM note_tag WHERE tag_id = ?")
            .bind(id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let card_ids: Vec<CardId> =
        sqlx::query_scalar(r"SELECT card_id FROM card_tag WHERE tag_id = ?")
            .bind(id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    let payload = DeleteTagPayload {
        id: Some(tag.id),
        name: tag.name,
        description: tag.description,
        query: tag.query,
        auto_delete: tag.auto_delete,
        reschedule: tag.reschedule,
        created_at: tag.created_at,
        updated_at: tag.updated_at,
        note_ids,
        card_ids,
        review_log_ids: review_ids_with_tag(db, id).await?,
    };
    delete_tag_event(db, payload, log).await
}

pub async fn delete_tag_event(
    db: &SqlitePool,
    payload: DeleteTagPayload,
    log: bool,
) -> Result<(), Error> {
    delete_tag_logged(db, payload, log)
        .await
        .map(|_event_id| ())
}

/// [`delete_tag_event`], returning the id of the event logged.
async fn delete_tag_logged(
    db: &SqlitePool,
    payload: DeleteTagPayload,
    log: bool,
) -> Result<Option<i64>, Error> {
    let id = payload.id.ok_or_else(|| {
        Error::Library(LibraryError::InvalidConfig(
            "DeleteTagPayload missing id".to_string(),
        ))
    })?;
    sqlx::query(r"DELETE FROM tag WHERE id = ?")
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    if !log {
        return Ok(None);
    }
    let event_ids = insert_events(
        db,
        &[(EventType::DeleteTag, to_value(&payload).unwrap())],
        Utc::now(),
        None,
    )
    .await?;
    Ok(event_ids.first().copied())
}

pub async fn list_tags(db: &SqlitePool, opts: FilterOptions) -> Result<Vec<TagResponse>, Error> {
    let limit = opts.limit.unwrap_or(TAG_DEFAULT_LIMIT);
    let offset = (opts.page.unwrap_or(1).saturating_sub(1)) * limit;
    let items = sqlx::query_as(r"SELECT * FROM tag ORDER by id LIMIT ? OFFSET ?")
        .bind(limit as u32)
        .bind(offset as u32)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let responses = items
        .iter()
        .map(TagResponse::new)
        .collect::<Vec<TagResponse>>();
    Ok(responses)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub async fn create_tag_helper(
        pool: &SqlitePool,
        name: &str,
        description: &str,
    ) -> TagResponse {
        let request = CreateTagRequest {
            name: name.to_string(),
            description: description.to_string(),
            query: None,
            auto_delete: false,
            reschedule: true,
        };
        let tag_res = create_tag(pool, request, false).await;
        assert!(tag_res.is_ok());
        if let Ok(tag) = tag_res {
            assert_eq!(tag.name, name);
            assert_eq!(tag.description, description);

            // Check database and verify item with id exists
            let tag_res2: Result<Tag, sqlx::Error> =
                sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
                    .bind(tag.id)
                    .fetch_one(pool)
                    .await;
            assert!(tag_res2.is_ok());
            if let Ok(tag) = tag_res2 {
                assert_eq!(tag.name, name);
                assert_eq!(tag.description, description);
            }
            return tag;
        }
        unreachable!();
    }

    #[sqlx::test]
    async fn test_create_tag(pool: SqlitePool) -> () {
        // Create parent tag
        let _parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description").await;

        // Create child tag
        let _child_tag = create_tag_helper(&pool, "Child tag name", "Child tag description").await;

        // Create tag with duplicate name
        let request = CreateTagRequest {
            name: "Child tag name".to_string(),
            description: String::new(),
            query: None,
            auto_delete: false,
            reschedule: true,
        };
        let tag_res = create_tag(&pool, request, false).await;
        assert!(tag_res.is_err());
    }

    #[sqlx::test]
    async fn test_get_tag(pool: SqlitePool) -> () {
        // Create parent tag
        let _parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description").await;

        // Create child tag
        let child_tag = create_tag_helper(&pool, "Child tag name", "Child tag description").await;

        let tag_res = get_tag(&pool, child_tag.id).await;
        if let Ok(tag) = tag_res {
            assert_eq!(tag.name, "Child tag name");
            assert_eq!(tag.description, "Child tag description");
        }
    }

    #[sqlx::test]
    async fn test_update_tag(pool: SqlitePool) -> () {
        // Create parent tag
        let parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description").await;
        let parent_tag_id = parent_tag.id;

        // Create tag so it can be updated
        let tag = create_tag_helper(&pool, "Child tag name", "Child tag description").await;

        // Update tag
        let request = UpdateTagRequest {
            tag_to_modify: TagSelector::Id(tag.id),
            name: Some("Updated name".to_string()),
            description: None,
            query: None,
            auto_delete: None,
            reschedule: None,
        };
        let tag_res = update_tag(&pool, request, false).await;
        assert!(tag_res.is_ok());
        if let Ok(tag) = tag_res {
            assert_eq!(tag.name, "Updated name");
            assert_eq!(tag.description, "Child tag description");
        }

        // Check database and verify item with id has the new property
        let tag_res: Result<Tag, sqlx::Error> = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(tag.id)
            .fetch_one(&pool)
            .await;
        assert!(tag_res.is_ok());
        if let Ok(tag) = tag_res {
            assert_eq!(tag.name, "Updated name");
            assert_eq!(tag.description, "Child tag description");
        }

        // Renaming to an existing tag's name merges the source into the target
        let request = UpdateTagRequest {
            tag_to_modify: TagSelector::Id(tag.id),
            name: Some("Parent tag name".to_string()),
            description: None,
            query: None,
            auto_delete: None,
            reschedule: None,
        };
        let tag_res = update_tag(&pool, request, false).await;
        assert!(tag_res.is_ok());
        if let Ok(merged_tag) = tag_res {
            assert_eq!(merged_tag.name, "Parent tag name");
            assert_eq!(merged_tag.id, parent_tag_id);
        }

        // Source tag should no longer exist
        let source_gone: Option<Tag> = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(tag.id)
            .fetch_optional(&pool)
            .await
            .unwrap();
        assert!(source_gone.is_none());
    }

    async fn tag_names(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar(r"SELECT name FROM tag ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn renaming_a_tag_renames_the_tags_under_it(pool: SqlitePool) {
        let math = create_tag_helper(&pool, "math", "numbers").await;
        create_tag_helper(&pool, "math:calculus", "limits").await;
        create_tag_helper(&pool, "mathematics", "").await;
        create_tag_helper(&pool, "math-history", "").await;
        let request = UpdateTagRequest {
            tag_to_modify: TagSelector::Id(math.id),
            name: Some("maths".to_string()),
            description: Some("still numbers".to_string()),
            query: None,
            auto_delete: None,
            reschedule: None,
        };
        let renamed = update_tag(&pool, request, true).await.unwrap();
        assert_eq!(renamed.id, math.id);
        assert_eq!(renamed.name, "maths");
        assert_eq!(renamed.description, "still numbers");
        assert_eq!(
            tag_names(&pool).await,
            ["math-history", "mathematics", "maths", "maths:calculus"]
        );
        // The other changes are only for the tag itself.
        let calculus = get_tag_by_name(&pool, "maths:calculus").await.unwrap();
        assert_eq!(calculus.description, "limits");
        // One action, in one group led by the tag's own rename.
        let groups: Vec<Option<i64>> =
            sqlx::query_scalar(r"SELECT group_id FROM event ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(groups.len(), 2);
        assert!(groups[0].is_some() && groups.iter().all(|g| *g == groups[0]));
    }

    #[sqlx::test]
    async fn rename_tags_renames_an_ancestor_with_no_tag_of_its_own(pool: SqlitePool) {
        create_tag_helper(&pool, "math:calculus", "").await;
        create_tag_helper(&pool, "math:algebra", "").await;
        let renamed = rename_tags(&pool, "math", "maths", false).await.unwrap();
        assert_eq!(renamed.len(), 2);
        assert_eq!(tag_names(&pool).await, ["maths:algebra", "maths:calculus"]);

        // Moving to the top level drops the parent.
        rename_tags(&pool, "maths:algebra", "algebra", false)
            .await
            .unwrap();
        assert_eq!(tag_names(&pool).await, ["algebra", "maths:calculus"]);
    }

    #[sqlx::test]
    async fn rename_tags_refuses_bad_names(pool: SqlitePool) {
        create_tag_helper(&pool, "math", "").await;
        create_tag_helper(&pool, "math:calculus", "").await;
        assert!(
            rename_tags(&pool, "math", "math:calculus:math", false)
                .await
                .is_err()
        );
        assert!(rename_tags(&pool, "math", "", false).await.is_err());
        assert!(rename_tags(&pool, "math", "maths:", false).await.is_err());
        assert!(rename_tags(&pool, "physics", "maths", false).await.is_err());
        assert_eq!(tag_names(&pool).await, ["math", "math:calculus"]);
    }

    #[sqlx::test]
    async fn rename_tags_merges_a_subtag_whose_name_is_taken(pool: SqlitePool) {
        create_tag_helper(&pool, "math:calculus", "").await;
        let taken = create_tag_helper(&pool, "analysis:calculus", "").await;
        create_tag_helper(&pool, "math:calculus:limits", "").await;
        let renamed = rename_tags(&pool, "math:calculus", "analysis:calculus", false)
            .await
            .unwrap();
        assert_eq!(renamed[0].id, taken.id);
        assert_eq!(
            tag_names(&pool).await,
            ["analysis:calculus", "analysis:calculus:limits"]
        );
    }

    #[sqlx::test]
    async fn test_delete_tag(pool: SqlitePool) -> () {
        // Create tag so it can be deleted
        let tag = create_tag_helper(&pool, "Tag name", "Tag description").await;

        // Delete tag
        let delete_tag_res = delete_tag(&pool, tag.id, false).await;
        assert!(delete_tag_res.is_ok());

        // Check database and verify item with id does not exist
        let tag_res: Result<Tag, sqlx::Error> = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(tag.id)
            .fetch_one(&pool)
            .await;
        assert!(tag_res.is_err());
        // Workaround since sqlx::Error does not derive PartialEq
        assert_eq!(
            format!("{:?}", tag_res.unwrap_err()),
            format!("{:?}", sqlx::Error::RowNotFound)
        );
    }

    #[sqlx::test]
    async fn test_list_tags(pool: SqlitePool) -> () {
        // Create tags
        let _tag1 = create_tag_helper(&pool, "Tag 1 name", "Tag 1 description").await;
        let _tag2 = create_tag_helper(&pool, "Tag 2 name", "Tag 2 description").await;

        // List tags
        let list_tags_res = list_tags(
            &pool,
            FilterOptions {
                limit: Some(10),
                page: Some(1),
            },
        )
        .await;
        assert!(list_tags_res.is_ok());
        if let Ok(tags) = list_tags_res {
            assert_eq!(tags.len(), 2);
            assert_eq!(tags.first().unwrap().name, "Tag 1 name");
            assert_eq!(tags.last().unwrap().name, "Tag 2 name");
        }
    }

    #[sqlx::test]
    async fn test_merge_tag_logs_delete_event(pool: SqlitePool) {
        let source = create_tag_helper(&pool, "source", "desc").await;
        let _target = create_tag_helper(&pool, "target", "desc").await;
        let n_before = event_count(&pool).await;
        let _ = update_tag(
            &pool,
            UpdateTagRequest {
                tag_to_modify: TagSelector::Id(source.id),
                name: Some("target".to_string()),
                description: None,
                query: None,
                auto_delete: None,
                reschedule: None,
            },
            true,
        )
        .await
        .unwrap();
        assert_eq!(event_count(&pool).await, n_before + 1);
    }

    #[sqlx::test]
    async fn test_merge_tag_returns_target_response(pool: SqlitePool) {
        let source = create_tag_helper(&pool, "source", "desc").await;
        let target = create_tag_helper(&pool, "target", "desc").await;

        let result = update_tag(
            &pool,
            UpdateTagRequest {
                tag_to_modify: TagSelector::Id(source.id),
                name: Some("target".to_string()),
                description: None,
                query: None,
                auto_delete: None,
                reschedule: None,
            },
            false,
        )
        .await
        .unwrap();

        assert_eq!(result.id, target.id);
        assert_eq!(result.name, "target");
    }

    #[sqlx::test]
    async fn test_merge_tag_transfers_note_tags(pool: SqlitePool) {
        let parser = crate::api::parser::tests::create_parser_helper(&pool, "markdown").await;
        let note_id = crate::ids::insert_test_note(&pool, "n1", parser.id).await;

        let source = create_tag_helper(&pool, "source", "desc").await;
        let target = create_tag_helper(&pool, "target", "desc").await;

        sqlx::query(r"INSERT INTO note_tag (note_id, tag_id) VALUES (?, ?)")
            .bind(note_id)
            .bind(source.id)
            .execute(&pool)
            .await
            .unwrap();

        let _ = update_tag(
            &pool,
            UpdateTagRequest {
                tag_to_modify: TagSelector::Id(source.id),
                name: Some("target".to_string()),
                description: None,
                query: None,
                auto_delete: None,
                reschedule: None,
            },
            false,
        )
        .await
        .unwrap();

        let target_note_count: i64 =
            sqlx::query_scalar(r"SELECT COUNT(*) FROM note_tag WHERE note_id = ? AND tag_id = ?")
                .bind(note_id)
                .bind(target.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            target_note_count, 1,
            "target should have the note after merge"
        );
    }

    #[sqlx::test]
    async fn test_merge_tag_with_overlapping_note_tags(pool: SqlitePool) {
        let parser = crate::api::parser::tests::create_parser_helper(&pool, "markdown").await;
        let note_id = crate::ids::insert_test_note(&pool, "n1", parser.id).await;

        let source = create_tag_helper(&pool, "source", "desc").await;
        let target = create_tag_helper(&pool, "target", "desc").await;

        sqlx::query(r"INSERT INTO note_tag (note_id, tag_id) VALUES (?, ?)")
            .bind(note_id)
            .bind(source.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r"INSERT INTO note_tag (note_id, tag_id) VALUES (?, ?)")
            .bind(note_id)
            .bind(target.id)
            .execute(&pool)
            .await
            .unwrap();

        let _ = update_tag(
            &pool,
            UpdateTagRequest {
                tag_to_modify: TagSelector::Id(source.id),
                name: Some("target".to_string()),
                description: None,
                query: None,
                auto_delete: None,
                reschedule: None,
            },
            false,
        )
        .await
        .unwrap();

        let target_note_count: i64 =
            sqlx::query_scalar(r"SELECT COUNT(*) FROM note_tag WHERE note_id = ? AND tag_id = ?")
                .bind(note_id)
                .bind(target.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(target_note_count, 1, "no duplicate note_tag after merge");
    }

    async fn event_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM event")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn tag_create_logs_event_when_log_true(pool: SqlitePool) {
        let n_before = event_count(&pool).await;
        let _ = create_tag(
            &pool,
            CreateTagRequest {
                name: "logged".to_string(),
                description: String::new(),
                query: None,
                auto_delete: false,
                reschedule: true,
            },
            true,
        )
        .await
        .unwrap();
        assert_eq!(event_count(&pool).await, n_before + 1);
    }

    #[sqlx::test]
    async fn tag_create_does_not_log_when_log_false(pool: SqlitePool) {
        let n_before = event_count(&pool).await;
        let _ = create_tag(
            &pool,
            CreateTagRequest {
                name: "not_logged".to_string(),
                description: String::new(),
                query: None,
                auto_delete: false,
                reschedule: true,
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(event_count(&pool).await, n_before);
    }

    #[sqlx::test]
    async fn tag_update_logs_event_when_log_true(pool: SqlitePool) {
        let tag = create_tag_helper(&pool, "to_update", "desc").await;
        let n_before = event_count(&pool).await;
        let _ = update_tag(
            &pool,
            UpdateTagRequest {
                tag_to_modify: TagSelector::Id(tag.id),
                name: Some("updated".to_string()),
                description: None,
                query: None,
                auto_delete: None,
                reschedule: None,
            },
            true,
        )
        .await
        .unwrap();
        assert_eq!(event_count(&pool).await, n_before + 1);
    }

    async fn set_tag_timestamps(pool: &SqlitePool, id: TagId, ts: i64) {
        sqlx::query(r"UPDATE tag SET created_at = ?, updated_at = ? WHERE id = ?")
            .bind(ts)
            .bind(ts)
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[sqlx::test]
    async fn tag_timestamps_bump_on_update_and_rebuild_only(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        // Its filtered tag searches text, which needs `REGEXP`.
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "filtered".to_string(),
                description: String::new(),
                query: Some("dog".to_string()),
                auto_delete: false,
                reschedule: true,
            },
            false,
        )
        .await
        .unwrap();
        let old = 1_000_000;
        set_tag_timestamps(&pool, tag.id, old).await;

        rebuild_tag(&pool, tag.id).await.unwrap();
        let rebuilt = get_tag(&pool, tag.id).await.unwrap();
        assert_eq!(rebuilt.created_at.timestamp(), old);
        assert!(rebuilt.updated_at.timestamp() > old);

        set_tag_timestamps(&pool, tag.id, old).await;
        let updated = update_tag(
            &pool,
            UpdateTagRequest {
                tag_to_modify: TagSelector::Id(tag.id),
                name: None,
                description: Some("new".to_string()),
                query: None,
                auto_delete: None,
                reschedule: None,
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(updated.created_at.timestamp(), old);
        assert!(updated.updated_at.timestamp() > old);
        // The response reflects the stored row.
        let stored = get_tag(&pool, tag.id).await.unwrap();
        assert_eq!(stored.updated_at, updated.updated_at);
    }

    #[sqlx::test]
    async fn undo_delete_tag_restores_timestamps(pool: SqlitePool) {
        let tag = create_tag_helper(&pool, "restore_me", "desc").await;
        set_tag_timestamps(&pool, tag.id, 1_000_000).await;
        delete_tag(&pool, tag.id, true).await.unwrap();

        crate::api::undo::undo_event(
            &pool,
            crate::schema::undo::UndoEventRequest {
                event_id: None,
                undo_group: false,
            },
        )
        .await
        .unwrap();

        let restored = get_tag(&pool, tag.id).await.unwrap();
        assert_eq!(restored.created_at.timestamp(), 1_000_000);
        assert_eq!(restored.updated_at.timestamp(), 1_000_000);
    }

    #[sqlx::test]
    async fn create_tag_logs_stored_timestamps(pool: SqlitePool) {
        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "logged".to_string(),
                description: String::new(),
                query: None,
                auto_delete: false,
                reschedule: true,
            },
            true,
        )
        .await
        .unwrap();
        let payload: serde_json::Value =
            sqlx::query_scalar(r"SELECT payload FROM event ORDER BY id DESC LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let payload: CreateTagPayload = serde_json::from_value(payload).unwrap();
        assert_eq!(payload.created_at, tag.created_at);
        assert_eq!(payload.updated_at, tag.updated_at);
    }

    #[sqlx::test]
    async fn tag_delete_logs_event_when_log_true(pool: SqlitePool) {
        let tag = create_tag_helper(&pool, "to_delete", "desc").await;
        let n_before = event_count(&pool).await;
        delete_tag(&pool, tag.id, true).await.unwrap();
        assert_eq!(event_count(&pool).await, n_before + 1);
    }

    #[sqlx::test]
    async fn tag_delete_does_not_log_when_log_false(pool: SqlitePool) {
        let tag = create_tag_helper(&pool, "to_delete", "desc").await;
        let n_before = event_count(&pool).await;
        delete_tag(&pool, tag.id, false).await.unwrap();
        assert_eq!(event_count(&pool).await, n_before);
    }

    #[sqlx::test]
    async fn delete_tag_event_does_not_log_when_log_false(pool: SqlitePool) {
        let tag = create_tag_helper(&pool, "for_delete_event", "desc").await;
        let n_before = event_count(&pool).await;
        let payload = DeleteTagPayload {
            id: Some(tag.id),
            name: tag.name.clone(),
            description: tag.description.clone(),
            query: tag.query.clone(),
            auto_delete: tag.auto_delete,
            reschedule: tag.reschedule,
            created_at: tag.created_at,
            updated_at: tag.updated_at,
            note_ids: vec![],
            card_ids: vec![],
            review_log_ids: vec![],
        };
        delete_tag_event(&pool, payload, false).await.unwrap();
        assert_eq!(
            event_count(&pool).await,
            n_before,
            "apply_event path must not log"
        );
    }
}
