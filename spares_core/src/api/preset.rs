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
use sqlx::AssertSqlSafe;

use crate::Error;
use crate::api::queue::QueueCaps;
use crate::config::QueueConfig;
use crate::config::QueuePreset;
use crate::config::SparesExternalConfig;
use crate::model::CardId;
use crate::model::StateId;
use crate::schedulers::state_role;
use crate::schema::review::StateRole;

/// `text` as a SQL string literal.
pub(crate) fn sql_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A SQL expression for the index in `presets` of the preset that card `c` uses, or `NULL` if it
/// uses none. It only refers to the `c` alias, so it fits any query over cards.
///
/// A tag matches itself and its children, compared exactly: `lang` matches `lang` and
/// `lang:german`, but not `Lang` or `language`. `substr` rather than `LIKE`, since `_` and `%` are
/// wildcards in a `LIKE` pattern and `LIKE` ignores case.
///
/// The first preset that matches is the lowest index that any of the note's tags matches, so the
/// note's tags are read once rather than once per preset.
///
/// SQLite evaluates the subquery each time it appears in a query, so a query that refers to the
/// preset more than once should select from [`cards_with_preset`] instead.
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
            format!("WHEN {tags} THEN {index}")
        })
        .join(" ");
    format!(
        "(SELECT MIN(CASE {whens} END) FROM note_tag nt JOIN tag t ON t.id = nt.tag_id \
         WHERE nt.note_id = c.note_id)"
    )
}

/// The `card` table with each card's preset ([`preset_sql`]) as a `preset` column, to select
/// from as `c`. A query that refers to `c.preset` several times, like the queue's `ORDER BY`,
/// then looks up each card's preset once.
pub(crate) fn cards_with_preset(presets: &IndexMap<String, QueuePreset>) -> String {
    format!(
        "(SELECT c.*, {} AS preset FROM card c)",
        preset_sql(presets)
    )
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
    let rows: Vec<(CardId, Option<i64>)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT id, preset FROM (SELECT c.id, {preset} AS preset FROM card c WHERE 1 {card_filter}) \
         WHERE preset IS NOT NULL"
    )))
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
        self.of_role(state_role(state)?)
    }

    /// What is left for cards whose state has `role`.
    pub(crate) fn of_role(self, role: StateRole) -> Option<u32> {
        match role {
            StateRole::New => self.new,
            StateRole::Review => self.reviews,
            StateRole::Learning | StateRole::Relearning => None,
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
/// Each preset's cards are capped by what is left of its own limit, then each role's total by
/// what is left of the top-level one. For example, with 30 new `languages` cards and 30 others,
/// 10 left of the `languages` limit and 25 of the top-level one, 25 are let through: 10 of
/// `languages` and 30 others would make 40. Weights are scaled down with the counts, in
/// proportion. States left with nothing are left out.
///
/// A limit applies to all the states of its [`StateRole`] together, which [`cap_role`] shares
/// out among them.
pub(crate) fn cap_counts(
    rows: impl IntoIterator<Item = (StateId, Option<usize>, u32, f64)>,
    caps: &QueueCaps,
) -> HashMap<StateId, (u32, f64)> {
    let mut by_preset: HashMap<Option<usize>, HashMap<StateId, (u32, f64)>> = HashMap::new();
    for (state, preset, count, weight) in rows {
        if count == 0 {
            continue;
        }
        let entry = by_preset
            .entry(preset)
            .or_default()
            .entry(state)
            .or_default();
        entry.0 += count;
        entry.1 += weight;
    }
    let mut by_state: HashMap<StateId, (u32, f64)> = HashMap::new();
    for (preset, mut counts) in by_preset {
        if let Some(left) = preset.and_then(|index| caps.presets.get(index)) {
            cap_role(&mut counts, state_role, |state| left.of_state(state));
        }
        for (state, (count, weight)) in counts {
            let entry = by_state.entry(state).or_default();
            entry.0 += count;
            entry.1 += weight;
        }
    }
    cap_role(&mut by_state, state_role, |state| caps.of_state(state));
    by_state.retain(|_, (count, _)| *count > 0);
    by_state
}

/// Shrinks the `counts` of the states of each [`StateRole`] (as `role_of` gives it) that `cap_of`
/// caps, so they add up to at most the cap, each keeping its share of it and scaling its weight
/// with it. With a review limit of 10 and 15 cards in the review state and 5 in another of the
/// role, 8 and 2 are kept.
fn cap_role(
    counts: &mut HashMap<StateId, (u32, f64)>,
    role_of: impl Fn(StateId) -> Option<StateRole>,
    cap_of: impl Fn(StateId) -> Option<u32>,
) {
    let by_role = counts
        .keys()
        .filter_map(|state| Some((role_of(*state)?, *state)))
        .into_group_map();
    for mut states in by_role.into_values() {
        states.sort_unstable();
        // Every state of a role has the same cap
        let Some(cap) = cap_of(states[0]) else {
            continue;
        };
        let total: u32 = states.iter().map(|state| counts[state].0).sum();
        if total <= cap {
            continue;
        }
        // Largest remainder: each state gets the whole part of its share, and the cards left
        // over go to the largest fractions, the lowest state first on a tie.
        let share = |state: &StateId| u64::from(counts[state].0) * u64::from(cap);
        let mut kept: HashMap<StateId, u32> = states
            .iter()
            .map(|state| {
                let whole = share(state) / u64::from(total);
                (*state, u32::try_from(whole).expect("at most the cap"))
            })
            .collect();
        let left_over = cap - kept.values().sum::<u32>();
        let by_fraction = states
            .iter()
            .sorted_by_key(|state| std::cmp::Reverse(share(state) % u64::from(total)));
        for state in by_fraction.take(left_over as usize) {
            *kept.get_mut(state).expect("every state is kept") += 1;
        }
        for (state, kept) in kept {
            let (count, weight) = counts.get_mut(&state).expect("counted");
            *weight *= f64::from(kept) / f64::from(*count);
            *count = kept;
        }
    }
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
    use crate::model::NEW_CARD_STATE;
    use crate::schedulers::review_state;

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

    #[test]
    fn a_cap_is_shared_among_the_states_of_its_role() {
        // 2 is a review state, and so is a `mastered` 4 of some other scheduler
        let role_of = |state| match state {
            2 | 4 => Some(StateRole::Review),
            _ => state_role(state),
        };
        let cap_of = |state| (role_of(state) == Some(StateRole::Review)).then_some(10);
        let mut counts =
            HashMap::from([(2, (15, 30.0)), (4, (5, 5.0)), (NEW_CARD_STATE, (7, 7.0))]);
        cap_role(&mut counts, role_of, cap_of);
        assert_eq!(
            counts[&2],
            (8, 16.0),
            "7.5 and 2.5: the lower state gets the card left over"
        );
        assert_eq!(counts[&4], (2, 2.0));
        assert_eq!(counts[&NEW_CARD_STATE], (7, 7.0), "uncapped");

        // Under the cap, nothing changes
        let mut counts = HashMap::from([(2, (6, 6.0)), (4, (4, 4.0))]);
        cap_role(&mut counts, role_of, cap_of);
        assert_eq!(counts, HashMap::from([(2, (6, 6.0)), (4, (4, 4.0))]));
    }
}
