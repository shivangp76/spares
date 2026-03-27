use crate::{
    Error, LibraryError, TagErrorKind,
    api::undo::{
        insert_events,
        payloads::{CreateTagPayload, DeleteTagPayload, Transition, UpdateTagPayload},
    },
    model::{EventType, Tag, TagId},
    schema::{
        FilterOptions,
        tag::{CreateTagRequest, TagResponse, TagSelector, UpdateTagRequest},
    },
};
use chrono::Utc;
use serde_json::to_value;
use sqlx::sqlite::SqlitePool;

mod query;
pub use query::*;

const TAG_DEFAULT_LIMIT: usize = 100;
pub const DEFAULT_TAG_AUTO_DELETE: bool = true;

pub async fn create_tag(
    db: &SqlitePool,
    body: CreateTagRequest,
    log: bool,
) -> Result<TagResponse, Error> {
    let payload = CreateTagPayload {
        id: None,
        name: body.name,
        description: body.description,
        query: body.query,
        auto_delete: body.auto_delete,
    };
    create_tag_event(db, payload, log).await
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

    let id: i64 = if let Some(id) = payload.id {
        sqlx::query(
            r"INSERT INTO tag (id, name, description, query, auto_delete) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&payload.name)
        .bind(&payload.description)
        .bind(&payload.query)
        .bind(payload.auto_delete)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        id
    } else {
        sqlx::query_scalar(
            r"INSERT INTO tag (name, description, query, auto_delete) VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(&payload.name)
        .bind(&payload.description)
        .bind(&payload.query)
        .bind(payload.auto_delete)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?
    };
    let tag = Tag {
        id,
        name: payload.name.clone(),
        description: payload.description.clone(),
        query: payload.query.clone(),
        auto_delete: payload.auto_delete,
    };

    if let Some(ref query) = payload.query {
        // Execute query and add tag to all notes that match query
        tag_cards_from_query(db, query, tag.id).await?;
    }

    // Log event
    if log {
        let log_payload = CreateTagPayload {
            id: Some(tag.id),
            name: tag.name.clone(),
            description: tag.description.clone(),
            query: tag.query.clone(),
            auto_delete: tag.auto_delete,
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
        let existing_tag: Option<i64> = sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ?")
            .bind(name)
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if existing_tag.is_some() {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput("A tag with this name already exists.".to_string()),
            )));
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

    let _update_result = sqlx::query(
        r"UPDATE tag SET name = ?, description = ?, query = ?, auto_delete = ? WHERE id = ?",
    )
    .bind(&new_name)
    .bind(&new_description)
    .bind(&new_query)
    .bind(new_auto_delete)
    .bind(id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let updated_tag = Tag {
        id,
        name: new_name,
        description: new_description,
        query: new_query,
        auto_delete: new_auto_delete,
    };
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
    let payload = DeleteTagPayload {
        id: Some(tag.id),
        name: tag.name,
        description: tag.description,
        query: tag.query,
        auto_delete: tag.auto_delete,
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
    let offset = (opts.page.unwrap_or(1) - 1) * limit;
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
        let parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description").await;
        let parent_tag_id = parent_tag.id;

        // Create child tag
        let child_tag = create_tag_helper(&pool, "Child tag name", "Child tag description").await;

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
        let parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description").await;
        let parent_tag_id = parent_tag.id;

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

        // Updating tag with a duplicate name
        let request = UpdateTagRequest {
            tag_to_modify: TagSelector::Id(tag.id),
            name: Some("Parent tag name".to_string()),
            description: None,
            query: None,
            auto_delete: None,
        };
        let tag_res = update_tag(&pool, request, false).await;
        assert!(tag_res.is_err());
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
        };
        delete_tag_event(&pool, payload, false).await.unwrap();
        assert_eq!(
            event_count(&pool).await,
            n_before,
            "apply_event path must not log"
        );
    }
}
