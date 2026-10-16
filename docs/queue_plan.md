# Card queue plan

Work left to bring spares' card queue up to Anki's, and beyond. What is done is documented on the
code: the settings on `QueueConfig`, `LeechConfig` and `SparesExternalConfig` in
`spares_core::config`, and the rest where it is implemented (`cargo doc --workspace --no-deps
--document-private-items --open`).

Each improvement below says what is missing, when it matters (use case), a small example, and how
it could be built.

## Gaps in what is done

### Learn ahead limit offline

Offline review shows learning cards due today as soon as nothing else is left, ignoring
`learn_ahead_limit`.

- **Use case.** Studying on a phone on a train, the offline queue shows a card on a 1 hour step
  right after its first rating once the other cards are done, which defeats the step.
- **Example.** With `learn_ahead_limit = 1200` and steps of 1, 10 and 60 minutes, online review
  waits for a card on its 60 minute step until 20 minutes of it are left. Offline, the same card
  comes back at once.
- **How.** Send the limit in `ReviewQueueResponse`, and have `nextOfflineCard` skip learning cards
  due later than `now + learn_ahead_limit`, returning the time the next one becomes available so
  the page can wait for it, as online review does.

### Leech actions for late offline reviews

A review inserted into a card's history (an offline rating placed before a later review of the
card) does not go through `rate_card`, so it never tags or suspends.

- **Use case.** Reviewing on two devices, one of them offline. The offline device's lapse is
  synced after the other device already reviewed the card, so it is inserted into the history,
  and the lapse that makes the card a leech is never acted on.
- **Example.** `lapses_threshold = 8`, `action = "suspend"`. A card has 7 lapses. An offline Again
  is synced late and inserted before a later online Good: the card now has 8 lapses, but it is not
  suspended or tagged `leech`, and `spares card leeches` lists it while review keeps showing it.
- **How.** After replaying the history, compare the lapse count before and after the insertion,
  and if it crossed the threshold (or a later multiple of half of it), apply `LeechConfig::action`
  as `rate_card` would. Undo of the inserted review should undo it too.

### Cap the counts by the limits

`cards_left_by_state` and the statistics' cards due today count every due card, not just the ones
today's new and review limits allow.

- **Use case.** With a backlog after a holiday, the review page says 600 reviews left while the
  daily limit is 200, so the count never reaches 0 and the session looks endless.
- **Example.** `reviews_daily_limit = 200`, 600 reviews due, 50 already done today: the count
  would show 150 instead of 550. With `new_cards_daily_limit = 20` and 5 new cards done, new shows
  15 instead of every new card in the collection.
- **How.** Apply the same `daily_limits_left` the queue uses: `min(due, left of limit)` per state,
  including the new cards held back by the review limit unless `new_cards_ignore_review_limit` is
  set. The uncapped count could still be shown alongside, like "150 (600 due)".

## Queue presets per tag

Anki's deck presets give each deck its own limits, steps and order. spares has one global
`config.toml`, but tags can play the role of decks.

- **Use case.** Vocabulary does well with many new cards a day and a random order, while a math
  course wants few new cards, in the order the notes were written, with longer steps. Today both
  share one set of settings, so one of them is always set wrong.
- **Example.**

  ```toml
  [queue.presets.languages]
  tags = ["lang"]               # notes with `lang` or a `lang:` child tag
  new_cards_daily_limit = 40
  review_order = "random"
  learning_steps = [60, 300, 1800]

  [queue.presets.exam]
  tags = ["math:analysis"]
  review_order = "retrievability"
  ```

  A note tagged `lang:german` gets 40 new cards a day and a random order; one tagged
  `math:analysis` uses the top-level limits with retrievability order; one tagged both is in
  `languages`, since it is listed first.

- **What a preset can set.** Any of the queue settings (orders, mix, learn ahead, sibling
  burying), the daily limits, learning and relearning steps, and possibly desired retention.
  FSRS parameters stay global, since they are fitted to the whole review log.
- **Which preset applies.** A card has a preset through its note's tags, and a note can have
  several tags. A simple rule: the first preset listed in `config.toml` whose tags match wins, so
  the order of the presets is the priority. Cards with no match use the top-level settings.
- **Limits.** As in Anki v3, a preset's limits cap that preset's cards, and the top-level limits
  still cap the total. With a top-level limit of 50 new cards and `languages` at 40, a day can
  have 40 language cards and 10 others, but not 40 and 20. `daily_limits_left` would count today's
  reviews per preset, by joining `review_log` to the cards' tags, or by recording the preset in
  `review_log.custom_data` at rating time so that retagging a note does not rewrite history.
- **Order.** `queue_order_by` currently takes one `QueueConfig`. With presets, each card's sort
  keys come from its preset (a `CASE` over the preset), or, simpler, the session's preset is the
  one matching the review query's tag, and mixed sessions use the top-level order.
- **Scheduling.** `rate_card` would look up the card's preset for its steps before calling the
  scheduler, and `spares card reschedule` would do the same per card.
- **When.** Wait until the queue settings have been used for a while, so the settings presets copy
  have settled.

### Interleave by tag

Round robin across a session's tags, so a review is not 40 calculus cards in a row, then 40
chemistry ones. Interleaving helps tell similar concepts apart.

- **Use case.** Studying for several courses at once with `review_order = "due"`, cards of one
  course often come in long runs because they were added the same week. Mixing them makes each
  card a little harder to answer, which is what makes interleaving work.
- **Example.** 6 `math` cards and 3 `chem` cards, with `interleave_by = "top_level_tag"`:

  ```text
  before: m1 m2 m3 m4 m5 m6 c1 c2 c3
  after:  m1 c1 m2 m3 c2 m4 m5 c3 m6
  ```

- **How.** A `[queue]` setting such as `interleave_by = "top_level_tag"` (`math:analysis` counts as
  `math`). In SQL this is the same trick as `NewReviewMix::Interleaved`: number each card within
  its group (`ROW_NUMBER() OVER (PARTITION BY <group> ORDER BY <review keys>)`), divide by the
  group size, and sort by that fraction. A note with several top-level tags would use its first.
- It should come after the review order within each group, so the order still decides which cards
  of a group come first.

### Prerequisite-aware order

Show a note's cards only after the notes it links to (`note_link`) are known, extending the
"older notes first" rule to the links spares already has.

- **Use case.** A note on the chain rule links to the note defining derivatives. With
  `new_order = "random"` the chain rule's cards can come first, before the definition was ever
  seen, so they are guessed at rather than learned.
- **Example.** Notes A (derivative) and B (chain rule, links to A), both new. With
  `new_after_linked_notes = true`, B's cards are held back until A's cards have all left learning;
  if A's card lapses later, B's reviews due that day wait until A's card is relearned.
- **New cards.** Delay a new card while a note it links to still has new cards, or cards in
  learning. This is a `NOT EXISTS` over `note_link` joined to the linked note's cards, added to the
  eligibility clause in `build_review_filter_query`, so it can be a `[queue]` setting
  (`new_after_linked_notes = true`).
- **Reviews.** When a linked note's card lapses, bring forward (or bury until it is relearned) the
  reviews of the notes that link to it, since they likely depend on it.
- **Risks.** Links are found by fuzzy keyword matching, so a wrong link could hold cards back for
  good. Only links above a score threshold should count, and cycles must not block both notes. A
  card held back for more than a few days should be let through anyway.
