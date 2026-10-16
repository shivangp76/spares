mod graphs;
mod local_time;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use chrono::Days;
use chrono::Duration;
use graphs::CardFilter;
use graphs::due_by_day;
pub use graphs::get_statistics_graphs;
use local_time::Zone;
use local_time::date_of_day_number;
use local_time::day_number;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::preset::cap_counts;
use crate::api::preset::limited_preset_sql;
use crate::api::queue::QueueCaps;
use crate::api::review::daily_limits_left_with;
use crate::config::read_external_config;
use crate::model::NEW_CARD_STATE;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::get_scheduler_from_string;
use crate::schema::review::ReviewHistoryRequest;
use crate::schema::review::ReviewHistoryResponse;
use crate::schema::review::StatisticsRequest;
use crate::schema::review::StatisticsResponse;

#[expect(clippy::too_many_lines)]
pub async fn get_statistics(
    db: &SqlitePool,
    request: StatisticsRequest,
) -> Result<StatisticsResponse, Error> {
    let StatisticsRequest {
        scheduler_name,
        date: requested_date,
        tz,
    } = request;
    let scheduler = get_scheduler_from_string(scheduler_name.as_str())?;
    let zone = Zone::parse(tz.as_deref())?;
    let today = zone.date(requested_date);
    let today_start = zone.start_of(today).timestamp();
    let tomorrow_start = zone.start_of(today + Days::new(1)).timestamp();

    // Only graded reviews count as study, crams apart. `card_id IS NOT NULL` excludes orphaned
    // rows, whose card has been deleted.
    let studied = async {
        sqlx::query_as::<_, (u32, i64, i64)>(&format!(
            r"SELECT COUNT(DISTINCT card_id), COALESCE(SUM(recall_duration), 0),
                     COALESCE(SUM(rate_duration), 0)
              FROM review_log
              WHERE reviewed_at >= {today_start} AND reviewed_at < {tomorrow_start}
                AND kind = {review} AND card_id IS NOT NULL",
            review = ReviewLogKind::Review as u8,
        ))
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
    };
    // Crams are counted apart from reviews, one per rating, and so are the minutes spent on them.
    let crams = async {
        sqlx::query_as::<_, (u32, i64)>(&format!(
            r"SELECT COUNT(*), COALESCE(SUM(recall_duration + rate_duration), 0) FROM review_log
              WHERE reviewed_at >= {today_start} AND reviewed_at < {tomorrow_start}
                AND kind = {cram} AND card_id IS NOT NULL",
            cram = ReviewLogKind::Cram as u8,
        ))
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
    };
    let cards_by_state = async {
        sqlx::query_as::<_, (StateId, u32)>(
            r"SELECT state, COUNT(*) as count FROM card GROUP BY state",
        )
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
    };

    // Cap new and review cards by what is left of today's limits, and of their presets', as
    // review does, so the counts are what is left to study today. See
    // `get_cards_left_by_state_and_time_estimate`.
    let config = read_external_config()?;
    let due_today = async {
        let limits_left = daily_limits_left_with(db, &config, requested_date).await?;
        // New cards are due from when they are created, and cards due before today are still
        // due, so a review backlog after a holiday is not counted as nothing due. Cards buried by
        // the user or the scheduler count tomorrow instead, and suspended cards not at all, as
        // review shows neither.
        let due_day = zone.clock(today_start, tomorrow_start).day("c.due");
        let counts: Vec<(StateId, Option<i64>, u32)> = sqlx::query_as(&format!(
            r"SELECT c.state, {preset} AS preset, COUNT(*) FROM card c
              WHERE (c.state = {NEW_CARD_STATE} OR {due_day} <= {today_number})
                AND (c.special_state IS NULL OR c.special_state = {later_today})
              GROUP BY c.state, preset",
            preset = limited_preset_sql(&config.queue),
            today_number = day_number(today),
            later_today = SpecialState::BuriedUntilLaterToday as u8,
        ))
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        Ok::<_, Error>(
            cap_counts(
                counts.into_iter().map(|(state, preset, count)| {
                    (
                        state,
                        preset.and_then(|p| usize::try_from(p).ok()),
                        count,
                        0.0,
                    )
                }),
                &QueueCaps::from_limits(&limits_left),
            )
            .into_iter()
            .map(|(state, (count, _))| (state, count))
            .collect::<HashMap<_, _>>(),
        )
    };
    let due_later = async {
        let due = due_by_day(db, zone, &CardFilter::All, today, None).await?;
        Ok::<_, Error>(
            due.into_iter()
                .filter(|(day, _)| *day > day_number(today))
                .map(|(day, (learning, review))| (date_of_day_number(day), learning + review))
                .filter(|(_, count)| *count > 0)
                .collect::<HashMap<_, _>>(),
        )
    };
    let (
        (cards_studied_count, recall_seconds, rate_seconds),
        (cram_count, cram_seconds),
        cards_by_state,
        due_count_by_state,
        due_count_by_date,
        advance_safe_count,
        postpone_safe_count,
    ) = futures::try_join!(
        studied,
        crams,
        cards_by_state,
        due_today,
        due_later,
        scheduler.get_advance_safe_count(db, requested_date),
        scheduler.get_postpone_safe_count(db, requested_date),
    )?;
    Ok(StatisticsResponse {
        cards_studied_count,
        recall_duration: Duration::seconds(recall_seconds),
        rate_duration: Duration::seconds(rate_seconds),
        cram_count,
        cram_duration: Duration::seconds(cram_seconds),
        card_count_by_state: cards_by_state.into_iter().collect(),
        due_count_by_state,
        due_count_by_date,
        advance_safe_count,
        postpone_safe_count,
    })
}

/// Counts the graded reviews, and apart from them the crams, on each local date, for a review
/// heatmap, optionally of the cards matching a query and in a client's time zone.
pub async fn get_review_history(
    db: &SqlitePool,
    request: ReviewHistoryRequest,
) -> Result<ReviewHistoryResponse, Error> {
    let zone = Zone::parse(request.tz.as_deref())?;
    let cards = CardFilter::new(db, request.query.as_deref()).await?;
    // Reviews of since-deleted cards still count without a query, as they were studied all the
    // same
    let (first, last): (Option<i64>, Option<i64>) =
        sqlx::query_as(r"SELECT MIN(reviewed_at), MAX(reviewed_at) FROM review_log")
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let clock = zone.clock(first.unwrap_or_default(), last.unwrap_or_default());
    let rows: Vec<(i64, ReviewLogKind, u32)> = cards
        .bind(sqlx::query_as(&format!(
            r"SELECT {day} AS day, kind, COUNT(*) FROM review_log
              WHERE kind IN ({review}, {cram}){filter} GROUP BY day, kind",
            day = clock.day("reviewed_at"),
            review = ReviewLogKind::Review as u8,
            cram = ReviewLogKind::Cram as u8,
            filter = cards.sql("card_id"),
        )))
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let mut response = ReviewHistoryResponse {
        review_count_by_date: HashMap::new(),
        cram_count_by_date: HashMap::new(),
    };
    for (day, kind, count) in rows {
        let counts = match kind {
            ReviewLogKind::Cram => &mut response.cram_count_by_date,
            _ => &mut response.review_count_by_date,
        };
        counts.insert(date_of_day_number(day), count);
    }
    Ok(response)
}
