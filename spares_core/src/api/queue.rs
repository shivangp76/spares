//! The order review shows cards in, as configured by [`QueueConfig`].

use chrono::DateTime;
use chrono::Datelike;
use chrono::Local;
use chrono::Utc;
use itertools::Itertools;

use crate::config::NewCardOrder;
use crate::config::NewReviewMix;
use crate::config::QueueConfig;
use crate::config::ReviewOrder;
use crate::model::NEW_CARD_STATE;
use crate::schedulers::learning_states;

/// What is left of today's limits, which [`NewReviewMix::Interleaved`] spreads cards over. `None`
/// for no limit, as when studying a filtered tag.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct QueueCaps {
    pub(crate) new: Option<u32>,
    pub(crate) reviews: Option<u32>,
}

/// A sort key: a SQL expression over the `c` (card) and `n` (note) aliases, and whether it sorts
/// descending.
type Key = (String, bool);

fn asc(expr: impl Into<String>) -> Key {
    (expr.into(), false)
}

fn desc(expr: impl Into<String>) -> Key {
    (expr.into(), true)
}

/// A pseudorandom number in `0..1_000_003` for `id_expr`, the same all day and different the next.
/// The multiplier changes with the day, rather than an offset, so each day is a different
/// permutation rather than a rotation of the same one. Products stay below 2^40, so they cannot
/// overflow.
fn daily_hash(id_expr: &str, now: DateTime<Utc>) -> String {
    const PRIME: i64 = 1_000_003;
    let day = i64::from(now.with_timezone(&Local).date_naive().num_days_from_ce());
    let multiplier = (day * 7919 + 48271) % (PRIME - 1) + 1;
    format!("((({id_expr}) % {PRIME}) * {multiplier}) % {PRIME}")
}

/// When the card was last reviewed. `review_log(card_id, reviewed_at)` is indexed, so this is a
/// lookup per card.
const LAST_REVIEW: &str = "(SELECT MAX(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id)";

fn review_keys(order: ReviewOrder, now: DateTime<Utc>) -> Vec<Key> {
    let now_ts = now.timestamp();
    match order {
        ReviewOrder::Due => vec![asc("c.due"), asc("n.created_at")],
        // FSRS's retrievability falls as elapsed time over stability grows, with the same curve
        // for every card, so sorting by that ratio sorts by retrievability.
        ReviewOrder::Retrievability => vec![desc(format!(
            "({now_ts} - COALESCE({LAST_REVIEW}, c.due)) * 1.0 / MAX(c.stability, 0.0001)"
        ))],
        ReviewOrder::RelativeOverdueness => vec![desc(format!(
            "({now_ts} - c.due) * 1.0 / MAX(c.due - COALESCE({LAST_REVIEW}, c.due), 1)"
        ))],
        ReviewOrder::DifficultyDescending => vec![desc("c.difficulty")],
        ReviewOrder::DifficultyAscending => vec![asc("c.difficulty")],
        ReviewOrder::Random => vec![asc(daily_hash("c.id", now))],
        ReviewOrder::NoteCreated => vec![asc("n.created_at"), asc("n.id"), asc("c.due")],
    }
}

fn new_keys(order: NewCardOrder, now: DateTime<Utc>) -> Vec<Key> {
    match order {
        NewCardOrder::Created => vec![asc("n.created_at"), asc("n.id"), asc("c.\"order\"")],
        NewCardOrder::NewestFirst => {
            vec![desc("n.created_at"), desc("n.id"), asc("c.\"order\"")]
        }
        NewCardOrder::Random => vec![asc(daily_hash("c.id", now))],
        NewCardOrder::RandomNote => vec![
            asc(daily_hash("n.id", now)),
            asc("n.id"),
            asc("c.\"order\""),
        ],
    }
}

fn render(keys: &[Key]) -> String {
    keys.iter()
        .map(|(expr, descending)| format!("{expr} {}", if *descending { "DESC" } else { "ASC" }))
        .join(",\n    ")
}

/// The `ORDER BY` expressions (without the keyword) that put the cards matching a review's
/// `WHERE` clause in the order [`QueueConfig`] describes, as of `now`. The query must select
/// `FROM card c JOIN note n`.
pub(crate) fn queue_order_by(config: &QueueConfig, now: DateTime<Utc>, caps: QueueCaps) -> String {
    let now_ts = now.timestamp();
    let [learning, relearning] = learning_states();
    // The groups listed on `QueueConfig`, numbered from 0
    let group = format!(
        "CASE WHEN c.special_state IS NOT NULL THEN 3 \
         WHEN c.state IN ({learning}, {relearning}) THEN CASE WHEN c.due <= {now_ts} THEN 0 ELSE 2 END \
         ELSE 1 END"
    );
    let is_new = format!("c.state = {NEW_CARD_STATE}");

    // Within group 1, new and review cards are each sorted by their own keys. A key that only
    // applies to one kind is NULL for the other, so it ties.
    let only = |condition: &str, keys: Vec<Key>| -> Vec<Key> {
        keys.into_iter()
            .map(|(expr, descending)| {
                (format!("CASE WHEN {condition} THEN {expr} END"), descending)
            })
            .collect()
    };
    let mut within_kind = only(&is_new, new_keys(config.new_order, now));
    within_kind.extend(only(
        &format!("NOT ({is_new})"),
        review_keys(config.review_order, now),
    ));
    within_kind.push(asc("c.id"));

    let mix_keys = match config.new_review_mix {
        NewReviewMix::NewFirst => {
            let mut keys = vec![asc(format!("NOT ({is_new})"))];
            keys.extend(within_kind);
            keys
        }
        NewReviewMix::ReviewsFirst => {
            let mut keys = vec![asc(is_new.clone())];
            keys.extend(within_kind);
            keys
        }
        NewReviewMix::Interleaved => {
            // Each card's position within its kind, as a fraction of how many of that kind can
            // still be shown today. Sorting by it spreads the smaller kind evenly over the larger.
            // Cards past today's limits get fractions over 1, so they come after the rest.
            let partition = format!("PARTITION BY {group}, {is_new}");
            let count = format!("COUNT(*) OVER ({partition})");
            let capped = |cap: Option<u32>| match cap {
                Some(cap) => format!("MAX(MIN({count}, {cap}), 1)"),
                None => count.clone(),
            };
            vec![
                asc(format!(
                    "(ROW_NUMBER() OVER ({partition} ORDER BY {}) - 0.5) / \
                     CASE WHEN {is_new} THEN {} ELSE {} END",
                    render(&within_kind),
                    capped(caps.new),
                    capped(caps.reviews),
                )),
                // Reviews before new cards on a tie
                asc(is_new.clone()),
            ]
        }
    };

    let mut keys = vec![asc(group.clone())];
    keys.extend(only(&format!("{group} = 1"), mix_keys));
    // Groups 0, 2 and 3, and any remaining tie
    keys.extend([asc("c.due"), asc("n.created_at"), asc("c.id")]);
    render(&keys)
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::model::CardId;
    use crate::model::SpecialState;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;

    pub(crate) const NEW: i64 = 0;
    pub(crate) const LEARNING: i64 = 1;
    pub(crate) const REVIEW: i64 = 2;
    pub(crate) const RELEARNING: i64 = 3;

    /// Creates `n` notes of one card each, the `i`th note created `i` hours after the first, and
    /// returns their cards in the order of their notes.
    pub(crate) async fn create_cards(
        pool: &SqlitePool,
        n: usize,
        now: DateTime<Utc>,
    ) -> Vec<CardId> {
        let parser = create_parser_helper(pool, "markdown").await;
        let requests = (0..n)
            .map(|i| CreateNoteRequest {
                data: format!("Note {i}: {{{{answer {i}}}}}"),
                keywords: Vec::new(),
                tags: Vec::new(),
                is_suspended: false,
                custom_data: serde_json::Map::new(),
            })
            .collect();
        create_notes(
            pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests,
            },
            now,
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();
        let ids: Vec<(CardId, i64)> = sqlx::query_as(
            r"SELECT c.id, n.id FROM card c JOIN note n ON c.note_id = n.id ORDER BY n.id",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        for (i, (_, note_id)) in ids.iter().enumerate() {
            sqlx::query(r"UPDATE note SET created_at = ? WHERE id = ?")
                .bind((now + Duration::hours(i64::try_from(i).unwrap())).timestamp())
                .bind(note_id)
                .execute(pool)
                .await
                .unwrap();
        }
        ids.into_iter().map(|(card_id, _)| card_id).collect()
    }

    /// Sets a card's state, due date, stability and difficulty.
    pub(crate) async fn set_card(
        pool: &SqlitePool,
        card_id: CardId,
        state: i64,
        due: DateTime<Utc>,
        stability: f64,
        difficulty: f64,
    ) {
        sqlx::query(
            r"UPDATE card SET state = ?, due = ?, stability = ?, difficulty = ? WHERE id = ?",
        )
        .bind(state)
        .bind(due.timestamp())
        .bind(stability)
        .bind(difficulty)
        .bind(card_id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Records a Good review of `card_id` at `at`.
    pub(crate) async fn add_review(
        pool: &SqlitePool,
        card_id: CardId,
        at: DateTime<Utc>,
        previous_state: i64,
    ) {
        sqlx::query(
            r"INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name,
            scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
            VALUES (?, ?, 0, 3, 'fsrs', 0, 0, 0, ?, '{}')",
        )
        .bind(card_id)
        .bind(at.timestamp())
        .bind(previous_state)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Every unsuspended card, in queue order.
    async fn ordered(
        pool: &SqlitePool,
        config: &QueueConfig,
        now: DateTime<Utc>,
        caps: QueueCaps,
    ) -> Vec<CardId> {
        let query = format!(
            "SELECT c.id FROM card c JOIN note n ON c.note_id = n.id
            WHERE c.special_state IS NULL OR c.special_state = {}
            ORDER BY {}",
            SpecialState::BuriedUntilLaterToday as u8,
            queue_order_by(config, now, caps)
        );
        sqlx::query_scalar(&query).fetch_all(pool).await.unwrap()
    }

    fn config(
        review_order: ReviewOrder,
        new_order: NewCardOrder,
        new_review_mix: NewReviewMix,
    ) -> QueueConfig {
        QueueConfig {
            review_order,
            new_order,
            new_review_mix,
            ..QueueConfig::default()
        }
    }

    #[sqlx::test]
    async fn groups_come_in_order(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 5, now - Duration::days(10)).await;
        let [buried, later, review, new, learning] = ids[..] else {
            unreachable!()
        };
        set_card(
            &pool,
            learning,
            LEARNING,
            now - Duration::minutes(1),
            1.0,
            5.0,
        )
        .await;
        set_card(
            &pool,
            later,
            RELEARNING,
            now + Duration::minutes(5),
            1.0,
            5.0,
        )
        .await;
        set_card(&pool, review, REVIEW, now - Duration::days(1), 3.0, 5.0).await;
        set_card(&pool, buried, REVIEW, now - Duration::days(9), 3.0, 5.0).await;
        sqlx::query(r"UPDATE card SET special_state = ? WHERE id = ?")
            .bind(SpecialState::BuriedUntilLaterToday as u8)
            .bind(buried)
            .execute(&pool)
            .await
            .unwrap();

        let order = ordered(&pool, &QueueConfig::default(), now, QueueCaps::default()).await;
        assert_eq!(order, vec![learning, review, new, later, buried]);
    }

    #[sqlx::test]
    async fn review_orders(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 3, now - Duration::days(200)).await;
        let [a, b, c] = ids[..] else { unreachable!() };
        // a: reviewed 10 days ago, due 2 days ago, stability 10 (elapsed/S = 1, 25% overdue)
        // b: reviewed 100 days ago, due 1 day ago, stability 200 (elapsed/S = 0.5, ~1% overdue)
        // c: reviewed 5 days ago, due 3 days ago, stability 2 (elapsed/S = 2.5, 150% overdue)
        for (card, reviewed, due, stability, difficulty) in [
            (a, 10, 2, 10.0, 3.0),
            (b, 100, 1, 200.0, 9.0),
            (c, 5, 3, 2.0, 6.0),
        ] {
            set_card(
                &pool,
                card,
                REVIEW,
                now - Duration::days(due),
                stability,
                difficulty,
            )
            .await;
            add_review(&pool, card, now - Duration::days(reviewed), NEW).await;
        }
        let order = async |review_order| {
            ordered(
                &pool,
                &config(
                    review_order,
                    NewCardOrder::Created,
                    NewReviewMix::Interleaved,
                ),
                now,
                QueueCaps::default(),
            )
            .await
        };
        assert_eq!(order(ReviewOrder::Due).await, vec![c, a, b]);
        assert_eq!(order(ReviewOrder::Retrievability).await, vec![c, a, b]);
        assert_eq!(order(ReviewOrder::RelativeOverdueness).await, vec![c, a, b]);
        assert_eq!(
            order(ReviewOrder::DifficultyDescending).await,
            vec![b, c, a]
        );
        assert_eq!(order(ReviewOrder::DifficultyAscending).await, vec![a, c, b]);
        assert_eq!(order(ReviewOrder::NoteCreated).await, vec![a, b, c]);
        let mut random = order(ReviewOrder::Random).await;
        assert_eq!(random, order(ReviewOrder::Random).await, "the same all day");
        random.sort_unstable();
        assert_eq!(random, ids);
    }

    /// Retrievability and relative overdueness disagree when a card with a long interval is
    /// further past its due date in days but not in proportion.
    #[sqlx::test]
    async fn retrievability_differs_from_due(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 2, now - Duration::days(300)).await;
        let [long, short] = ids[..] else {
            unreachable!()
        };
        set_card(&pool, long, REVIEW, now - Duration::days(20), 100.0, 5.0).await;
        add_review(&pool, long, now - Duration::days(120), NEW).await;
        set_card(&pool, short, REVIEW, now - Duration::days(2), 1.0, 5.0).await;
        add_review(&pool, short, now - Duration::days(3), NEW).await;
        let order = async |review_order| {
            ordered(
                &pool,
                &config(
                    review_order,
                    NewCardOrder::Created,
                    NewReviewMix::Interleaved,
                ),
                now,
                QueueCaps::default(),
            )
            .await
        };
        assert_eq!(order(ReviewOrder::Due).await, vec![long, short]);
        assert_eq!(order(ReviewOrder::Retrievability).await, vec![short, long]);
        assert_eq!(
            order(ReviewOrder::RelativeOverdueness).await,
            vec![short, long]
        );
    }

    #[sqlx::test]
    async fn new_orders(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 4, now - Duration::days(1)).await;
        let order = async |new_order| {
            ordered(
                &pool,
                &config(ReviewOrder::Due, new_order, NewReviewMix::Interleaved),
                now,
                QueueCaps::default(),
            )
            .await
        };
        assert_eq!(order(NewCardOrder::Created).await, ids);
        let newest_first = ids.iter().rev().copied().collect::<Vec<_>>();
        assert_eq!(order(NewCardOrder::NewestFirst).await, newest_first);
        for random_order in [NewCardOrder::Random, NewCardOrder::RandomNote] {
            let mut random = order(random_order).await;
            assert_eq!(random, order(random_order).await, "the same all day");
            random.sort_unstable();
            assert_eq!(random, ids);
        }
    }

    #[sqlx::test]
    async fn random_order_changes_by_day(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 20, now - Duration::days(1)).await;
        let random = config(
            ReviewOrder::Due,
            NewCardOrder::Random,
            NewReviewMix::Interleaved,
        );
        let today = ordered(&pool, &random, now, QueueCaps::default()).await;
        let tomorrow = ordered(
            &pool,
            &random,
            now + Duration::days(1),
            QueueCaps::default(),
        )
        .await;
        assert_ne!(today, tomorrow);
        assert_ne!(today, ids);
    }

    #[sqlx::test]
    async fn new_review_mix(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 8, now - Duration::days(30)).await;
        // Two new cards and six reviews
        let (new, reviews) = ids.split_at(2);
        for (i, card) in reviews.iter().enumerate() {
            set_card(
                &pool,
                *card,
                REVIEW,
                now - Duration::days(10 - i64::try_from(i).unwrap()),
                5.0,
                5.0,
            )
            .await;
        }
        let order = async |mix, caps| {
            ordered(
                &pool,
                &config(ReviewOrder::Due, NewCardOrder::Created, mix),
                now,
                caps,
            )
            .await
        };
        let new_first = [new, reviews].concat();
        let reviews_first = [reviews, new].concat();
        assert_eq!(
            order(NewReviewMix::NewFirst, QueueCaps::default()).await,
            new_first
        );
        assert_eq!(
            order(NewReviewMix::ReviewsFirst, QueueCaps::default()).await,
            reviews_first
        );
        // The new cards are at 1/4 and 3/4 of the way through, after the reviews that tie them
        let r = reviews;
        assert_eq!(
            order(NewReviewMix::Interleaved, QueueCaps::default()).await,
            vec![r[0], r[1], new[0], r[2], r[3], r[4], new[1], r[5]]
        );
        // With only one new card left today, it is spread over all the reviews, and the other
        // comes after them.
        assert_eq!(
            order(
                NewReviewMix::Interleaved,
                QueueCaps {
                    new: Some(1),
                    reviews: None
                }
            )
            .await,
            vec![r[0], r[1], r[2], new[0], r[3], r[4], r[5], new[1]]
        );
        // Likewise for reviews: with three left today, the new cards are spread over those three
        assert_eq!(
            order(
                NewReviewMix::Interleaved,
                QueueCaps {
                    new: None,
                    reviews: Some(3)
                }
            )
            .await,
            vec![r[0], new[0], r[1], new[1], r[2], r[3], r[4], r[5]]
        );
    }
}

#[cfg(test)]
mod daily_limit_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::tests::NEW;
    use super::tests::REVIEW;
    use super::tests::add_review;
    use super::tests::create_cards;
    use super::tests::set_card;
    use super::*;
    use crate::api::review::DailyLimitsLeft;
    use crate::api::review::daily_limits_left_with;
    use crate::config::SparesExternalConfig;

    fn limits(new: u32, reviews: u32, new_cards_ignore_review_limit: bool) -> SparesExternalConfig {
        SparesExternalConfig {
            new_cards_daily_limit: new,
            reviews_daily_limit: reviews,
            new_cards_ignore_review_limit,
            ..SparesExternalConfig::default()
        }
    }

    #[sqlx::test]
    async fn review_limit_holds_back_new_cards(pool: SqlitePool) {
        let now = Utc::now();
        // Three reviews due and two new cards
        let ids = create_cards(&pool, 5, now - Duration::days(30)).await;
        for card in &ids[..3] {
            set_card(&pool, *card, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        }
        let left = async |config: &SparesExternalConfig| {
            daily_limits_left_with(&pool, config, now).await.unwrap()
        };

        // Room for 5 cards, of which the reviews due take 3
        assert_eq!(
            left(&limits(10, 5, false)).await,
            DailyLimitsLeft { new: 2, reviews: 5 }
        );
        assert_eq!(
            left(&limits(10, 5, true)).await,
            DailyLimitsLeft {
                new: 10,
                reviews: 5
            }
        );
        // More reviews due than the limit leaves no room for new cards
        assert_eq!(
            left(&limits(10, 2, false)).await,
            DailyLimitsLeft { new: 0, reviews: 2 }
        );

        // Studying a review card uses up one review, and leaves one fewer due
        add_review(&pool, ids[0], now, REVIEW).await;
        set_card(&pool, ids[0], REVIEW, now + Duration::days(3), 5.0, 5.0).await;
        assert_eq!(
            left(&limits(10, 5, false)).await,
            DailyLimitsLeft { new: 2, reviews: 4 }
        );

        // Studying a new card uses up a new card and, unless ignored, a review
        add_review(&pool, ids[3], now, NEW).await;
        assert_eq!(
            left(&limits(10, 5, false)).await,
            DailyLimitsLeft { new: 1, reviews: 3 }
        );
        assert_eq!(
            left(&limits(10, 5, true)).await,
            DailyLimitsLeft { new: 9, reviews: 4 }
        );
    }

    /// Reviews of earlier days, and of cards in their (re)learning steps, do not count.
    #[sqlx::test]
    async fn only_todays_reviews_count(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 1, now - Duration::days(30)).await;
        set_card(&pool, ids[0], REVIEW, now + Duration::days(5), 5.0, 5.0).await;
        add_review(&pool, ids[0], now - Duration::days(2), REVIEW).await;
        add_review(&pool, ids[0], now, super::tests::RELEARNING).await;
        assert_eq!(
            daily_limits_left_with(&pool, &limits(10, 5, false), now)
                .await
                .unwrap(),
            DailyLimitsLeft { new: 5, reviews: 5 }
        );
    }
}

#[cfg(test)]
mod learn_ahead_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::tests::LEARNING;
    use super::tests::create_cards;
    use super::tests::set_card;
    use super::*;
    use crate::api::offline_review::get_review_queue;
    use crate::api::review::get_review_card;
    use crate::api::review::next_review_card_available_at;
    use crate::parsers::get_all_parsers;
    use crate::schema::review::GetReviewCardRequest;
    use crate::schema::review::ReviewQueueRequest;

    /// The default learn ahead limit
    const LIMIT: Duration = Duration::minutes(20);

    async fn next_card(pool: &SqlitePool, now: DateTime<Utc>) -> Option<i64> {
        get_review_card(
            pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .map(|card| card.card_id)
    }

    async fn available_at(pool: &SqlitePool, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        next_review_card_available_at(pool, GetReviewCardRequest { filter: None }, now)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn learning_cards_wait_for_the_learn_ahead_limit(pool: SqlitePool) {
        // Midday, so that an hour later is still today
        let now = Local::now()
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap()
            .to_utc();
        let ids = create_cards(&pool, 1, now - Duration::days(1)).await;
        let card = ids[0];

        // Due within the limit: shown now
        set_card(&pool, card, LEARNING, now + Duration::minutes(5), 1.0, 5.0).await;
        assert_eq!(next_card(&pool, now).await, Some(card));

        // Due later: held back until it comes within the limit
        let due = now + Duration::hours(1);
        set_card(&pool, card, LEARNING, due, 1.0, 5.0).await;
        assert_eq!(next_card(&pool, now).await, None);
        assert_eq!(available_at(&pool, now).await, Some(due - LIMIT));
        assert_eq!(next_card(&pool, due - LIMIT).await, Some(card));

        // The offline queue has it anyway, since its client picks when to show it
        let queue = get_review_queue(
            &pool,
            ReviewQueueRequest {
                filter: None,
                limit: 10,
                days_ahead: 0,
            },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();
        assert_eq!(queue.cards.len(), 1);

        // Due tomorrow: done for today
        set_card(&pool, card, LEARNING, now + Duration::days(1), 1.0, 5.0).await;
        assert_eq!(next_card(&pool, now).await, None);
        assert_eq!(available_at(&pool, now).await, None);
    }
}

#[cfg(test)]
mod sibling_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::tests::REVIEW;
    use super::tests::set_card;
    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::review::rate_card_with_config;
    use crate::api::undo::redo_event;
    use crate::api::undo::undo_event;
    use crate::config::SparesExternalConfig;
    use crate::model::CardId;
    use crate::model::SpecialState;
    use crate::parsers::get_all_parsers;
    use crate::schedulers::get_scheduler_from_string;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::review::RatingSubmission;
    use crate::schema::undo::RedoEventRequest;
    use crate::schema::undo::UndoEventRequest;

    /// The cards of one note with four clozes, in order.
    async fn siblings(pool: &SqlitePool, now: DateTime<Utc>) -> Vec<CardId> {
        let parser = create_parser_helper(pool, "markdown").await;
        create_notes(
            pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "{{a}} {{b}} {{c}} {{d}}".to_string(),
                    keywords: Vec::new(),
                    tags: Vec::new(),
                    is_suspended: false,
                    custom_data: serde_json::Map::new(),
                }],
            },
            now - Duration::days(30),
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();
        sqlx::query_scalar(r#"SELECT id FROM card ORDER BY "order""#)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    async fn special_states(pool: &SqlitePool, ids: &[CardId]) -> Vec<Option<SpecialState>> {
        let mut states = Vec::new();
        for id in ids {
            states.push(
                sqlx::query_scalar(r"SELECT special_state FROM card WHERE id = ?")
                    .bind(id)
                    .fetch_one(pool)
                    .await
                    .unwrap(),
            );
        }
        states
    }

    async fn rate(
        pool: &SqlitePool,
        config: &SparesExternalConfig,
        card_id: CardId,
        now: DateTime<Utc>,
    ) {
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        rate_card_with_config(
            pool,
            scheduler.as_ref(),
            config,
            RatingSubmission {
                card_id,
                rating: 3,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(1),
                tag_id: None,
            },
            now,
            true,
        )
        .await
        .unwrap();
    }

    fn burying(new: bool, review: bool) -> SparesExternalConfig {
        let mut config = SparesExternalConfig::default();
        config.queue.bury_new_siblings = new;
        config.queue.bury_review_siblings = review;
        config
    }

    #[sqlx::test]
    async fn rating_buries_siblings_and_undo_unburies_them(pool: SqlitePool) {
        let now = Utc::now();
        let ids = siblings(&pool, now).await;
        let [rated, new, due_review, later_review] = ids[..] else {
            panic!("expected 4 cards, got {ids:?}")
        };
        set_card(&pool, due_review, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        set_card(
            &pool,
            later_review,
            REVIEW,
            now + Duration::days(5),
            5.0,
            5.0,
        )
        .await;
        let others = [new, due_review, later_review];
        let buried = Some(SpecialState::SchedulerBuried);

        rate(&pool, &burying(true, false), rated, now).await;
        assert_eq!(special_states(&pool, &others).await, [buried, None, None]);
        undo_event(
            &pool,
            UndoEventRequest {
                event_id: None,
                undo_group: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(special_states(&pool, &others).await, [None, None, None]);

        // Only review siblings due today are buried
        rate(&pool, &burying(true, true), rated, now).await;
        assert_eq!(special_states(&pool, &others).await, [buried, buried, None]);
        undo_event(
            &pool,
            UndoEventRequest {
                event_id: None,
                undo_group: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(special_states(&pool, &others).await, [None, None, None]);
        redo_event(
            &pool,
            RedoEventRequest {
                event_id: None,
                redo_group: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(special_states(&pool, &others).await, [buried, buried, None]);
    }

    #[sqlx::test]
    async fn siblings_are_not_buried_by_default_or_for_past_days(pool: SqlitePool) {
        let now = Utc::now();
        let ids = siblings(&pool, now).await;
        rate(&pool, &SparesExternalConfig::default(), ids[0], now).await;
        assert_eq!(special_states(&pool, &ids[1..]).await, [None, None, None]);
        // A review from yesterday, synced late
        rate(&pool, &burying(true, true), ids[1], now - Duration::days(1)).await;
        assert_eq!(special_states(&pool, &ids[2..]).await, [None, None]);
    }
}
