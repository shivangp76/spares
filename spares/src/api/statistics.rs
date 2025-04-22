use crate::{
    Error,
    config::read_external_config,
    helpers::get_start_end_local_date,
    model::{Card, NEW_CARD_STATE, SpecialState, StateId},
    schedulers::get_scheduler_from_string,
    schema::review::{StatisticsRequest, StatisticsResponse},
};
use chrono::{Duration, Local, Utc};
use itertools::Itertools;
use sqlx::sqlite::SqlitePool;
use std::collections::HashMap;

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::too_many_lines, reason = "off by a few")]
pub async fn get_statistics(
    db: &SqlitePool,
    request: StatisticsRequest,
) -> Result<StatisticsResponse, Error> {
    let StatisticsRequest {
        scheduler_name,
        date: requested_date,
    } = request;
    let scheduler = get_scheduler_from_string(scheduler_name.as_str())?;

    // Get cards reviewed on `requested_date`
    let (lower_limit, upper_limit) = get_start_end_local_date(&requested_date);
    let cards_studied_on_requested_date: Vec<(i64, i64, StateId)> = sqlx::query_as(
        r"SELECT card_id, duration, previous_state FROM review_log WHERE reviewed_at >= ? AND reviewed_at <= ?",
    )
    .bind(lower_limit.timestamp())
    .bind(upper_limit.timestamp())
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let study_time = cards_studied_on_requested_date
        .iter()
        .map(|(_, duration, _)| Duration::seconds(*duration))
        .sum();
    let cards_studied_on_requested_date_unique = cards_studied_on_requested_date
        .iter()
        .unique_by(|(card_id, _, _)| card_id)
        .collect::<Vec<_>>();
    let new_cards_studied_on_requested_date = cards_studied_on_requested_date_unique
        .iter()
        .filter(|(_, _, state)| *state == NEW_CARD_STATE)
        .count();
    let cards_studied_on_requested_date_count = cards_studied_on_requested_date_unique.len() as u32;

    // Get cards by state
    let cards_by_state_vec: Vec<(StateId, u32)> =
        sqlx::query_as(r"SELECT state, COUNT(*) as count FROM card GROUP BY state")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let cards_by_state = cards_by_state_vec.into_iter().collect::<HashMap<_, _>>();

    // Get all cards
    let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let (new_cards, not_new_cards): (Vec<_>, Vec<_>) = cards
        .into_iter()
        .partition(|card| card.state == NEW_CARD_STATE);
    let mut cards_by_due = not_new_cards
        .into_iter()
        .map(|card| (card.due.with_timezone(&Local).date_naive(), card))
        .into_group_map();
    // Move all new cards from dates before `requested_date` to `requested_date`
    cards_by_due
        .entry(requested_date.with_timezone(&Local).date_naive())
        .and_modify(|v| v.extend(new_cards.clone()))
        .or_insert(new_cards);

    // Move buried cards to the next day
    if let Some(cards_due) = cards_by_due.get_mut(&Utc::now().with_timezone(&Local).date_naive()) {
        let (buried_cards, not_buried_cards): (Vec<_>, Vec<_>) =
            cards_due.clone().into_iter().partition(|card| {
                card.special_state == Some(SpecialState::UserBuried)
                    || card.special_state == Some(SpecialState::SchedulerBuried)
            });
        *cards_due = not_buried_cards;
        cards_by_due
            .entry(
                (requested_date + Duration::days(1))
                    .with_timezone(&Local)
                    .date_naive(),
            )
            .and_modify(|v| v.extend(buried_cards.clone()))
            .or_insert(buried_cards);
    }

    // Group cards due on `requested_date` by state
    let mut num_cards_due_by_state = cards_by_due
        .get(&requested_date.with_timezone(&Local).date_naive())
        .map(|cards| {
            cards
                .iter()
                .map(|card| (card.state, card))
                .into_group_map()
                .into_iter()
                .map(|(state, cards)| (state, cards.len() as u32))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    // Decrease new cards by the number of new cards already reviewed on `requested_date`
    if let Some(new_cards_count) = num_cards_due_by_state.get_mut(&NEW_CARD_STATE) {
        let config = read_external_config()?;
        let capped_new_cards = i64::from((*new_cards_count).min(config.new_cards_daily_limit));
        *new_cards_count =
            u32::try_from((capped_new_cards - (new_cards_studied_on_requested_date as i64)).max(0))
                .unwrap();
    }
    num_cards_due_by_state.retain(|_, &mut v| v != 0);

    let future_due = cards_by_due
        .into_iter()
        .filter(|(date, cards)| {
            !cards.is_empty() && *date > requested_date.with_timezone(&Local).date_naive()
        })
        .map(|(date, cards)| (date, cards.len() as u32))
        .collect::<HashMap<_, _>>();

    let advance_safe_count = scheduler.get_advance_safe_count(db, requested_date).await?;
    let postpone_safe_count = scheduler
        .get_postpone_safe_count(db, requested_date)
        .await?;
    Ok(StatisticsResponse {
        cards_studied_count: cards_studied_on_requested_date_count,
        study_time,
        card_count_by_state: cards_by_state,
        due_count_by_state: num_cards_due_by_state,
        due_count_by_date: future_due,
        advance_safe_count,
        postpone_safe_count,
    })
}
