use chrono::Utc;
use serde_json::to_value;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::TagErrorKind;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::DeleteTagPayload;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateTagPayload;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::NoteId;
use crate::model::Tag;
use crate::model::TagId;
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
        created_at: now,
        updated_at: now,
        note_ids: vec![],
        card_ids: vec![],
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
    sqlx::query_as(
        r"INSERT INTO tag (name, description, query, auto_delete) VALUES (?, '', NULL, ?) RETURNING *",
    )
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

    // A NULL `id` makes SQLite assign one. Undoing a `DeleteTag` supplies it, along with the
    // original timestamps, so the tag comes back exactly as it was.
    let tag: Tag = sqlx::query_as(
        r"INSERT INTO tag (id, name, description, query, auto_delete, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        RETURNING *",
    )
    .bind(payload.id)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&payload.query)
    .bind(payload.auto_delete)
    .bind(payload.created_at.timestamp())
    .bind(payload.updated_at.timestamp())
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    if let Some(ref query) = payload.query {
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

    // Log event
    if log {
        let log_payload = CreateTagPayload {
            id: Some(tag.id),
            name: tag.name.clone(),
            description: tag.description.clone(),
            query: tag.query.clone(),
            auto_delete: tag.auto_delete,
            created_at: tag.created_at,
            updated_at: tag.updated_at,
            note_ids: payload.note_ids.clone(),
            card_ids: payload.card_ids.clone(),
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
    };
    update_tag(db, body, log).await
}

async fn merge_tag_into(
    db: &SqlitePool,
    source_id: TagId,
    source_tag: &Tag,
    target_id: TagId,
    log: bool,
) -> Result<TagResponse, Error> {
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
        created_at: source_tag.created_at,
        updated_at: source_tag.updated_at,
        note_ids,
        card_ids,
    };
    delete_tag_event(db, delete_payload, log).await?;

    get_tag(db, target_id).await
}

pub async fn update_tag(
    db: &SqlitePool,
    body: UpdateTagRequest,
    log: bool,
) -> Result<TagResponse, Error> {
    let id = tag_selector_to_id(db, body.tag_to_modify).await?;
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
        r"UPDATE tag SET name = ?, description = ?, query = ?, auto_delete = ?, updated_at = strftime('%s', 'now') WHERE id = ? RETURNING *",
    )
    .bind(&new_name)
    .bind(&new_description)
    .bind(&new_query)
    .bind(new_auto_delete)
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
        };
        let _event_id = insert_events(
            db,
            &[(EventType::UpdateTag, to_value(&payload).unwrap())],
            Utc::now(),
            None,
        )
        .await?;
    }

    Ok(tag_response)
}

pub async fn delete_tag(db: &SqlitePool, id: i64, log: bool) -> Result<(), Error> {
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
        created_at: tag.created_at,
        updated_at: tag.updated_at,
        note_ids,
        card_ids,
    };
    delete_tag_event(db, payload, log).await
}

pub async fn delete_tag_event(
    db: &SqlitePool,
    payload: DeleteTagPayload,
    log: bool,
) -> Result<(), Error> {
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

    if log {
        let _event_id = insert_events(
            db,
            &[(EventType::DeleteTag, to_value(&payload).unwrap())],
            Utc::now(),
            None,
        )
        .await?;
    }

    Ok(())
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
        let ts = Utc::now().timestamp();
        let parser = crate::api::parser::tests::create_parser_helper(&pool, "markdown").await;
        let note_id: i64 = sqlx::query_scalar(
            r"INSERT INTO note (data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind("n1")
        .bind(ts)
        .bind(ts)
        .bind(parser.id)
        .bind(serde_json::json!({}).to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

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
        let ts = Utc::now().timestamp();
        let parser = crate::api::parser::tests::create_parser_helper(&pool, "markdown").await;
        let note_id: i64 = sqlx::query_scalar(
            r"INSERT INTO note (data, created_at, updated_at, parser_id, custom_data) VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind("n1")
        .bind(ts)
        .bind(ts)
        .bind(parser.id)
        .bind(serde_json::json!({}).to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

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
    async fn tag_timestamps_bump_on_update_and_rebuild_only(pool: SqlitePool) {
        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "filtered".to_string(),
                description: String::new(),
                query: Some("dog".to_string()),
                auto_delete: false,
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
            created_at: tag.created_at,
            updated_at: tag.updated_at,
            note_ids: vec![],
            card_ids: vec![],
        };
        delete_tag_event(&pool, payload, false).await.unwrap();
        assert_eq!(
            event_count(&pool).await,
            n_before,
            "apply_event path must not log"
        );
    }
}
