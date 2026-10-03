//! Choosing a card's due date among the days its scheduler allows.
//!
//! A scheduler proposes a due date and, through [`SrsScheduler::due_candidates`], the range of
//! dates it would accept instead (its fuzz range). This module picks one of those dates. Each
//! candidate day is weighted by:
//!
//! - **Easy days**: the workload percentage of its weekday, or zero if it is one of the specific
//!   easy dates.
//! - **Load balancing**: how many other cards are already due that day, relative to how many
//!   that day should get according to its easy-day weight. A day that already has more than its
//!   share is less likely to be chosen.
//! - **Sibling dispersal**: how close it is to the due dates of other cards of the same note. A
//!   day a sibling is already due on is never chosen unless every candidate is.
//!
//! A date is then drawn with probability proportional to its weight, so with everything disabled
//! this is plain uniform fuzz. This is the same approach as Anki's load balancer, and replaces
//! `fsrs4anki-helper`'s separate easy days, load balance and disperse siblings features.
//!
//! [`SrsScheduler::due_candidates`]: super::SrsScheduler::due_candidates

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Datelike;
use chrono::Local;
use chrono::NaiveDate;
use chrono::Utc;
use rand::Rng;
use rand::distr::Distribution;
use rand::distr::weighted::WeightedIndex;
use sqlx::SqlitePool;

use crate::Error;
use crate::config::SparesExternalConfig;
use crate::helpers::get_start_end_local_date;
use crate::model::Card;
use crate::model::CardId;
use crate::model::NoteId;
use crate::model::ReviewLog;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::schedulers::SrsScheduler;

/// The state of a card that is in review.
///
/// Only cards in review compete for days: new and learning cards have no review-sized due date.
const REVIEW_STATE: StateId = 2;

/// Converts an instant to the calendar day it falls on for the user.
pub type LocalDate = fn(DateTime<Utc>) -> NaiveDate;

fn local_date(instant: DateTime<Utc>) -> NaiveDate {
    instant.with_timezone(&Local).date_naive()
}

/// The due dates that new placements must take into account.
#[derive(Debug)]
pub struct PlacementContext {
    local_date: LocalDate,
    today: NaiveDate,
    /// Every card in review that competes for days, with its note and due date.
    cards: HashMap<CardId, (NoteId, DateTime<Utc>)>,
    /// How many of `cards` are due on each day. Overdue cards count towards today.
    due_counts: HashMap<NaiveDate, u32>,
}

impl PlacementContext {
    pub fn new(local_date: LocalDate, at: DateTime<Utc>) -> Self {
        Self {
            local_date,
            today: local_date(at),
            cards: HashMap::new(),
            due_counts: HashMap::new(),
        }
    }

    /// Loads every card that competes for days.
    pub async fn load_all(db: &SqlitePool, at: DateTime<Utc>) -> Result<Self, Error> {
        Self::load(db, at, None, None).await
    }

    /// Loads what is needed to place `card` on one of `candidates`: the cards due on those days
    /// and the card's siblings.
    pub async fn load_for(
        db: &SqlitePool,
        at: DateTime<Utc>,
        card: &Card,
        candidates: &[DateTime<Utc>],
    ) -> Result<Self, Error> {
        let (Some(first), Some(last)) = (candidates.iter().min(), candidates.iter().max()) else {
            return Ok(Self::new(local_date, at));
        };
        // Overdue cards count towards today, so a window that reaches today must include them.
        let from = if local_date(*first) <= local_date(at) {
            DateTime::<Utc>::MIN_UTC
        } else {
            get_start_end_local_date(first).0
        };
        let to = get_start_end_local_date(last).1;
        Self::load(db, at, Some((from, to)), Some(card.note_id)).await
    }

    async fn load(
        db: &SqlitePool,
        at: DateTime<Utc>,
        due_between: Option<(DateTime<Utc>, DateTime<Utc>)>,
        note_id: Option<NoteId>,
    ) -> Result<Self, Error> {
        let (from, to) =
            due_between.unwrap_or((DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC));
        // Buried cards still compete: they come back. Suspended cards are not studied at all.
        let rows: Vec<(CardId, NoteId, i64)> = sqlx::query_as(
            r"SELECT id, note_id, due FROM card
              WHERE state = ?
                AND (special_state IS NULL OR special_state != ?)
                AND ((due BETWEEN ? AND ?) OR note_id = ?)",
        )
        .bind(REVIEW_STATE)
        .bind(SpecialState::Suspended)
        .bind(from.timestamp())
        .bind(to.timestamp())
        .bind(note_id)
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        let mut context = Self::new(local_date, at);
        for (card_id, note_id, due) in rows {
            let due = DateTime::from_timestamp(due, 0).unwrap_or(DateTime::<Utc>::MIN_UTC);
            context.insert(card_id, note_id, due);
        }
        Ok(context)
    }

    fn day_of(&self, due: DateTime<Utc>) -> NaiveDate {
        (self.local_date)(due).max(self.today)
    }

    pub fn insert(&mut self, card_id: CardId, note_id: NoteId, due: DateTime<Utc>) {
        self.remove(card_id);
        *self.due_counts.entry(self.day_of(due)).or_insert(0) += 1;
        self.cards.insert(card_id, (note_id, due));
    }

    pub fn remove(&mut self, card_id: CardId) {
        if let Some((_, due)) = self.cards.remove(&card_id) {
            let day = self.day_of(due);
            if let Some(count) = self.due_counts.get_mut(&day) {
                *count -= 1;
            }
        }
    }

    /// Records `card`'s current due date, or drops it if it no longer competes for days.
    pub fn update(&mut self, card: &Card) {
        if card.state == REVIEW_STATE && card.special_state != Some(SpecialState::Suspended) {
            self.insert(card.id, card.note_id, card.due);
        } else {
            self.remove(card.id);
        }
    }

    fn sibling_days(&self, card: &Card) -> Vec<NaiveDate> {
        self.cards
            .iter()
            .filter(|(card_id, (note_id, _))| **card_id != card.id && *note_id == card.note_id)
            .map(|(_, (_, due))| self.day_of(*due))
            .collect()
    }

    /// Picks one of `candidates` for `card` and records it.
    ///
    /// The card's own previous due date is not counted against any day.
    ///
    /// # Panics
    ///
    /// If `candidates` is empty.
    pub fn place<R: Rng + ?Sized>(
        &mut self,
        config: &SparesExternalConfig,
        card: &Card,
        candidates: &[DateTime<Utc>],
        rng: &mut R,
    ) -> DateTime<Utc> {
        assert!(!candidates.is_empty(), "nowhere to place card {}", card.id);
        self.remove(card.id);
        let sibling_days = if config.disperse_siblings {
            self.sibling_days(card)
        } else {
            Vec::new()
        };
        let days = candidates
            .iter()
            .map(|candidate| self.day_of(*candidate))
            .collect::<Vec<_>>();

        let easy_day_weights = days
            .iter()
            .map(|day| easy_day_weight(config, *day))
            .collect::<Vec<_>>();
        let load_weights = days
            .iter()
            .zip(&easy_day_weights)
            .map(|(day, easy_day_weight)| {
                if !config.load_balance || *easy_day_weight == 0.0 {
                    return 1.0;
                }
                let due_count = f64::from(self.due_counts.get(day).copied().unwrap_or(0));
                // Compared against the day's share, so that load balancing does not undo easy
                // days by filling the days they leave empty. Squared as in Anki's load balancer.
                (due_count / easy_day_weight + 1.0).powi(-2)
            })
            .collect::<Vec<_>>();
        let sibling_weights = days
            .iter()
            .map(|day| sibling_weight(*day, &sibling_days))
            .collect::<Vec<_>>();

        // Each constraint is dropped, least important first, if together they rule out every
        // candidate: it is better to land next to a sibling than on a vacation day.
        let weightings = [
            easy_day_weights
                .iter()
                .zip(&load_weights)
                .zip(&sibling_weights)
                .map(|((easy, load), sibling)| easy * load * sibling)
                .collect::<Vec<_>>(),
            easy_day_weights
                .iter()
                .zip(&load_weights)
                .map(|(easy, load)| easy * load)
                .collect::<Vec<_>>(),
            vec![1.0; candidates.len()],
        ];
        let weights = weightings
            .into_iter()
            .find(|weights| weights.iter().sum::<f64>() > 0.0)
            .expect("uniform weights are never all zero");
        let dist = WeightedIndex::new(&weights).expect("weights are finite and not all zero");
        let chosen = candidates[dist.sample(rng)];
        self.insert(card.id, card.note_id, chosen);
        chosen
    }
}

/// How much of the normal workload `day` should get, where 1.0 is an ordinary day.
fn easy_day_weight(config: &SparesExternalConfig, day: NaiveDate) -> f64 {
    if config.easy_days.specific_dates.contains(&day) {
        return 0.0;
    }
    // `days_to_workload_percentage` is normalized to sum to 1 across the week.
    config
        .easy_days
        .days_to_workload_percentage
        .get(&day.weekday())
        .map_or(1.0, |percentage| percentage * 7.0)
}

/// Halves the penalty for every day further from the nearest sibling, from 0 on the same day.
fn sibling_weight(day: NaiveDate, sibling_days: &[NaiveDate]) -> f64 {
    sibling_days
        .iter()
        .map(|sibling_day| (day - *sibling_day).num_days().unsigned_abs())
        .min()
        .map_or(1.0, |distance| {
            1.0 - 0.5_f64.powi(i32::try_from(distance).unwrap_or(i32::MAX))
        })
}

/// Picks `card`'s due date, after `scheduler` has proposed `card.due`.
///
/// Returns `card.due` unchanged when the scheduler does not allow it to move.
pub async fn place_due(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    config: &SparesExternalConfig,
    card: &Card,
    review_logs: &[ReviewLog],
    at: DateTime<Utc>,
) -> Result<DateTime<Utc>, Error> {
    let candidates = scheduler.due_candidates(config, card, review_logs, at);
    match candidates.as_slice() {
        [] => Ok(card.due),
        [only] => Ok(*only),
        _ => {
            let mut context = PlacementContext::load_for(db, at, card, &candidates).await?;
            Ok(context.place(config, card, &candidates, &mut rand::rng()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::Duration;
    use chrono::TimeZone;
    use chrono::Weekday;
    use rand::RngExt;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    fn utc_date(instant: DateTime<Utc>) -> NaiveDate {
        instant.date_naive()
    }

    /// Noon on 2024-01-01 (a Monday) plus `days`.
    fn day(days: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap() + Duration::days(days)
    }

    fn config(load_balance: bool, disperse_siblings: bool) -> SparesExternalConfig {
        let mut config = SparesExternalConfig {
            load_balance,
            disperse_siblings,
            ..Default::default()
        };
        config.validate().unwrap();
        config
    }

    fn card(id: CardId, note_id: NoteId) -> Card {
        Card {
            id,
            note_id,
            state: REVIEW_STATE,
            ..Card::new(day(0))
        }
    }

    fn context() -> PlacementContext {
        PlacementContext::new(utc_date, day(0))
    }

    /// Places `card` `runs` times and returns how often each candidate was chosen.
    fn histogram(
        context: &mut PlacementContext,
        config: &SparesExternalConfig,
        card: &Card,
        candidates: &[DateTime<Utc>],
        runs: u32,
    ) -> HashMap<DateTime<Utc>, u32> {
        let mut rng = StdRng::seed_from_u64(7);
        let mut chosen = HashMap::new();
        for _ in 0..runs {
            *chosen
                .entry(context.place(config, card, candidates, &mut rng))
                .or_insert(0) += 1;
        }
        chosen
    }

    #[test]
    fn without_constraints_every_candidate_is_used() {
        let candidates = (10..=14).map(day).collect::<Vec<_>>();
        let chosen = histogram(
            &mut context(),
            &config(false, false),
            &card(1, 1),
            &candidates,
            500,
        );
        assert_eq!(chosen.len(), candidates.len(), "plain fuzz is uniform");
    }

    #[test]
    fn avoids_specific_easy_dates() {
        let mut config = config(true, true);
        config.easy_days.specific_dates = [12, 13].map(|d| utc_date(day(d))).into();
        let candidates = (10..=14).map(day).collect::<Vec<_>>();
        let chosen = histogram(&mut context(), &config, &card(1, 1), &candidates, 200);
        assert!(!chosen.contains_key(&day(12)) && !chosen.contains_key(&day(13)));
    }

    #[test]
    fn avoids_easy_weekdays() {
        let mut config = SparesExternalConfig::default();
        config
            .easy_days
            .days_to_workload_percentage
            .insert(Weekday::Sat, 0.0);
        config.validate().unwrap();
        // 2024-01-06 is a Saturday.
        let candidates = (3..=7).map(day).collect::<Vec<_>>();
        let chosen = histogram(&mut context(), &config, &card(1, 1), &candidates, 200);
        assert!(!chosen.contains_key(&day(5)));
    }

    #[test]
    fn keeps_siblings_apart() {
        let mut context = context();
        context.insert(2, 1, day(11));
        context.insert(3, 1, day(13));
        // Not a sibling: a different note.
        context.insert(4, 2, day(12));
        let candidates = (10..=14).map(day).collect::<Vec<_>>();
        let chosen = histogram(
            &mut context,
            &config(false, true),
            &card(1, 1),
            &candidates,
            200,
        );
        assert!(!chosen.contains_key(&day(11)) && !chosen.contains_key(&day(13)));
        assert!(chosen.contains_key(&day(12)));
    }

    #[test]
    fn sibling_dispersal_can_be_disabled() {
        let mut context = context();
        context.insert(2, 1, day(11));
        let candidates = (10..=12).map(day).collect::<Vec<_>>();
        let chosen = histogram(
            &mut context,
            &config(false, false),
            &card(1, 1),
            &candidates,
            200,
        );
        assert!(chosen.contains_key(&day(11)));
    }

    #[test]
    fn prefers_the_least_loaded_day() {
        let mut context = context();
        let mut id = 100;
        for (offset, count) in [(10, 30), (11, 0), (12, 30)] {
            for _ in 0..count {
                id += 1;
                context.insert(id, id, day(offset));
            }
        }
        let candidates = (10..=12).map(day).collect::<Vec<_>>();
        // Each placement adds to the chosen day, so place a fresh card id every time and remove
        // it again to measure the preference alone.
        let config = config(true, false);
        let mut rng = StdRng::seed_from_u64(7);
        let mut on_empty_day = 0;
        for _ in 0..200 {
            let placed = context.place(&config, &card(1, 1), &candidates, &mut rng);
            context.remove(1);
            on_empty_day += u32::from(placed == day(11));
        }
        assert!(
            on_empty_day > 190,
            "chose the empty day {on_empty_day} times out of 200"
        );
    }

    #[test]
    fn placing_a_card_moves_its_own_count() {
        let mut context = context();
        context.insert(1, 1, day(20));
        let config = config(true, true);
        let placed = context.place(
            &config,
            &card(1, 1),
            &[day(10)],
            &mut StdRng::seed_from_u64(0),
        );
        assert_eq!(placed, day(10));
        assert_eq!(context.due_counts.get(&utc_date(day(20))), Some(&0));
        assert_eq!(context.due_counts.get(&utc_date(day(10))), Some(&1));
        assert!(
            context.sibling_days(&card(1, 1)).is_empty(),
            "a card is not its own sibling"
        );
    }

    #[test]
    fn overdue_cards_count_towards_today() {
        let mut context = context();
        context.insert(1, 1, day(-30));
        assert_eq!(context.due_counts.get(&utc_date(day(0))), Some(&1));
    }

    #[test]
    fn falls_back_when_every_candidate_is_ruled_out() {
        let mut config = config(true, true);
        config.easy_days.specific_dates = (10..=12).map(|d| utc_date(day(d))).collect();
        let mut context = context();
        context.insert(2, 1, day(11));
        let candidates = (10..=12).map(day).collect::<Vec<_>>();
        let chosen = histogram(&mut context, &config, &card(1, 1), &candidates, 300);
        assert_eq!(
            chosen.len(),
            3,
            "with every day a vacation day, any day will do"
        );

        // A vacation day outranks a sibling: only the sibling constraint is dropped.
        config.easy_days.specific_dates = [utc_date(day(10))].into();
        let mut context = PlacementContext::new(utc_date, day(0));
        context.insert(2, 1, day(11));
        context.insert(3, 1, day(12));
        let chosen = histogram(&mut context, &config, &card(1, 1), &candidates, 300);
        assert!(!chosen.contains_key(&day(10)));
    }

    /// Whatever the constraints, the chosen date is always one of the candidates.
    #[test]
    fn always_chooses_a_candidate() {
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..500 {
            let mut config = config(rng.random(), rng.random());
            config.easy_days.specific_dates = (0..rng.random_range(0..10))
                .map(|_| utc_date(day(rng.random_range(0..30))))
                .collect::<HashSet<_>>();
            let mut context = context();
            for id in 2..rng.random_range(2..40) {
                context.insert(id, rng.random_range(1..4), day(rng.random_range(-5..30)));
            }
            let start = rng.random_range(1..25);
            let candidates = (start..start + rng.random_range(1..6))
                .map(day)
                .collect::<Vec<_>>();
            let placed = context.place(&config, &card(1, 1), &candidates, &mut rng);
            assert!(candidates.contains(&placed));
        }
    }
}
