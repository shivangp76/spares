# Card queue plan

Features that bring spares' card queue up to Anki's, and beyond. Settings are documented on the
structs in `spares_core::config` (`cargo doc -p spares_core --open`), so this file only tracks
what is done and what is left.

## Done

| # | Feature | Where it is set | Notes |
|---|---|---|---|
| 1 | Review order | `[queue]` `review_order`, `new_order`, `new_review_mix` (`QueueConfig`) | Learning cards whose step is over always come first. New cards are now interleaved with reviews by default, rather than sorted with them by due date. |
| 2 | Daily review limit | `reviews_daily_limit`, `new_cards_ignore_review_limit` | Default 9999 (no limit). As in Anki, new cards only fill the room the reviews due today leave under the limit. |
| 3 | Learn ahead limit | `[queue]` `learn_ahead_limit` (default 20 min) | `POST /api/review/next-available` says when the next held-back learning card can be shown; the CLI and the web app wait for it. |
| 4 | Sibling burying | `[queue]` `bury_new_siblings`, `bury_review_siblings` | Off by default. Part of the rating's undo event. Not done in filtered tags or for reviews of earlier days synced late. |
| 5 | Leech actions | `[leech]` `action` (`tag`, `suspend`, `none`), `tag_name` | Default `tag`, so leeches' notes now get a `leech` tag. Repeats every half threshold. The tag joins the rating's undo group. |
| 6 | Query sort sets the review order | `sort_by_asc` / `sort_by_desc` in a review query or a filtered tag's query | Overrides the `[queue]` orders for new and review cards. |
| 7 | Filtered tags without rescheduling | `reschedule` column on tags (`spares tag add/edit --reschedule false`, web tags page) | Only this part of step 7 was done. |

### Known gaps in what is done

- **Cram reviews are not logged.** A rating in a tag with `reschedule = false` writes no
  `review_log` row, so it is missing from statistics and the heatmap. Logging it needs a new
  `ReviewLogKind`, and `review_log`'s `CHECK ((kind = 0) = (rating IS NOT NULL))` means rebuilding
  the table. Every reader that replays reviews would then have to skip that kind.
- **Offline review ignores the learn ahead limit.** The downloaded queue includes learning cards
  whenever they are due, and the web client shows them as soon as nothing else is left, as before.
  `nextOfflineCard` could apply the limit itself, given it in `ReviewQueueResponse`.
- **Late offline reviews skip leech actions.** A review inserted into a card's history (because a
  later review of the card already synced) does not go through `rate_card`, so it never tags or
  suspends.
- **Counts are not capped by the limits.** `cards_left_by_state` and the statistics' cards due
  today count every due card, not just those today's new and review limits allow.
- **Review snapshots only partly respect the review limit.** A query with `limit` leaves out
  review cards once the daily limit is used up, but otherwise only its own `limit`s apply.
- **The rest of step 7.** Filtered tags still leave after 2 Goods or 1 Easy
  (`Fsrs::filtered_tag_schedule`); a per-tag exit rule would be a column like `reschedule`.

## Step 8: queue presets per tag

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
- Wait until steps 1–3 have been used for a while, so the settings presets copy have settled.

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
