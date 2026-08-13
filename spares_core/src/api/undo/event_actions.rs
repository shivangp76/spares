use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::api::fetch_batched_query;
use crate::api::max_rows_for;
use crate::api::placeholders_2d;
use crate::api::undo::EVENT_VERSION;
use crate::model::EventType;

pub async fn insert_events(
    db: &SqlitePool,
    events: &[(EventType, Value)],
    at: DateTime<Utc>,
    group_id: Option<i64>,
) -> Result<Vec<i64>, Error> {
    let event_ids: Vec<i64> = fetch_batched_query(db, events, max_rows_for(5), async |db, chunk| {
        let query_str = format!(
            "INSERT INTO event (kind, created_at, version, group_id, payload) VALUES {} RETURNING id",
            placeholders_2d(chunk.len(), 5)
        );
        let mut query = sqlx::query_scalar(&query_str);
        for (kind, payload) in chunk {
            query = query.bind(kind);
            query = query.bind(at.timestamp());
            query = query.bind(EVENT_VERSION);
            query = query.bind(group_id);
            query = query.bind(payload);
        }
        query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })
    })
    .await?;
    Ok(event_ids)
}

/// Create a group of events. Returns the `group_id` (which is the id of the first event).
pub async fn create_event_group(
    db: &SqlitePool,
    events: Vec<(EventType, Value)>,
    at: DateTime<Utc>,
) -> Result<i64, Error> {
    if events.is_empty() {
        return Err(Error::Library(LibraryError::InvalidConfig(
            "Cannot create an empty event group".to_string(),
        )));
    }

    // Insert first event to get group_id
    let (first_kind, first_payload) = events.first().unwrap();
    let group_id_vec = insert_events(db, &[(*first_kind, first_payload.clone())], at, None).await?;
    let group_id = *group_id_vec.first().unwrap();

    // Set the first event's group_id to its own id so it is included in group queries
    sqlx::query(r"UPDATE event SET group_id = ? WHERE id = ?")
        .bind(group_id)
        .bind(group_id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    // Insert remaining events with the same group_id
    for (kind, payload) in events.iter().skip(1) {
        insert_events(db, &[(*kind, payload.clone())], at, Some(group_id)).await?;
    }
    Ok(group_id)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use sqlx::SqlitePool;

    use super::*;
    use crate::model::EventType;

    #[sqlx::test]
    async fn insert_events_returns_ids_in_order(pool: SqlitePool) {
        let at = Utc::now();
        let events = [
            (EventType::CreateParser, json!({"name": "p1"})),
            (EventType::CreateParser, json!({"name": "p2"})),
        ];
        let ids = insert_events(&pool, &events, at, None).await.unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids[0] < ids[1]);

        let row: (i64,) = sqlx::query_as("SELECT id FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, ids[0]);
    }

    #[sqlx::test]
    async fn insert_events_stores_group_id(pool: SqlitePool) {
        let at = Utc::now();
        let events = [(EventType::CreateParser, json!({"name": "p1"}))];
        let group_id = 42_i64;
        let ids = insert_events(&pool, &events, at, Some(group_id))
            .await
            .unwrap();
        assert_eq!(ids.len(), 1);

        let row: (Option<i64>,) = sqlx::query_as("SELECT group_id FROM event WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, Some(group_id));
    }

    #[sqlx::test]
    async fn create_event_group_sets_same_group_id_on_all_events(pool: SqlitePool) {
        let at = Utc::now();
        let events = vec![
            (EventType::CreateParser, json!({"name": "p1"})),
            (EventType::CreateParser, json!({"name": "p2"})),
            (EventType::CreateParser, json!({"name": "p3"})),
        ];
        let group_id = create_event_group(&pool, events, at).await.unwrap();

        let rows: Vec<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT id, group_id FROM event WHERE group_id = ? OR id = ? ORDER BY id",
        )
        .bind(group_id)
        .bind(group_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 3, "all three events must belong to the group");
        for (id, gid) in &rows {
            assert_eq!(
                *gid,
                Some(group_id),
                "each event must have group_id = {}",
                group_id
            );
            assert!(*id >= group_id);
        }
        assert_eq!(rows[0].0, group_id, "first event id must be the group_id");
    }

    #[sqlx::test]
    async fn create_event_group_first_event_has_group_id_self(pool: SqlitePool) {
        let at = Utc::now();
        let events = vec![
            (EventType::CreateParser, json!({"name": "p1"})),
            (EventType::CreateParser, json!({"name": "p2"})),
        ];
        let group_id = create_event_group(&pool, events, at).await.unwrap();

        let first_group_id: Option<i64> =
            sqlx::query_scalar("SELECT group_id FROM event WHERE id = ?")
                .bind(group_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            first_group_id,
            Some(group_id),
            "first event's group_id must equal its own id so group queries include it"
        );
    }

    #[sqlx::test]
    async fn create_event_group_empty_returns_error(pool: SqlitePool) {
        let at = Utc::now();
        let err = create_event_group(&pool, vec![], at).await.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("empty"),
            "expected error for empty group: {}",
            msg
        );
    }
}
