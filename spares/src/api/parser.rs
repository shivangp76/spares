use crate::{
    Error,
    model::Parser,
    schema::{
        FilterOptions,
        parser::{CreateParserRequest, ParserResponse, UpdateParserRequest},
    },
};
use sqlx::sqlite::SqlitePool;

const PARSERS_DEFAULT_LIMIT: usize = 100;

pub async fn create_parser(
    db: &SqlitePool,
    body: CreateParserRequest,
) -> Result<ParserResponse, Error> {
    let (id,): (i64,) = sqlx::query_as(r"INSERT INTO parser (name) VALUES (?) RETURNING id")
        .bind(body.name)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ParserResponse::new(&parser))
}

pub async fn get_parser(db: &SqlitePool, id: i64) -> Result<ParserResponse, Error> {
    let parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ParserResponse::new(&parser))
}

pub async fn update_parser(
    db: &SqlitePool,
    body: UpdateParserRequest,
    id: i64,
) -> Result<ParserResponse, Error> {
    let existing_parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    // Update (if empty, use old value)
    let _update_result = sqlx::query(r"UPDATE parser SET name = ? WHERE id = ?")
        .bind(
            body.name
                .clone()
                .unwrap_or_else(|| existing_parser.name.clone()),
        )
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let updated_item: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ParserResponse::new(&updated_item))
}

pub async fn delete_parser(db: &SqlitePool, id: i64) -> Result<(), Error> {
    let _query_result = sqlx::query(r"DELETE FROM parser WHERE id = ?")
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}

pub async fn list_parsers(
    db: &SqlitePool,
    opts: FilterOptions,
) -> Result<Vec<ParserResponse>, Error> {
    let limit = opts.limit.unwrap_or(PARSERS_DEFAULT_LIMIT);
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    let items = sqlx::query_as(r"SELECT * FROM parser ORDER by id LIMIT ? OFFSET ?")
        .bind(limit as u32)
        .bind(offset as u32)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let responses = items
        .iter()
        .map(ParserResponse::new)
        .collect::<Vec<ParserResponse>>();
    Ok(responses)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub async fn create_parser_helper(pool: &SqlitePool, parser_name: &str) -> ParserResponse {
        let request = CreateParserRequest {
            name: parser_name.to_string(),
        };
        let parser_res = create_parser(pool, request).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, parser_name);
        return parser;
    }

    #[sqlx::test]
    async fn test_create_parser(pool: SqlitePool) -> () {
        // Create parser
        let parser = create_parser_helper(&pool, "markdown").await;

        // Check database and verify item with id exists
        let parser_res: Result<Parser, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
                .bind(parser.id)
                .fetch_one(&pool)
                .await;
        dbg!(&parser_res);
        assert!(parser_res.is_ok());
        assert_eq!(parser_res.unwrap().name, "markdown");
    }

    #[sqlx::test]
    async fn test_get_parser(pool: SqlitePool) -> () {
        // Create parser
        let request = CreateParserRequest {
            name: "Parser to get".to_string(),
        };
        let parser_res = create_parser(&pool, request).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "Parser to get");

        // Get parser
        let id = parser.id;
        let parser_res = get_parser(&pool, id).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "Parser to get");
    }

    #[sqlx::test]
    async fn test_update_parser(pool: SqlitePool) -> () {
        // Create parser so it can be updated
        let request = CreateParserRequest {
            name: "To be updated".to_string(),
        };
        let parser_res = create_parser(&pool, request).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "To be updated");

        // Update parser
        let request = UpdateParserRequest {
            name: Some("Updated name".to_string()),
        };
        let id = parser.id;
        let parser_res = update_parser(&pool, request, id).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "Updated name");

        // Check database and verify item with id has the new property
        let parser_res: Result<Parser, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
                .bind(parser.id)
                .fetch_one(&pool)
                .await;
        assert!(parser_res.is_ok());
        assert_eq!(parser_res.unwrap().name, "Updated name");

        // Verify original value persists if field is not changed
        // Update parser
        let request = UpdateParserRequest { name: None };
        let id = parser.id;
        let parser_res = update_parser(&pool, request, id).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "Updated name");
    }

    #[sqlx::test]
    async fn test_delete_parser(pool: SqlitePool) -> () {
        // Create parser so it can be deleted
        let request = CreateParserRequest {
            name: "To be deleted".to_string(),
        };
        let parser_res = create_parser(&pool, request).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "To be deleted");

        // Delete parser
        let delete_res = delete_parser(&pool, parser.id).await;
        assert!(delete_res.is_ok());

        // Check database and verify item with id does not exist
        let parser_res: Result<Parser, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
                .bind(parser.id)
                .fetch_one(&pool)
                .await;
        assert!(parser_res.is_err());
        // Workaround since sqlx::Error does not derive PartialEq
        assert_eq!(
            format!("{:?}", parser_res.unwrap_err()),
            format!("{:?}", sqlx::Error::RowNotFound)
        );
    }

    #[sqlx::test]
    async fn test_list_parsers(pool: SqlitePool) -> () {
        // Create parsers
        let request = CreateParserRequest {
            name: "First parser to list".to_string(),
        };
        let parser_res = create_parser(&pool, request).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "First parser to list");

        let request = CreateParserRequest {
            name: "Second parser to list".to_string(),
        };
        let parser_res = create_parser(&pool, request).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "Second parser to list");

        // List parsers
        let parser_res = list_parsers(
            &pool,
            FilterOptions {
                page: None,
                limit: None,
            },
        )
        .await;
        assert!(parser_res.is_ok());
        let parsers = parser_res.unwrap();
        assert_eq!(parsers.len(), 2);
        assert_eq!(parsers.first().unwrap().name, "First parser to list");
        assert_eq!(parsers.last().unwrap().name, "Second parser to list");
    }
}
