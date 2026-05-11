use chrono::Utc;
use serde_json::to_value;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::CreateParserPayload;
use crate::api::undo::payloads::DeleteParserPayload;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateParserPayload;
use crate::model::EventType;
use crate::model::NoteId;
use crate::model::Parser;
use crate::schema::FilterOptions;
use crate::schema::parser::CreateParserRequest;
use crate::schema::parser::ParserResponse;
use crate::schema::parser::UpdateParserRequest;

const PARSERS_DEFAULT_LIMIT: usize = 100;

pub async fn create_parser(
    db: &SqlitePool,
    body: CreateParserRequest,
    log: bool,
) -> Result<ParserResponse, Error> {
    let payload = CreateParserPayload {
        id: None,
        name: body.name,
    };
    create_parser_event(db, payload, log).await
}

pub async fn create_parser_event(
    db: &SqlitePool,
    payload: CreateParserPayload,
    log: bool,
) -> Result<ParserResponse, Error> {
    let id: i64 = if let Some(id) = payload.id {
        sqlx::query(r"INSERT INTO parser (id, name) VALUES (?, ?)")
            .bind(id)
            .bind(&payload.name)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        id
    } else {
        sqlx::query_scalar(r"INSERT INTO parser (name) VALUES (?) RETURNING id")
            .bind(payload.name)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
    };
    let parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    let parser_response = ParserResponse::new(&parser);

    // Log event
    if log {
        let payload = CreateParserPayload {
            id: Some(parser.id),
            name: parser.name,
        };
        let _event_id = insert_events(
            db,
            &[(EventType::CreateParser, to_value(&payload).unwrap())],
            Utc::now(),
            None,
        )
        .await?;
    }

    Ok(parser_response)
}

pub async fn get_parser(db: &SqlitePool, id: i64) -> Result<ParserResponse, Error> {
    let parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ParserResponse::new(&parser))
}

pub(crate) async fn get_parser_name(db: &SqlitePool, id: i64) -> Result<String, Error> {
    let parser_name: String = sqlx::query_scalar(r"SELECT name FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(parser_name)
}

pub async fn update_parser_event(
    db: &SqlitePool,
    payload: UpdateParserPayload,
    id: i64,
    log: bool,
) -> Result<ParserResponse, Error> {
    let body = UpdateParserRequest {
        name: payload.name.map(|t| t.after),
    };
    update_parser(db, body, id, log).await
}

pub async fn update_parser(
    db: &SqlitePool,
    body: UpdateParserRequest,
    id: i64,
    log: bool,
) -> Result<ParserResponse, Error> {
    let existing_parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    // Update (if empty, use old value)
    let new_name = body
        .name
        .clone()
        .unwrap_or_else(|| existing_parser.name.clone());
    let _update_result = sqlx::query(r"UPDATE parser SET name = ? WHERE id = ?")
        .bind(&new_name)
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let updated_parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    let parser_response = ParserResponse::new(&updated_parser);

    // Log event
    if log {
        let payload = UpdateParserPayload {
            id,
            name: body.name.map(|_| Transition {
                before: existing_parser.name,
                after: updated_parser.name,
            }),
        };
        let _event_id = insert_events(
            db,
            &[(EventType::UpdateParser, to_value(&payload).unwrap())],
            Utc::now(),
            None,
        )
        .await?;
    }

    Ok(parser_response)
}

pub async fn delete_parser(db: &SqlitePool, id: i64, log: bool) -> Result<(), Error> {
    let parser: Parser = sqlx::query_as(r"SELECT * FROM parser WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let note_ids: Vec<NoteId> = sqlx::query_scalar(r"SELECT id FROM note WHERE parser_id = ?")
        .bind(id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let payload = DeleteParserPayload {
        id: Some(parser.id),
        name: parser.name,
        note_ids,
    };
    delete_parser_event(db, payload, log).await
}

pub async fn delete_parser_event(
    db: &SqlitePool,
    payload: DeleteParserPayload,
    log: bool,
) -> Result<(), Error> {
    let id = payload.id.ok_or_else(|| {
        Error::Library(LibraryError::InvalidConfig(
            "DeleteParserPayload missing id".to_string(),
        ))
    })?;
    sqlx::query(r"DELETE FROM parser WHERE id = ?")
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    if log {
        let _event_id = insert_events(
            db,
            &[(EventType::DeleteParser, to_value(&payload).unwrap())],
            Utc::now(),
            None,
        )
        .await?;
    }

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
        let parser_res = create_parser(pool, request, true).await;
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
        assert!(parser_res.is_ok());
        assert_eq!(parser_res.unwrap().name, "markdown");
    }

    #[sqlx::test]
    async fn test_get_parser(pool: SqlitePool) -> () {
        // Create parser
        let request = CreateParserRequest {
            name: "Parser to get".to_string(),
        };
        let parser_res = create_parser(&pool, request, true).await;
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
        let parser_res = create_parser(&pool, request, true).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "To be updated");

        // Update parser
        let request = UpdateParserRequest {
            name: Some("Updated name".to_string()),
        };
        let id = parser.id;
        let parser_res = update_parser(&pool, request, id, true).await;
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
        let parser_res = update_parser(&pool, request, id, true).await;
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
        let parser_res = create_parser(&pool, request, true).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "To be deleted");

        // Delete parser
        let delete_res = delete_parser(&pool, parser.id, true).await;
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
        let parser_res = create_parser(&pool, request, true).await;
        assert!(parser_res.is_ok());
        let parser = parser_res.unwrap();
        assert_eq!(parser.name, "First parser to list");

        let request = CreateParserRequest {
            name: "Second parser to list".to_string(),
        };
        let parser_res = create_parser(&pool, request, true).await;
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

    /// Count rows in the event table (for testing that log = true/false is respected).
    async fn event_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM event")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn parser_create_logs_event_when_log_true(pool: SqlitePool) {
        let n_before = event_count(&pool).await;
        let _ = create_parser(
            &pool,
            CreateParserRequest {
                name: "logged".to_string(),
            },
            true,
        )
        .await
        .unwrap();
        assert_eq!(event_count(&pool).await, n_before + 1);
    }

    #[sqlx::test]
    async fn parser_create_does_not_log_when_log_false(pool: SqlitePool) {
        let n_before = event_count(&pool).await;
        let _ = create_parser(
            &pool,
            CreateParserRequest {
                name: "not_logged".to_string(),
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(event_count(&pool).await, n_before);
    }

    #[sqlx::test]
    async fn parser_update_logs_event_when_log_true(pool: SqlitePool) {
        let p = create_parser_helper(&pool, "to_update").await;
        let n_before = event_count(&pool).await;
        let _ = update_parser(
            &pool,
            UpdateParserRequest {
                name: Some("updated".to_string()),
            },
            p.id,
            true,
        )
        .await
        .unwrap();
        assert_eq!(event_count(&pool).await, n_before + 1);
    }

    #[sqlx::test]
    async fn parser_update_does_not_log_when_log_false(pool: SqlitePool) {
        let p = create_parser_helper(&pool, "to_update").await;
        let n_before = event_count(&pool).await;
        let _ = update_parser(&pool, UpdateParserRequest { name: None }, p.id, false)
            .await
            .unwrap();
        assert_eq!(event_count(&pool).await, n_before);
    }

    #[sqlx::test]
    async fn parser_delete_logs_event_when_log_true(pool: SqlitePool) {
        let p = create_parser_helper(&pool, "to_delete").await;
        let n_before = event_count(&pool).await;
        delete_parser(&pool, p.id, true).await.unwrap();
        assert_eq!(event_count(&pool).await, n_before + 1);
    }

    #[sqlx::test]
    async fn parser_delete_does_not_log_when_log_false(pool: SqlitePool) {
        let p = create_parser_helper(&pool, "to_delete").await;
        let n_before = event_count(&pool).await;
        delete_parser(&pool, p.id, false).await.unwrap();
        assert_eq!(event_count(&pool).await, n_before);
    }

    #[sqlx::test]
    async fn delete_parser_event_does_not_log_when_log_false(pool: SqlitePool) {
        let p = create_parser_helper(&pool, "for_delete_event").await;
        let n_before = event_count(&pool).await;
        let payload = DeleteParserPayload {
            id: Some(p.id),
            name: p.name.clone(),
            note_ids: vec![],
        };
        delete_parser_event(&pool, payload, false).await.unwrap();
        assert_eq!(
            event_count(&pool).await,
            n_before,
            "apply_event path must not log"
        );
    }
}
