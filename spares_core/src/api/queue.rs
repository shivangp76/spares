//! The order review shows cards in, as configured by [`QueueConfig`].

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Datelike;
use chrono::Duration;
use chrono::Local;
use chrono::Utc;
use indoc::formatdoc;
use itertools::Itertools;
use sqlx::FromRow;
use sqlx::SqlitePool;

use crate::Error;

use crate::api::preset::PresetLimitsLeft;
use crate::api::preset::preset_index;
use crate::api::preset::sql_string;
use crate::api::review::DailyLimitsLeft;
use crate::api::review::REVIEW_CARD_COLUMNS;
use crate::api::review::ReviewCard;
use crate::api::review::ReviewFilterQuery;
use crate::api::review::get_cards_left_by_state_and_time_estimate;
use crate::config::InterleaveBy;
use crate::config::NewCardOrder;
use crate::config::NewReviewMix;
use crate::config::QueueConfig;
use crate::config::QueuePreset;
use crate::config::ReviewOrder;
use crate::config::SparesExternalConfig;
use crate::model::CardId;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::retrievability_key_sql;
use crate::schedulers::state_ids_sql;
use crate::schedulers::state_role;
use crate::schedulers::states_with_role;
use crate::schedulers::step_states;
use crate::schema::review::StateRole;

/// What is left of today's limits, which [`NewReviewMix::Interleaved`] spreads cards over. `None`
/// for no limit, as when studying a filtered tag.
#[derive(Clone, Debug, Default)]
pub(crate) struct QueueCaps {
    pub(crate) new: Option<u32>,
    pub(crate) reviews: Option<u32>,
    /// What is left of each preset's own limits, by its index in [`QueueConfig::presets`]. Empty
    /// when no preset has limits, or for a filtered tag.
    pub(crate) presets: Vec<PresetLimitsLeft>,
}

impl QueueCaps {
    /// The caps of what is left of `limits`.
    pub(crate) fn from_limits(limits: &DailyLimitsLeft) -> Self {
        Self {
            new: Some(limits.new),
            reviews: Some(limits.reviews),
            presets: limits.presets.clone(),
        }
    }

    /// The top-level cap of cards in `state`. Learning and relearning cards are never capped.
    pub(crate) fn of_state(&self, state: StateId) -> Option<u32> {
        match state_role(state)? {
            StateRole::New => self.new,
            StateRole::Review => self.reviews,
            StateRole::Learning | StateRole::Relearning => None,
        }
    }

    /// The caps to spread cards over, for [`queue_order_by`]: how many new and review cards can
    /// actually be shown, out of `cards_left_by_state`, the counts [`cap_counts`] let through.
    ///
    /// Below the top-level caps when a preset's own limit holds back some of its cards. For
    /// example, with 20 new cards left today but 5 of them of a preset with 5 left of its limit
    /// and 15 others, only 20 can be shown either way, while with 30 of that preset and 3
    /// others, only 8 can, so the new cards are spread over the reviews as 8, not 20.
    ///
    /// [`cap_counts`]: crate::api::preset::cap_counts
    pub(crate) fn showable(&self, cards_left_by_state: &HashMap<StateId, u32>) -> Self {
        let left = |role: StateRole| {
            states_with_role(&[role])
                .iter()
                .filter_map(|state| cards_left_by_state.get(state))
                .sum()
        };
        Self {
            new: self.new.map(|_| left(StateRole::New)),
            reviews: self.reviews.map(|_| left(StateRole::Review)),
            presets: Vec::new(),
        }
    }
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
///
/// Crams are not reviews here: a card crammed yesterday but last reviewed a month ago has the
/// retrievability of a month, which is what decides how urgent its review is. The `16` is
/// [`ReviewLogKind::FIRST_UNREPLAYED`](crate::model::ReviewLogKind::FIRST_UNREPLAYED), written out
/// since a `const` cannot be formatted.
const LAST_REVIEW: &str =
    "(SELECT MAX(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind < 16)";

fn review_keys(config: &SparesExternalConfig, order: ReviewOrder, now: DateTime<Utc>) -> Vec<Key> {
    let now_ts = now.timestamp();
    match order {
        ReviewOrder::Due => vec![asc("c.due"), asc("n.created_at")],
        // The scheduler's key falls as retrievability rises.
        ReviewOrder::Retrievability => vec![desc(retrievability_key_sql(
            config,
            &format!("({now_ts} - COALESCE({LAST_REVIEW}, c.due))"),
            |key| key.sql.clone(),
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

/// The sort keys of cards in the order each card's preset uses: `keys(order)` for the order of
/// the card's preset (`preset_order` of it, or `top_level` if it sets none), and `top_level` for
/// cards of no preset.
///
/// Cards are gathered by order, each order's cards together, in the order of the first preset
/// that uses it, with the top level after the presets. For example, with presets `languages`
/// (random) and `exam` (no order of its own) and a top-level order by due date, the `languages`
/// cards come first, in a random order, then the `exam` cards and those of no preset, together,
/// by due date. With no preset ordering differently from the top level, these are just
/// `keys(top_level)`. With [`QueueConfig::interleave_by`], this gathering only applies within
/// each subject.
fn keys_by_preset<O: Copy + PartialEq>(
    presets: &indexmap::IndexMap<String, QueuePreset>,
    top_level: O,
    preset_order: impl Fn(&QueuePreset) -> Option<O>,
    keys: impl Fn(O) -> Vec<Key>,
) -> Vec<Key> {
    let preset_orders = presets
        .values()
        .map(|preset| preset_order(preset).unwrap_or(top_level))
        .collect::<Vec<_>>();
    let mut orders: Vec<O> = Vec::new();
    for order in preset_orders.iter().chain([&top_level]) {
        if !orders.contains(order) {
            orders.push(*order);
        }
    }
    if orders.len() == 1 {
        return keys(top_level);
    }
    let position = |order: &O| orders.iter().position(|o| o == order).unwrap();
    // The index in `orders` of the card's order. The simple `CASE` evaluates the preset once.
    let whens = preset_orders
        .iter()
        .enumerate()
        .map(|(index, order)| format!("WHEN {index} THEN {}", position(order)))
        .join(" ");
    let order_index = format!("(CASE c.preset {whens} ELSE {} END)", position(&top_level));
    let mut all_keys = vec![asc(order_index.clone())];
    for (index, order) in orders.iter().enumerate() {
        all_keys.extend(keys(*order).into_iter().map(|(expr, descending)| {
            (
                format!("CASE WHEN {order_index} = {index} THEN {expr} END"),
                descending,
            )
        }));
    }
    all_keys
}

/// The subject of card `c` that [`QueueConfig::interleave_by`] mixes by, as a SQL expression, or
/// `None` if it mixes nothing.
fn subject_sql(config: &SparesExternalConfig) -> Option<String> {
    match config.queue.interleave_by {
        InterleaveBy::None => None,
        InterleaveBy::TopLevelTag => Some(format!(
            "(SELECT MIN(CASE WHEN instr(t.name, ':') > 0 \
             THEN substr(t.name, 1, instr(t.name, ':') - 1) ELSE t.name END) \
             FROM note_tag nt JOIN tag t ON t.id = nt.tag_id \
             WHERE nt.note_id = c.note_id AND t.name NOT IN ({}, {}))",
            sql_string(&config.flagged_tag_name),
            sql_string(&config.leech.tag_name),
        )),
    }
}

fn render(keys: &[Key]) -> String {
    keys.iter()
        .map(|(expr, descending)| format!("{expr} {}", if *descending { "DESC" } else { "ASC" }))
        .join(",\n    ")
}

/// The `ORDER BY` expressions (without the keyword) that put the cards matching a review's
/// `WHERE` clause in the order [`QueueConfig`] describes, as of `now`. The query must select
/// `FROM {cards_with_preset} c JOIN note n JOIN parser p`, with
/// [`cards_with_preset`](crate::api::preset::cards_with_preset) of `config`'s presets.
///
/// `query_sort` is the sort the review's query asks for (see
/// [`Evaluator::card_order_by`](crate::search::evaluator::Evaluator::card_order_by)). Unless it is
/// empty, it orders new and review cards together in place of [`QueueConfig::new_order`],
/// [`QueueConfig::review_order`], [`QueueConfig::new_review_mix`] and
/// [`QueueConfig::interleave_by`], and of the orders of [presets](QueueConfig::presets).
///
/// Each card is ordered by its preset's [`QueuePreset::new_order`] or
/// [`QueuePreset::review_order`], with cards of presets that order differently gathered as
/// [`QueuePreset::review_order`] describes. `caps` should be [`QueueCaps::showable`], so the new
/// cards a preset's limit holds back are not counted when spreading them over the reviews.
pub(crate) fn queue_order_by(
    external_config: &SparesExternalConfig,
    now: DateTime<Utc>,
    caps: &QueueCaps,
    query_sort: &[String],
) -> String {
    let config = &external_config.queue;
    let now_ts = now.timestamp();
    let in_steps = step_states().iter().join(", ");
    // The groups listed on `QueueConfig`, numbered from 0
    let group = format!(
        "CASE WHEN c.special_state IS NOT NULL THEN 3 \
         WHEN c.state IN ({in_steps}) THEN CASE WHEN c.due <= {now_ts} THEN 0 ELSE 2 END \
         ELSE 1 END"
    );
    let is_new = format!("c.state IN ({})", state_ids_sql(&[StateRole::New]));

    // Within group 1, new and review cards are each sorted by their own keys. A key that only
    // applies to one kind is NULL for the other, so it ties.
    let only = |condition: &str, keys: Vec<Key>| -> Vec<Key> {
        keys.into_iter()
            .map(|(expr, descending)| {
                (format!("CASE WHEN {condition} THEN {expr} END"), descending)
            })
            .collect()
    };
    let mut within_kind = only(
        &is_new,
        keys_by_preset(
            &config.presets,
            config.new_order,
            |preset| preset.new_order,
            |order| new_keys(order, now),
        ),
    );
    within_kind.extend(only(
        &format!("NOT ({is_new})"),
        keys_by_preset(
            &config.presets,
            config.review_order,
            |preset| preset.review_order,
            |order| review_keys(external_config, order, now),
        ),
    ));
    within_kind.push(asc("c.id"));

    // With `interleave_by`, each card's position within its subject, as a fraction of the
    // subject's size, among the cards of its kind. Sorting a kind by it spreads each subject over
    // the kind in proportion to its size, like the new cards of `NewReviewMix::Interleaved`, and
    // keeps each subject in its own order. Sorting by it first also takes the place of
    // gathering the cards of presets that order differently, which `within_kind` still does
    // within a subject.
    let subject_position = subject_sql(external_config).map(|subject| {
        let partition = format!("PARTITION BY {group}, {is_new}, {subject}");
        format!(
            "(ROW_NUMBER() OVER ({partition} ORDER BY {}) - 0.5) / COUNT(*) OVER ({partition})",
            render(&within_kind)
        )
    });
    // The order of each kind on its own
    let kind_keys = subject_position
        .iter()
        .map(|position| asc(position.clone()))
        .chain(within_kind.iter().cloned())
        .collect::<Vec<_>>();

    let query_keys = query_sort
        .iter()
        .map(|term| match term.rsplit_once(' ') {
            Some((expr, "DESC")) => desc(expr),
            Some((expr, "ASC")) => asc(expr),
            _ => asc(term.as_str()),
        })
        .chain([asc("c.id")])
        .collect::<Vec<_>>();
    let mix_keys = match config.new_review_mix {
        _ if !query_sort.is_empty() => query_keys,
        NewReviewMix::NewFirst => {
            let mut keys = vec![asc(format!("NOT ({is_new})"))];
            keys.extend(kind_keys);
            keys
        }
        NewReviewMix::ReviewsFirst => {
            let mut keys = vec![asc(is_new.clone())];
            keys.extend(kind_keys);
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
            // The card's position within its kind, less a half. Mixed by subject, that is
            // estimated from its position within its subject: with each subject spread evenly,
            // a card 1/4 of the way through its subject is about 1/4 of the way through its kind.
            // Counting it exactly would rank cards by `subject_position`, a window function, in
            // another window's `ORDER BY`, which SQLite does not allow. The estimate is off by
            // at most about one card per subject, and is exact with one subject.
            let position = match &subject_position {
                Some(subject_position) => format!("{subject_position} * {count}"),
                None => format!(
                    "ROW_NUMBER() OVER ({partition} ORDER BY {}) - 0.5",
                    render(&within_kind)
                ),
            };
            let mut keys = vec![
                asc(format!(
                    "({position}) / CASE WHEN {is_new} THEN {} ELSE {} END",
                    capped(caps.new),
                    capped(caps.reviews),
                )),
                // Reviews before new cards on a tie
                asc(is_new.clone()),
            ];
            // Cards of two subjects of the same size tie, so come in their kind's order. Without
            // subjects, positions are unique within a kind, so this is not needed.
            if subject_position.is_some() {
                keys.extend(kind_keys);
            }
            keys
        }
    };

    let mut keys = vec![asc(group.clone())];
    keys.extend(only(&format!("{group} = 1"), mix_keys));
    // Groups 0, 2 and 3, and any remaining tie
    keys.extend([asc("c.due"), asc("n.created_at"), asc("c.id")]);
    render(&keys)
}

/// The cards [`QueueConfig::new_after_linked_notes`] and
/// [`QueueConfig::reviews_after_linked_notes`] hold back, as a SQL condition over the `c` and `n`
/// aliases starting with `AND NOT`, to append to a review's `WHERE` clause. Empty when both are
/// off.
///
/// `card_due_limit` is the end of the day, by which a relearning card holds reviews back.
/// `query_card_ids` are the ids, comma separated, of the cards matching the review's query, if it
/// has one: only those hold other cards back.
pub(crate) fn linked_notes_clause(
    config: &QueueConfig,
    card_due_limit: DateTime<Utc>,
    query_card_ids: Option<&str>,
) -> String {
    waiting_on_links(config, card_due_limit, query_card_ids)
        .map(|waiting| format!("\nAND NOT {waiting}"))
        .unwrap_or_default()
}

/// The cards [`linked_notes_clause`] holds back, as an `EXISTS` condition. `None` when both
/// settings are off.
pub(crate) fn waiting_on_links(
    config: &QueueConfig,
    card_due_limit: DateTime<Utc>,
    query_card_ids: Option<&str>,
) -> Option<String> {
    let new = state_ids_sql(&[StateRole::New]);
    let mut holds = Vec::new();
    if config.new_after_linked_notes {
        holds.push(format!(
            "(c.state IN ({new}) AND lc.state IN ({}))",
            state_ids_sql(&[StateRole::New, StateRole::Learning])
        ));
    }
    if config.reviews_after_linked_notes {
        holds.push(format!(
            "(c.state IN ({}) AND lc.state IN ({}) AND lc.due <= {})",
            state_ids_sql(&[StateRole::Review]),
            state_ids_sql(&[StateRole::Relearning]),
            card_due_limit.timestamp()
        ));
    }
    if holds.is_empty() {
        return None;
    }
    let in_query = query_card_ids
        .map(|ids| format!(" AND lc.id IN ({ids})"))
        .unwrap_or_default();
    // Only older notes hold newer ones back, which is a strict order, so there are no cycles
    Some(formatdoc! {"
        EXISTS (
            SELECT 1 FROM note_link nl
            JOIN note ln ON ln.id = nl.linked_note_id
            JOIN card lc ON lc.note_id = ln.id
            WHERE nl.parent_note_id = c.note_id
                AND nl.score <= {max_score}
                AND (ln.created_at, ln.id) < (n.created_at, n.id)
                AND (lc.special_state IS NULL OR lc.special_state IN ({scheduler_buried}, {buried_until_later_today}))
                AND ({holds}){in_query}
        )",
        max_score = config.linked_note_max_score,
        scheduler_buried = SpecialState::SchedulerBuried as u8,
        buried_until_later_today = SpecialState::BuriedUntilLaterToday as u8,
        holds = holds.join(" OR "),
    })
}

/// A card of [`ordered_queue`].
#[derive(FromRow)]
pub(crate) struct QueueRow {
    #[sqlx(flatten)]
    pub(crate) card: ReviewCard,
    pub(crate) due: DateTime<Utc>,
    /// The card's preset, if it has limits of its own (see
    /// [`limited_preset_sql`](crate::api::preset::limited_preset_sql))
    pub(crate) preset: Option<i64>,
}

/// The daily limit that was used up when a card's turn came in [`ordered_queue`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UsedUpLimit {
    /// [`SparesExternalConfig::new_cards_daily_limit`] or
    /// [`SparesExternalConfig::reviews_daily_limit`]
    TopLevel,
    /// That of the preset at this index in [`QueueConfig::presets`]
    Preset(usize),
}

/// The cards a review would show, in order, from [`ordered_queue`].
pub(crate) struct OrderedQueue {
    pub(crate) rows: Vec<QueueRow>,
    /// The cards left out because a limit was used up by the cards before them, in order.
    pub(crate) skipped: Vec<(CardId, UsedUpLimit)>,
    /// What [`get_cards_left_by_state_and_time_estimate`] counts for the review.
    pub(crate) cards_left_by_state: HashMap<StateId, u32>,
    pub(crate) time_estimate: Duration,
}

/// The cards matching `filter` and due by `due_limit`, in the order review shows them as of
/// `requested_date`, at most `limit` of them.
///
/// New and review cards are kept while what is left of today's limits, and of their
/// [preset](QueuePreset)'s, lets them through, in turn: with 5 new cards left today, the first 5
/// new cards are kept and the others skipped, wherever they are in the order. Learning cards and
/// the cards of filtered tags are never capped. A limit already used up at the start never gets
/// here, since [`build_review_filter_query`](crate::api::review::build_review_filter_query)
/// leaves its cards out of `filter.where_clause`.
pub(crate) async fn ordered_queue(
    db: &SqlitePool,
    filter: &ReviewFilterQuery,
    config: &SparesExternalConfig,
    requested_date: DateTime<Utc>,
    due_limit: DateTime<Utc>,
    limit: Option<usize>,
) -> Result<OrderedQueue, Error> {
    let (cards_left_by_state, time_estimate) =
        get_cards_left_by_state_and_time_estimate(db, filter).await?;
    // Ordered like `get_review_card`. There is no `LIMIT` since cards beyond today's limits are
    // skipped below.
    let order_by = queue_order_by(
        config,
        requested_date,
        &filter.caps.showable(&cards_left_by_state),
        &filter.query_sort,
    );
    let query_str = formatdoc! {"
        SELECT {REVIEW_CARD_COLUMNS}, c.due as due, {preset_sql} as preset
        FROM {cards} c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE {where_clause}
        ORDER BY
            {order_by}",
        preset_sql = filter.preset_sql,
        cards = filter.cards,
        where_clause = filter.where_clause,
    };
    let mut query = sqlx::query_as(&query_str);
    if !filter.is_filtered_tag {
        query = query.bind(due_limit.timestamp());
    }
    let all_rows: Vec<QueueRow> = query
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    // What is left of the top-level limits, and of each preset's
    let mut allowed = filter.caps.clone();
    let mut rows = Vec::new();
    let mut skipped = Vec::new();
    for row in all_rows {
        if limit.is_some_and(|limit| rows.len() >= limit) {
            break;
        }
        // Filtered tags have no limits, and learning cards are never held back
        let state = row.card.card_state;
        let preset = preset_index(row.preset);
        let preset_left = preset
            .and_then(|index| allowed.presets.get(index))
            .and_then(|left| left.of_state(state));
        if let Some(index) = preset.filter(|_| preset_left == Some(0)) {
            skipped.push((row.card.card_id, UsedUpLimit::Preset(index)));
            continue;
        }
        if allowed.of_state(state) == Some(0) {
            skipped.push((row.card.card_id, UsedUpLimit::TopLevel));
            continue;
        }
        let decrement = |left: &mut Option<u32>| *left = left.map(|left| left - 1);
        let preset_left = preset.and_then(|index| allowed.presets.get_mut(index));
        match state_role(state) {
            Some(StateRole::New) => {
                decrement(&mut allowed.new);
                if let Some(left) = preset_left {
                    decrement(&mut left.new);
                }
            }
            Some(StateRole::Review) => {
                decrement(&mut allowed.reviews);
                if let Some(left) = preset_left {
                    decrement(&mut left.reviews);
                }
            }
            _ => {}
        }
        rows.push(row);
    }
    Ok(OrderedQueue {
        rows,
        skipped,
        cards_left_by_state,
        time_estimate,
    })
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
        ordered_by(pool, config, now, caps, &[]).await
    }

    /// Every unsuspended card, in queue order with `query_sort`.
    async fn ordered_by(
        pool: &SqlitePool,
        config: &QueueConfig,
        now: DateTime<Utc>,
        caps: QueueCaps,
        query_sort: &[String],
    ) -> Vec<CardId> {
        let query = format!(
            "SELECT c.id FROM {} c JOIN note n ON c.note_id = n.id JOIN parser p ON n.parser_id = p.id
            WHERE c.special_state IS NULL OR c.special_state = {}
            ORDER BY {}",
            crate::api::preset::cards_with_preset(&config.presets),
            SpecialState::BuriedUntilLaterToday as u8,
            queue_order_by(
                &SparesExternalConfig {
                    queue: config.clone(),
                    ..Default::default()
                },
                now,
                &caps,
                query_sort
            )
        );
        sqlx::query_scalar(&query).fetch_all(pool).await.unwrap()
    }

    #[sqlx::test]
    async fn query_sort_replaces_new_and_review_order(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 4, now - Duration::days(30)).await;
        let [new, easy_review, hard_review, learning] = ids[..] else {
            unreachable!()
        };
        sqlx::query(r"UPDATE card SET difficulty = 5.0 WHERE id = ?")
            .bind(new)
            .execute(&pool)
            .await
            .unwrap();
        set_card(
            &pool,
            easy_review,
            REVIEW,
            now - Duration::days(3),
            5.0,
            2.0,
        )
        .await;
        set_card(
            &pool,
            hard_review,
            REVIEW,
            now - Duration::days(1),
            5.0,
            9.0,
        )
        .await;
        set_card(
            &pool,
            learning,
            LEARNING,
            now - Duration::minutes(1),
            1.0,
            1.0,
        )
        .await;
        let sort = ["c.difficulty DESC".to_string()];
        let order = ordered_by(
            &pool,
            &config(
                ReviewOrder::Due,
                NewCardOrder::Created,
                NewReviewMix::ReviewsFirst,
            ),
            now,
            QueueCaps::default(),
            &sort,
        )
        .await;
        // The learning card whose step is over still comes first
        assert_eq!(order, vec![learning, hard_review, new, easy_review]);
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
                    reviews: None,
                    presets: Vec::new(),
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
                    reviews: Some(3),
                    presets: Vec::new(),
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
            DailyLimitsLeft {
                new: 2,
                reviews: 5,
                presets: Vec::new(),
            }
        );
        assert_eq!(
            left(&limits(10, 5, true)).await,
            DailyLimitsLeft {
                new: 10,
                reviews: 5,
                presets: Vec::new(),
            }
        );
        // More reviews due than the limit leaves no room for new cards
        assert_eq!(
            left(&limits(10, 2, false)).await,
            DailyLimitsLeft {
                new: 0,
                reviews: 2,
                presets: Vec::new(),
            }
        );

        // Studying a review card uses up one review, and leaves one fewer due
        add_review(&pool, ids[0], now, REVIEW).await;
        set_card(&pool, ids[0], REVIEW, now + Duration::days(3), 5.0, 5.0).await;
        assert_eq!(
            left(&limits(10, 5, false)).await,
            DailyLimitsLeft {
                new: 2,
                reviews: 4,
                presets: Vec::new(),
            }
        );

        // Studying a new card uses up a new card and, unless ignored, a review
        add_review(&pool, ids[3], now, NEW).await;
        assert_eq!(
            left(&limits(10, 5, false)).await,
            DailyLimitsLeft {
                new: 1,
                reviews: 3,
                presets: Vec::new(),
            }
        );
        assert_eq!(
            left(&limits(10, 5, true)).await,
            DailyLimitsLeft {
                new: 9,
                reviews: 4,
                presets: Vec::new(),
            }
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
            DailyLimitsLeft {
                new: 5,
                reviews: 5,
                presets: Vec::new(),
            }
        );
    }

    /// The counts of what is left show no more than today's limits leave room for, and the time
    /// estimate shrinks with them.
    #[sqlx::test]
    async fn counts_are_capped_by_the_limits(pool: SqlitePool) {
        use indexmap::IndexMap;

        use crate::api::preset::cards_with_preset;
        use crate::api::review::ReviewFilterQuery;
        use crate::api::review::get_cards_left_by_state_and_time_estimate;

        let now = Utc::now();
        let state = |s: i64| crate::model::StateId::try_from(s).unwrap();
        // Six reviews overdue, two learning cards and four new cards, never reviewed, so 30
        // seconds each
        let ids = create_cards(&pool, 12, now - Duration::days(30)).await;
        for card in &ids[..6] {
            set_card(&pool, *card, REVIEW, now - Duration::days(10), 5.0, 5.0).await;
        }
        for card in &ids[6..8] {
            set_card(&pool, *card, super::tests::LEARNING, now, 1.0, 5.0).await;
        }
        let counts = async |caps: QueueCaps| {
            let filter = ReviewFilterQuery {
                cards: cards_with_preset(&IndexMap::new()),
                where_clause: "c.due <= ?".to_string(),
                is_filtered_tag: false,
                card_due_limit: now,
                caps,
                preset_sql: "NULL".to_string(),
                held_back_clause: None,
                query_sort: Vec::new(),
                filter_cards_clause: "TRUE".to_string(),
                waiting_clause: None,
            };
            get_cards_left_by_state_and_time_estimate(&pool, &filter)
                .await
                .unwrap()
        };

        let (by_state, time) = counts(QueueCaps::default()).await;
        assert_eq!(by_state.get(&state(NEW)), Some(&4));
        assert_eq!(by_state.get(&state(REVIEW)), Some(&6));
        assert_eq!(time, Duration::seconds(12 * 30));

        // Learning cards are never capped, and a used up limit drops its state
        let (by_state, time) = counts(QueueCaps {
            new: Some(0),
            reviews: Some(2),
            presets: Vec::new(),
        })
        .await;
        assert_eq!(by_state.get(&state(NEW)), None);
        assert_eq!(by_state.get(&state(REVIEW)), Some(&2));
        assert_eq!(by_state.get(&state(super::tests::LEARNING)), Some(&2));
        assert_eq!(time, Duration::seconds(4 * 30));

        // A cap above the count leaves it as is
        let (by_state, _) = counts(QueueCaps {
            new: Some(3),
            reviews: Some(100),
            presets: Vec::new(),
        })
        .await;
        assert_eq!(by_state.get(&state(NEW)), Some(&3));
        assert_eq!(by_state.get(&state(REVIEW)), Some(&6));
    }

    /// The statistics' cards due today count overdue reviews, and are capped like the review
    /// counts, with the default limit of 20 new cards.
    #[sqlx::test]
    async fn statistics_due_counts_are_capped(pool: SqlitePool) {
        use crate::api::statistics::get_statistics;
        use crate::schema::review::StatisticsRequest;

        let now = Utc::now();
        let state = |s: i64| crate::model::StateId::try_from(s).unwrap();
        let ids = create_cards(&pool, 30, now - Duration::days(30)).await;
        for card in &ids[..3] {
            set_card(&pool, *card, REVIEW, now - Duration::days(10), 5.0, 5.0).await;
        }
        let due = async || {
            get_statistics(
                &pool,
                StatisticsRequest {
                    scheduler_name: "fsrs".to_string(),
                    date: now,
                    tz: None,
                },
            )
            .await
            .unwrap()
            .due_count_by_state
        };
        let by_state = due().await;
        assert_eq!(by_state.get(&state(NEW)), Some(&20));
        assert_eq!(by_state.get(&state(REVIEW)), Some(&3));

        // Five new cards studied today leave 15 of the 20
        for card in &ids[3..8] {
            add_review(&pool, *card, now, NEW).await;
        }
        assert_eq!(due().await.get(&state(NEW)), Some(&15));
    }
}

#[cfg(test)]
mod learn_ahead_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::tests::LEARNING;
    use super::tests::REVIEW;
    use super::tests::create_cards;
    use super::tests::set_card;
    use super::*;
    use crate::api::offline_review::get_review_queue;
    use crate::api::review::get_review_card;
    use crate::api::review::next_review_card_available_at;
    use crate::parsers::get_all_parsers;
    use crate::schema::review::GetReviewCardFilterRequest;
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
    async fn review_query_sort_sets_the_order(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 2, now - Duration::days(30)).await;
        set_card(&pool, ids[0], REVIEW, now - Duration::days(3), 5.0, 2.0).await;
        set_card(&pool, ids[1], REVIEW, now - Duration::days(1), 5.0, 9.0).await;
        let first = async |query: &str| {
            get_review_card(
                &pool,
                GetReviewCardRequest {
                    filter: Some(GetReviewCardFilterRequest::Query(query.to_string())),
                },
                now,
                &get_all_parsers(),
            )
            .await
            .unwrap()
            .unwrap()
            .card_id
        };
        assert_eq!(first("c.id>0").await, ids[0]);
        assert_eq!(first("c.id>0 sort_by_desc=c.difficulty").await, ids[1]);
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

        // The offline queue has it anyway, with when the limit lets its client show it
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
        assert_eq!(queue.cards[0].shown_from, Some(due - LIMIT));

        // A review card is never held back
        set_card(&pool, card, REVIEW, now - Duration::days(1), 1.0, 5.0).await;
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
        assert_eq!(queue.cards[0].shown_from, None);

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

#[cfg(test)]
mod linked_note_tests {
    use chrono::Duration;
    use indexmap::IndexMap;
    use sqlx::SqlitePool;

    use super::tests::LEARNING;
    use super::tests::NEW;
    use super::tests::RELEARNING;
    use super::tests::REVIEW;
    use super::tests::create_cards;
    use super::tests::set_card;
    use super::*;
    use crate::api::preset::cards_with_preset;
    use crate::api::review::ReviewFilterQuery;
    use crate::api::review::get_cards_left_by_state_and_time_estimate;
    use crate::helpers::get_start_end_local_date;
    use crate::model::CardId;

    fn holding(new: bool, reviews: bool) -> QueueConfig {
        QueueConfig {
            new_after_linked_notes: new,
            reviews_after_linked_notes: reviews,
            ..QueueConfig::default()
        }
    }

    /// Links the note of `from` to the note of `to`, with `score`, or to no note if `to` is `None`.
    async fn link(pool: &SqlitePool, from: CardId, to: Option<CardId>, score: f64) {
        sqlx::query(
            r#"INSERT INTO note_link
                (parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score)
            VALUES (
                (SELECT note_id FROM card WHERE id = ?),
                (SELECT note_id FROM card WHERE id = ?),
                0, 'keyword', 'keyword', ?
            )"#,
        )
        .bind(from)
        .bind(to)
        .bind(score)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn set_special_state(pool: &SqlitePool, card: CardId, state: Option<SpecialState>) {
        sqlx::query(r"UPDATE card SET special_state = ? WHERE id = ?")
            .bind(state)
            .bind(card)
            .execute(pool)
            .await
            .unwrap();
    }

    /// The `where_clause` of an unfiltered review, as `build_review_filter_query` builds it.
    fn where_clause(
        config: &QueueConfig,
        now: DateTime<Utc>,
        query_card_ids: Option<&str>,
    ) -> String {
        let (_, card_due_limit) = get_start_end_local_date(&now);
        format!(
            "c.special_state IS NULL AND c.due <= ?{}",
            linked_notes_clause(config, card_due_limit, query_card_ids)
        )
    }

    /// The cards a review would show today.
    async fn shown_with(
        pool: &SqlitePool,
        config: &QueueConfig,
        now: DateTime<Utc>,
        query_card_ids: Option<&str>,
    ) -> Vec<CardId> {
        let (_, card_due_limit) = get_start_end_local_date(&now);
        sqlx::query_scalar(&format!(
            "SELECT c.id FROM card c JOIN note n ON c.note_id = n.id WHERE {} ORDER BY c.id",
            where_clause(config, now, query_card_ids)
        ))
        .bind(card_due_limit.timestamp())
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn shown(pool: &SqlitePool, config: &QueueConfig, now: DateTime<Utc>) -> Vec<CardId> {
        shown_with(pool, config, now, None).await
    }

    #[sqlx::test]
    async fn new_cards_wait_until_linked_notes_are_learned(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b] = create_cards(&pool, 2, now - Duration::days(30)).await[..] else {
            unreachable!()
        };
        link(&pool, b, Some(a), 0.0).await;
        let config = holding(true, false);

        assert_eq!(shown(&pool, &config, now).await, [a]);
        set_card(&pool, a, LEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        assert_eq!(shown(&pool, &config, now).await, [a]);
        set_card(&pool, a, REVIEW, now + Duration::days(3), 5.0, 5.0).await;
        assert_eq!(shown(&pool, &config, now).await, [b]);
        // A lapse holds back new cards no more, only reviews
        set_card(&pool, a, RELEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        assert_eq!(shown(&pool, &config, now).await, [a, b]);
    }

    #[sqlx::test]
    async fn only_close_matches_hold_cards_back(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b, c] = create_cards(&pool, 3, now - Duration::days(30)).await[..] else {
            unreachable!()
        };
        // A shared stem only, and a link that matched nothing
        link(&pool, b, Some(a), 4.5).await;
        link(&pool, c, None, 0.0).await;
        let config = holding(true, true);
        assert_eq!(shown(&pool, &config, now).await, [a, b, c]);

        let loose = QueueConfig {
            linked_note_max_score: 5.0,
            ..config
        };
        assert_eq!(shown(&pool, &loose, now).await, [a, c]);
    }

    #[sqlx::test]
    async fn only_older_notes_hold_newer_ones_back(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b, c] = create_cards(&pool, 3, now - Duration::days(30)).await[..] else {
            unreachable!()
        };
        link(&pool, a, Some(b), 0.0).await;
        link(&pool, b, Some(a), 0.0).await;
        link(&pool, c, Some(c), 0.0).await;
        assert_eq!(shown(&pool, &holding(true, false), now).await, [a, c]);
    }

    #[sqlx::test]
    async fn only_cards_that_come_back_hold_cards_back(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b] = create_cards(&pool, 2, now - Duration::days(30)).await[..] else {
            unreachable!()
        };
        link(&pool, b, Some(a), 0.0).await;
        let config = holding(true, false);

        for state in [SpecialState::Suspended, SpecialState::UserBuried] {
            set_special_state(&pool, a, Some(state)).await;
            assert_eq!(shown(&pool, &config, now).await, [b], "{state:?}");
        }
        for state in [
            SpecialState::SchedulerBuried,
            SpecialState::BuriedUntilLaterToday,
        ] {
            set_special_state(&pool, a, Some(state)).await;
            assert!(shown(&pool, &config, now).await.is_empty(), "{state:?}");
        }
        set_special_state(&pool, a, None).await;
        assert_eq!(shown(&pool, &config, now).await, [a]);
    }

    #[sqlx::test]
    async fn cards_outside_the_query_hold_nothing_back(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b] = create_cards(&pool, 2, now - Duration::days(30)).await[..] else {
            unreachable!()
        };
        link(&pool, b, Some(a), 0.0).await;
        let config = holding(true, false);
        let only_b = b.to_string();
        assert_eq!(shown_with(&pool, &config, now, Some(&only_b)).await, [a, b]);
        let both = format!("{a}, {b}");
        assert_eq!(shown_with(&pool, &config, now, Some(&both)).await, [a]);
    }

    #[sqlx::test]
    async fn reviews_wait_while_linked_notes_relearn_today(pool: SqlitePool) {
        let now = Utc::now();
        let [a, b, new] = create_cards(&pool, 3, now - Duration::days(30)).await[..] else {
            unreachable!()
        };
        link(&pool, b, Some(a), 0.0).await;
        link(&pool, new, Some(a), 0.0).await;
        set_card(&pool, a, REVIEW, now + Duration::days(3), 5.0, 5.0).await;
        set_card(&pool, b, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        let config = holding(false, true);
        assert_eq!(shown(&pool, &config, now).await, [b, new]);

        set_card(&pool, a, RELEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        assert_eq!(shown(&pool, &config, now).await, [a, new]);
        // Due tomorrow, so b is let through today
        set_card(&pool, a, RELEARNING, now + Duration::days(1), 1.0, 5.0).await;
        assert_eq!(shown(&pool, &config, now).await, [b, new]);
        // A new card of a note in review does not wait for its relearning cards
        set_card(&pool, b, NEW, now - Duration::days(1), 0.0, 0.0).await;
        set_card(&pool, a, LEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        assert_eq!(shown(&pool, &config, now).await, [a, b, new]);
    }

    #[sqlx::test]
    async fn held_reviews_leave_room_for_new_cards(pool: SqlitePool) {
        use crate::api::review::daily_limits_left_with;

        let now = Utc::now();
        // `a` relearning, two reviews due linking to it, one not linking, and two new cards
        let ids = create_cards(&pool, 6, now - Duration::days(30)).await;
        let [a, held_1, held_2, free, ..] = ids[..] else {
            unreachable!()
        };
        set_card(&pool, a, RELEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        for card in [held_1, held_2, free] {
            set_card(&pool, card, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        }
        link(&pool, held_1, Some(a), 0.0).await;
        link(&pool, held_2, Some(a), 0.0).await;
        let new_left = async |queue: QueueConfig| {
            let config = SparesExternalConfig {
                new_cards_daily_limit: 10,
                reviews_daily_limit: 4,
                queue,
                ..SparesExternalConfig::default()
            };
            daily_limits_left_with(&pool, &config, now)
                .await
                .unwrap()
                .new
        };
        // Three reviews due take 3 of the 4
        assert_eq!(new_left(holding(false, false)).await, 1);
        // Only the review not held back is shown today, so it alone takes room
        assert_eq!(new_left(holding(false, true)).await, 3);
    }

    #[sqlx::test]
    async fn counts_match_the_cards_shown(pool: SqlitePool) {
        let now = Utc::now();
        let ids = create_cards(&pool, 4, now - Duration::days(30)).await;
        let [a, b, c, d] = ids[..] else {
            unreachable!()
        };
        link(&pool, b, Some(a), 0.0).await;
        link(&pool, c, Some(a), 0.0).await;
        link(&pool, d, Some(c), 0.0).await;
        set_card(&pool, c, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        set_card(&pool, a, RELEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        let config = holding(true, true);
        let shown = shown(&pool, &config, now).await;
        assert_eq!(shown, [a, b, d]);

        let (_, card_due_limit) = get_start_end_local_date(&now);
        let filter = ReviewFilterQuery {
            cards: cards_with_preset(&IndexMap::new()),
            where_clause: where_clause(&config, now, None),
            is_filtered_tag: false,
            card_due_limit,
            caps: QueueCaps::default(),
            preset_sql: "NULL".to_string(),
            held_back_clause: None,
            query_sort: Vec::new(),
            filter_cards_clause: "TRUE".to_string(),
            waiting_clause: None,
        };
        let (by_state, _) = get_cards_left_by_state_and_time_estimate(&pool, &filter)
            .await
            .unwrap();
        let state = |s: i64| StateId::try_from(s).unwrap();
        assert_eq!(
            by_state,
            HashMap::from([(state(NEW), 2), (state(RELEARNING), 1)])
        );
    }

    #[sqlx::test]
    async fn search_lists_the_cards_waiting(pool: SqlitePool) {
        use crate::search::evaluator::Evaluator;
        use crate::search::evaluator::waiting_on_links_condition;

        let now = Utc::now();
        let [a, b, c] = create_cards(&pool, 3, now - Duration::days(30)).await[..] else {
            unreachable!()
        };
        link(&pool, b, Some(a), 0.0).await;
        link(&pool, c, Some(a), 4.5).await;
        let matching = async |config: &QueueConfig, value: &str| -> Vec<CardId> {
            sqlx::query_scalar(&format!(
                "SELECT c.id FROM card c JOIN note n ON c.note_id = n.id WHERE {} ORDER BY c.id",
                waiting_on_links_condition(config, value)
            ))
            .fetch_all(&pool)
            .await
            .unwrap()
        };
        let on = holding(true, false);
        assert_eq!(matching(&on, "= true").await, [b]);
        assert_eq!(matching(&on, "= false").await, [a, c]);
        // Only what review holds back: reviews do not wait with just new cards on
        set_card(&pool, a, RELEARNING, now - Duration::minutes(1), 1.0, 5.0).await;
        set_card(&pool, b, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        assert_eq!(matching(&on, "= true").await, Vec::<CardId>::new());
        assert_eq!(matching(&holding(false, true), "= true").await, [b]);

        // The settings are off by default, so the search matches nothing
        let search = async |query: &str| {
            let mut ids = Evaluator::new(query).get_card_ids(&pool).await.unwrap();
            ids.sort_unstable();
            ids
        };
        assert_eq!(
            search("c.waiting_on_links=true").await,
            Vec::<CardId>::new()
        );
        assert_eq!(search("c.waiting_on_links=false").await, [a, b, c]);
        assert_eq!(search("-c.waiting_on_links=true").await, [a, b, c]);
    }

    #[test]
    fn nothing_is_held_back_by_default() {
        let now = Utc::now();
        assert_eq!(
            linked_notes_clause(&QueueConfig::default(), now, Some("1")),
            ""
        );
        // Only the half whose setting is on
        assert!(!linked_notes_clause(&holding(true, false), now, None).contains("lc.due"));
    }
}

#[cfg(test)]
mod leech_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::tests::REVIEW;
    use super::tests::create_cards;
    use super::tests::set_card;
    use super::*;
    use crate::api::review::rate_card_with_config;
    use crate::api::undo::undo_event;
    use crate::config::LeechAction;
    use crate::config::LeechConfig;
    use crate::config::SparesExternalConfig;
    use crate::model::CardId;
    use crate::model::SpecialState;
    use crate::schedulers::get_scheduler_from_string;
    use crate::schema::review::RatingSubmission;
    use crate::schema::undo::UndoEventRequest;

    const AGAIN: u32 = 1;

    /// Forgets `card_id`, a card in review, returning the rating's event.
    async fn lapse(
        pool: &SqlitePool,
        config: &SparesExternalConfig,
        card_id: CardId,
        now: DateTime<Utc>,
    ) -> i64 {
        set_card(pool, card_id, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        rate_card_with_config(
            pool,
            scheduler.as_ref(),
            config,
            RatingSubmission {
                card_id,
                rating: AGAIN,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(1),
                tag_id: None,
            },
            now,
            true,
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn special_state(pool: &SqlitePool, card_id: CardId) -> Option<SpecialState> {
        sqlx::query_scalar(r"SELECT special_state FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn note_tags(pool: &SqlitePool, card_id: CardId) -> Vec<String> {
        sqlx::query_scalar(
            r"SELECT t.name FROM tag t JOIN note_tag nt ON nt.tag_id = t.id
            JOIN card c ON c.note_id = nt.note_id WHERE c.id = ?",
        )
        .bind(card_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[sqlx::test]
    async fn leeches_are_tagged_and_suspended_until_undone(pool: SqlitePool) {
        let now = Utc::now();
        let card = create_cards(&pool, 1, now - Duration::days(30)).await[0];
        let config = SparesExternalConfig {
            leech: LeechConfig {
                lapses_threshold: 2,
                action: LeechAction::Suspend,
                ..LeechConfig::default()
            },
            ..SparesExternalConfig::default()
        };

        lapse(&pool, &config, card, now).await;
        assert_eq!(special_state(&pool, card).await, None);
        assert_eq!(note_tags(&pool, card).await, Vec::<String>::new());

        let event = lapse(&pool, &config, card, now).await;
        assert_eq!(
            special_state(&pool, card).await,
            Some(SpecialState::Suspended)
        );
        assert_eq!(note_tags(&pool, card).await, ["leech"]);

        // As review clients undo a rating
        undo_event(
            &pool,
            UndoEventRequest {
                event_id: Some(event),
                undo_group: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(special_state(&pool, card).await, None);
        assert_eq!(note_tags(&pool, card).await, Vec::<String>::new());
    }

    #[sqlx::test]
    async fn tag_action_keeps_reviewing(pool: SqlitePool) {
        let now = Utc::now();
        let card = create_cards(&pool, 1, now - Duration::days(30)).await[0];
        let config = SparesExternalConfig {
            leech: LeechConfig {
                lapses_threshold: 1,
                ..LeechConfig::default()
            },
            ..SparesExternalConfig::default()
        };
        lapse(&pool, &config, card, now).await;
        assert_eq!(special_state(&pool, card).await, None);
        assert_eq!(note_tags(&pool, card).await, ["leech"]);
    }
}

#[cfg(test)]
mod reschedule_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::tests::REVIEW;
    use super::tests::create_cards;
    use super::tests::set_card;
    use super::*;
    use crate::api::fetch_review_logs_for_replay;
    use crate::api::review::rate_card;
    use crate::api::review::reschedule_all_cards;
    use crate::api::statistics::get_review_history;
    use crate::api::statistics::get_statistics;
    use crate::api::tag::create_tag;
    use crate::api::undo::undo_event;
    use crate::config::SparesExternalConfig;
    use crate::model::Card;
    use crate::model::CardId;
    use crate::model::ReviewLog;
    use crate::model::ReviewLogKind;
    use crate::model::TagId;
    use crate::schedulers::get_scheduler_from_string;
    use crate::schedulers::review_state;
    use crate::schema::review::RatingSubmission;
    use crate::schema::review::ReviewHistoryRequest;
    use crate::schema::review::StatisticsRequest;
    use crate::schema::tag::CreateTagRequest;
    use crate::schema::undo::UndoEventRequest;

    const GOOD: u32 = 3;
    const EASY: u32 = 4;

    async fn card(pool: &SqlitePool, card_id: CardId) -> Card {
        sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn in_tag(pool: &SqlitePool, card_id: CardId, tag_id: TagId) -> bool {
        sqlx::query_scalar(r"SELECT COUNT(*) > 0 FROM card_tag WHERE card_id = ? AND tag_id = ?")
            .bind(card_id)
            .bind(tag_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn rate(pool: &SqlitePool, card_id: CardId, tag_id: TagId, rating: u32) -> i64 {
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        rate_card(
            pool,
            scheduler.as_ref(),
            RatingSubmission {
                card_id,
                rating,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(1),
                tag_id: Some(tag_id),
            },
            Utc::now(),
            true,
        )
        .await
        .unwrap()
        .unwrap()
    }

    #[sqlx::test]
    async fn cramming_leaves_the_schedule_alone(pool: SqlitePool) {
        let now = Utc::now();
        let card_id = create_cards(&pool, 1, now - Duration::days(30)).await[0];
        set_card(&pool, card_id, REVIEW, now + Duration::days(10), 20.0, 4.0).await;
        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "cram".to_string(),
                description: String::new(),
                query: Some("c.id>0".to_string()),
                auto_delete: false,
                reschedule: false,
            },
            false,
        )
        .await
        .unwrap();
        assert!(!tag.reschedule);
        let before = card(&pool, card_id).await;

        rate(&pool, card_id, tag.id, GOOD).await;
        let after = card(&pool, card_id).await;
        assert_eq!(
            (after.due, after.stability, after.difficulty, after.state),
            (
                before.due,
                before.stability,
                before.difficulty,
                before.state
            )
        );
        assert!(
            in_tag(&pool, card_id, tag.id).await,
            "one Good is not enough to leave"
        );
        // The rating is logged as a cram, under the tag, with no interval
        let logs: Vec<ReviewLog> = sqlx::query_as(r"SELECT * FROM review_log")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].kind, ReviewLogKind::Cram);
        assert_eq!(
            (logs[0].rating, logs[0].tag_id, logs[0].scheduled_time),
            (Some(GOOD), Some(tag.id), None)
        );
        assert_eq!(logs[0].previous_state, review_state());
        assert!(
            fetch_review_logs_for_replay(&pool, card_id)
                .await
                .unwrap()
                .is_empty(),
            "replay does not see crams"
        );

        // Easy finishes the card's run through the tag, and undo puts it back and deletes its
        // cram
        let event = rate(&pool, card_id, tag.id, EASY).await;
        assert!(!in_tag(&pool, card_id, tag.id).await);
        assert_eq!(cram_count(&pool).await, 2);
        undo_event(
            &pool,
            UndoEventRequest {
                event_id: Some(event),
                undo_group: true,
            },
        )
        .await
        .unwrap();
        assert!(in_tag(&pool, card_id, tag.id).await);
        assert_eq!(card(&pool, card_id).await.custom_data, after.custom_data);
        assert_eq!(cram_count(&pool).await, 1);
    }

    async fn cram_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar(r"SELECT COUNT(*) FROM review_log WHERE kind = ?")
            .bind(ReviewLogKind::Cram)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn cram_tag(pool: &SqlitePool) -> TagId {
        create_tag(
            pool,
            CreateTagRequest {
                name: "exam-cram".to_string(),
                description: String::new(),
                query: Some("c.id>0".to_string()),
                auto_delete: false,
                reschedule: false,
            },
            false,
        )
        .await
        .unwrap()
        .id
    }

    /// The example of `ReviewLogKind::Cram`: crams count in the statistics and the heatmap,
    /// apart from reviews, but a reschedule replays the card as if they never happened.
    #[sqlx::test]
    async fn crams_count_in_statistics_but_not_in_replay(pool: SqlitePool) {
        let now = Utc::now();
        let card_id = create_cards(&pool, 1, now - Duration::days(30)).await[0];
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        // Learned a week ago, then crammed today, Again (which would be a lapse) and Good
        for reviewed_at in [now - Duration::days(8), now - Duration::days(7)] {
            rate_card(
                &pool,
                scheduler.as_ref(),
                RatingSubmission {
                    card_id,
                    rating: GOOD,
                    recall_duration: Duration::seconds(5),
                    rate_duration: Duration::seconds(1),
                    tag_id: None,
                },
                reviewed_at,
                false,
            )
            .await
            .unwrap();
        }
        let learned = card(&pool, card_id).await;
        assert_eq!(learned.state, review_state());
        let tag_id = cram_tag(&pool).await;
        rate(&pool, card_id, tag_id, 1).await;
        rate(&pool, card_id, tag_id, GOOD).await;

        let statistics = get_statistics(
            &pool,
            StatisticsRequest {
                scheduler_name: "fsrs".to_string(),
                date: now,
                tz: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(statistics.cards_studied_count, 0, "no review today");
        assert_eq!(statistics.cram_count, 2);
        assert_eq!(statistics.cram_duration, Duration::seconds(12));
        let history = get_review_history(&pool, ReviewHistoryRequest::default())
            .await
            .unwrap();
        let today = now.with_timezone(&Local).date_naive();
        assert_eq!(history.cram_count_by_date.get(&today), Some(&2));
        assert_eq!(history.review_count_by_date.values().sum::<u32>(), 2);

        // Neither the cram rated Again nor a reschedule changes the card
        assert_eq!(scheduler.lapses(&pool, card_id).await.unwrap(), 0);
        reschedule_all_cards(
            &pool,
            scheduler.as_ref(),
            &SparesExternalConfig::default(),
            now,
        )
        .await
        .unwrap();
        let rescheduled = card(&pool, card_id).await;
        assert_eq!(
            (
                rescheduled.stability,
                rescheduled.difficulty,
                rescheduled.state
            ),
            (learned.stability, learned.difficulty, learned.state)
        );
    }

    #[test]
    fn last_review_skips_unreplayed_kinds() {
        assert!(LAST_REVIEW.contains(&format!("rl.kind < {}", ReviewLogKind::FIRST_UNREPLAYED)));
    }
}

#[cfg(test)]
mod preset_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::tests::NEW;
    use super::tests::REVIEW;
    use super::tests::add_review;
    use super::tests::set_card;
    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::api::preset::CardConfigs;
    use crate::api::preset::card_presets;
    use crate::api::preset::cards_with_preset;
    use crate::api::preset::preset_sql;
    use crate::api::review::DailyLimitsLeft;
    use crate::api::review::daily_limits_left_with;
    use crate::api::review::rate_card_with_config;
    use crate::config::SparesExternalConfig;
    use crate::model::CardId;
    use crate::model::SpecialState;
    use crate::parsers::get_all_parsers;
    use crate::schedulers::get_scheduler_from_string;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::review::RatingSubmission;

    /// The example on [`QueuePreset`].
    const EXAMPLE: &str = r#"
        [queue]
        review_order = "due"

        [queue.presets.languages]
        tags = ["lang"]                  # notes tagged `lang`, or `lang:german`, `lang:french:verbs`...
        new_cards_daily_limit = 40
        new_order = "random"
        review_order = "random"
        learning_steps = [60, 300, 1800] # 1, 5 and 30 minutes

        [queue.presets.exam]
        tags = ["math:analysis", "math:algebra"]
        new_cards_daily_limit = 5
        review_order = "retrievability"
        relearning_steps = [600, 3600]
        bury_new_siblings = true
    "#;

    pub(super) fn parse(toml: &str) -> SparesExternalConfig {
        let mut config: SparesExternalConfig = toml_edit::de::from_str(toml).unwrap();
        config.validate().unwrap();
        config
    }

    fn invalid(toml: &str) -> String {
        let mut config: SparesExternalConfig = match toml_edit::de::from_str(toml) {
            Ok(config) => config,
            Err(error) => return error.to_string(),
        };
        config.validate().unwrap_err()
    }

    /// One note of one card for each list of tags, in order, the `i`th created `i` hours after
    /// the first, returning their cards.
    pub(super) async fn tagged_cards(
        pool: &SqlitePool,
        tags: &[&[&str]],
        now: DateTime<Utc>,
    ) -> Vec<CardId> {
        let parser = create_parser_helper(pool, "markdown").await;
        for (i, note_tags) in tags.iter().enumerate() {
            create_notes(
                pool,
                CreateNotesRequest {
                    parser_id: parser.id,
                    requests: vec![CreateNoteRequest {
                        data: format!("Note {i}: {{{{answer {i}}}}}"),
                        keywords: Vec::new(),
                        tags: note_tags.iter().map(ToString::to_string).collect(),
                        is_suspended: false,
                        custom_data: serde_json::Map::new(),
                    }],
                },
                now + Duration::hours(i64::try_from(i).unwrap()),
                &get_all_parsers(),
                false,
            )
            .await
            .unwrap();
        }
        sqlx::query_scalar(r"SELECT c.id FROM card c JOIN note n ON c.note_id = n.id ORDER BY n.id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[test]
    fn documented_example_parses_in_order() {
        let config = parse(EXAMPLE);
        let names = config.queue.presets.keys().collect::<Vec<_>>();
        assert_eq!(names, ["languages", "exam"], "listed order is priority");
        let languages = &config.queue.presets["languages"];
        assert_eq!(languages.new_cards_daily_limit, Some(40));
        assert_eq!(languages.reviews_daily_limit, None);
        assert_eq!(
            languages.learning_steps,
            Some(vec![
                Duration::minutes(1),
                Duration::minutes(5),
                Duration::minutes(30)
            ])
        );
        assert_eq!(
            config.queue.presets["exam"].review_order,
            Some(ReviewOrder::Retrievability)
        );
        // Written back as read
        let written = toml_edit::ser::to_string_pretty(&config).unwrap();
        assert_eq!(parse(&written).queue.presets, config.queue.presets);
    }

    #[test]
    fn linked_note_settings_parse() {
        let config = parse("");
        assert!(!config.queue.new_after_linked_notes);
        assert!(!config.queue.reviews_after_linked_notes);
        assert_eq!(config.queue.linked_note_max_score, 1.5);
        let config = parse(
            "[queue]\nnew_after_linked_notes = true\nreviews_after_linked_notes = true\nlinked_note_max_score = 2.5\n",
        );
        assert!(config.queue.new_after_linked_notes);
        assert!(config.queue.reviews_after_linked_notes);
        assert_eq!(config.queue.linked_note_max_score, 2.5);
        // Global only
        let error =
            invalid("[queue.presets.math]\ntags = [\"math\"]\nnew_after_linked_notes = true\n");
        assert!(error.contains("new_after_linked_notes"), "{error}");
    }

    #[test]
    fn invalid_presets_are_rejected() {
        let error = invalid("[queue.presets.empty]\nnew_cards_daily_limit = 5\n");
        assert!(error.contains("queue.presets.empty.tags"), "{error}");
        let error = invalid("[queue.presets.steps]\ntags = [\"a\"]\nlearning_steps = [600, 60]\n");
        assert!(
            error.contains("queue.presets.steps.learning_steps"),
            "{error}"
        );
        // A setting presets cannot have, or a typo, is not silently ignored
        let error =
            invalid("[queue.presets.mix]\ntags = [\"a\"]\nnew_review_mix = \"new_first\"\n");
        assert!(error.contains("new_review_mix"), "{error}");
        let error =
            invalid("[queue.presets.mix]\ntags = [\"a\"]\ninterleave_by = \"top_level_tag\"\n");
        assert!(error.contains("interleave_by"), "{error}");
        let error = invalid("[queue.presets.typo]\ntags = [\"a\"]\nnew_card_daily_limit = 5\n");
        assert!(error.contains("new_card_daily_limit"), "{error}");
    }

    /// The first preset listed whose tag the note has, exactly or as a parent, wins.
    #[sqlx::test]
    async fn cards_use_the_first_matching_preset(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[
                &["lang"],
                &["lang:german"],
                &["math:analysis:limits"],
                &["math:analysis", "lang:french"],
                &["language", "math", "old:lang"],
                &[],
            ],
            now,
        )
        .await;
        let config = parse(EXAMPLE);
        let presets = card_presets(&pool, &preset_sql(&config.queue.presets))
            .await
            .unwrap();
        let of = |i: usize| presets.get(&ids[i]).copied();
        assert_eq!(
            (0..ids.len()).map(of).collect::<Vec<_>>(),
            [Some(0), Some(0), Some(1), Some(0), None, None]
        );
    }

    /// Each preset's limits cap its own cards, and the top-level limits the total.
    #[sqlx::test]
    async fn preset_limits_cap_their_cards(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[&["lang"], &["lang"], &["lang"], &[], &[], &["exam"]],
            now - Duration::days(30),
        )
        .await;
        let mut config = parse(
            r#"
            new_cards_daily_limit = 4
            new_cards_ignore_review_limit = true

            [queue.presets.languages]
            tags = ["lang"]
            new_cards_daily_limit = 2

            [queue.presets.exam]
            tags = ["exam"]
            reviews_daily_limit = 1
            "#,
        );
        let left = async |config: &SparesExternalConfig| {
            daily_limits_left_with(&pool, config, now).await.unwrap()
        };
        assert_eq!(
            left(&config).await,
            DailyLimitsLeft {
                new: 4,
                reviews: u32::MAX,
                presets: vec![
                    PresetLimitsLeft {
                        new: Some(2),
                        reviews: None
                    },
                    PresetLimitsLeft {
                        new: None,
                        reviews: Some(1)
                    },
                ],
            }
        );

        // Two language cards studied use up its limit, and half the top-level one
        add_review(&pool, ids[0], now, NEW).await;
        add_review(&pool, ids[1], now, NEW).await;
        let limits = left(&config).await;
        assert_eq!((limits.new, limits.presets[0].new), (2, Some(0)));
        // So review leaves out the third language card, but not the others
        let preset = crate::api::preset::limited_preset_sql(&config.queue);
        let shown: Vec<CardId> = sqlx::query_scalar(&format!(
            "SELECT c.id FROM card c WHERE c.state = {NEW} AND c.id NOT IN ({}, {}) {}",
            ids[0],
            ids[1],
            crate::api::review::daily_limits_clause(&limits, &preset)
        ))
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(shown, ids[3..]);
        // And it is counted as nothing left to study
        let counted = crate::api::preset::cap_counts(
            [
                (0, Some(0), 1, 0.0),
                (0, None, 2, 0.0),
                (0, Some(1), 1, 0.0),
            ],
            &QueueCaps::from_limits(&limits),
        );
        assert_eq!(
            counted[&0].0, 2,
            "the top-level limit caps the other 3 at 2"
        );

        // Without ignoring the review limit, the exam preset's reviews due leave no room for its
        // new cards
        config.new_cards_ignore_review_limit = false;
        set_card(&pool, ids[5], REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        let limits = left(&config).await;
        assert_eq!(
            limits.presets[1],
            PresetLimitsLeft {
                new: Some(0),
                reviews: Some(1)
            }
        );
    }

    /// Cards of presets that order differently come one order after another, each in its own.
    #[sqlx::test]
    async fn presets_order_their_cards(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[&[], &["lang"], &["exam"], &["lang"], &[]],
            now - Duration::days(1),
        )
        .await;
        let [other_1, lang_1, exam, lang_2, other_2] = ids[..] else {
            unreachable!()
        };
        let mut config = parse(
            r#"
            [queue]
            new_review_mix = "new_first"

            [queue.presets.languages]
            tags = ["lang"]
            new_order = "newest_first"

            [queue.presets.exam]
            tags = ["exam"]
            new_cards_daily_limit = 5
            "#,
        )
        .queue;
        let order = async |config: &QueueConfig| {
            let query = format!(
                "SELECT c.id FROM {} c JOIN note n ON c.note_id = n.id ORDER BY {}",
                cards_with_preset(&config.presets),
                queue_order_by(
                    &SparesExternalConfig {
                        queue: config.clone(),
                        ..Default::default()
                    },
                    now,
                    &QueueCaps::default(),
                    &[]
                )
            );
            sqlx::query_scalar::<_, CardId>(&query)
                .fetch_all(&pool)
                .await
                .unwrap()
        };
        // Languages first, newest first, then the exam and other cards together, in the
        // top-level order
        assert_eq!(
            order(&config).await,
            [lang_2, lang_1, other_1, exam, other_2]
        );
        // A preset ordering like the top level changes nothing
        config.presets["languages"].new_order = Some(NewCardOrder::Created);
        assert_eq!(order(&config).await, ids);
    }

    /// Rating a card uses its preset's steps and sibling burying, and a reschedule replays it
    /// with them.
    #[sqlx::test]
    async fn rating_uses_the_preset_steps(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(&pool, &[&["lang"], &[]], now - Duration::days(1)).await;
        let config = parse(EXAMPLE);
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        let due_after_good = async |card_id: CardId| -> Duration {
            rate_card_with_config(
                &pool,
                scheduler.as_ref(),
                &config,
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
            let due: DateTime<Utc> = sqlx::query_scalar(r"SELECT due FROM card WHERE id = ?")
                .bind(card_id)
                .fetch_one(&pool)
                .await
                .unwrap();
            due - now
        };
        // Good on a new card moves it to the second step: 5 minutes for `languages`, the
        // top-level 10 minutes for the other
        let lang = due_after_good(ids[0]).await;
        let other = due_after_good(ids[1]).await;
        assert!(
            (lang - Duration::minutes(5)).num_seconds().abs() <= 1,
            "{lang}"
        );
        assert!(
            (other - Duration::minutes(10)).num_seconds().abs() <= 1,
            "{other}"
        );

        let configs = CardConfigs::load(&pool, &config, None).await.unwrap();
        assert_eq!(configs.get(ids[0]).learning_steps[1], Duration::minutes(5));
        assert_eq!(configs.get(ids[1]).learning_steps[1], Duration::minutes(10));
        assert_eq!(
            configs.base().learning_steps[1],
            Duration::minutes(10),
            "placement uses the top level"
        );
    }

    #[sqlx::test]
    async fn rating_buries_siblings_by_preset(pool: SqlitePool) {
        let now = Utc::now();
        let parser = create_parser_helper(&pool, "markdown").await;
        create_notes(
            &pool,
            CreateNotesRequest {
                parser_id: parser.id,
                requests: vec![CreateNoteRequest {
                    data: "{{a}} {{b}}".to_string(),
                    keywords: Vec::new(),
                    tags: vec!["math:analysis".to_string()],
                    is_suspended: false,
                    custom_data: serde_json::Map::new(),
                }],
            },
            now - Duration::days(1),
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();
        let ids: Vec<CardId> = sqlx::query_scalar(r#"SELECT id FROM card ORDER BY "order""#)
            .fetch_all(&pool)
            .await
            .unwrap();
        // The top level does not bury, but `exam` does
        let config = parse(EXAMPLE);
        assert!(!config.queue.bury_new_siblings);
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        rate_card_with_config(
            &pool,
            scheduler.as_ref(),
            &config,
            RatingSubmission {
                card_id: ids[0],
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
        let sibling: Option<SpecialState> =
            sqlx::query_scalar(r"SELECT special_state FROM card WHERE id = ?")
                .bind(ids[1])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sibling, Some(SpecialState::SchedulerBuried));
    }
}

#[cfg(test)]
mod interleave_tests {
    use chrono::Duration;
    use sqlx::SqlitePool;

    use super::preset_tests::parse;
    use super::preset_tests::tagged_cards;
    use super::tests::REVIEW;
    use super::tests::set_card;
    use super::*;
    use crate::api::preset::cards_with_preset;

    /// Every card, in queue order under `toml`, with `caps` and `query_sort`.
    async fn ordered(
        pool: &SqlitePool,
        toml: &str,
        now: DateTime<Utc>,
        caps: &QueueCaps,
        query_sort: &[String],
    ) -> Vec<CardId> {
        let config = parse(toml);
        let query = format!(
            "SELECT c.id FROM {} c JOIN note n ON c.note_id = n.id ORDER BY {}",
            cards_with_preset(&config.queue.presets),
            queue_order_by(&config, now, caps, query_sort)
        );
        sqlx::query_scalar(&query).fetch_all(pool).await.unwrap()
    }

    /// The example on [`QueueConfig::interleave_by`].
    #[sqlx::test]
    async fn subjects_take_turns_in_proportion(pool: SqlitePool) {
        let now = Utc::now();
        let mut tags: Vec<&[&str]> = vec![&["math:analysis"]; 6];
        tags.extend([&["chem"][..]; 3]);
        let ids = tagged_cards(&pool, &tags, now - Duration::days(10)).await;
        for card in &ids {
            set_card(&pool, *card, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        }
        let (m, c) = ids.split_at(6);
        let interleaved = "[queue]\ninterleave_by = \"top_level_tag\"\n";
        let none = QueueCaps::default();
        assert_eq!(ordered(&pool, "", now, &none, &[]).await, ids);
        assert_eq!(
            ordered(&pool, interleaved, now, &none, &[]).await,
            [m[0], c[0], m[1], m[2], c[1], m[3], m[4], c[2], m[5]]
        );
        // A query's sort takes its place
        let sort = ["n.created_at DESC".to_string()];
        let mut newest_first = ids.clone();
        newest_first.reverse();
        assert_eq!(
            ordered(&pool, interleaved, now, &none, &sort).await,
            newest_first
        );
    }

    /// Each kind is mixed by subject, then the kinds are mixed, within today's limits.
    #[sqlx::test]
    async fn new_cards_and_reviews_are_each_mixed(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[
                &["math"],
                &["math"],
                &["chem"],
                &["chem"],
                &["math"],
                &["math"],
            ],
            now - Duration::days(10),
        )
        .await;
        let [rm1, rm2, rc1, rc2, nm1, nm2] = ids[..] else {
            unreachable!()
        };
        for card in [rm1, rm2, rc1, rc2] {
            set_card(&pool, card, REVIEW, now - Duration::days(1), 5.0, 5.0).await;
        }
        let toml = |mix: &str| {
            format!("[queue]\ninterleave_by = \"top_level_tag\"\nnew_review_mix = \"{mix}\"\n")
        };
        let none = QueueCaps::default();
        assert_eq!(
            ordered(&pool, &toml("reviews_first"), now, &none, &[]).await,
            [rm1, rc1, rm2, rc2, nm1, nm2]
        );
        // The new cards' positions are estimated from their subject, so tie with the reviews
        // 1/4 and 3/4 of the way through, and come after them
        assert_eq!(
            ordered(&pool, &toml("interleaved"), now, &none, &[]).await,
            [rm1, rc1, nm1, rm2, rc2, nm2]
        );
        // With two reviews left, one of each subject, the new cards are spread over those two
        let two_reviews = QueueCaps {
            reviews: Some(2),
            ..QueueCaps::default()
        };
        assert_eq!(
            ordered(&pool, &toml("interleaved"), now, &two_reviews, &[]).await,
            [nm1, rm1, rc1, nm2, rm2, rc2]
        );
    }

    /// Subjects take turns across presets that order differently, which are only gathered within
    /// a subject.
    #[sqlx::test]
    async fn subjects_take_turns_across_presets(pool: SqlitePool) {
        let now = Utc::now();
        let ids = tagged_cards(
            &pool,
            &[&["math"], &["lang"], &["math"], &["lang"]],
            now - Duration::days(1),
        )
        .await;
        let [math_1, lang_1, math_2, lang_2] = ids[..] else {
            unreachable!()
        };
        let toml = |interleave_by: &str| {
            format!(
                "[queue]\nnew_review_mix = \"new_first\"\ninterleave_by = \"{interleave_by}\"\n\
                 [queue.presets.languages]\ntags = [\"lang\"]\nnew_order = \"newest_first\"\n"
            )
        };
        let none = QueueCaps::default();
        assert_eq!(
            ordered(&pool, &toml("none"), now, &none, &[]).await,
            [lang_2, lang_1, math_1, math_2]
        );
        assert_eq!(
            ordered(&pool, &toml("top_level_tag"), now, &none, &[]).await,
            [lang_2, math_1, lang_1, math_2]
        );
    }

    /// A note's subject is its first top-level tag, leaving out the flagged and leech tags.
    #[sqlx::test]
    async fn subject_is_the_first_top_level_tag(pool: SqlitePool) {
        let now = Utc::now();
        tagged_cards(
            &pool,
            &[
                &["math:analysis", "chem"],
                &["math:algebra:groups"],
                &["leech", "math"],
                &["flagged"],
                &[],
            ],
            now,
        )
        .await;
        let config = parse("[queue]\ninterleave_by = \"top_level_tag\"\n");
        let subject = subject_sql(&config).unwrap();
        let subjects: Vec<Option<String>> =
            sqlx::query_scalar(&format!("SELECT {subject} FROM card c ORDER BY c.id"))
                .fetch_all(&pool)
                .await
                .unwrap();
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            subjects,
            [some("chem"), some("math"), some("math"), None, None]
        );
        assert_eq!(subject_sql(&parse("")), None);
    }
}
