//! Searches saved by name, which `spares saved-search` and the web app's notes and cards pages
//! list and run. They are kept by the server, so the CLI and the web app share them when they use
//! the same server. They are not synced between replicas.

use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::schema::saved_search::SavedSearch;
use crate::search::QueryReturnItemType;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

/// An error in a search being saved, which the server returns as a bad request.
fn invalid(message: &str) -> Error {
    LibraryError::Search {
        message: message.to_string(),
        span: None,
    }
    .into()
}

/// Every saved search, by name.
pub async fn list_saved_searches(db: &SqlitePool) -> Result<Vec<SavedSearch>, Error> {
    sqlx::query_as("SELECT name, query FROM saved_search ORDER BY name")
        .fetch_all(db)
        .await
        .map_err(sqlx_error)
}

/// The saved search called `name`, if there is one.
pub async fn get_saved_search(db: &SqlitePool, name: &str) -> Result<Option<SavedSearch>, Error> {
    sqlx::query_as("SELECT name, query FROM saved_search WHERE name = ?")
        .bind(name)
        .fetch_optional(db)
        .await
        .map_err(sqlx_error)
}

/// Saves `search`, replacing the query of a search with the same name. The name can't be empty
/// once spaces around it are left out, and the query must be one that can be searched.
pub async fn save_search(db: &SqlitePool, search: SavedSearch) -> Result<SavedSearch, Error> {
    let name = search.name.trim();
    if name.is_empty() {
        return Err(invalid("A saved search needs a name"));
    }
    let query = search.query.trim();
    if query.is_empty() {
        return Err(invalid("A saved search needs a query"));
    }
    // Checks the query, with the same error as searching it
    crate::search::explain(query, QueryReturnItemType::Notes)?;
    sqlx::query(
        "INSERT INTO saved_search (name, query) VALUES (?, ?)
         ON CONFLICT (name) DO UPDATE SET query = excluded.query, updated_at = strftime('%s', 'now')",
    )
    .bind(name)
    .bind(query)
    .execute(db)
    .await
    .map_err(sqlx_error)?;
    Ok(SavedSearch {
        name: name.to_string(),
        query: query.to_string(),
    })
}

/// Deletes the saved search called `name`, returning whether there was one.
pub async fn delete_saved_search(db: &SqlitePool, name: &str) -> Result<bool, Error> {
    let result = sqlx::query("DELETE FROM saved_search WHERE name = ?")
        .bind(name)
        .execute(db)
        .await
        .map_err(sqlx_error)?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(name: &str, query: &str) -> SavedSearch {
        SavedSearch {
            name: name.to_string(),
            query: query.to_string(),
        }
    }

    #[sqlx::test]
    async fn saves_lists_and_deletes(pool: SqlitePool) {
        save_search(&pool, search(" hard ", "tag=chess c.difficulty>7"))
            .await
            .unwrap();
        save_search(&pool, search("due", "c.due<=today"))
            .await
            .unwrap();
        // Saving under a name again replaces its query.
        save_search(&pool, search("hard", "c.difficulty>8"))
            .await
            .unwrap();
        assert_eq!(
            list_saved_searches(&pool).await.unwrap(),
            [
                search("due", "c.due<=today"),
                search("hard", "c.difficulty>8")
            ]
        );
        assert_eq!(
            get_saved_search(&pool, "hard").await.unwrap(),
            Some(search("hard", "c.difficulty>8"))
        );
        assert!(delete_saved_search(&pool, "due").await.unwrap());
        assert!(!delete_saved_search(&pool, "due").await.unwrap());
        assert_eq!(get_saved_search(&pool, "due").await.unwrap(), None);
    }

    #[sqlx::test]
    async fn rejects_what_cant_be_searched(pool: SqlitePool) {
        assert!(save_search(&pool, search(" ", "dog")).await.is_err());
        assert!(save_search(&pool, search("a", "")).await.is_err());
        let error = save_search(&pool, search("a", "tag=(math"))
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Library(LibraryError::Search { .. })),
            "{error}"
        );
        assert_eq!(list_saved_searches(&pool).await.unwrap(), Vec::new());
    }
}
