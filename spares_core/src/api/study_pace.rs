//! How long reviews take, from the review log, for the time estimate of what is left of a review
//! (see [`get_cards_left_by_state_and_time_estimate`]).
//!
//! [`get_cards_left_by_state_and_time_estimate`]: crate::api::review::get_cards_left_by_state_and_time_estimate

use std::collections::HashMap;

use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::preset::preset_index;
use crate::model::ReviewLogKind;
use crate::model::StateId;

/// The time of a review when there is nothing to go on, as on a new collection.
const DEFAULT_SECONDS_PER_REVIEW: f64 = 30.0;

/// The longest a review counts for. Longer ones are most likely of a card left open while away,
/// which would make every estimate after it too long.
pub(crate) const MAX_REVIEW_SECONDS: i64 = 15 * 60;

/// How many of the latest reviews of each preset and state the pace is measured over, so it
/// follows how fast reviews are now rather than how fast they were when the cards were new to
/// the user.
const RECENT_REVIEWS: u32 = 200;

/// How many reviews a preset needs in a state for its own pace. With fewer, the pace of every
/// card in that state is used, so a preset studied a few times is not estimated from one slow
/// review.
const MIN_PRESET_REVIEWS: u32 = 10;

/// How many reviews in a state a card needs for its own time in that state to be used, rather
/// than its preset's pace. One review could be an outlier.
pub(crate) const MIN_CARD_REVIEWS: u32 = 2;

/// SQL for the seconds a review took, capped at [`MAX_REVIEW_SECONDS`], from `review_log` columns
/// prefixed with `alias`.
pub(crate) fn capped_review_seconds(alias: &str) -> String {
    format!(
        "MIN(COALESCE({alias}.recall_duration, 0) + COALESCE({alias}.rate_duration, 0), {MAX_REVIEW_SECONDS})"
    )
}

/// How long a review of a card in some state takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StudyPace {
    pub(crate) seconds_per_review: f64,
}

impl Default for StudyPace {
    fn default() -> Self {
        Self {
            seconds_per_review: DEFAULT_SECONDS_PER_REVIEW,
        }
    }
}

/// The [`StudyPace`] of each preset and state, from the latest [`RECENT_REVIEWS`] reviews of each.
///
/// Presets are told apart since their cards can take very different times: a proof can take
/// minutes to work through where a word takes seconds.
#[derive(Debug, Default)]
pub(crate) struct StudyPaces {
    /// By preset (`None` for cards of no preset) and state, for those with at least
    /// [`MIN_PRESET_REVIEWS`] reviews.
    by_preset: HashMap<(Option<usize>, StateId), StudyPace>,
    /// By state, over every preset.
    by_state: HashMap<StateId, StudyPace>,
}

impl StudyPaces {
    /// The paces of the reviews of `cards`, the card table with each card's preset as a `preset`
    /// column (see [`cards_with_preset`](crate::api::preset::cards_with_preset)). The state of a
    /// review is the one the card was in when reviewed.
    pub(crate) async fn load(db: &SqlitePool, cards: &str) -> Result<Self, Error> {
        let seconds = capped_review_seconds("rl");
        let review = ReviewLogKind::Review as u8;
        let query = format!(
            r"WITH ranked AS (
                SELECT c.preset, rl.previous_state AS state, {seconds} AS seconds,
                    ROW_NUMBER() OVER (
                        PARTITION BY c.preset, rl.previous_state ORDER BY rl.reviewed_at DESC
                    ) AS preset_rank,
                    ROW_NUMBER() OVER (
                        PARTITION BY rl.previous_state ORDER BY rl.reviewed_at DESC
                    ) AS state_rank
                FROM review_log rl JOIN {cards} c ON c.id = rl.card_id
                WHERE rl.kind = {review}
            )
            SELECT 1, preset, state, COUNT(*), AVG(seconds) FROM ranked
            WHERE preset_rank <= {RECENT_REVIEWS} GROUP BY preset, state
            UNION ALL
            SELECT 0, NULL, state, COUNT(*), AVG(seconds) FROM ranked
            WHERE state_rank <= {RECENT_REVIEWS} GROUP BY state"
        );
        let rows: Vec<(bool, Option<i64>, StateId, u32, f64)> = sqlx::query_as(&query)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let mut paces = Self::default();
        for (of_preset, preset, state, reviews, seconds_per_review) in rows {
            let pace = StudyPace { seconds_per_review };
            if !of_preset {
                paces.by_state.insert(state, pace);
            } else if reviews >= MIN_PRESET_REVIEWS {
                paces.by_preset.insert((preset_index(preset), state), pace);
            }
        }
        Ok(paces)
    }

    /// The pace of cards of `preset` in `state`: the preset's own, else that of every card in
    /// `state`, else [`StudyPace::default`].
    pub(crate) fn of(&self, preset: Option<usize>, state: StateId) -> StudyPace {
        self.by_preset
            .get(&(preset, state))
            .or_else(|| self.by_state.get(&state))
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use chrono::Duration;
    use chrono::Utc;
    use indexmap::IndexMap;

    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::preset::cards_with_preset;
    use crate::api::queue::QueueCaps;
    use crate::api::queue::tests::NEW;
    use crate::api::queue::tests::REVIEW;
    use crate::api::queue::tests::set_card;
    use crate::api::review::ReviewFilterQuery;
    use crate::api::review::get_cards_left_by_state_and_time_estimate;
    use crate::config::QueuePreset;
    use crate::model::CardId;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;

    /// A `math` preset, for notes tagged `math`.
    fn math() -> IndexMap<String, QueuePreset> {
        IndexMap::from([(
            "math".to_string(),
            QueuePreset {
                tags: vec!["math".to_string()],
                ..Default::default()
            },
        )])
    }

    /// One new card for each list of tags, in order.
    async fn cards(pool: &SqlitePool, tags: &[&[&str]], now: DateTime<Utc>) -> Vec<CardId> {
        let parser = create_parser_helper(pool, "markdown").await;
        let requests = tags
            .iter()
            .enumerate()
            .map(|(i, tags)| CreateNoteRequest {
                data: format!("Note {i}: {{{{answer {i}}}}}"),
                keywords: Vec::new(),
                tags: tags.iter().map(ToString::to_string).collect(),
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
        sqlx::query_scalar(r"SELECT c.id FROM card c JOIN note n ON c.note_id = n.id ORDER BY n.id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// Records a review of `card_id` in `state` at `at`, taking `seconds`.
    async fn review(
        pool: &SqlitePool,
        card_id: CardId,
        at: DateTime<Utc>,
        state: i64,
        seconds: i64,
    ) {
        sqlx::query(
            r"INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name,
            scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
            VALUES (?, ?, 0, 3, 'fsrs', 0, ?, 0, ?, '{}')",
        )
        .bind(card_id)
        .bind(at.timestamp())
        .bind(seconds)
        .bind(state)
        .execute(pool)
        .await
        .unwrap();
    }

    /// The estimate of the cards due by `now`, with `presets`.
    async fn estimate(
        pool: &SqlitePool,
        presets: &IndexMap<String, QueuePreset>,
        now: DateTime<Utc>,
    ) -> Duration {
        let filter = ReviewFilterQuery {
            cards: cards_with_preset(presets),
            where_clause: "c.due <= ?".to_string(),
            is_filtered_tag: false,
            card_due_limit: now,
            caps: QueueCaps::default(),
            preset_sql: "NULL".to_string(),
            held_back_clause: None,
            query_sort: Vec::new(),
        };
        get_cards_left_by_state_and_time_estimate(pool, &filter)
            .await
            .unwrap()
            .1
    }

    /// Hides `card_id` from the estimate, for cards only there for their reviews.
    async fn not_due(pool: &SqlitePool, card_id: CardId, now: DateTime<Utc>) {
        set_card(pool, card_id, REVIEW, now + Duration::days(30), 5.0, 5.0).await;
    }

    #[sqlx::test]
    async fn new_cards_take_as_long_as_new_cards_have_lately(pool: SqlitePool) {
        let now = Utc::now();
        let ids = cards(&pool, &[&[], &[], &[]], now - Duration::days(10)).await;
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await,
            Duration::seconds(3 * 30)
        );

        review(&pool, ids[0], now - Duration::days(2), NEW, 60).await;
        review(&pool, ids[1], now - Duration::days(1), NEW, 120).await;
        not_due(&pool, ids[0], now).await;
        not_due(&pool, ids[1], now).await;
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await,
            Duration::seconds(90)
        );
    }

    /// A card's slow first sight as a new card does not count toward its reviews.
    #[sqlx::test]
    async fn a_cards_own_time_is_of_its_state_now(pool: SqlitePool) {
        let now = Utc::now();
        let ids = cards(&pool, &[&[]], now - Duration::days(30)).await;
        review(&pool, ids[0], now - Duration::days(20), NEW, 600).await;
        review(&pool, ids[0], now - Duration::days(10), REVIEW, 10).await;
        set_card(&pool, ids[0], REVIEW, now - Duration::hours(1), 5.0, 5.0).await;
        // One review of its own is not enough, so it takes as long as reviews have
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await,
            Duration::seconds(10)
        );

        review(&pool, ids[0], now - Duration::days(5), REVIEW, 20).await;
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await,
            Duration::seconds(15)
        );
    }

    #[sqlx::test]
    async fn presets_have_their_own_pace(pool: SqlitePool) {
        let now = Utc::now();
        let tags: Vec<&[&str]> = [&["math"][..]; 11]
            .into_iter()
            .chain([&[][..]; 2])
            .collect();
        let ids = cards(&pool, &tags, now - Duration::days(30)).await;
        // Ten slow new math cards, and one quick new card of no preset
        for (i, card) in ids[..10].iter().enumerate() {
            review(
                &pool,
                *card,
                now - Duration::hours(i64::try_from(i).unwrap() + 1),
                NEW,
                200,
            )
            .await;
            not_due(&pool, *card, now).await;
        }
        review(&pool, ids[11], now - Duration::hours(1), NEW, 90).await;
        not_due(&pool, ids[11], now).await;
        // With no presets, both new cards take as long as every new card has
        let average = (10 * 200 + 90) / 11;
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await.num_seconds(),
            2 * average
        );
        // The math card takes as long as math cards have, and the other one as long as cards of no
        // preset, which has too few reviews of its own for its own pace
        assert_eq!(
            estimate(&pool, &math(), now).await.num_seconds(),
            200 + average
        );
    }

    /// A card left open for an hour counts for 15 minutes.
    #[sqlx::test]
    async fn long_reviews_are_capped(pool: SqlitePool) {
        let now = Utc::now();
        let ids = cards(&pool, &[&[], &[]], now - Duration::days(10)).await;
        review(&pool, ids[0], now - Duration::days(1), NEW, 3600).await;
        not_due(&pool, ids[0], now).await;
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await,
            Duration::seconds(MAX_REVIEW_SECONDS)
        );
    }
}
