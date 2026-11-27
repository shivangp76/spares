//! When a goal's new cards would be learned with today's settings: its presets' limits and orders,
//! sibling burying and [`new_after_linked_notes`](crate::config::QueueConfig::new_after_linked_notes),
//! studying every day. See [`project`].
//!
//! The pace of the last two weeks says nothing about what the queue lets through. With linked
//! notes held back, most of an exam's new cards can be waiting on older notes, so a part can be
//! capped well below its limit however high the limit is set.

use std::collections::HashMap;

use itertools::Itertools;
use sqlx::AssertSqlSafe;
use sqlx::SqlitePool;

use crate::Error;
use crate::api::note_position::ensure_note_positions_in;
use crate::api::note_position::position_sql;
use crate::api::preset::preset_index;
use crate::api::preset::preset_sql;
use crate::api::preset::resolve_preset_queries;
use crate::api::queue::waiting_chain_weights;
use crate::config::BuryNewSiblings;
use crate::config::NewCardOrder;
use crate::config::SparesExternalConfig;
use crate::model::CardId;
use crate::model::NoteId;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::state_role;
use crate::schema::review::StateRole;

/// The most days projected: past them, a card is taken never to be learned.
pub(crate) const HORIZON_DAYS: u32 = 3 * 365;

/// The runs of a projection whose orders are random, each with its own draw.
const RANDOM_RUNS: u64 = 5;

/// Where a card of the projection starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Start {
    New,
    /// In its learning steps: it holds its note's dependents back today, and is learned tomorrow.
    Learning,
    /// Reviewed, relearning, or suspended: it holds nothing back.
    Done,
}

/// A card of the goal, for [`simulate`].
#[derive(Clone, Debug)]
pub(crate) struct SimCard {
    pub(crate) id: CardId,
    /// Its note's index in [`SimInput::notes`]
    pub(crate) note: usize,
    /// Its position in its note
    pub(crate) order: i64,
    pub(crate) start: Start,
    /// The index of its preset in [`SimInput::presets`]
    pub(crate) preset: Option<usize>,
}

/// A note of the goal, for [`simulate`].
#[derive(Clone, Debug)]
pub(crate) struct SimNote {
    pub(crate) id: NoteId,
    pub(crate) created_at: i64,
    /// Its place in the order of notes (see [`crate::api::note_position`]), which
    /// [`NewCardOrder::Created`] follows
    pub(crate) position: i64,
    /// The indices of the notes of the goal before it that it links to, which hold its new cards
    /// back
    pub(crate) prerequisites: Vec<usize>,
}

/// How a preset (or the top level, for cards of none) shows new cards.
#[derive(Clone, Debug)]
pub(crate) struct SimPreset {
    pub(crate) name: String,
    /// Its own daily limit of new cards, if it has one
    pub(crate) limit: Option<u32>,
    pub(crate) order: NewCardOrder,
    pub(crate) bury_new_siblings: BuryNewSiblings,
}

/// What [`simulate`] runs on.
#[derive(Clone, Debug)]
pub(crate) struct SimInput {
    pub(crate) cards: Vec<SimCard>,
    pub(crate) notes: Vec<SimNote>,
    pub(crate) presets: Vec<SimPreset>,
    /// The settings of cards of no preset
    pub(crate) top_level: SimPreset,
    /// [`SparesExternalConfig::new_cards_daily_limit`], which caps all new cards together
    pub(crate) daily_limit: u32,
    /// Whether linked notes hold new cards back at all
    pub(crate) hold_on_links: bool,
    /// The day, counted from today as 0, up to which presets' unused limits are counted, as
    /// [`PresetUse`] reports
    pub(crate) count_until: u32,
}

/// How much of a preset's daily limit its cards could use, from [`simulate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PresetUse {
    /// The new cards its limit let through, summed over the days it still had new cards, before
    /// [`SimInput::count_until`]
    pub(crate) slots: u64,
    /// Those of `slots` left unused while some of its new cards were held back by linked notes
    pub(crate) lost_to_links: u64,
}

/// What [`simulate`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SimOutput {
    /// For each run, for each card of [`SimInput::cards`], the day (today being 0) it is first
    /// studied, or `None` if it starts learned or isn't studied within [`HORIZON_DAYS`]. One run
    /// unless some order is random.
    pub(crate) studied_on: Vec<Vec<Option<u32>>>,
    /// For each preset of [`SimInput::presets`], summed over the runs
    pub(crate) presets: Vec<PresetUse>,
}

impl PresetUse {
    /// Counts a day on which the preset still had new cards, of which it took `taken` and
    /// `held_back` waited on links. `top_level_left` is whether the top-level limit had room left,
    /// since slots it took aren't the links' doing.
    fn count_day(&mut self, limit: u32, taken: u32, held_back: u32, top_level_left: bool) {
        self.slots += u64::from(limit);
        if top_level_left {
            let unused = limit.saturating_sub(taken);
            self.lost_to_links += u64::from(unused.min(held_back));
        }
    }
}

/// Whether some new card of `input` comes in a random order, so it takes several runs.
fn draws_at_random(input: &SimInput) -> bool {
    input
        .cards
        .iter()
        .filter(|card| card.start == Start::New)
        .any(|card| {
            matches!(
                input.preset(card).order,
                NewCardOrder::Random | NewCardOrder::RandomNote
            )
        })
}

/// The notes that link to each note of `input`, which it holds back.
fn dependents(input: &SimInput) -> Vec<Vec<usize>> {
    let mut dependents = vec![Vec::new(); input.notes.len()];
    for (note, sim_note) in input.notes.iter().enumerate() {
        for &prerequisite in &sim_note.prerequisites {
            dependents[prerequisite].push(note);
        }
    }
    dependents
}

/// At the start of day `day`, the notes holding their dependents back (with a new card, or today
/// a card in its steps), and each note's new cards, `new_left` being the cards still new.
fn note_states(input: &SimInput, new_left: &[bool], day: u32) -> (Vec<bool>, Vec<u32>) {
    let mut busy = vec![false; input.notes.len()];
    let mut new_cards = vec![0_u32; input.notes.len()];
    for (card, new) in input.cards.iter().zip(new_left) {
        if *new || (day == 0 && card.start == Start::Learning) {
            busy[card.note] = true;
        }
        if *new {
            new_cards[card.note] += 1;
        }
    }
    (busy, new_cards)
}

impl SimInput {
    fn preset(&self, card: &SimCard) -> &SimPreset {
        card.preset
            .and_then(|index| self.presets.get(index))
            .unwrap_or(&self.top_level)
    }
}

/// A hash of `value` for random orders, different each day and run.
fn draw(value: i64, day: u32, run: u64) -> i64 {
    #[expect(clippy::cast_sign_loss, reason = "the bits of an id")]
    let mut x = (value as u64) ^ (u64::from(day) << 40) ^ (run << 56);
    // splitmix64
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    #[expect(clippy::cast_possible_wrap, reason = "only the order matters")]
    let x = (x ^ (x >> 31)) as i64;
    x
}

/// Studies the goal day after day, as review shows new cards, and returns when each card is first
/// studied.
///
/// Each day, from today:
///
/// - The notes with a new card, or (today) a card in its learning steps, hold back the new cards
///   of the notes that link to them, as [`waiting_on_links`](crate::api::queue::waiting_on_links)
///   does when reviewing the goal's search: only the goal's cards hold others back.
/// - New cards are taken in the queue's order: by preset, gathered by order as
///   [`QueueConfig`](crate::config::QueueConfig) describes, then by each preset's
///   [`new_order`](crate::config::QueuePreset::new_order). A card is skipped while its note is
///   held back, its preset's limit is used up, or its preset buries new siblings and a card of
///   its note was taken today. The top-level limit caps them all.
/// - A card taken today is learned tomorrow: every card passes its steps the day it is first
///   studied, and only then lets its note's dependents through.
///
/// That is conservative: in a session, a note's dependents can come up as soon as its cards leave
/// their steps. It also leaves out other study, which shares the top-level limit, and today's
/// cards already studied, which use up some of today's limits.
pub(crate) fn simulate(input: &SimInput) -> SimOutput {
    let runs = if draws_at_random(input) {
        RANDOM_RUNS
    } else {
        1
    };
    let orders = input
        .presets
        .iter()
        .chain([&input.top_level])
        .map(|preset| preset.order)
        .unique()
        .collect::<Vec<_>>();
    let dependents = dependents(input);
    let mut presets = vec![PresetUse::default(); input.presets.len()];
    let mut studied_on = Vec::new();
    for run in 0..runs {
        let mut studied = vec![None; input.cards.len()];
        let mut new_left = input
            .cards
            .iter()
            .map(|card| card.start == Start::New)
            .collect::<Vec<_>>();
        let mut remaining = new_left.iter().filter(|new| **new).count();
        for day in 0..HORIZON_DAYS {
            if remaining == 0 {
                break;
            }
            let (busy, new_cards) = note_states(input, &new_left, day);
            let weights = if input.hold_on_links {
                waiting_chain_weights(&new_cards, &dependents)
            } else {
                vec![0; input.notes.len()]
            };
            let held = |card: &SimCard| {
                input.hold_on_links
                    && input.notes[card.note]
                        .prerequisites
                        .iter()
                        .any(|&prerequisite| busy[prerequisite])
            };
            let queue = day_queue(input, &orders, &new_left, &weights, day, run);

            let mut taken_total = 0_u32;
            let mut taken = vec![0_u32; input.presets.len()];
            let mut held_back = vec![0_u32; input.presets.len()];
            let mut had_new = vec![false; input.presets.len()];
            let mut touched = vec![false; input.notes.len()];
            for index in queue {
                let card = &input.cards[index];
                let preset = input.preset(card);
                if let Some(p) = card.preset {
                    had_new[p] = true;
                }
                let limit = preset.limit.unwrap_or(u32::MAX);
                let preset_taken = card.preset.map_or(taken_total, |p| taken[p]);
                if taken_total >= input.daily_limit || preset_taken >= limit {
                    continue;
                }
                if held(card) {
                    if let Some(p) = card.preset {
                        held_back[p] += 1;
                    }
                    continue;
                }
                let buries = match preset.bury_new_siblings {
                    BuryNewSiblings::No => false,
                    BuryNewSiblings::Yes => true,
                    // A note holding back others with new cards is learned in a day.
                    BuryNewSiblings::UnlessBlocking => weights[card.note] == 0,
                };
                if buries && touched[card.note] {
                    continue;
                }
                touched[card.note] = true;
                new_left[index] = false;
                remaining -= 1;
                studied[index] = Some(day);
                taken_total += 1;
                if let Some(p) = card.preset {
                    taken[p] += 1;
                }
            }
            if day < input.count_until {
                let top_level_left = taken_total < input.daily_limit;
                for (p, preset) in input.presets.iter().enumerate() {
                    let Some(limit) = preset.limit.filter(|_| had_new[p]) else {
                        continue;
                    };
                    presets[p].count_day(limit, taken[p], held_back[p], top_level_left);
                }
            }
        }
        studied_on.push(studied);
    }
    SimOutput {
        studied_on,
        presets,
    }
}

/// The indices of the cards still new, `new_left`, in the order review shows them on day `day` of
/// run `run`, with each note's `weights` for [`NewCardOrder::Unblocking`]. `orders` are the order
/// groups of `keys_by_preset` in [`crate::api::queue`]: each order, in the order of the first
/// preset using it, the top level's last.
fn day_queue(
    input: &SimInput,
    orders: &[NewCardOrder],
    new_left: &[bool],
    weights: &[u32],
    day: u32,
    run: u64,
) -> Vec<usize> {
    let key = |card: &SimCard| {
        let note = &input.notes[card.note];
        let order = input.preset(card).order;
        let group = if orders.len() == 1 {
            0
        } else {
            orders.iter().position(|o| *o == order).unwrap_or(0)
        };
        let within = match order {
            NewCardOrder::Created => (0, note.position, note.id.packed(), card.order),
            NewCardOrder::NewestFirst => (0, -note.created_at, -note.id.packed(), card.order),
            NewCardOrder::Random => (0, draw(card.id, day, run), 0, 0),
            NewCardOrder::RandomNote => (
                0,
                draw(note.id.packed(), day, run),
                note.id.packed(),
                card.order,
            ),
            NewCardOrder::Unblocking => (
                -i64::from(weights[card.note]),
                note.position,
                note.id.packed(),
                card.order,
            ),
        };
        (group, within, card.id)
    };
    (0..input.cards.len())
        .filter(|&index| new_left[index])
        .sorted_by_cached_key(|&index| key(&input.cards[index]))
        .collect()
}

/// A card of the goal, as [`load`] reads it.
#[derive(sqlx::FromRow)]
struct CardRow {
    id: CardId,
    note_id: NoteId,
    order: i64,
    state: StateId,
    special_state: Option<u8>,
    preset: Option<i64>,
}

/// The goal's cards, notes and links, and `config`'s presets, for [`simulate`]. `cards_sql` is
/// the SQL of the goal's card ids; `count_until` is [`SimInput::count_until`].
pub(crate) async fn load(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    cards_sql: &str,
    count_until: u32,
) -> Result<SimInput, Error> {
    ensure_note_positions_in(db, &config.queue).await?;
    resolve_preset_queries(db, config).await?;
    let queue = &config.queue;
    let rows: Vec<CardRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT c.id, c.note_id, c.\"order\", c.state, c.special_state, {} AS preset \
         FROM card c WHERE c.id IN ({cards_sql})",
        preset_sql(&queue.presets)
    )))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let note_ids = rows.iter().map(|row| row.note_id).unique().collect_vec();
    let created: Vec<(NoteId, i64, i64)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT n.id, n.created_at, COALESCE({}, -1) FROM note n WHERE n.id IN ({})",
        position_sql("n"),
        note_ids.iter().map(|id| id.packed()).join(",")
    )))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let mut notes = created
        .into_iter()
        .sorted_by_key(|(id, created_at, position)| (*position, *created_at, id.packed()))
        .map(|(id, created_at, position)| SimNote {
            id,
            created_at,
            position,
            prerequisites: Vec::new(),
        })
        .collect_vec();
    let index_of = notes
        .iter()
        .enumerate()
        .map(|(index, note)| (note.id, index))
        .collect::<HashMap<_, _>>();
    // Only links between the goal's notes hold anything back, and only a note holds back the notes
    // after it. Notes are sorted by position, so that is a lower index.
    let links: Vec<(NoteId, NoteId)> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT DISTINCT note_id, prerequisite_id FROM note_prerequisite \
         WHERE score <= ? AND note_id IN ({ids}) AND prerequisite_id IN ({ids})",
        ids = note_ids.iter().map(|id| id.packed()).join(",")
    )))
    .bind(queue.linked_note_max_score)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    for (parent, linked) in links {
        if let (Some(&parent), Some(&linked)) = (index_of.get(&parent), index_of.get(&linked))
            && linked < parent
            && !notes[parent].prerequisites.contains(&linked)
        {
            notes[parent].prerequisites.push(linked);
        }
    }
    let suspended = SpecialState::Suspended as u8;
    let cards = rows
        .into_iter()
        .filter_map(|row| {
            let start = match state_role(row.state) {
                _ if row.special_state == Some(suspended) => Start::Done,
                Some(StateRole::New) | None => Start::New,
                Some(StateRole::Learning) => Start::Learning,
                Some(StateRole::Review | StateRole::Relearning) => Start::Done,
            };
            Some(SimCard {
                id: row.id,
                note: *index_of.get(&row.note_id)?,
                order: row.order,
                start,
                preset: preset_index(row.preset),
            })
        })
        .collect();
    let presets = queue
        .presets
        .iter()
        .map(|(name, preset)| SimPreset {
            name: name.clone(),
            limit: preset.new_cards_daily_limit,
            order: preset.new_order.unwrap_or(queue.new_order),
            bury_new_siblings: preset.bury_new_siblings.unwrap_or(queue.bury_new_siblings),
        })
        .collect();
    Ok(SimInput {
        cards,
        notes,
        presets,
        top_level: SimPreset {
            name: String::new(),
            limit: None,
            order: queue.new_order,
            bury_new_siblings: queue.bury_new_siblings,
        },
        daily_limit: config.new_cards_daily_limit,
        hold_on_links: queue.new_after_linked_notes,
        count_until,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `notes` notes of `per_note` new cards each, each note linking to the one before it, of one
    /// preset with `limit` new cards a day.
    fn chain(notes: usize, per_note: usize, limit: u32, bury: bool) -> SimInput {
        let notes_vec = (0..notes)
            .map(|index| SimNote {
                id: NoteId::from_packed(i64::try_from(index).unwrap() + 1),
                created_at: i64::try_from(index).unwrap(),
                position: i64::try_from(index).unwrap(),
                prerequisites: index.checked_sub(1).into_iter().collect(),
            })
            .collect();
        let cards = (0..notes * per_note)
            .map(|index| SimCard {
                id: i64::try_from(index).unwrap() + 1,
                note: index / per_note,
                order: i64::try_from(index % per_note).unwrap(),
                start: Start::New,
                preset: Some(0),
            })
            .collect();
        SimInput {
            cards,
            notes: notes_vec,
            presets: vec![SimPreset {
                name: "exam".to_string(),
                limit: Some(limit),
                order: NewCardOrder::Created,
                bury_new_siblings: bury.into(),
            }],
            top_level: SimPreset {
                name: String::new(),
                limit: None,
                order: NewCardOrder::Created,
                bury_new_siblings: BuryNewSiblings::No,
            },
            daily_limit: u32::MAX,
            hold_on_links: true,
            count_until: 100,
        }
    }

    fn days(output: &SimOutput) -> Vec<Option<u32>> {
        assert_eq!(output.studied_on.len(), 1, "one run");
        output.studied_on[0].clone()
    }

    /// A chain of 3 notes of 3 cards: without burying, a note a day, as each waits for the one
    /// before. With burying, a card of a note a day, so each note takes 3 days and the next starts
    /// after.
    #[test]
    fn a_chain_is_learned_a_note_at_a_time() {
        let output = simulate(&chain(3, 3, 10, false));
        let day = Some;
        assert_eq!(
            days(&output),
            [
                day(0),
                day(0),
                day(0),
                day(1),
                day(1),
                day(1),
                day(2),
                day(2),
                day(2)
            ]
        );
        // Each day 7 of the 10 slots are free while 3 or 6 cards wait on links.
        assert_eq!(
            output.presets,
            [PresetUse {
                slots: 30,
                lost_to_links: 6 + 3
            }]
        );
        let output = simulate(&chain(3, 3, 10, true));
        assert_eq!(
            days(&output),
            [
                day(0),
                day(1),
                day(2),
                day(3),
                day(4),
                day(5),
                day(6),
                day(7),
                day(8)
            ]
        );
        // Without links, burying alone spreads the notes over 3 days.
        let mut free = chain(3, 3, 10, true);
        free.hold_on_links = false;
        assert_eq!(
            days(&simulate(&free)),
            [
                day(0),
                day(1),
                day(2),
                day(0),
                day(1),
                day(2),
                day(0),
                day(1),
                day(2)
            ]
        );
        // A limit of 2 a day without burying: the first note's 3 cards take 2 days.
        assert_eq!(
            days(&simulate(&chain(3, 3, 2, false))),
            [
                day(0),
                day(0),
                day(1),
                day(2),
                day(2),
                day(3),
                day(4),
                day(4),
                day(5)
            ]
        );
    }

    /// `"unless_blocking"` learns a note others wait on in a day, and spreads the rest.
    #[test]
    fn unless_blocking_learns_the_heads_of_chains_at_once() {
        let mut input = chain(3, 3, 10, true);
        input.presets[0].bury_new_siblings = BuryNewSiblings::UnlessBlocking;
        let day = Some;
        assert_eq!(
            days(&simulate(&input)),
            [
                day(0),
                day(0),
                day(0),
                day(1),
                day(1),
                day(1),
                day(2),
                day(3),
                day(4)
            ]
        );
    }

    /// `"unblocking"` starts with the head of the longest chain.
    #[test]
    fn unblocking_starts_the_longest_chain_first() {
        // Notes 0 and 1 stand alone, then a chain of 2 -> 3 -> 4, a card each, a card a day.
        let mut input = chain(5, 1, 1, false);
        input.notes[1].prerequisites.clear();
        input.notes[2].prerequisites.clear();
        let day = Some;
        assert_eq!(
            days(&simulate(&input)),
            [day(0), day(1), day(2), day(3), day(4)],
            "created order"
        );
        input.presets[0].order = NewCardOrder::Unblocking;
        assert_eq!(
            days(&simulate(&input)),
            [day(2), day(3), day(0), day(1), day(4)],
            "the chain's head first, then its next note, which note 4 still waits on"
        );
    }

    /// A note's cards learning today hold its dependents back until tomorrow, and cards already
    /// learned aren't studied again.
    #[test]
    fn learning_cards_hold_back_today_only() {
        let mut input = chain(2, 2, 10, false);
        input.cards[0].start = Start::Learning;
        input.cards[1].start = Start::Done;
        let day = Some;
        assert_eq!(days(&simulate(&input)), [None, None, day(1), day(1)]);
    }

    /// Presets ordered differently are gathered by order, the first preset's first, and the
    /// top-level limit is shared.
    #[test]
    fn presets_follow_their_own_orders() {
        let mut input = chain(4, 1, 10, false);
        input.hold_on_links = false;
        input.daily_limit = 2;
        input.presets[0].order = NewCardOrder::NewestFirst;
        input.presets.push(SimPreset {
            name: "other".to_string(),
            limit: Some(1),
            order: NewCardOrder::Created,
            bury_new_siblings: BuryNewSiblings::No,
        });
        // Cards 1 and 2 (the older notes) are of `other`, 3 and 4 of `exam`, newest first.
        input.cards[0].preset = Some(1);
        input.cards[1].preset = Some(1);
        let day = Some;
        assert_eq!(
            days(&simulate(&input)),
            [day(1), day(2), day(0), day(0)],
            "exam's two take today's two slots; other's limit of 1 is then a card a day"
        );
        // Random orders run several draws.
        input.presets[0].order = NewCardOrder::Random;
        let output = simulate(&input);
        assert_eq!(
            output.studied_on.len(),
            usize::try_from(RANDOM_RUNS).unwrap()
        );
        assert!(
            output
                .studied_on
                .iter()
                .all(|run| run.iter().all(Option::is_some))
        );
    }
}
