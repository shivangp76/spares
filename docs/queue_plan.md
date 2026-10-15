# Card queue plan

Work left to bring spares' card queue up to Anki's, and beyond. What is done is documented on the
code: the settings on `QueueConfig`, `LeechConfig` and `SparesExternalConfig` in
`spares_core::config`, and the rest where it is implemented (`cargo doc --workspace --no-deps
--document-private-items --open`).

## Gaps in what is done

- **Log cram reviews.** A rating in a filtered tag with `reschedule = false` writes no
  `review_log` row, so it is missing from statistics and the heatmap (see
  `rate_card_without_rescheduling`). Logging it needs a new `ReviewLogKind`, a rebuild of
  `review_log` for its `CHECK`, and every replay of reviews taught to skip that kind.
- **Learn ahead limit offline.** `nextOfflineCard` could apply the limit itself, given it in
  `ReviewQueueResponse`.
- **Leech actions for late offline reviews.** A review inserted into a card's history does not go
  through `rate_card`, so it never tags or suspends.
- **Cap the counts by the limits.** `cards_left_by_state` and the statistics' cards due today
  should count only the cards today's new and review limits allow.
- **Review limit in review snapshots.** A query with `limit` only leaves out review cards once the
  daily limit is used up; it could also cap them at what is left of it.
- **Per-tag exit rule for filtered tags.** Cards still leave a filtered tag after 2 Goods or 1 Easy
  (`Fsrs::filtered_tag_schedule`); a rule per tag would be a column like `reschedule`.

## Queue presets per tag

Anki's deck presets give each deck its own limits, steps and order. spares has one global
`config.toml`, but tags can play the role of decks.

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

- **What a preset can set.** Any of the queue settings (orders, mix, learn ahead, sibling
  burying), the daily limits, learning and relearning steps, and possibly desired retention.
  FSRS parameters stay global, since they are fitted to the whole review log.
- **Which preset applies.** A card has a preset through its note's tags, and a note can have
  several tags. A simple rule: the first preset listed in `config.toml` whose tags match wins, so
  the order of the presets is the priority. Cards with no match use the top-level settings.
- **Limits.** As in Anki v3, a preset's limits cap that preset's cards, and the top-level limits
  still cap the total. `daily_limits_left` would count today's reviews per preset, by joining
  `review_log` to the cards' tags, or by recording the preset in `review_log.custom_data` at
  rating time so that retagging a note does not rewrite history.
- **Order.** `queue_order_by` currently takes one `QueueConfig`. With presets, each card's sort
  keys come from its preset (a `CASE` over the preset), or, simpler, the session's preset is the
  one matching the review query's tag, and mixed sessions use the top-level order.
- **Scheduling.** `rate_card` would look up the card's preset for its steps before calling the
  scheduler, and `spares card reschedule` would do the same per card.
- Wait until the queue settings have been used for a while, so the settings presets copy have
  settled.

## Further ideas

### Interleave by tag

Round robin across a session's tags, so a review is not 40 calculus cards in a row, then 40
chemistry ones. Interleaving helps tell similar concepts apart.

- A `[queue]` setting such as `interleave_by = "top_level_tag"` (`math:analysis` counts as `math`).
- In SQL this is the same trick as `NewReviewMix::Interleaved`: number each card within its
  group (`ROW_NUMBER() OVER (PARTITION BY <group> ORDER BY <review keys>)`), divide by the group
  size, and sort by that fraction. A note with several top-level tags would use its first.
- It should come after the review order within each group, so the order still decides which
  cards of a group come first.

### Prerequisite-aware order

Show a note's cards only after the notes it links to (`note_link`) are known, extending the
"older notes first" rule to the links spares already has.

- For new cards: delay a new card while a note it links to still has new cards, or cards in
  learning. This is a `NOT EXISTS` over `note_link` joined to the linked note's cards, added to the
  eligibility clause in `build_review_filter_query`, so it can be a `[queue]` setting
  (`new_after_linked_notes = true`).
- For reviews: when a linked note's card lapses, bring forward (or bury until it is relearned)
  the reviews of the notes that link to it, since they likely depend on it.
- Links are found by fuzzy keyword matching, so a wrong link could hold cards back for good.
  Only links above a score threshold should count, and cycles must not block both notes.
