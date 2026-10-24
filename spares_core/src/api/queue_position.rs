//! Where cards are in a review's queue, and why the others are not in it.

use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use indoc::formatdoc;
use log::warn;
use sqlx::FromRow;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::TagErrorKind;
use crate::api::preset::preset_index;
use crate::api::queue::OrderedQueue;
use crate::api::queue::UsedUpLimit;
use crate::api::queue::ordered_queue;
use crate::api::review::ReviewFilterQuery;
use crate::api::review::build_review_filter_query;
use crate::api::review::todays_review_snapshot;
use crate::api::review::unbury_cards_and_update_config;
use crate::config::SparesExternalConfig;
use crate::config::read_external_config;
use crate::model::CardId;
use crate::model::NoteId;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::state_role;
use crate::schema::review::GetReviewCardFilterRequest;
use crate::schema::review::HeldBackCard;
use crate::schema::review::NotShownCard;
use crate::schema::review::NotShownReason;
use crate::schema::review::QueueCard;
use crate::schema::review::QueuePosition;
use crate::schema::review::QueuePositionRequest;
use crate::schema::review::QueuePositionResponse;
use crate::schema::review::StateRole;
use crate::search::evaluator::Evaluator;
use crate::search::query_has_limit;

/// A card of the review's filter that it does not show now, with what decides why.
#[derive(FromRow)]
struct CardRow {
    card_id: CardId,
    note_id: NoteId,
    card_order: u32,
    card_state: StateId,
    due: DateTime<Utc>,
    special_state: Option<u8>,
    /// The card's preset, by its index in
    /// [`QueueConfig::presets`](crate::config::QueueConfig::presets)
    preset: Option<i64>,
    waiting: bool,
}

impl CardRow {
    fn card(&self) -> QueueCard {
        QueueCard {
            card_id: self.card_id,
            note_id: self.note_id,
            card_order: self.card_order,
            card_state: self.card_state,
            due: self.due,
            note_preview: String::new(),
        }
    }
}

/// Where the cards matching `match_query` are in the queue of the review with `filter`, as of
/// `requested_date`, and why review would not show the others today.
///
/// For example, in the review of `tag="probability:grad:2" or
/// linked_from_note=(tag="probability:grad:2")`, the cards of notes matching `data~re:"key.Uni"`
/// may be at positions 3 and 4, with a third held back until its learning step is nearly over and
/// a fourth over today's new card limit.
///
/// - Positions count from 1, in the order review shows cards now (see
///   [`QueueConfig`](crate::config::QueueConfig)), with new and review cards past today's limits
///   left out.
/// - Learning and relearning cards held back by
///   [`QueueConfig::learn_ahead_limit`](crate::config::QueueConfig::learn_ahead_limit) are listed
///   apart, with when they can be shown. They have no fixed position, since review shows them once
///   nothing else is left and their time has come.
/// - The other matching cards of the filter are listed with a [`NotShownReason`], and those not in
///   the filter at all are counted.
///
/// Without `match_query`, the whole queue is listed, as a preview of a session.
///
/// A position holds only until the next card is rated: rating brings learning cards back, changes
/// what is left of the limits that new and review cards are spread over, and may bury siblings.
///
/// A query with `limit` is reviewed as a snapshot built when its review starts (see
/// [`create_review_snapshot_tag`](crate::api::review::create_review_snapshot_tag)). Its positions
/// are those in today's snapshot, and with none yet this is an error rather than building one by
/// looking. A filtered tag has no limits and holds nothing back, so the only reasons for it are
/// [`NotShownReason::Suspended`] and [`NotShownReason::Buried`].
///
/// Buried cards are unburied first if the day has changed, as opening review does.
pub async fn queue_positions(
    db: &SqlitePool,
    QueuePositionRequest {
        filter,
        match_query,
        limit,
    }: QueuePositionRequest,
    requested_date: DateTime<Utc>,
) -> Result<QueuePositionResponse, Error> {
    let matched: Option<HashSet<CardId>> = match &match_query {
        Some(query) => Some(
            Evaluator::new(query)
                .get_card_ids(db)
                .await
                .map_err(match_query_error)?
                .into_iter()
                .collect(),
        ),
        None => None,
    };
    unbury_cards_and_update_config(db).await?;
    let review_filter = review_filter_of(db, filter, requested_date).await?;
    let config = read_external_config()?;
    let OrderedQueue { rows, skipped, .. } = ordered_queue(
        db,
        &review_filter,
        &config,
        requested_date,
        review_filter.card_due_limit,
        None,
    )
    .await?;
    let limit = limit.map_or(usize::MAX, |limit| limit as usize);
    let is_match = |card_id: CardId| matched.as_ref().is_none_or(|ids| ids.contains(&card_id));

    let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let mut matching = 0;
    let mut positions = Vec::new();
    let mut in_queue = HashSet::new();
    for (index, row) in rows.into_iter().enumerate() {
        in_queue.insert(row.card.card_id);
        if !is_match(row.card.card_id) {
            continue;
        }
        matching += 1;
        if positions.len() < limit {
            positions.push(QueuePosition {
                position: u32::try_from(index + 1).unwrap_or(u32::MAX),
                card: QueueCard {
                    card_id: row.card.card_id,
                    note_id: row.card.note_id,
                    card_order: row.card.card_order,
                    card_state: row.card.card_state,
                    due: row.due,
                    note_preview: String::new(),
                },
            });
        }
    }

    let held_back: Vec<HeldBackCard> = held_back_cards(db, &review_filter, &config)
        .await?
        .into_iter()
        .filter(|card| is_match(card.card.card_id))
        .collect();

    let Some(matched) = matched else {
        let mut response = QueuePositionResponse {
            total,
            matching,
            positions,
            held_back,
            not_shown_total: 0,
            not_shown: Vec::new(),
            not_in_review: 0,
        };
        add_note_previews(db, &mut response).await?;
        return Ok(response);
    };
    let in_filter = cards_of_filter(db, &review_filter, &matched).await?;
    let not_in_review = u32::try_from(matched.len() - in_filter.len()).unwrap_or(u32::MAX);
    let held_back_ids: HashSet<CardId> = held_back.iter().map(|card| card.card.card_id).collect();
    let skipped: HashMap<CardId, UsedUpLimit> = skipped.into_iter().collect();
    let not_shown: Vec<NotShownCard> = in_filter
        .into_iter()
        .filter(|row| !in_queue.contains(&row.card_id) && !held_back_ids.contains(&row.card_id))
        .map(|row| NotShownCard {
            reason: not_shown_reason(&row, &review_filter, &config, skipped.get(&row.card_id)),
            card: row.card(),
        })
        .collect();
    let not_shown_total = u32::try_from(not_shown.len()).unwrap_or(u32::MAX);
    let mut response = QueuePositionResponse {
        total,
        matching,
        positions,
        held_back,
        not_shown_total,
        not_shown: not_shown.into_iter().take(limit).collect(),
        not_in_review,
    };
    add_note_previews(db, &mut response).await?;
    Ok(response)
}

/// How many characters of a note's text [`QueueCard::note_preview`] has, as the web app's list of
/// notes shows.
const NOTE_PREVIEW_CHARS: usize = 100;

/// The start of `data` on one line, at most [`NOTE_PREVIEW_CHARS`] long, with `…` where it is cut.
fn note_preview(data: &str) -> String {
    let text = data.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= NOTE_PREVIEW_CHARS {
        return text;
    }
    let mut preview: String = text.chars().take(NOTE_PREVIEW_CHARS).collect();
    preview.push('…');
    preview
}

/// Fills in [`QueueCard::note_preview`] of each card of `response`, from its note.
async fn add_note_previews(
    db: &SqlitePool,
    response: &mut QueuePositionResponse,
) -> Result<(), Error> {
    let cards = response
        .positions
        .iter_mut()
        .map(|p| &mut p.card)
        .chain(response.held_back.iter_mut().map(|c| &mut c.card))
        .chain(response.not_shown.iter_mut().map(|c| &mut c.card))
        .collect::<Vec<_>>();
    if cards.is_empty() {
        return Ok(());
    }
    let ids = cards
        .iter()
        .map(|card| card.card_id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let notes: Vec<(CardId, String)> = sqlx::query_as(&format!(
        "SELECT c.id, n.data FROM card c JOIN note n ON c.note_id = n.id WHERE c.id IN ({ids})"
    ))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let data: HashMap<CardId, String> = notes.into_iter().collect();
    for card in cards {
        if let Some(data) = data.get(&card.card_id) {
            card.note_preview = note_preview(data);
        }
    }
    Ok(())
}

/// `error` in the match query, marked as being in it, so a client can tell it from one in the
/// filter, which may be a query too, and mark it in the right box.
fn match_query_error(error: Error) -> Error {
    match error {
        Error::Library(LibraryError::Search { message, span, .. }) => {
            Error::Library(LibraryError::Search {
                message,
                span,
                query_field: Some("match_query"),
            })
        }
        error => error,
    }
}

fn invalid(message: String) -> Error {
    Error::Library(LibraryError::Tag(TagErrorKind::InvalidInput(message)))
}

/// The review with `filter`, which for a query with `limit` is today's snapshot of it.
async fn review_filter_of(
    db: &SqlitePool,
    filter: Option<GetReviewCardFilterRequest>,
    requested_date: DateTime<Utc>,
) -> Result<ReviewFilterQuery, Error> {
    let filter = match filter {
        Some(GetReviewCardFilterRequest::Query(query)) if query_has_limit(&query) => {
            let tag_id = todays_review_snapshot(db, &query, requested_date)
                .await?
                .ok_or_else(|| {
                    invalid(format!(
                        "`{query}` uses `limit`, so its review works through a snapshot built when \
                         it starts, and there is none yet today. Start the review first."
                    ))
                })?;
            Some(GetReviewCardFilterRequest::FilteredTag { tag_id })
        }
        filter => filter,
    };
    build_review_filter_query(db, filter.as_ref(), requested_date, true)
        .await?
        .ok_or_else(|| invalid("No filtered tag has this id.".to_string()))
}

/// The learning and relearning cards that the learn ahead limit holds back in the review with
/// `review_filter`, soonest first.
async fn held_back_cards(
    db: &SqlitePool,
    review_filter: &ReviewFilterQuery,
    config: &SparesExternalConfig,
) -> Result<Vec<HeldBackCard>, Error> {
    let Some(clause) = &review_filter.held_back_clause else {
        return Ok(Vec::new());
    };
    let cards: Vec<(CardId, NoteId, u32, StateId, DateTime<Utc>)> =
        sqlx::query_as(&formatdoc! {r#"
            SELECT c.id, c.note_id, c."order", c.state, c.due
            FROM card c
            WHERE {clause}
            ORDER BY c.due, c.id"#
        })
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(cards
        .into_iter()
        .map(
            |(card_id, note_id, card_order, card_state, due)| HeldBackCard {
                shown_from: due - config.queue.learn_ahead_limit,
                card: QueueCard {
                    card_id,
                    note_id,
                    card_order,
                    card_state,
                    due,
                    note_preview: String::new(),
                },
            },
        )
        .collect())
}

/// The cards of `card_ids` in the filter of `review_filter`, whether or not it shows them today,
/// in the order of their notes.
async fn cards_of_filter(
    db: &SqlitePool,
    review_filter: &ReviewFilterQuery,
    card_ids: &HashSet<CardId>,
) -> Result<Vec<CardRow>, Error> {
    if card_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = card_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let waiting = review_filter.waiting_clause.as_deref().unwrap_or("FALSE");
    sqlx::query_as(&formatdoc! {r#"
        SELECT c.id AS card_id, c.note_id, c."order" AS card_order, c.state AS card_state, c.due,
            c.special_state, c.preset, {waiting} AS waiting
        FROM {cards} c
        JOIN note n ON c.note_id = n.id
        WHERE {filter_cards} AND c.id IN ({ids})
        ORDER BY n.created_at, n.id, c."order""#,
        cards = review_filter.cards,
        filter_cards = review_filter.filter_cards_clause,
    })
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })
}

/// Why review with `review_filter` does not show `row`, a card of its filter that is neither in
/// the queue nor held back by the learn ahead limit. `skipped` is the limit that was used up when
/// its turn came in the queue, if one was.
///
/// These follow the review's `WHERE` clause (see
/// [`build_review_filter_query`]): a card is left out if it is suspended or buried, not due, or
/// waiting on a linked note, or by [`ordered_queue`] once a limit is used up. A limit used up
/// before the review starts leaves its cards out of the `WHERE` clause instead.
fn not_shown_reason(
    row: &CardRow,
    review_filter: &ReviewFilterQuery,
    config: &SparesExternalConfig,
    skipped: Option<&UsedUpLimit>,
) -> NotShownReason {
    let preset_name = |index: usize| {
        config
            .queue
            .presets
            .get_index(index)
            .map(|(name, _)| name.clone())
    };
    let limit_reason = |preset: Option<String>| {
        if state_role(row.card_state) == Some(StateRole::New) {
            NotShownReason::NewLimit { preset }
        } else {
            NotShownReason::ReviewLimit { preset }
        }
    };
    match row.special_state {
        Some(state) if state == SpecialState::Suspended as u8 => return NotShownReason::Suspended,
        // Shown today unless it waits on a linked note
        Some(state) if state == SpecialState::BuriedUntilLaterToday as u8 && row.waiting => {
            return NotShownReason::WaitingOnLinkedNote;
        }
        Some(_) => return NotShownReason::Buried,
        None => {}
    }
    if !review_filter.is_filtered_tag && row.due > review_filter.card_due_limit {
        return NotShownReason::NotDue;
    }
    if row.waiting {
        return NotShownReason::WaitingOnLinkedNote;
    }
    match skipped {
        Some(UsedUpLimit::Preset(index)) => return limit_reason(preset_name(*index)),
        Some(UsedUpLimit::TopLevel) => return limit_reason(None),
        None => {}
    }
    // A limit used up before the review started
    let preset = preset_index(row.preset);
    if let Some(index) = preset.filter(|index| {
        review_filter
            .caps
            .presets
            .get(*index)
            .and_then(|left| left.of_state(row.card_state))
            == Some(0)
    }) {
        return limit_reason(preset_name(index));
    }
    // Only new and review cards have a cap
    if review_filter.caps.of_state(row.card_state) == Some(0) {
        return limit_reason(None);
    }
    warn!(
        "Card {} is in the review's filter but neither in its queue nor left out for a known reason",
        row.card_id
    );
    NotShownReason::NotDue
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use indexmap::IndexMap;

    use super::*;
    use crate::api::offline_review::get_review_queue;
    use crate::api::preset::cards_with_preset;
    use crate::api::queue::QueueCaps;
    use crate::api::queue::tests::LEARNING;
    use crate::api::queue::tests::NEW;
    use crate::api::queue::tests::REVIEW;
    use crate::api::queue::tests::add_review;
    use crate::api::queue::tests::create_cards;
    use crate::api::queue::tests::set_card;
    use crate::api::queue::waiting_on_links;
    use crate::api::review::get_review_card;
    use crate::config::QueueConfig;
    use crate::helpers::get_start_end_local_date;
    use crate::model::NEW_CARD_STATE;
    use crate::parsers::get_all_parsers;
    use crate::schema::review::GetReviewCardRequest;
    use crate::schema::review::ReviewQueueRequest;

    async fn positions(
        pool: &SqlitePool,
        filter: Option<GetReviewCardFilterRequest>,
        match_query: Option<&str>,
        now: DateTime<Utc>,
    ) -> QueuePositionResponse {
        queue_positions(
            pool,
            QueuePositionRequest {
                filter,
                match_query: match_query.map(ToString::to_string),
                limit: None,
            },
            now,
        )
        .await
        .unwrap()
    }

    fn card_ids(positions: &[QueuePosition]) -> Vec<CardId> {
        positions.iter().map(|p| p.card.card_id).collect()
    }

    fn reasons(response: &QueuePositionResponse) -> Vec<(CardId, NotShownReason)> {
        response
            .not_shown
            .iter()
            .map(|card| (card.card.card_id, card.reason.clone()))
            .collect()
    }

    fn id_query(cards: &[CardId]) -> String {
        cards
            .iter()
            .map(|id| format!("c.id={id}"))
            .collect::<Vec<_>>()
            .join(" or ")
    }

    async fn set_special_state(pool: &SqlitePool, card: CardId, state: SpecialState) {
        sqlx::query(r"UPDATE card SET special_state = ? WHERE id = ?")
            .bind(state as u8)
            .bind(card)
            .execute(pool)
            .await
            .unwrap();
    }

    /// 3 reviews due, a learning card due now and 22 new cards, 2 more than the default limit of
    /// 20 lets through.
    async fn setup(pool: &SqlitePool, now: DateTime<Utc>) -> Vec<CardId> {
        let ids = create_cards(pool, 26, now - Duration::days(30)).await;
        for (i, card) in ids[..3].iter().enumerate() {
            let days = i64::try_from(i).unwrap() + 1;
            set_card(pool, *card, REVIEW, now - Duration::days(days), 5.0, 5.0).await;
        }
        set_card(pool, ids[3], LEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        ids
    }

    /// The whole queue is the cards `get_review_queue` downloads, in the same order, and review
    /// shows the first of them next.
    #[sqlx::test]
    async fn positions_agree_with_the_review_queue(pool: SqlitePool) {
        let now = Utc::now();
        setup(&pool, now).await;
        let response = positions(&pool, None, None, now).await;
        let queue = get_review_queue(
            &pool,
            ReviewQueueRequest {
                filter: None,
                limit: 100,
                days_ahead: 0,
            },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();
        let queue_ids: Vec<CardId> = queue.cards.iter().map(|card| card.card.card_id).collect();
        assert_eq!(card_ids(&response.positions), queue_ids);
        assert_eq!(response.total, 24);
        assert_eq!(response.matching, 24);
        let card = &response.positions[0].card;
        let data: String = sqlx::query_scalar(r"SELECT data FROM note WHERE id = ?")
            .bind(card.note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(card.note_preview, note_preview(&data));
        assert_ne!(card.note_preview, "");
        assert_eq!(
            response
                .positions
                .iter()
                .map(|p| p.position)
                .collect::<Vec<_>>(),
            (1..=24).collect::<Vec<_>>()
        );

        let next = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.positions[0].card.card_id, next.card_id);
    }

    #[test]
    fn note_previews_are_one_line_and_cut() {
        assert_eq!(
            note_preview("# Title\n\nSome  {{text}}\n"),
            "# Title Some {{text}}"
        );
        let long = "é".repeat(150);
        let preview = note_preview(&long);
        assert_eq!(preview.chars().count(), NOTE_PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }

    #[sqlx::test]
    async fn cards_over_the_new_limit_are_not_shown(pool: SqlitePool) {
        let now = Utc::now();
        let ids = setup(&pool, now).await;
        let response = positions(&pool, None, Some(&id_query(&ids[4..])), now).await;
        assert_eq!(response.matching, 20);
        assert_eq!(
            reasons(&response),
            [
                (ids[24], NotShownReason::NewLimit { preset: None }),
                (ids[25], NotShownReason::NewLimit { preset: None }),
            ]
        );

        // With the limit used up before the review, no new card reaches the queue
        for card in &ids[6..26] {
            add_review(&pool, *card, now, NEW).await;
        }
        let response = positions(&pool, None, Some(&id_query(&ids[4..6])), now).await;
        assert_eq!(response.matching, 0);
        assert_eq!(
            reasons(&response),
            [
                (ids[4], NotShownReason::NewLimit { preset: None }),
                (ids[5], NotShownReason::NewLimit { preset: None }),
            ]
        );
    }

    #[sqlx::test]
    async fn learning_cards_beyond_the_learn_ahead_limit_are_held_back(pool: SqlitePool) {
        let now = Utc::now();
        let ids = setup(&pool, now).await;
        let due = now + Duration::hours(1);
        set_card(&pool, ids[3], LEARNING, due, 1.0, 5.0).await;
        let response = positions(&pool, None, Some(&id_query(&ids[..4])), now).await;
        assert_eq!(card_ids(&response.positions).len(), 3);
        assert!(!card_ids(&response.positions).contains(&ids[3]));
        assert_eq!(response.held_back.len(), 1);
        let held_back = &response.held_back[0];
        assert_eq!(held_back.card.card_id, ids[3]);
        assert_eq!(
            held_back.shown_from.timestamp(),
            (due - Duration::minutes(20)).timestamp()
        );
        assert_eq!(response.not_shown, []);
    }

    #[sqlx::test]
    async fn a_card_filter_locates_only_its_cards(pool: SqlitePool) {
        let now = Utc::now();
        let ids = setup(&pool, now).await;
        let response = positions(&pool, None, Some("c.state=0"), now).await;
        assert_eq!(response.matching, 20);
        assert!(
            response
                .positions
                .iter()
                .all(|p| p.card.card_state == NEW_CARD_STATE)
        );
        assert_eq!(response.not_shown.len(), 2);
        assert!(!card_ids(&response.positions).contains(&ids[0]));
    }

    #[sqlx::test]
    async fn reasons_follow_the_card(pool: SqlitePool) {
        let now = Utc::now();
        let ids = setup(&pool, now).await;
        // As opening review today already has, so the card buried below stays buried
        unbury_cards_and_update_config(&pool).await.unwrap();
        set_special_state(&pool, ids[0], SpecialState::Suspended).await;
        set_special_state(&pool, ids[1], SpecialState::SchedulerBuried).await;
        set_card(&pool, ids[2], REVIEW, now + Duration::days(3), 5.0, 5.0).await;
        let response = positions(&pool, None, Some(&id_query(&ids[..3])), now).await;
        assert_eq!(response.positions, []);
        assert_eq!(
            reasons(&response),
            [
                (ids[0], NotShownReason::Suspended),
                (ids[1], NotShownReason::Buried),
                (ids[2], NotShownReason::NotDue),
            ]
        );
    }

    /// Cards of the review's notes are the only ones that wait on linked notes, so the reason is
    /// checked on the cards of a filter built by hand, with the setting on.
    #[sqlx::test]
    async fn new_cards_wait_on_linked_new_cards(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 2, now - Duration::days(1)).await;
        // The second note, created later, links to the first, whose card is new
        sqlx::query(
            r#"INSERT INTO note_link
                (parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score)
            VALUES (
                (SELECT note_id FROM card WHERE id = ?),
                (SELECT note_id FROM card WHERE id = ?),
                0, 'keyword', 'keyword', 1.0
            )"#,
        )
        .bind(ids[1])
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
        let config = QueueConfig {
            new_after_linked_notes: true,
            ..QueueConfig::default()
        };
        let (_, card_due_limit) = get_start_end_local_date(&now);
        let filter = ReviewFilterQuery {
            cards: cards_with_preset(&IndexMap::new()),
            where_clause: String::new(),
            is_filtered_tag: false,
            card_due_limit,
            caps: QueueCaps::default(),
            preset_sql: "NULL".to_string(),
            held_back_clause: None,
            query_sort: Vec::new(),
            filter_cards_clause: "TRUE".to_string(),
            waiting_clause: waiting_on_links(&config, card_due_limit, None),
        };
        let rows = cards_of_filter(&pool, &filter, &ids.iter().copied().collect())
            .await
            .unwrap();
        let reasons: Vec<NotShownReason> = rows
            .iter()
            .map(|row| not_shown_reason(row, &filter, &SparesExternalConfig::default(), None))
            .collect();
        assert_eq!(reasons[1], NotShownReason::WaitingOnLinkedNote);
        assert!(!rows[0].waiting);
    }

    #[sqlx::test]
    async fn cards_outside_the_filter_are_counted(pool: SqlitePool) {
        let now = Utc::now();
        let ids = setup(&pool, now).await;
        let filter = GetReviewCardFilterRequest::Query(id_query(&ids[..2]));
        let response = positions(&pool, Some(filter), Some(&id_query(&ids[..5])), now).await;
        assert_eq!(response.total, 2);
        assert_eq!(response.matching, 2);
        assert_eq!(response.not_in_review, 3);
        assert_eq!(response.not_shown, []);
    }

    #[sqlx::test]
    async fn errors_in_the_match_query_say_so(pool: SqlitePool) {
        let now = Utc::now();
        let request = |filter: &str, match_query: &str| QueuePositionRequest {
            filter: Some(GetReviewCardFilterRequest::Query(filter.to_string())),
            match_query: Some(match_query.to_string()),
            limit: None,
        };
        let query_field = |error: Error| match error {
            Error::Library(LibraryError::Search {
                span, query_field, ..
            }) => {
                assert!(span.is_some());
                query_field
            }
            error => panic!("{error:?}"),
        };
        let error = queue_positions(&pool, request("c.id>0", "bad (("), now)
            .await
            .unwrap_err();
        assert_eq!(query_field(error), Some("match_query"));
        let error = queue_positions(&pool, request("bad ((", "c.id>0"), now)
            .await
            .unwrap_err();
        assert_eq!(query_field(error), None);
    }

    #[sqlx::test]
    async fn a_limit_query_needs_todays_snapshot(pool: SqlitePool) {
        let now = Utc::now();
        setup(&pool, now).await;
        let query = "c.state=0 limit=5".to_string();
        let error = queue_positions(
            &pool,
            QueuePositionRequest {
                filter: Some(GetReviewCardFilterRequest::Query(query.clone())),
                match_query: None,
                limit: None,
            },
            now,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("Start the review first"),
            "{error}"
        );
        let tags: u32 = sqlx::query_scalar(r"SELECT COUNT(*) FROM tag WHERE name LIKE 'review-%'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tags, 0);

        crate::api::review::create_review_snapshot_tag(
            &pool,
            crate::schema::review::ReviewSnapshotRequest {
                query: query.clone(),
            },
            now,
        )
        .await
        .unwrap();
        let response = positions(
            &pool,
            Some(GetReviewCardFilterRequest::Query(query)),
            None,
            now,
        )
        .await;
        assert_eq!(response.total, 5);
    }
}
