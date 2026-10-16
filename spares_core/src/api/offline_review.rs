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
//!
//! # For the reviewer
//!
//! The web app can be installed as an app (e.g. "Add to Home Screen" on a phone) and opened
//! without a connection. This needs the server to be reached over HTTPS, or at `localhost`.
//!
//! - While the Review page is open and online, it downloads the next 200 cards due today and
//!   tomorrow for the default review, and for each filter a session is started with, along with
//!   what they display. New cards are included up to what is left of today's limit.
//! - When the server cannot be reached, a review shows those cards instead, and the navbar shows
//!   "Offline". Ratings and buries are saved on the device and sent once the server can be reached
//!   again, and the navbar shows how many are waiting. Other actions, such as editing a note or
//!   suspending a card, need the server.
//! - A card rated Again offline is not shown again until the rating has been sent, since the
//!   device does not schedule cards itself.
//! - Learning and relearning cards wait for
//!   [`QueueConfig::learn_ahead_limit`](crate::config::QueueConfig::learn_ahead_limit) as they do
//!   online: once nothing else is left, a card whose step ends within the limit is shown, and
//!   otherwise the review waits for the next one, showing when it will be. For example, studying
//!   on a phone on a train with steps of 1, 10 and 60 minutes and the default limit of 20
//!   minutes, a card on its 60 minute step comes back 40 minutes after it was rated (if nothing
//!   else is left by then), not right after the other cards are done. The limit is the one the
//!   server had when the cards were downloaded.
//! - A rating sent after a later review of the same card, e.g. from another device that synced
//!   first, is placed at its time in the card's history, which is then replayed. Such a rating
//!   cannot be undone from the review page.
//! - A lapse placed in the history still triggers the
//!   [leech action](crate::config::LeechConfig::action) when it brings the card's lapses to the
//!   threshold, as a lapse rated online does. For example, reviewing on a laptop online and on a
//!   phone offline with `lapses_threshold = 8` and `action = "suspend"`: a card with 7 lapses is
//!   rated Again on the phone, then Good on the laptop. When the phone syncs, its Again is placed
//!   before the Good, the card now has 8 lapses, and it is tagged `leech` and suspended. Since
//!   the rating cannot be undone, the leech action is its own undo event:
//!   `spares undo --undo-group` right after the sync unsuspends the card and removes the tag.
//! - A rating from an earlier day does not bury the card's siblings, since they would stay buried
//!   a day too long.
//! - Undo takes back a rating or bury that has not been sent yet. Logging out keeps unsent
//!   actions, which are sent the next time you log in to the same server.

use chrono::DateTime;
use chrono::Duration;
use chrono::Local;
use chrono::Utc;
use indoc::formatdoc;
use sqlx::FromRow;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::fetch_review_logs_for_replay;
use crate::api::queue::queue_order_by;
use crate::api::review::REVIEW_CARD_COLUMNS;
use crate::api::review::ReviewCard;
use crate::api::review::ReviewFilterQuery;
use crate::api::review::build_review_filter_query;
use crate::api::review::bury_card;
use crate::api::review::daily_limits_left;
use crate::api::review::get_cards_left_by_state_and_time_estimate;
use crate::api::review::insert_review;
use crate::api::review::rate_card_with_config;
use crate::api::review::review_card_response;
use crate::api::review::tag_leech;
use crate::api::review::unbury_cards_and_update_config;
use crate::api::undo::insert_events;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::config::LeechAction;
use crate::config::LeechConfig;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::model::Card;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::NEW_CARD_STATE;
use crate::model::NoteId;
use crate::model::ReviewLog;
use crate::model::SpecialState;
use crate::parsers::Parseable;
use crate::schedulers::SrsScheduler;
use crate::schedulers::effective_review_logs;
use crate::schedulers::get_scheduler_from_string;
use crate::schedulers::learning_states;
use crate::schedulers::review_state;
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
/// of the day, plus those due on the next `days_ahead` days. New and review cards are capped by
/// what is left of today's limits, since the client cannot know tomorrow's. Learning cards are
/// included whenever they are due, regardless of the learn ahead limit, with the time the limit
/// lets them be shown as [`ReviewQueueCard::shown_from`], since the client picks when to show
/// them.
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

    let limits_left = daily_limits_left(db, requested_date).await?;
    let new_cards_left_today = limits_left.new;
    let Some(ReviewFilterQuery {
        where_clause,
        is_filtered_tag,
        card_due_limit,
        caps,
        query_sort,
        ..
    }) = build_review_filter_query(db, filter.as_ref(), requested_date, false).await?
    else {
        return Ok(ReviewQueueResponse {
            cards: Vec::new(),
            new_cards_left_today,
        });
    };

    // Ordered like `get_review_card`. There is no `LIMIT` since new cards beyond today's limit are
    // skipped below.
    let queue_config = read_external_config()?.queue;
    let order_by = queue_order_by(&queue_config, requested_date, caps, &query_sort);
    let query_str = formatdoc! {"
        SELECT {REVIEW_CARD_COLUMNS}, c.due as due
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE {where_clause}
        ORDER BY
            {order_by}"
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
        caps,
    )
    .await?;

    let mut new_cards_allowed = new_cards_left_today;
    let mut reviews_allowed = limits_left.reviews;
    let mut cards = Vec::new();
    for QueueRow { card, due } in rows {
        if cards.len() >= limit as usize {
            break;
        }
        // Filtered tags have no limits, and learning cards are never held back
        let allowed = if is_filtered_tag {
            None
        } else if card.card_state == NEW_CARD_STATE {
            Some(&mut new_cards_allowed)
        } else if card.card_state == review_state() {
            Some(&mut reviews_allowed)
        } else {
            None
        };
        if let Some(allowed) = allowed {
            if *allowed == 0 {
                continue;
            }
            *allowed -= 1;
        }
        // Held back like `build_review_filter_query` does with `learn_ahead`, except that the
        // client compares it to its own clock
        let shown_from = (!is_filtered_tag && learning_states().contains(&card.card_state))
            .then(|| due - queue_config.learn_ahead_limit);
        let card = review_card_response(
            db,
            card,
            cards_left_by_state.clone(),
            time_estimate,
            all_parsers,
        )
        .await?;
        cards.push(ReviewQueueCard {
            due,
            shown_from,
            card,
        });
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
    request: SyncOfflineActionsRequest,
    now: DateTime<Utc>,
) -> Result<SyncOfflineActionsResponse, Error> {
    let config = read_external_config()?;
    sync_offline_actions_with_config(db, &config, request, now).await
}

/// [`sync_offline_actions`] with `config` rather than the one in `config.toml`.
pub(crate) async fn sync_offline_actions_with_config(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    SyncOfflineActionsRequest {
        scheduler_name,
        actions,
    }: SyncOfflineActionsRequest,
    now: DateTime<Utc>,
) -> Result<SyncOfflineActionsResponse, Error> {
    let scheduler = get_scheduler_from_string(&scheduler_name)?;

    let mut order = (0..actions.len()).collect::<Vec<_>>();
    order.sort_by_key(|&i| (actions[i].performed_at, i));
    let mut outcomes = actions.iter().map(|_| None).collect::<Vec<_>>();
    for i in order {
        let outcome =
            apply_offline_action(db, scheduler.as_ref(), config, &actions[i], now).await?;
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
    // A rating in a filtered tag that does not reschedule writes a cram, which replay skips, so
    // there is no history to insert it into: it is logged at `performed_at` whatever came after.
    let reschedules = match submission.tag_id {
        Some(tag_id) => sqlx::query_scalar(r"SELECT reschedule FROM tag WHERE id = ?")
            .bind(tag_id)
            .fetch_optional(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
            .unwrap_or(true),
        None => true,
    };
    if !reschedules {
        // Nothing to record for a card suspended since the queue was fetched
        if card.special_state == Some(SpecialState::Suspended) {
            return Ok(None);
        }
        return rate_card_with_config(
            db,
            scheduler,
            config,
            submission.clone(),
            performed_at,
            true,
        )
        .await;
    }
    if is_latest && card.special_state != Some(SpecialState::Suspended) {
        return rate_card_with_config(
            db,
            scheduler,
            config,
            submission.clone(),
            performed_at,
            true,
        )
        .await;
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
/// The card keeps its special state. Unlike [`rate_card`](crate::api::review::rate_card), no undo
/// event is recorded for the review, and a filtered tag's scheduling data is not updated: the
/// review only records the tag it was done under.
///
/// A lapse that brings the card's lapses to where [`LeechConfig::acts_on`] says to act still gets
/// [`LeechConfig::action`], as a lapse rated online would (see [`act_on_leech`]). Lapses are
/// counted from each review's recorded previous state, so inserting a lapse adds exactly one,
/// whatever came after it.
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
    let lapses_before = scheduler.lapses(db, card.id).await?;
    insert_review(db, &review_log, submission.tag_id).await?;
    let lapses = scheduler.lapses(db, card.id).await?;

    let (card_id, note_id) = (card.id, card.note_id);
    let review_logs = fetch_review_logs_for_replay(db, card.id).await?;
    scheduler
        .reschedule(db, config, vec![(card, review_logs)], now)
        .await?;
    // After the replay, which leaves the special state alone, so a suspension is not undone by it
    if lapses > lapses_before && config.leech.acts_on(lapses) {
        act_on_leech(db, &config.leech, card_id, note_id, now).await?;
    }
    Ok(())
}

/// Applies [`LeechConfig::action`] to a card that a review inserted into its history made a
/// leech, at `at`, when the review was received.
///
/// [`rate_card`](crate::api::review::rate_card) puts the leech action in the rating's undo group,
/// but an inserted review has no undo event, so the action is its own: the suspension is an
/// [`EventType::UpdateCards`] event, which the tag's events join, so `spares undo --undo-group`
/// unsuspends the card and untags its note. A card that is already suspended (e.g. suspended by
/// hand since the offline device fetched it) is only tagged. A buried card is suspended, and
/// undoing that buries it again.
async fn act_on_leech(
    db: &SqlitePool,
    leech: &LeechConfig,
    card_id: CardId,
    note_id: NoteId,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    let suspension = match leech.action {
        LeechAction::Suspend => suspend_leech(db, card_id, at).await?,
        LeechAction::Tag | LeechAction::None => None,
    };
    // Boxed, since updating a note makes a large future for every caller to carry
    Box::pin(tag_leech(db, leech, note_id, at, true, suspension)).await
}

/// Suspends `card_id`, returning the event to undo it with, or `None` if it was already
/// suspended.
async fn suspend_leech(
    db: &SqlitePool,
    card_id: CardId,
    at: DateTime<Utc>,
) -> Result<Option<i64>, Error> {
    let before: Option<SpecialState> =
        sqlx::query_scalar(r"SELECT special_state FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    if before == Some(SpecialState::Suspended) {
        return Ok(None);
    }
    sqlx::query(r"UPDATE card SET special_state = ?, updated_at = ? WHERE id = ?")
        .bind(SpecialState::Suspended)
        .bind(at.timestamp())
        .bind(card_id)
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let payload = UpdateCardPayload {
        card_id,
        order: None,
        back_type: None,
        due: None,
        stability: None,
        difficulty: None,
        desired_retention: None,
        special_state: Some(Transition {
            before,
            after: Some(SpecialState::Suspended),
        }),
        state: None,
        custom_data: None,
    };
    let event_ids = insert_events(
        db,
        &[(
            EventType::UpdateCards,
            serde_json::to_value(vec![payload]).unwrap(),
        )],
        at,
        None,
    )
    .await?;
    Ok(event_ids.first().copied())
}
