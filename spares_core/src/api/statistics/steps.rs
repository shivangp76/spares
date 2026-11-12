//! `POST /api/statistics/steps`, over the cards of one preset. See
//! [`StepsStatsRequest`] and `schedulers::fsrs::steps_stats`.

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::api::preset::preset_sql;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::model::DEFAULT_DESIRED_RETENTION;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::schedulers::fsrs::steps_stats::MIN_REVIEWS;
use crate::schedulers::fsrs::steps_stats::steps_stats;
use crate::schedulers::fsrs::utils::decay;
use crate::schema::statistics::StepsStatsRequest;
use crate::schema::statistics::StepsStatsResponse;

pub async fn get_steps_stats(
    db: &SqlitePool,
    request: StepsStatsRequest,
    now: DateTime<Utc>,
) -> Result<StepsStatsResponse, Error> {
    let config = read_external_config()?;
    get_steps_stats_with(db, &config, request, now).await
}

pub(crate) async fn get_steps_stats_with(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    StepsStatsRequest {
        preset,
        period_days,
    }: StepsStatsRequest,
    now: DateTime<Utc>,
) -> Result<StepsStatsResponse, Error> {
    let presets = &config.queue.presets;
    let index = preset
        .as_deref()
        .map(|name| {
            presets.get_index_of(name).ok_or_else(|| {
                Error::Library(LibraryError::Scheduler(SchedulerErrorKind::InvalidInput(
                    format!(
                        "no preset is named `{name}`. The presets are: {}",
                        presets
                            .keys()
                            .map(|name| format!("`{name}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )))
            })
        })
        .transpose()?;
    // The cards whose preset is `index`, which only refers to `c`
    let cards = format!(
        "{} IS {}",
        preset_sql(presets),
        index.map_or_else(|| "NULL".to_string(), |index| index.to_string())
    );
    let fetch_logs = sqlx::query_as::<_, ReviewLog>(AssertSqlSafe(format!(
        "SELECT rl.* FROM review_log rl
         JOIN (SELECT c.id FROM card c WHERE {cards}) c ON c.id = rl.card_id
         WHERE rl.kind IN ({review}, {forget})
         ORDER BY rl.card_id, rl.reviewed_at ASC, rl.id ASC",
        review = ReviewLogKind::Review as u8,
        forget = ReviewLogKind::Forget as u8,
    )))
    .fetch_all(db);
    let fetch_retention = sqlx::query_scalar::<_, Option<f64>>(AssertSqlSafe(format!(
        "SELECT AVG(c.desired_retention) FROM card c WHERE {cards}"
    )))
    .fetch_one(db);
    let (review_logs, desired_retention) =
        futures::try_join!(fetch_logs, fetch_retention).map_err(|e| Error::Sqlx { source: e })?;
    let desired_retention = desired_retention.unwrap_or(DEFAULT_DESIRED_RETENTION);

    let since = period_days.map(|days| now - Duration::days(i64::from(days)));
    let (groups, recommended) = steps_stats(&review_logs, since, decay(config), desired_retention);
    let config = config.for_preset(index);
    let seconds = |steps: &[Duration]| steps.iter().map(Duration::num_seconds).collect();
    Ok(StepsStatsResponse {
        presets: presets.keys().cloned().collect(),
        desired_retention,
        min_reviews: MIN_REVIEWS,
        groups,
        learning_steps: seconds(&config.learning_steps),
        relearning_steps: seconds(&config.relearning_steps),
        recommended,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use indexmap::IndexMap;

    use super::*;
    use crate::api::queue::tests::create_cards;
    use crate::config::QueueConfig;
    use crate::config::QueuePreset;
    use crate::model::CardId;
    use crate::schema::statistics::StepsGroupKind;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap()
    }

    async fn review(
        pool: &SqlitePool,
        card_id: CardId,
        at: DateTime<Utc>,
        rating: u32,
        previous_state: u32,
    ) {
        sqlx::query(
            r"INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name,
              scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
              VALUES (?, ?, 0, ?, 'fsrs', 600, 5, 1, ?, '{}')",
        )
        .bind(card_id)
        .bind(at.timestamp())
        .bind(rating)
        .bind(previous_state)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn tag(pool: &SqlitePool, card_id: CardId, name: &str) {
        let tag_id: i64 = sqlx::query_scalar(
            "INSERT INTO tag (name, description, auto_delete) VALUES (?, '', 0) RETURNING id",
        )
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO note_tag (note_id, tag_id) SELECT note_id, ? FROM card WHERE id = ?",
        )
        .bind(tag_id)
        .bind(card_id)
        .execute(pool)
        .await
        .unwrap();
    }

    fn config() -> SparesExternalConfig {
        SparesExternalConfig {
            learning_steps: vec![Duration::minutes(1), Duration::minutes(10)],
            queue: QueueConfig {
                presets: IndexMap::from([(
                    "lang".to_string(),
                    QueuePreset {
                        tags: vec!["lang".to_string()],
                        learning_steps: Some(vec![Duration::minutes(5)]),
                        ..QueuePreset::default()
                    },
                )]),
                ..QueueConfig::default()
            },
            ..SparesExternalConfig::default()
        }
    }

    #[sqlx::test]
    async fn a_preset_is_fitted_with_its_own_cards_and_steps(pool: SqlitePool) {
        let [plain, lang, old]: [CardId; 3] =
            create_cards(&pool, 3, now()).await.try_into().unwrap();
        tag(&pool, lang, "lang:german").await;
        let start = now() - Duration::days(2);
        for card in [plain, lang] {
            review(&pool, card, start, 1, 0).await;
            review(&pool, card, start + Duration::minutes(10), 3, 1).await;
        }
        // First reviewed before the period
        review(&pool, old, now() - Duration::days(40), 1, 0).await;
        review(
            &pool,
            old,
            now() - Duration::days(40) + Duration::minutes(10),
            3,
            1,
        )
        .await;

        let config = config();
        let stats = |preset: Option<&str>, period_days| {
            get_steps_stats_with(
                &pool,
                &config,
                StepsStatsRequest {
                    preset: preset.map(str::to_string),
                    period_days,
                },
                now(),
            )
        };
        let again = |response: &StepsStatsResponse| {
            response
                .groups
                .iter()
                .find(|group| group.kind == StepsGroupKind::Again)
                .unwrap()
                .reviews
        };

        let top = stats(None, Some(30)).await.unwrap();
        assert_eq!(top.presets, ["lang"]);
        assert_eq!(again(&top), 1);
        assert_eq!(top.learning_steps, [60, 600]);
        assert!((top.desired_retention - 0.9).abs() < 1e-9);
        assert_eq!(again(&stats(None, None).await.unwrap()), 2);

        let lang = stats(Some("lang"), Some(30)).await.unwrap();
        assert_eq!(again(&lang), 1);
        assert_eq!(lang.learning_steps, [300]);
        assert_eq!(lang.groups.len(), 5);

        let error = stats(Some("exam"), None).await.unwrap_err().to_string();
        assert!(error.contains("`lang`"), "{error}");
    }
}
