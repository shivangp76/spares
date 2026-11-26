//! How long reviews take, from the review log, for the time estimate of what is left of a review
//! (see [`get_cards_left_by_state_and_time_estimate`]).
//!
//! [`get_cards_left_by_state_and_time_estimate`]: crate::api::review::get_cards_left_by_state_and_time_estimate

use std::collections::HashMap;
use std::collections::HashSet;

use sqlx::AssertSqlSafe;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::statistics::local_time::Zone;
use crate::model::ReviewLogKind;
use crate::model::StateId;

/// The time of a review when there is nothing to go on, as on a new collection.
const DEFAULT_SECONDS_PER_REVIEW: f64 = 30.0;

/// The longest a review counts for in study pace and the time estimates of review when
/// [`SparesExternalConfig::maximum_answer_time`](crate::config::SparesExternalConfig::maximum_answer_time)
/// is unset. Longer ones are most likely of a card left open while away, which would make every
/// estimate after it too long.
pub(crate) const MAX_REVIEW_SECONDS: i64 = 15 * 60;

/// How many of the latest days a card was studied, of each preset and state, the pace is
/// measured over, so it follows how fast reviews are now rather than how fast they were when the
/// cards were new to the user.
const RECENT_CARD_DAYS: u32 = 200;

/// How many days of studying its cards a preset needs in a state for its own pace. With fewer,
/// the pace of every card in that state is used, so a preset studied a few times is not estimated
/// from one slow review.
const MIN_PRESET_CARD_DAYS: u32 = 10;

/// How many reviews in a state a card needs for its own time in that state to be used, rather
/// than its preset's pace. One review could be an outlier.
pub(crate) const MIN_CARD_REVIEWS: u32 = 2;

/// The `recall_duration` and `rate_duration` columns of `review_log` as `alias` (none if empty).
fn raw_duration_columns(alias: &str) -> (String, String) {
    let column = |name: &str| {
        if alias.is_empty() {
            name.to_string()
        } else {
            format!("{alias}.{name}")
        }
    };
    (column("recall_duration"), column("rate_duration"))
}

/// [`raw_duration_columns`] with `NULL` read as 0.
fn duration_columns(alias: &str) -> (String, String) {
    let (recall, rate) = raw_duration_columns(alias);
    (
        format!("COALESCE({recall}, 0)"),
        format!("COALESCE({rate}, 0)"),
    )
}

/// SQL for the seconds a review took, capped at `maximum` (see
/// [`SparesExternalConfig::maximum_answer_time`](crate::config::SparesExternalConfig::maximum_answer_time)),
/// from `review_log` columns of `alias` (none if empty). Reviews logged before the cap may be
/// longer, so it is applied on read. With no `maximum`, the stored seconds, `NULL` when either
/// column is.
pub(crate) fn capped_answer_seconds(alias: &str, maximum: Option<i64>) -> String {
    let Some(maximum) = maximum else {
        let (recall, rate) = raw_duration_columns(alias);
        return format!("({recall} + {rate})");
    };
    let (recall, rate) = duration_columns(alias);
    format!("MIN(MAX({recall}, 0) + MAX({rate}, 0), {maximum})")
}

/// SQL for the seconds of recall of a review, capped as
/// [`SparesExternalConfig::cap_answer_time`](crate::config::SparesExternalConfig::cap_answer_time)
/// caps them (see [`capped_answer_seconds`]).
pub(crate) fn capped_recall_seconds(alias: &str, maximum: Option<i64>) -> String {
    let Some(maximum) = maximum else {
        return raw_duration_columns(alias).0;
    };
    let (recall, _) = duration_columns(alias);
    format!("MIN(MAX({recall}, 0), {maximum})")
}

/// SQL for the seconds of rating of a review: what the recall leaves of `maximum` (see
/// [`capped_recall_seconds`]).
pub(crate) fn capped_rate_seconds(alias: &str, maximum: Option<i64>) -> String {
    let Some(maximum) = maximum else {
        return raw_duration_columns(alias).1;
    };
    let (recall, rate) = duration_columns(alias);
    format!("MIN(MAX({rate}, 0), {maximum} - MIN(MAX({recall}, 0), {maximum}))")
}

/// SQL for the seconds a review counts for in study pace and the time estimates of review:
/// [`capped_answer_seconds`], or with no `maximum`, at most [`MAX_REVIEW_SECONDS`] with `NULL`
/// read as 0.
pub(crate) fn pace_answer_seconds(alias: &str, maximum: Option<i64>) -> String {
    if maximum.is_some() {
        return capped_answer_seconds(alias, maximum);
    }
    let (recall, rate) = duration_columns(alias);
    format!("MIN({recall} + {rate}, {MAX_REVIEW_SECONDS})")
}

/// How long studying a card in some state takes on a day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StudyPace {
    pub(crate) seconds_per_review: f64,
    /// How many times a card is reviewed on a day it is studied, from this state: more than once
    /// for a new card that goes through its learning steps, or a review card rated Again and
    /// relearned.
    pub(crate) reviews_per_day: f64,
}

impl Default for StudyPace {
    fn default() -> Self {
        Self {
            seconds_per_review: DEFAULT_SECONDS_PER_REVIEW,
            reviews_per_day: 1.0,
        }
    }
}

/// The [`StudyPace`] of each preset and state, from the latest [`RECENT_CARD_DAYS`] days a card
/// was studied, of each. A day's state is the one the card was in at its first review that day,
/// and its reviews are all of that day's, so a new card's day counts its learning steps.
///
/// Presets are told apart since their cards can take very different times: a proof can take
/// minutes to work through where a word takes seconds.
#[derive(Debug, Default)]
pub(crate) struct StudyPaces {
    /// By preset (`None` for cards of no preset) and state, for those with at least
    /// [`MIN_PRESET_CARD_DAYS`].
    by_preset: HashMap<(Option<usize>, StateId), StudyPace>,
    /// By state, over every preset.
    by_state: HashMap<StateId, StudyPace>,
}

impl StudyPaces {
    /// The paces of the reviews of `cards`, the card table with each card's preset as a `preset`
    /// column (see [`cards_with_preset`](crate::api::preset::cards_with_preset)), told apart by
    /// local day.
    pub(crate) async fn load(
        db: &SqlitePool,
        cards: &str,
        zone: Zone,
        maximum_answer_seconds: Option<i64>,
        needed: &HashSet<(Option<usize>, StateId)>,
    ) -> Result<Self, Error> {
        if needed.is_empty() {
            return Ok(Self::default());
        }
        let error = |source| Error::Sqlx { source };
        // Most reads need no writer lock. If maintenance is needed, take the write
        // lock and reread the metadata, since another request may have refreshed it.
        for writable in [false, true] {
            let mut tx = if writable {
                db.begin_with("BEGIN IMMEDIATE").await
            } else {
                db.begin().await
            }
            .map_err(error)?;
            let paces = Self::from_cache(
                &mut tx,
                cards,
                zone,
                maximum_answer_seconds,
                needed,
                writable,
            )
            .await
            .map_err(error)?;
            tx.commit().await.map_err(error)?;
            if let Some(paces) = paces {
                return Ok(paces);
            }
        }
        unreachable!("a write transaction can refresh the cache")
    }

    async fn from_cache(
        conn: &mut sqlx::SqliteConnection,
        cards: &str,
        zone: Zone,
        maximum: Option<i64>,
        needed: &HashSet<(Option<usize>, StateId)>,
        writable: bool,
    ) -> Result<Option<Self>, sqlx::Error> {
        let (first, last): (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT (SELECT MIN(reviewed_at) FROM review_log), (SELECT MAX(reviewed_at) FROM review_log)"
        ).fetch_one(&mut *conn).await?;
        let (Some(first), Some(last)) = (first, last) else {
            return Ok(Some(Self::default()));
        };
        // Include complete local days. A sync can add an earlier review to the
        // oldest cached day, including when a zone or its day-start hour changes.
        let start_of_day = |at| {
            chrono::DateTime::from_timestamp(at, 0)
                .map_or(at, |at| zone.start_of(zone.date(at)).timestamp())
        };
        let earliest = start_of_day(first);
        let recent = chrono::DateTime::from_timestamp(last, 0)
            .and_then(|at| zone.date(at).checked_sub_days(chrono::Days::new(14)))
            .map_or(earliest, |day| zone.start_of(day).timestamp().max(earliest));
        let mut stored: Option<(i64, String)> =
            sqlx::query_as("SELECT since, rules FROM study_day_cache")
                .fetch_optional(&mut *conn)
                .await?;
        let mut since = stored
            .as_ref()
            .map_or(recent, |(start, _)| recent.min(start_of_day(*start)));
        let seconds = pace_answer_seconds("rl", maximum);
        let mut dirty: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM study_day_dirty)")
            .fetch_one(&mut *conn)
            .await?;
        loop {
            // The actual local-clock expression includes DST transitions and the day-start hour.
            let day = zone.clock(since, last).day("rl.reviewed_at");
            let rules = format!("{day}/{seconds}");
            let rebuild = stored
                .as_ref()
                .is_none_or(|(start, key)| *start != since || *key != rules);
            if rebuild || dirty {
                if !writable {
                    return Ok(None);
                }
                Self::refresh_days(conn, since, &day, &seconds, rebuild).await?;
                sqlx::query("INSERT OR REPLACE INTO study_day_cache VALUES (1, ?, ?)")
                    .bind(since)
                    .bind(&rules)
                    .execute(&mut *conn)
                    .await?;
                stored = Some((since, rules));
                dirty = false;
            }
            let mut paces = Self::default();
            let mut complete = true;
            for &(preset, state) in needed {
                let filter = preset.map_or_else(
                    || "AND c.preset IS NULL".to_owned(),
                    |index| format!("AND c.preset = {index}"),
                );
                let (days, pace) = recent_pace(conn, cards, state, &filter).await?;
                if days < RECENT_CARD_DAYS && since > first {
                    complete = false;
                    break;
                }
                if days >= MIN_PRESET_CARD_DAYS {
                    paces.by_preset.insert((preset, state), pace);
                } else if let std::collections::hash_map::Entry::Vacant(entry) =
                    paces.by_state.entry(state)
                {
                    let (_, overall) = recent_pace(conn, cards, state, "").await?;
                    entry.insert(overall);
                }
            }
            if complete {
                return Ok(Some(paces));
            }
            since = earliest;
        }
    }

    async fn refresh_days(
        conn: &mut sqlx::SqliteConnection,
        since: i64,
        day: &str,
        seconds: &str,
        rebuild: bool,
    ) -> Result<(), sqlx::Error> {
        let dirty_days = format!("SELECT rl.card_id, {day} FROM study_day_dirty rl");
        let select = if rebuild {
            sqlx::query("DELETE FROM study_day")
                .execute(&mut *conn)
                .await?;
            String::new()
        } else {
            let remove = format!("DELETE FROM study_day WHERE (card_id, day) IN ({dirty_days})");
            sqlx::query(AssertSqlSafe(remove))
                .execute(&mut *conn)
                .await?;
            // The first condition lets SQLite seek into the card's review history;
            // the second limits recomputation to the local days that actually changed.
            format!(
                "AND rl.card_id IN (SELECT card_id FROM study_day_dirty)
                AND (rl.card_id, {day}) IN ({dirty_days})"
            )
        };
        let review = ReviewLogKind::Review as u8;
        let insert = format!(
            "INSERT INTO study_day
            SELECT card_id, day, state, COUNT(*), SUM(seconds) FROM (
                SELECT rl.card_id, {day} AS day, {seconds} AS seconds,
                    FIRST_VALUE(rl.previous_state) OVER (
                        PARTITION BY rl.card_id, {day} ORDER BY rl.reviewed_at, rl.id
                    ) AS state
                FROM review_log rl WHERE rl.kind = {review} AND rl.card_id IS NOT NULL
                    AND rl.reviewed_at >= {since} {select}
            ) GROUP BY card_id, day"
        );
        sqlx::query(AssertSqlSafe(insert))
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM study_day_dirty")
            .execute(&mut *conn)
            .await?;
        Ok(())
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

/// The index walks newest days first. A card ID breaks ties on the same day, as
/// in the original grouping by card and day. Preset membership is always current.
async fn recent_pace(
    conn: &mut sqlx::SqliteConnection,
    cards: &str,
    state: StateId,
    filter: &str,
) -> Result<(u32, StudyPace), sqlx::Error> {
    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(reviews), 0), CAST(COALESCE(SUM(seconds), 0) AS REAL) FROM (
        SELECT d.reviews, d.seconds FROM study_day d JOIN {cards} c ON c.id = d.card_id
        WHERE d.state = ? {filter} ORDER BY d.day DESC, d.card_id LIMIT {RECENT_CARD_DAYS})"
    );
    let (days, reviews, seconds): (u32, u32, f64) = sqlx::query_as(AssertSqlSafe(sql))
        .bind(state)
        .fetch_one(conn)
        .await?;
    let pace = if days == 0 {
        StudyPace::default()
    } else {
        StudyPace {
            seconds_per_review: seconds / f64::from(reviews),
            reviews_per_day: f64::from(reviews) / f64::from(days),
        }
    };
    Ok((days, pace))
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
    use crate::api::queue::tests::LEARNING;
    use crate::api::queue::tests::NEW;
    use crate::api::queue::tests::REVIEW;
    use crate::api::queue::tests::local_midday;
    use crate::api::queue::tests::set_card;
    use crate::api::review::ReviewFilterQuery;
    use crate::api::review::get_cards_left_by_state_and_time_estimate;
    use crate::config::QueuePreset;
    use crate::config::SparesExternalConfig;
    use crate::helpers::get_start_end_local_date;
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

    #[sqlx::test]
    async fn sparse_preset_includes_older_card_days(pool: SqlitePool) {
        let now = local_midday();
        let ids = cards(&pool, &[&["math"], &[]], now).await;
        for days in 20..32 {
            review(&pool, ids[0], now - Duration::days(days), 2, 65).await;
        }
        review(&pool, ids[1], now, 2, 1).await;
        let paces = StudyPaces::load(
            &pool,
            &cards_with_preset(&math()),
            Zone::parse(Some("Europe/Berlin"), 4).unwrap(),
            None,
            &HashSet::from([(Some(0), 2)]),
        )
        .await
        .unwrap();
        assert_eq!(paces.of(Some(0), 2).seconds_per_review, 65.0);
    }

    #[sqlx::test]
    async fn cached_days_follow_edits_deletions_and_day_boundaries(pool: SqlitePool) {
        let at = DateTime::from_timestamp(1_767_225_600, 0).unwrap(); // Midnight UTC
        let id = cards(&pool, &[&[]], at).await[0];
        let load = async |hour, cap| {
            StudyPaces::load(
                &pool,
                &cards_with_preset(&math()),
                Zone::parse(Some("UTC"), hour).unwrap(),
                cap,
                &HashSet::from([(None, 2)]),
            )
            .await
            .unwrap()
            .of(None, 2)
        };
        review(&pool, id, at - Duration::minutes(10), 2, 10).await;
        review(&pool, id, at + Duration::minutes(10), 2, 30).await;
        assert_eq!(load(0, None).await.reviews_per_day, 1.);
        assert_eq!(load(4, None).await.reviews_per_day, 2.);
        assert_eq!(load(4, Some(10)).await.seconds_per_review, 10.);
        sqlx::query("UPDATE review_log SET recall_duration=70 WHERE recall_duration=30")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(load(4, None).await.seconds_per_review, 40.);
        sqlx::query("DELETE FROM review_log WHERE recall_duration=70")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(load(4, None).await.seconds_per_review, 10.);
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
            zone: crate::api::queue::tests::zone(),
            maximum_answer_seconds: SparesExternalConfig::default().maximum_answer_seconds(),
            caps: QueueCaps::default(),
            preset_sql: "NULL".to_string(),
            held_back_clause: None,
            query_sort: Vec::new(),
            filter_cards_clause: "TRUE".to_string(),
            waiting_clause: None,
            unblocking: "0".to_string(),
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

    /// A new card is estimated with the learning steps it goes through on its first day, and a
    /// card already reviewed today with one more review.
    #[sqlx::test]
    async fn a_day_counts_every_review_of_a_card(pool: SqlitePool) {
        // Midday, so that yesterday's reviews 10 minutes apart were both yesterday
        let now = local_midday();
        let ids = cards(&pool, &[&[], &[]], now - Duration::days(10)).await;
        // Seen as new and then once more in its learning steps, a minute and a half in all
        let yesterday = now - Duration::days(1);
        review(&pool, ids[0], yesterday, NEW, 60).await;
        review(
            &pool,
            ids[0],
            yesterday + Duration::minutes(10),
            LEARNING,
            30,
        )
        .await;
        not_due(&pool, ids[0], now).await;
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await,
            Duration::seconds(90)
        );

        // Seen once today, so it has one more review of its learning steps to go, at the 30
        // seconds learning cards have taken
        let (day_start, _) = get_start_end_local_date(&now, crate::api::queue::tests::zone());
        review(&pool, ids[1], day_start, NEW, 60).await;
        set_card(&pool, ids[1], LEARNING, day_start, 1.0, 5.0).await;
        assert_eq!(
            estimate(&pool, &IndexMap::new(), now).await,
            Duration::seconds(30)
        );
    }

    /// A learning card held back by the learn ahead limit is not counted, but is still to be
    /// reviewed today, so its time is.
    #[sqlx::test]
    async fn held_back_cards_take_time(pool: SqlitePool) {
        let now = Utc::now();
        let ids = cards(&pool, &[&[]], now - Duration::days(10)).await;
        set_card(&pool, ids[0], LEARNING, now + Duration::hours(1), 1.0, 5.0).await;
        let filter = ReviewFilterQuery {
            cards: cards_with_preset(&IndexMap::new()),
            where_clause: "c.due <= ?".to_string(),
            is_filtered_tag: false,
            card_due_limit: now,
            zone: crate::api::queue::tests::zone(),
            maximum_answer_seconds: SparesExternalConfig::default().maximum_answer_seconds(),
            caps: QueueCaps::default(),
            preset_sql: "NULL".to_string(),
            held_back_clause: Some(format!(
                "c.state = {LEARNING} AND c.due > {}",
                now.timestamp()
            )),
            query_sort: Vec::new(),
            filter_cards_clause: "TRUE".to_string(),
            waiting_clause: None,
            unblocking: "0".to_string(),
        };
        let (by_state, time) = get_cards_left_by_state_and_time_estimate(&pool, &filter)
            .await
            .unwrap();
        assert!(by_state.is_empty());
        assert_eq!(time, Duration::seconds(30));
    }

    /// Reviews logged before the cap are capped when read, as [`SparesExternalConfig::cap_answer_time`]
    /// caps them when they are logged.
    #[sqlx::test]
    async fn answer_seconds_are_capped_on_read(pool: SqlitePool) {
        for (recall, rate, expected) in [
            ("5", "2", (5, 2, 7)),
            ("50", "30", (50, 10, 60)),
            ("600", "30", (60, 0, 60)),
            ("NULL", "90", (0, 60, 60)),
            ("NULL", "NULL", (0, 0, 0)),
        ] {
            let sql = format!(
                "SELECT {}, {}, {} FROM (SELECT {recall} AS recall_duration, {rate} AS rate_duration) rl",
                capped_recall_seconds("rl", Some(60)),
                capped_rate_seconds("rl", Some(60)),
                capped_answer_seconds("rl", Some(60)),
            );
            let row: (i64, i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(row, expected, "{recall} {rate}");
        }
    }

    /// With no cap, the stored times are read as they are, `NULL` when either is, and study
    /// pace counts a review for at most [`MAX_REVIEW_SECONDS`].
    #[sqlx::test]
    async fn answer_seconds_are_not_capped_when_unset(pool: SqlitePool) {
        for (recall, rate, expected) in [
            ("600", "3000", (Some(600), Some(3000), Some(3600), 900)),
            ("-5", "2", (Some(-5), Some(2), Some(-3), -3)),
            ("NULL", "90", (None, Some(90), None, 90)),
        ] {
            let sql = format!(
                "SELECT {}, {}, {}, {} FROM (SELECT {recall} AS recall_duration, {rate} AS rate_duration) rl",
                capped_recall_seconds("rl", None),
                capped_rate_seconds("rl", None),
                capped_answer_seconds("rl", None),
                pace_answer_seconds("rl", None),
            );
            let row: (Option<i64>, Option<i64>, Option<i64>, i64) =
                sqlx::query_as(sqlx::AssertSqlSafe(sql))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(row, expected, "{recall} {rate}");
        }
    }

    /// A card left open for an hour counts for [`MAX_REVIEW_SECONDS`] with no
    /// [`SparesExternalConfig::maximum_answer_time`].
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
