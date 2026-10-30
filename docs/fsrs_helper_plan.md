# Plan: what fsrs4anki-helper does that spares does not

[fsrs4anki-helper](https://github.com/open-spaced-repetition/fsrs4anki-helper) is the Anki add-on that spares' FSRS features come from. `spares_core/src/schedulers/fsrs/mod.rs` lists how each of its features maps. Spares already has:

- Reschedule from the full review history (`spares card reschedule`) and Optimize.
- Advance and Postpone (`fsrs/reposition.rs`).
- Load balancing, easy days (by weekday and specific dates) and sibling dispersal, as one placement step (`schedulers/placement.rs`).
- True retention (`statistics/graphs.rs`).

Four things are missing, in the order to build them:

1. Reschedule overwrites due dates that were set by hand. A bug more than a missing feature.
2. Summary numbers for FSRS: daily load, and average retrievability and stability.
3. Schedule a break.
4. Steps stats: recommended learning and relearning steps.

The last section lists what is left out on purpose.

## 1. Keep due dates set by hand when rescheduling

The helper's reschedule skips a card whose due date was set by hand unless `reschedule_set_due_date` is on. Spares' does not:

- "Set Card Due Date" and "Set Note Due Date" in `spares review` go through `PATCH /api/cards` (`update_cards`, `api/card.rs`). That changes `card.due` and logs an `UpdateCards` event, but writes no `review_log` row.
- `spares card postpone` and `spares card advance` write a `ReviewLogKind::Reschedule` row.
- `reschedule_all_cards` (`api/review.rs`) fetches only rows with `kind < FIRST_UNREPLAYED`. `SrsScheduler::reschedule` (`schedulers/mod.rs`) then sets every card's due date from the replay. Neither kind of move survives, so `spares card reschedule`, `spares card optimize --apply`, or a change to a card's desired retention undoes them without warning.

### Changes

- **Record the move.** When `update_cards` changes `due` on a card that is not new, also write a `ReviewLogKind::Reschedule` row with `RescheduleData { previous_due, due }`, the same row `move_cards` writes. The payload gets an `origin` field, `Manual`, `Postpone` or `Advance`, which defaults to `Postpone`/`Advance` from the sign of the move when it is missing from old rows. Card info then shows `Set due +3d` next to `Postpone +3d` (`spares_cli/src/card_info.rs`). Undo has to delete the row, as it already does for postpone (`api/undo`).
- **Honour it in reschedule.** A card is *pinned* if its latest row of kind `Reschedule` is newer than its latest replayed row. That means it was moved by hand and has not been reviewed since. `reschedule_all_cards` finds pinned cards in one query (`MAX(reviewed_at)` per card and kind group). `SrsScheduler::reschedule` still recomputes their memory state, but keeps `card.due`, places none of them, and adds all of them to `PlacementContext` before placing the other cards, so those cards balance around them.
- **Setting.** `reschedule_moved_cards = false` in `config.toml`, documented on `SparesExternalConfig` like the other settings (rustdoc, not a separate page). With `true`, reschedule works as it does today. The prompt in `spares_cli/src/card.rs` says how many pinned cards are being kept, and that this setting overrides it.
- **Postponed and advanced cards count as pinned too.** They are user decisions just like a date set by hand.
- A change to one card's desired retention (`update_cards` → `reschedule` of that card) follows the same rule.

### Tests

In `api/scheduling_tests.rs`:

- A card given a due date by hand keeps it through `reschedule_all_cards`, and its stability is still recomputed.
- The same for a postponed card.
- After a later review, the card is no longer pinned.
- With `reschedule_moved_cards = true`, the card is moved.
- Other cards are placed around a pinned card. For example, with `load_balance`, a pinned card counts toward its day.
- Undoing a due date set by hand removes its row.

## 2. Daily load, average retrievability and average stability

The helper's FSRS Stats report three numbers:

- **Daily load:** the sum of `1 / max(1, interval in days)` over the cards in review. It estimates how many reviews a day the collection costs once it settles (SuperMemo calls this *burden*).
- **Average retrievability** of the cards in review today.
- **Average stability.**

`summarize_cards` (`api/statistics/graphs.rs`) already loops over these cards with their intervals, stabilities and recall. It already returns `total_knowledge` (the sum of recall) and histograms. Add `daily_load`, `average_retrievability` and `average_stability` to `Distributions`, computed in the same loop. They are `None` when no scheduler has a forgetting curve, like `total_knowledge`. Show them in the statistics summary on the CLI (`spares_cli/src/statistics.rs`) and on `StatisticsPage.tsx`. Update `types/spares.ts` too.

Test it in `api/statistics/tests.rs` with a few cards whose intervals and stabilities are known.

## 3. Schedule a break

You say you will be away from day `A` for `n` days, and that the reviews should be spread over the `m` days after you return. The helper moves every card that is overdue or due before `A + n` onto `[A + n, A + n + m)`. Its cost function keeps each card as close to its original date as possible.

Spares has two of the pieces already:

- `easy_days.specific_dates` gives a day zero weight in placement.
- Postpone (`reposition.rs`) knows how to estimate the harm a delay does: `approx_retention_after_postpone`, and `is_card_safe`.

Neither does the whole job alone. Specific dates only move a card within its fuzz range, which is a few days at most, so a two-week break cannot be skipped this way. Postpone moves a count of cards, not a range of dates.

### Design

`spares card break --from 2026-12-20 --days 10 --spread 7`, plus `POST /api/cards/break` with the same fields. The core is a new `MoveCardAction::Break { start, days, spread }` in `reposition.rs`:

1. **Choose the cards.** Every card in review, not suspended or buried, that is due before `start + days`. If `start` is today, that includes the overdue cards. Cards in learning steps are left alone, as postpone does.
2. **Order them by harm.** Sort by how much retrievability each card would lose if moved to the end of the spread (with `retrievability` and the card's stability): the card with the most to lose goes first, so it gets the earliest day. This is the helper's "minimise deviation" in the form postpone already uses.
3. **Place them.** Use `PlacementContext` with the candidate days limited to `[start + days, start + days + spread)`. Load balancing, easy days and sibling dispersal then apply as they do anywhere else. Unlike fuzz, every day in the window is a candidate.
4. **Record it.** Write a `Reschedule` row with origin `Break` for each moved card, and log one event so undo restores them all, as postpone does. Under §1, these cards are then pinned.
5. **Keep it from refilling.** Add the break's dates to `easy_days.specific_dates`, so cards rated before the break that land in it are pushed out where their fuzz allows. The helper does not do this. The config change is written with `toml_edit`, like `write_fsrs_parameters` (`config.rs`), which keeps the rest of the file and its comments.

Before moving anything, print the counts: how many cards, the busiest day after the break, and the mean drop in retrievability. Ask for confirmation, like reschedule does. `--dry-run` only prints them.

Not in the first version: advancing some cards to the days *before* a break that starts in the future. With retrievability as the cost this is a small extension: a card due in the break whose recall falls less if it is advanced than if it is postponed goes before the break instead.

### Tests

- Every chosen card ends up in the window.
- Cards due after the break are untouched.
- Load balancing spreads the cards over the window instead of piling them onto its first day.
- Siblings land on different days when the window allows it.
- Undo restores every card.
- With §1 in place, `reschedule_all_cards` afterwards keeps the cards where the break put them.

## 4. Steps stats

The helper's Steps Stats estimate how quickly a new card is forgotten after its first rating, and from that recommend learning steps:

- **First learning step:** from S(Again), the stability after a first rating of Again.
- **Second learning step:** from the smallest of these, since a Hard step is halfway between the two steps:
  - S(Again then Good)
  - S(Good)
  - 2 × S(Hard) − the first step
- **Relearning step:** from the stability after Again on a card in review.

A recommended step is the time at which recall falls to the desired retention: `optimal_interval_days` (`fsrs/mod.rs`) applied to that stability, but in seconds.

### Design

`spares_core::schedulers::fsrs::steps_stats`:

- **Group the reviews.** For each card, pair each review with the next one. Group by the first rating, or the first two for Again then Good, and by whether the card was new or in review. Only cards first reviewed in the chosen period, and only pairs where the next review comes before a day has passed: that is what steps are for. Crams and `Reschedule` rows are left out, as everywhere.
- **Fit a stability per group.** Find the one value of S whose forgetting curve, with the configured decay (`utils::retrievability`), gives the lowest log loss on (seconds elapsed, recalled) pairs. Recalled means rated above Again. A one-dimensional search over a log-spaced grid is enough. Report the number of reviews in each group, and no recommendation for groups that are too small (fewer than 100, say).
- **Scope.** Group by preset: steps are set per `QueuePreset`, so a preset's cards are fitted together, like the helper's "decks sharing a preset".
- **Show it.** `spares card steps [--preset NAME] [--days 365]` prints a table: group, count, fitted S, recommended step, and the current step next to it. The statistics page shows the same table when a preset is chosen.

Test it with a generated log whose recall follows a known stability, and check that the fitted value comes back within a tolerance.

## Left out on purpose

- **Flatten.** Cap reviews per day by postponing the excess. As `fsrs/mod.rs` says, load balancing covers most of it, and a hard cap delays cards with no regard for the harm. The break (§3) and postpone cover the cases where a cap is wanted.
- **Remedy Hard misuse.** Rewriting Hard to Again in the review log changes history that sync, undo and the optimizer treat as fixed. If it is ever wanted, it should be a filter in the optimizer's training set (`fsrs/optimize.rs`), not an edit to the log.
- **Reschedule only recently reviewed cards.** Placement needs every card's due date to balance against. The helper needed it to fix cards after syncing reviews from apps without FSRS, which cannot happen here: the server schedules every review.
- **Auto-reschedule or disperse after sync.** Same reason.
- **Reset custom data and manual reschedule entries.** These are Anki revlog housekeeping with no counterpart here.
- **Small things to pick up as they come:**
  - D, S and R shown under the answer buttons. Card info has them already.
  - A target-retrievability browser column. Search and sort by retrievability exist already.
  - Exporting the review log in the research dataset format.
  - The interval visualiser.
