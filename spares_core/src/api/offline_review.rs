//! Reviewing on a web client without a connection to the server.
//!
//! While online, the client downloads the cards a review would show next ([`get_review_queue`]).
//! Offline, it shows those and records what was done with them, then submits the actions once it
//! is back online ([`sync_offline_actions`]).
//!
//! The review log is what makes this safe. A card's memory state is a replay of its log, so a
//! review that arrives after later reviews of the same card (e.g. from another device that synced
//! first) is inserted at its place in the history and the card is replayed, rather than scheduled
//! as if it happened last.

use chrono::DateTime;
use chrono::Duration;
use chrono::Local;
use chrono::Utc;
use indoc::formatdoc;
use sqlx::FromRow;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::fetch_review_logs_for_replay;
use crate::api::review::REVIEW_CARD_COLUMNS;
use crate::api::review::ReviewCard;
use crate::api::review::ReviewFilterQuery;
use crate::api::review::build_review_filter_query;
use crate::api::review::bury_card;
use crate::api::review::get_cards_left_by_state_and_time_estimate;
use crate::api::review::insert_review;
use crate::api::review::new_cards_left;
use crate::api::review::rate_card;
use crate::api::review::review_card_response;
use crate::api::review::unbury_cards_and_update_config;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::model::Card;
use crate::model::NEW_CARD_STATE;
use crate::model::ReviewLog;
use crate::model::SpecialState;
use crate::parsers::Parseable;
use crate::schedulers::SrsScheduler;
use crate::schedulers::effective_review_logs;
use crate::schedulers::get_scheduler_from_string;
use crate::schema::review::OfflineActionOutcome;
use crate::schema::review::OfflineActionResult;
use crate::schema::review::OfflineStudyAction;
use crate::schema::review::OfflineStudyActionKind;
use crate::schema::review::RatingSubmission;
use crate::schema::review::ReviewQueueCard;
use crate::schema::review::ReviewQueueRequest;
use crate::schema::review::ReviewQueueResponse;
use crate::schema::review::SyncOfflineActionsRequest;
use crate::schema::review::SyncOfflineActionsResponse;

#[derive(FromRow)]
struct QueueRow {
    #[sqlx(flatten)]
    card: ReviewCard,
    due: DateTime<Utc>,
}

/// The cards a review with `filter` would show next, in order, as of `requested_date`.
///
/// Like [`get_review_card`](crate::api::review::get_review_card), cards are those due by the end
/// of the day, plus those due on the next `days_ahead` days. New cards are capped by what is left
/// of today's limit, since the client cannot know tomorrow's.
pub async fn get_review_queue(
    db: &SqlitePool,
    ReviewQueueRequest {
        filter,
        limit,
        days_ahead,
    }: ReviewQueueRequest,
    requested_date: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<ReviewQueueResponse, Error> {
    unbury_cards_and_update_config(db).await?;

    let new_cards_left_today = new_cards_left(db, requested_date).await?;
    let Some(ReviewFilterQuery {
        where_clause,
        is_filtered_tag,
        card_due_limit,
    }) = build_review_filter_query(db, filter.as_ref(), requested_date).await?
    else {
        return Ok(ReviewQueueResponse {
            cards: Vec::new(),
            new_cards_left_today,
        });
    };

    // Ordered like `get_review_card`. There is no `LIMIT` since new cards beyond today's limit are
    // skipped below.
    let query_str = formatdoc! {"
        SELECT {REVIEW_CARD_COLUMNS}, c.due as due
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE {where_clause}
        ORDER BY
            CASE WHEN c.special_state IS NULL THEN 0 ELSE 1 END ASC,
            c.due ASC,
            n.created_at ASC"
    };
    let mut query = sqlx::query_as(&query_str);
    if !is_filtered_tag {
        query = query.bind((card_due_limit + Duration::days(i64::from(days_ahead))).timestamp());
    }
    let rows: Vec<QueueRow> = query
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    let (cards_left_by_state, time_estimate) = get_cards_left_by_state_and_time_estimate(
        db,
        &where_clause,
        card_due_limit,
        is_filtered_tag,
    )
    .await?;

    let mut new_cards_allowed = new_cards_left_today;
    let mut cards = Vec::new();
    for QueueRow { card, due } in rows {
        if cards.len() >= limit as usize {
            break;
        }
        if card.card_state == NEW_CARD_STATE {
            if new_cards_allowed == 0 {
                continue;
            }
            new_cards_allowed -= 1;
        }
        let card = review_card_response(
            db,
            card,
            cards_left_by_state.clone(),
            time_estimate,
            all_parsers,
        )
        .await?;
        cards.push(ReviewQueueCard { due, card });
    }
    Ok(ReviewQueueResponse {
        cards,
        new_cards_left_today,
    })
}

/// Applies actions performed offline, in the order they were performed.
///
/// Clients send their online actions this way too, so an action whose response was lost can be
/// resent without applying it twice.
///
/// An action that cannot be applied, e.g. because its card has since been deleted, is reported
/// as [`OfflineActionOutcome::Rejected`] rather than failing the batch, so the client can drop it.
/// Other errors fail the request. The actions applied before the error are recorded as received,
/// so the client can resend the whole batch.
pub async fn sync_offline_actions(
    db: &SqlitePool,
    SyncOfflineActionsRequest {
        scheduler_name,
        actions,
    }: SyncOfflineActionsRequest,
    now: DateTime<Utc>,
) -> Result<SyncOfflineActionsResponse, Error> {
    let scheduler = get_scheduler_from_string(&scheduler_name)?;
    let config = read_external_config()?;

    let mut order = (0..actions.len()).collect::<Vec<_>>();
    order.sort_by_key(|&i| (actions[i].performed_at, i));
    let mut outcomes = actions.iter().map(|_| None).collect::<Vec<_>>();
    for i in order {
        let outcome =
            apply_offline_action(db, scheduler.as_ref(), &config, &actions[i], now).await?;
        outcomes[i] = Some(outcome);
    }
    let results = actions
        .into_iter()
        .zip(outcomes)
        .map(|(action, outcome)| OfflineActionResult {
            client_id: action.client_id,
            outcome: outcome.expect("every action is applied"),
        })
        .collect();
    Ok(SyncOfflineActionsResponse { results })
}

async fn apply_offline_action(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    config: &SparesExternalConfig,
    OfflineStudyAction {
        client_id,
        performed_at,
        action,
    }: &OfflineStudyAction,
    now: DateTime<Utc>,
) -> Result<OfflineActionOutcome, Error> {
    let received: Option<i64> =
        sqlx::query_scalar(r"SELECT 1 FROM offline_action WHERE client_id = ?")
            .bind(client_id)
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    if received.is_some() {
        return Ok(OfflineActionOutcome::Duplicate);
    }

    // A client whose clock is ahead of the server's would otherwise record reviews the server has
    // not reached yet.
    let performed_at = (*performed_at).min(now);
    let result = match action {
        OfflineStudyActionKind::Rate(submission) => {
            rate_offline(db, scheduler, config, submission, performed_at, now).await
        }
        OfflineStudyActionKind::Bury { card_id } => {
            // A bury only lasts until the end of the day, so one from an earlier day is over.
            if performed_at.with_timezone(&Local).date_naive()
                < now.with_timezone(&Local).date_naive()
            {
                Ok(None)
            } else {
                bury_card(db, scheduler, *card_id, performed_at, true).await
            }
        }
    };
    let outcome = match result {
        Ok(event_id) => OfflineActionOutcome::Applied { event_id },
        Err(error) => OfflineActionOutcome::Rejected {
            message: rejection_message(error)?,
        },
    };

    sqlx::query(
        r"INSERT INTO offline_action (client_id, performed_at, received_at) VALUES (?, ?, ?)",
    )
    .bind(client_id)
    .bind(performed_at.timestamp())
    .bind(now.timestamp())
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(outcome)
}

/// Why an action cannot be applied, if `error` is about the action rather than the server.
/// Otherwise, returns `error` to fail the request with.
fn rejection_message(error: Error) -> Result<String, Error> {
    match error {
        // A card or filtered tag that no longer exists
        Error::Sqlx {
            source: sqlx::Error::RowNotFound,
        } => Ok("its card or tag no longer exists".to_string()),
        // Invalid input, e.g. burying a card that is suspended
        Error::Library(error) => Ok(error.to_string()),
        error => Err(error),
    }
}

/// Rates a card as of `performed_at`.
///
/// Returns the event to undo it with, unless it was inserted into the card's history rather
/// than appended to it.
async fn rate_offline(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    config: &SparesExternalConfig,
    submission: &RatingSubmission,
    performed_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<i64>, Error> {
    let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(submission.card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let review_logs = fetch_review_logs_for_replay(db, card.id).await?;

    // Rating a card takes it out of any burial, but a card suspended since the queue was fetched
    // stays suspended.
    let is_latest = review_logs
        .last()
        .is_none_or(|review_log| review_log.reviewed_at <= performed_at);
    if is_latest && card.special_state != Some(SpecialState::Suspended) {
        return rate_card(db, scheduler, submission.clone(), performed_at, true).await;
    }

    insert_review_into_history(
        db,
        scheduler,
        config,
        card,
        review_logs,
        submission,
        performed_at,
        now,
    )
    .await?;
    Ok(None)
}

/// Adds a review at `performed_at` to a card whose log may continue past it, and replays the
/// card's log to give it the memory state and due date the whole history implies.
///
/// The card keeps its special state. Unlike [`rate_card`], no undo event is recorded, and a
/// filtered tag's scheduling data is not updated: the review only records the tag it was done
/// under.
#[allow(
    clippy::too_many_arguments,
    reason = "the card and its log, the review, and when it is applied"
)]
async fn insert_review_into_history(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    config: &SparesExternalConfig,
    card: Card,
    review_logs: Vec<ReviewLog>,
    submission: &RatingSubmission,
    performed_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), Error> {
    let earlier = review_logs
        .into_iter()
        .filter(|review_log| review_log.reviewed_at <= performed_at)
        .collect::<Vec<_>>();
    // The card as it stood when it was reviewed
    let memory_state = if earlier.is_empty() {
        Card {
            desired_retention: card.desired_retention,
            ..Card::new(card.created_at)
        }
    } else {
        scheduler.compute_memory_state(config, earlier.clone(), card.desired_retention)?
    };
    let card_then = Card {
        due: memory_state.due,
        stability: memory_state.stability,
        difficulty: memory_state.difficulty,
        state: memory_state.state,
        special_state: None,
        ..card.clone()
    };
    let (_, review_log) = scheduler.schedule(
        config,
        &card_then,
        effective_review_logs(&earlier).last().cloned(),
        submission.rating,
        performed_at,
        submission.recall_duration,
        submission.rate_duration,
    )?;
    insert_review(db, &review_log, submission.tag_id).await?;

    let review_logs = fetch_review_logs_for_replay(db, card.id).await?;
    scheduler
        .reschedule(db, config, vec![(card, review_logs)], now)
        .await
}
