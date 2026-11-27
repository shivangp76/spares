//! Retention per tag, for the Tags page: the mean predicted retrievability of each tag's reviewed
//! cards today and a week from now, with the scheduler's own forgetting curve, so the numbers match
//! the Statistics page and `c.retrievability` in searches.
//!
//! A parent tag counts its children's cards, as `tag=math` finds them, each card once however many
//! of its tags are under the parent. A card's tags are its note's and its own (`card_tag`).
//! Filtered tags are left out. Cards are those the Statistics page counts: not new, with a review
//! (crams and forgets aside).
//!
//! It is one pass: each card's retrievability is worked out once, added to each of its tags and
//! their ancestors, which come from splitting the tag names at `:`, rather than joining each tag to
//! its descendants in SQL.
use std::collections::HashMap;
use std::collections::HashSet;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use crate::Error;
use crate::config::read_external_config;
use crate::model::CardId;
use crate::model::ReviewLogKind;
use crate::schedulers::CardMemory;
use crate::schedulers::get_scheduler_from_string;
use crate::schedulers::state_ids_sql;
use crate::schema::review::StateRole;

/// How far ahead the second figure looks.
pub const HORIZON_DAYS: f64 = 7.0;

/// The retention of one tag's reviewed cards, its descendants' included.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TagRetention {
    pub name: String,
    pub card_count: u32,
    /// The mean predicted retrievability now, from 0 to 1
    pub today: f64,
    /// ... and in [`HORIZON_DAYS`] days, without reviews in between
    pub week: f64,
}

/// A reviewed card's memory and last review.
type CardRow = (CardId, f64, f64, f64, i64);

/// The retention of every tag with reviewed cards, by name (see the module docs), sorted by name.
/// Empty for a scheduler without a forgetting curve.
pub async fn tag_retention(
    db: &SqlitePool,
    scheduler_name: &str,
    now: DateTime<Utc>,
) -> Result<Vec<TagRetention>, Error> {
    let scheduler = get_scheduler_from_string(scheduler_name)?;
    let config = read_external_config()?;
    let Some(retrievability) = scheduler.retrievability(&config) else {
        return Ok(Vec::new());
    };
    let new = state_ids_sql(&[StateRole::New]);
    let review = ReviewLogKind::Review as u8;
    // As one JSON array each, which is several times faster to read than a row each
    let cards_sql = format!(
        "SELECT json_group_array(json_array(c.id, c.stability, c.stability_fast, c.difficulty, last))
         FROM (SELECT c.*, (SELECT MAX(rl.reviewed_at) FROM review_log rl
                            WHERE rl.card_id = c.id AND rl.kind = {review}) AS last
               FROM card c WHERE c.state NOT IN ({new})) c
         WHERE last IS NOT NULL"
    );
    let tags_sql = format!(
        "SELECT json_group_array(json_array(card_id, name)) FROM (
             SELECT c.id AS card_id, t.name FROM card c
             JOIN note_tag nt ON nt.note_id = c.note_id JOIN tag t ON t.id = nt.tag_id
             WHERE t.query IS NULL AND c.state NOT IN ({new})
             UNION
             SELECT ct.card_id, t.name FROM card_tag ct JOIN card c ON c.id = ct.card_id
             JOIN tag t ON t.id = ct.tag_id
             WHERE t.query IS NULL AND c.state NOT IN ({new}))"
    );
    let decode = |e: serde_json::Error| Error::Sqlx {
        source: sqlx::Error::Decode(Box::new(e)),
    };
    let (cards,): (String,) = sqlx::query_as(AssertSqlSafe(cards_sql))
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let (tags,): (String,) = sqlx::query_as(AssertSqlSafe(tags_sql))
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let cards: Vec<CardRow> = serde_json::from_str(&cards).map_err(decode)?;
    let tags: Vec<(CardId, String)> = serde_json::from_str(&tags).map_err(decode)?;
    let now = now.timestamp();
    Ok(roll_up(
        cards
            .into_iter()
            .map(|(id, stability, stability_fast, difficulty, last)| {
                #[expect(clippy::cast_precision_loss, reason = "seconds are far below 2^52")]
                let elapsed = (now - last).max(0) as f64 / 86_400.0;
                let memory = CardMemory {
                    stability,
                    stability_fast,
                    difficulty,
                };
                (
                    id,
                    retrievability(memory, elapsed),
                    retrievability(memory, elapsed + HORIZON_DAYS),
                )
            }),
        tags,
    ))
}

/// Adds each card's retrievability today and in a week to each of its tags and their ancestors,
/// once per tag name however many of the card's tags it is an ancestor of.
fn roll_up(
    cards: impl IntoIterator<Item = (CardId, f64, f64)>,
    tags: Vec<(CardId, String)>,
) -> Vec<TagRetention> {
    let mut tags_of: HashMap<CardId, Vec<String>> = HashMap::new();
    for (card, tag) in tags {
        tags_of.entry(card).or_default().push(tag);
    }
    let mut sums: HashMap<String, (u32, f64, f64)> = HashMap::new();
    let mut names: HashSet<&str> = HashSet::new();
    for (card, today, week) in cards {
        let Some(tags) = tags_of.get(&card) else {
            continue;
        };
        names.clear();
        for tag in tags {
            // `a:b:c` counts for `a`, `a:b` and `a:b:c`
            names.extend(
                tag.match_indices(':')
                    .map(|(at, _)| &tag[..at])
                    .chain([tag.as_str()]),
            );
        }
        for name in &names {
            let sum = sums.entry((*name).to_string()).or_default();
            sum.0 += 1;
            sum.1 += today;
            sum.2 += week;
        }
    }
    let mut retention: Vec<TagRetention> = sums
        .into_iter()
        .map(|(name, (card_count, today, week))| TagRetention {
            name,
            card_count,
            today: today / f64::from(card_count),
            week: week / f64::from(card_count),
        })
        .collect();
    retention.sort_by(|a, b| a.name.cmp(&b.name));
    retention
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn tag(card: CardId, name: &str) -> (CardId, String) {
        (card, name.to_string())
    }

    #[test]
    fn a_parent_counts_each_card_under_it_once() {
        let retention = roll_up(
            [(1, 0.9, 0.8), (2, 0.5, 0.4), (3, 0.7, 0.6)],
            vec![
                tag(1, "math:algebra"),
                tag(1, "math:calculus"),
                tag(2, "math:calculus:limits"),
                tag(3, "spanish"),
                // A card without a review isn't in `cards`
                tag(4, "math"),
            ],
        );
        let by_name: HashMap<&str, &TagRetention> =
            retention.iter().map(|r| (r.name.as_str(), r)).collect();
        assert_eq!(
            by_name["math"].card_count, 2,
            "card 1 once, under two children"
        );
        assert!((by_name["math"].today - 0.7).abs() < 1e-9);
        assert!((by_name["math"].week - 0.6).abs() < 1e-9);
        assert_eq!(by_name["math:calculus"].card_count, 2);
        assert_eq!(by_name["math:calculus:limits"].card_count, 1);
        assert_eq!(by_name["math:algebra"].card_count, 1);
        assert_eq!(by_name["spanish"].card_count, 1);
        assert_eq!(retention.len(), 5);
    }

    #[sqlx::test]
    async fn retention_follows_the_scheduler_curve_and_skips_filtered_tags(pool: SqlitePool) {
        let now = Utc::now();
        crate::api::scheduler::tests::seed_review_history(&pool, 3, now - Duration::days(40)).await;
        let note: i64 = sqlx::query_scalar("SELECT note_id FROM card LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        for (name, query) in [("bio:cells", None), ("due", Some("c.due<0"))] {
            sqlx::query(
                "INSERT INTO tag (name, description, query, auto_delete, reschedule, created_at, updated_at, usn)
                 VALUES (?, '', ?, 0, 0, 0, 0, -1)",
            )
            .bind(name)
            .bind(query)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO note_tag (note_id, tag_id) SELECT ?, id FROM tag WHERE name = ?",
            )
            .bind(note)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        }
        let retention = tag_retention(&pool, "fsrs", now).await.unwrap();
        let names: Vec<&str> = retention.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["bio", "bio:cells"], "the filtered tag is left out");
        let cells = &retention[1];
        assert_eq!(cells.card_count, 3);
        assert!(cells.today > 0.0 && cells.today <= 1.0);
        assert!(cells.week < cells.today, "memory fades over the week");
    }

    /// Times it on a copy of a real collection: `SPARES_TAG_RETENTION_BENCH=<dir>`, a directory
    /// with `collection.sqlite` and `config/config.toml`, and
    /// `cargo test --release -p spares_core --lib -- --ignored tag_retention_bench --nocapture`.
    #[tokio::test]
    #[ignore = "needs a collection to time"]
    async fn tag_retention_bench() {
        let dir = std::path::PathBuf::from(std::env::var("SPARES_TAG_RETENTION_BENCH").unwrap());
        let pool = SqlitePool::connect(&format!(
            "sqlite://{}",
            dir.join("collection.sqlite").display()
        ))
        .await
        .unwrap();
        let collection = std::sync::Arc::new(crate::collection::Collection::new(
            "bench",
            pool.clone(),
            crate::collection::CollectionPaths::in_dir(&dir),
        ));
        collection
            .scope(async {
                let mut times = Vec::new();
                let mut tags = 0;
                for _ in 0..21 {
                    let start = std::time::Instant::now();
                    tags = tag_retention(&pool, "fsrs", Utc::now())
                        .await
                        .unwrap()
                        .len();
                    times.push(start.elapsed());
                }
                times.sort();
                println!(
                    "tag retention: {tags} tags, median {:?}, min {:?}, max {:?}",
                    times[10], times[0], times[20]
                );
            })
            .await;
    }
}
