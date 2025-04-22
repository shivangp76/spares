use crate::{
    Error, LibraryError, TagErrorKind,
    model::Tag,
    schema::{
        FilterOptions,
        tag::{CreateTagRequest, TagResponse, UpdateTagRequest},
    },
};
use sqlx::sqlite::SqlitePool;

mod query;
pub use query::*;

const TAG_DEFAULT_LIMIT: usize = 100;
pub const DEFAULT_TAG_AUTO_DELETE: bool = true;

pub async fn create_tag(db: &SqlitePool, body: CreateTagRequest) -> Result<TagResponse, Error> {
    // First, check if a tag with the same name already exists
    // This is enforced manually instead of setting the primary key of the table to `tag.name` so this restriction can be removed in the future, if desired.
    let existing_tag: Option<(i64,)> = sqlx::query_as(r"SELECT id FROM tag WHERE name = ?")
        .bind(&body.name)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    if existing_tag.is_some() {
        return Err(Error::Library(LibraryError::Tag(
            TagErrorKind::InvalidInput("A tag with this name already exists.".to_string()),
        )));
    }

    if let Some(ref query) = body.query {
        verify_filtered_tag_query(db, query.as_str()).await?;
    }

    let (id,): (i64,) = sqlx::query_as(
        r"INSERT INTO tag (name, description, parent_id, query, auto_delete) VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(body.parent_id)
    .bind(&body.query)
    .bind(body.auto_delete)
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    if let Some(parent_id) = body.parent_id {
        if id == parent_id {
            let _ = delete_tag(db, id).await;
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput(
                    "Cannot insert tag whose parent id is itself.".to_string(),
                ),
            )));
        }
    }
    let tag = Tag {
        id,
        name: body.name.clone(),
        description: body.description.clone(),
        parent_id: body.parent_id,
        query: body.query.clone(),
        auto_delete: body.auto_delete,
    };

    if let Some(ref query) = body.query {
        // Execute query and add tag to all notes that match query
        tag_cards_from_query(db, query, tag.id).await?;
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

pub async fn update_tag(
    db: &SqlitePool,
    body: UpdateTagRequest,
    id: i64,
) -> Result<TagResponse, Error> {
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
    let new_parent_id = body.parent_id.unwrap_or(existing_tag.parent_id);
    let new_query = body
        .query
        .clone()
        .unwrap_or_else(|| existing_tag.query.clone());
    let new_auto_delete = body.auto_delete.unwrap_or(existing_tag.auto_delete);
    if body.name.is_some() {
        let existing_tag: Option<(i64,)> = sqlx::query_as(r"SELECT id FROM tag WHERE name = ?")
            .bind(body.name.unwrap())
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if existing_tag.is_some() {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput("A tag with this name already exists.".to_string()),
            )));
        }
    }

    if let Some(query) = body.query.flatten() {
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
        r"UPDATE tag SET name = ?, description = ?, parent_id = ?, query = ?, auto_delete = ? WHERE id = ?",
    )
    .bind(&new_name)
    .bind(&new_description)
    .bind(new_parent_id)
    .bind(&new_query)
    .bind(new_auto_delete)
    .bind(id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let updated_item = Tag {
        id,
        name: new_name,
        parent_id: new_parent_id,
        description: new_description,
        query: new_query,
        auto_delete: new_auto_delete,
    };
    Ok(TagResponse::new(&updated_item))
}

pub async fn delete_tag(db: &SqlitePool, id: i64) -> Result<(), Error> {
    let _query_result = sqlx::query(r"DELETE FROM tag WHERE id = ?")
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
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
mod tests {
    use super::*;

    async fn create_tag_helper(
        pool: &SqlitePool,
        name: &str,
        description: &str,
        parent_id: &Option<i64>,
    ) -> TagResponse {
        let request = CreateTagRequest {
            name: name.to_string(),
            description: description.to_string(),
            parent_id: *parent_id,
            query: None,
            auto_delete: false,
        };
        let tag_res = create_tag(pool, request).await;
        assert!(tag_res.is_ok());
        if let Ok(tag) = tag_res {
            assert_eq!(tag.name, name);
            assert_eq!(tag.description, description);
            assert_eq!(tag.parent_id, *parent_id);

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
                assert_eq!(tag.parent_id, *parent_id);
            }
            return tag;
        }
        unreachable!();
    }

    #[sqlx::test]
    async fn test_create_tag(pool: SqlitePool) -> () {
        // Create parent tag
        let parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description", &None).await;
        let parent_tag_id = parent_tag.id;

        // Create child tag
        let child_tag = create_tag_helper(
            &pool,
            "Child tag name",
            "Child tag description",
            &Some(parent_tag_id),
        )
        .await;

        // Create tag with invalid parent_id
        let request = CreateTagRequest {
            name: "Invalid tag".to_string(),
            description: String::new(),
            parent_id: Some((parent_tag_id + child_tag.id) * 200),
            query: None,
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_err());

        // Create tag with duplicate name
        let request = CreateTagRequest {
            name: "Child tag name".to_string(),
            description: String::new(),
            parent_id: None,
            query: None,
            auto_delete: false,
        };
        let tag_res = create_tag(&pool, request).await;
        assert!(tag_res.is_err());
    }

    #[sqlx::test]
    async fn test_get_tag(pool: SqlitePool) -> () {
        // Create parent tag
        let parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description", &None).await;
        let parent_tag_id = parent_tag.id;

        // Create child tag
        let child_tag = create_tag_helper(
            &pool,
            "Child tag name",
            "Child tag description",
            &Some(parent_tag_id),
        )
        .await;

        let tag_res = get_tag(&pool, child_tag.id).await;
        if let Ok(tag) = tag_res {
            assert_eq!(tag.name, "Child tag name");
            assert_eq!(tag.description, "Child tag description");
            assert_eq!(tag.parent_id, Some(parent_tag_id));
        }
    }

    #[sqlx::test]
    async fn test_update_tag(pool: SqlitePool) -> () {
        // Create parent tag
        let parent_tag =
            create_tag_helper(&pool, "Parent tag name", "Parent tag description", &None).await;
        let parent_tag_id = parent_tag.id;

        // Create tag so it can be updated
        let tag = create_tag_helper(
            &pool,
            "Child tag name",
            "Child tag description",
            &Some(parent_tag_id),
        )
        .await;

        // Update tag
        let request = UpdateTagRequest {
            name: Some("Updated name".to_string()),
            description: None,
            parent_id: Some(None),
            query: None,
            auto_delete: None,
        };
        let tag_res = update_tag(&pool, request, tag.id).await;
        assert!(tag_res.is_ok());
        if let Ok(tag) = tag_res {
            assert_eq!(tag.name, "Updated name");
            assert_eq!(tag.description, "Child tag description");
            assert_eq!(tag.parent_id, None);
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
            assert_eq!(tag.parent_id, None);
        }

        // Updating tag with a duplicate name
        let request = UpdateTagRequest {
            name: Some("Parent tag name".to_string()),
            description: None,
            parent_id: None,
            query: None,
            auto_delete: None,
        };
        let tag_res = update_tag(&pool, request, tag.id).await;
        assert!(tag_res.is_err());
    }

    #[sqlx::test]
    async fn test_delete_tag(pool: SqlitePool) -> () {
        // Create tag so it can be deleted
        let tag = create_tag_helper(&pool, "Tag name", "Tag description", &None).await;

        // Delete tag
        let delete_tag_res = delete_tag(&pool, tag.id).await;
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
        let _tag1 = create_tag_helper(&pool, "Tag 1 name", "Tag 1 description", &None).await;
        let _tag2 = create_tag_helper(&pool, "Tag 2 name", "Tag 2 description", &None).await;

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
}
