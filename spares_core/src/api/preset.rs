//! Which [preset](crate::config::QueuePreset) each card uses, and what that changes.
//!
//! A card's preset is the first of [`QueueConfig::presets`] that matches one of its note's tags.
//! It is found in SQL ([`preset_sql`]), so the queue can sort, filter and count by it without
//! loading cards, and read back in Rust where a card is rated ([`CardConfigs`]).
//!
//! What a preset changes, and where:
//!
//! - Its daily limits ([`PresetLimitsLeft`]) are computed with the top-level ones by
//!   [`daily_limits_left`](crate::api::review::daily_limits_left). Review leaves out a preset's
//!   new or review cards once its own limit is used up, and [`cap_counts`] caps the counts of
//!   what is left by both.
//! - Its orders are applied per card by [`queue_order_by`](crate::api::queue::queue_order_by).
//! - Its steps and sibling burying are applied when one of its cards is rated or replayed,
//!   through [`SparesExternalConfig::for_preset`].
//!
//! For example, with presets `languages` (tag `lang`) and `exam` (tag `math:analysis`), in that
//! order, a card of a note tagged `lang:german` and `math:analysis` uses `languages` (index 0), one
//! tagged `math:analysis:limits` uses `exam` (index 1), and one tagged `math` alone uses none.

use std::borrow::Cow;
use std::collections::HashMap;

use indexmap::IndexMap;
use itertools::Itertools;

use crate::Error;
use crate::api::queue::QueueCaps;
use crate::config::QueueConfig;
use crate::config::QueuePreset;
use crate::config::SparesExternalConfig;
use crate::model::CardId;
use crate::model::NEW_CARD_STATE;
use crate::model::StateId;
use crate::schedulers::review_state;

/// `text` as a SQL string literal.
fn sql_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A SQL expression for the index in `presets` of the preset that card `c` uses, or `NULL` if it
/// uses none. It only refers to the `c` alias, so it fits any query over cards.
///
/// A tag matches itself and its children, compared exactly: `lang` matches `lang` and
/// `lang:german`, but not `Lang` or `language`. `substr` rather than `LIKE`, since `_` and `%` are
/// wildcards in a `LIKE` pattern and `LIKE` ignores case.
pub(crate) fn preset_sql(presets: &IndexMap<String, QueuePreset>) -> String {
    if presets.is_empty() {
        return "NULL".to_string();
    }
    let whens = presets
        .values()
        .enumerate()
        .map(|(index, preset)| {
            let tags = preset
                .tags
                .iter()
                .map(|tag| {
                    let prefix = format!("{tag}:");
                    format!(
                        "t.name = {} OR substr(t.name, 1, {}) = {}",
                        sql_string(tag),
                        prefix.chars().count(),
                        sql_string(&prefix)
                    )
                })
                .join(" OR ");
            format!(
                "WHEN EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON t.id = nt.tag_id \
                 WHERE nt.note_id = c.note_id AND ({tags})) THEN {index}"
            )
        })
        .join(" ");
    format!("(CASE {whens} END)")
}

/// [`preset_sql`] if some preset has daily limits of its own, and `NULL` otherwise, so queries that
/// only need the preset for its limits skip looking it up.
pub(crate) fn limited_preset_sql(queue: &QueueConfig) -> String {
    if queue.presets.values().any(QueuePreset::has_limits) {
        preset_sql(&queue.presets)
    } else {
        "NULL".to_string()
    }
}

/// [`card_presets_where`] of every card.
#[cfg(test)]
pub(crate) async fn card_presets<'e, E>(
    db: E,
    preset: &str,
) -> Result<HashMap<CardId, usize>, Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    card_presets_where(db, preset, "").await
}

/// The preset of each card matching `card_filter` (a condition starting with `AND `) that uses
/// one, from `preset`, the SQL of a card's preset (such as [`preset_sql`]). Empty, without a
/// query, if `preset` is `NULL`.
async fn card_presets_where<'e, E>(
    db: E,
    preset: &str,
    card_filter: &str,
) -> Result<HashMap<CardId, usize>, Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    if preset == "NULL" {
        return Ok(HashMap::new());
    }
    let rows: Vec<(CardId, Option<i64>)> = sqlx::query_as(&format!(
        "SELECT id, preset FROM (SELECT c.id, {preset} AS preset FROM card c WHERE 1 {card_filter}) \
         WHERE preset IS NOT NULL"
    ))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(rows
        .into_iter()
        .filter_map(|(card_id, preset)| Some((card_id, preset_index(preset)?)))
        .collect())
}

/// What is left of one preset's own daily limits. `None` where the preset sets no limit, so only
/// the top-level limit applies to its cards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PresetLimitsLeft {
    pub(crate) new: Option<u32>,
    pub(crate) reviews: Option<u32>,
}

impl PresetLimitsLeft {
    /// What is left for cards in `state`. Learning and relearning cards are never limited.
    pub(crate) fn of_state(self, state: StateId) -> Option<u32> {
        if state == NEW_CARD_STATE {
            self.new
        } else if state == review_state() {
            self.reviews
        } else {
            None
        }
    }
}

/// Converts the preset index SQL returns (see [`preset_sql`]) to an index into
/// [`QueueConfig::presets`].
pub(crate) fn preset_index(preset: Option<i64>) -> Option<usize> {
    preset.and_then(|index| usize::try_from(index).ok())
}

/// How many cards today's limits let through, out of `rows` of cards counted by state and preset
/// (with a weight, such as the time they take), by state: the count, and the weight of the cards
/// counted.
///
/// Each preset's cards are capped by what is left of its own limit, then each state's total by
/// what is left of the top-level one. For example, with 30 new `languages` cards and 30 others,
/// 10 left of the `languages` limit and 25 of the top-level one, 25 are let through: 10 of
/// `languages` and 30 others would make 40. Weights are scaled down with the counts, in
/// proportion. States left with nothing are left out.
pub(crate) fn cap_counts(
    rows: impl IntoIterator<Item = (StateId, Option<usize>, u32, f64)>,
    caps: &QueueCaps,
) -> HashMap<StateId, (u32, f64)> {
    let mut by_state: HashMap<StateId, (u32, f64)> = HashMap::new();
    for (state, preset, count, weight) in rows {
        if count == 0 {
            continue;
        }
        let cap = preset
            .and_then(|index| caps.presets.get(index))
            .and_then(|left| left.of_state(state));
        let capped = cap.map_or(count, |cap| count.min(cap));
        let entry = by_state.entry(state).or_default();
        entry.0 += capped;
        entry.1 += weight * f64::from(capped) / f64::from(count);
    }
    for (state, (count, weight)) in &mut by_state {
        if let Some(cap) = caps.of_state(*state)
            && *count > cap
        {
            *weight *= f64::from(cap) / f64::from(*count);
            *count = cap;
        }
    }
    by_state.retain(|_, (count, _)| *count > 0);
    by_state
}

/// The settings each card is rated or replayed with: the top-level ones, with the steps and
/// sibling burying of the card's preset (see [`SparesExternalConfig::for_preset`]).
///
/// For example, with a `languages` preset whose `learning_steps = [60, 300, 1800]`, a card of a
/// note tagged `lang:german` that is rated Good on its first step waits 5 minutes for the next,
/// while a card of an untagged note waits the top-level 10 minutes.
pub struct CardConfigs<'a> {
    config: &'a SparesExternalConfig,
    /// [`SparesExternalConfig::for_preset`] of each preset, by index
    by_preset: Vec<Cow<'a, SparesExternalConfig>>,
    /// The preset of each card loaded that uses one
    presets: HashMap<CardId, usize>,
}

impl<'a> CardConfigs<'a> {
    /// The settings of `card_ids`, or of every card if `None`. Cards are only looked up if some
    /// preset changes how cards are rated.
    pub(crate) async fn load<'e, E>(
        db: E,
        config: &'a SparesExternalConfig,
        card_ids: Option<&[CardId]>,
    ) -> Result<Self, Error>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        let by_preset = (0..config.queue.presets.len())
            .map(|index| config.for_preset(Some(index)))
            .collect::<Vec<_>>();
        if by_preset
            .iter()
            .all(|preset_config| matches!(preset_config, Cow::Borrowed(_)))
        {
            return Ok(Self {
                config,
                by_preset,
                presets: HashMap::new(),
            });
        }
        let card_filter = card_ids.map_or_else(String::new, |ids| {
            format!("AND c.id IN ({})", ids.iter().join(", "))
        });
        let presets =
            card_presets_where(db, &preset_sql(&config.queue.presets), &card_filter).await?;
        Ok(Self {
            config,
            by_preset,
            presets,
        })
    }

    /// The settings `card_id` is rated with. The top-level ones for a card that was not loaded.
    pub(crate) fn get(&self, card_id: CardId) -> &SparesExternalConfig {
        self.presets
            .get(&card_id)
            .and_then(|index| self.by_preset.get(*index))
            .map_or(self.config, AsRef::as_ref)
    }

    /// The top-level settings, for what presets cannot change, such as placement.
    pub(crate) fn base(&self) -> &SparesExternalConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presets(toml: &str) -> IndexMap<String, QueuePreset> {
        let config: SparesExternalConfig = toml_edit::de::from_str(toml).unwrap();
        config.queue.presets
    }

    #[test]
    fn no_presets_is_null() {
        assert_eq!(preset_sql(&IndexMap::new()), "NULL");
    }

    #[test]
    fn tags_are_quoted() {
        let sql = preset_sql(&presets(
            "[queue.presets.quoted]\ntags = [\"it's\", \"naïve_tag\"]\n",
        ));
        assert!(sql.contains("t.name = 'it''s'"), "{sql}");
        // Lengths are in characters, as `substr` counts them
        assert!(
            sql.contains("substr(t.name, 1, 10) = 'naïve_tag:'"),
            "{sql}"
        );
    }

    #[test]
    fn caps_apply_per_preset_then_in_total() {
        let caps = QueueCaps {
            new: Some(25),
            reviews: None,
            presets: vec![PresetLimitsLeft {
                new: Some(10),
                reviews: None,
            }],
        };
        let review = review_state();
        let counted = cap_counts(
            [
                (NEW_CARD_STATE, Some(0), 30, 30.0),
                (NEW_CARD_STATE, None, 30, 60.0),
                (review, Some(0), 4, 8.0),
            ],
            &caps,
        );
        // 10 of the preset's (10 seconds) and 30 others (60 seconds), scaled to 25 of 40
        let (count, time) = counted[&NEW_CARD_STATE];
        assert_eq!(count, 25);
        assert!((time - 70.0 * 25.0 / 40.0).abs() < 1e-9, "{time}");
        assert_eq!(counted[&review], (4, 8.0));
        // A used up preset limit drops its cards
        let caps = QueueCaps {
            presets: vec![PresetLimitsLeft {
                new: Some(0),
                reviews: None,
            }],
            ..QueueCaps::default()
        };
        assert!(cap_counts([(NEW_CARD_STATE, Some(0), 3, 1.0)], &caps).is_empty());
    }
}
