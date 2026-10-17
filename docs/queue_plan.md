# Card queue plan

Work left to bring spares' card queue up to Anki's, and beyond. What is done is documented on the
code: the settings on `QueueConfig`, `QueuePreset`, `LeechConfig` and `SparesExternalConfig` in
`spares_core::config`, and the rest where it is implemented (`cargo doc --workspace --no-deps
--document-private-items --open`).

Each improvement below says what is missing, when it matters (use case), a small example, and how
it could be built.

## Interleave by tag

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

## Held reviews in the room left for new cards

Leave the reviews that `reviews_after_linked_notes` holds back out of the reviews due that
`daily_limits_left_with` counts against `reviews_daily_limit`.

- **Use case.** Unless `new_cards_ignore_review_limit` is set, new cards only fill the room the
  reviews due today leave under the review limit. Held reviews are not shown today, so the room
  they take is lost.
- **Example.** `reviews_daily_limit = 100`, 95 reviews due, 10 of them held while a linked note
  relearns. Only 5 new cards are allowed, though 85 reviews are shown and 15 could be.
- **How.** Add `linked_notes_clause` to the due reviews query in `daily_limits_left_with`. The
  difference is small, since a review is only held while a card it links to relearns that day.

## Lapses by the scheduler's pass rating

Count a lapse as any failed review of a card in the Review state, using `SrsScheduler::is_pass`,
rather than FSRS's `Again` rating specifically.

- **Use case.** Statistics already decide pass and fail with `is_pass`, so true retention and
  answer buttons work for any scheduler. Leeches don't: under a scheduler whose lowest rating is
  not FSRS's id 1, a card's lapse count stays at 0 and it never becomes a leech.
- **Example.** A scheduler with ratings Fail and Pass, where Fail has id 0. Failing a review card
  8 times never counts a lapse, so the leech tag is never added and the card is never suspended.
- **How.** `get_leeches` and `lapses` in `schedulers/fsrs/mod.rs` bind
  `rating_to_number(FsrsRating::Again)`. Bind the ratings `is_pass` rejects instead
  (`rating IN (SELECT value FROM json_each(?))`), which gives the same result for FSRS, and move
  both into default methods on `SrsScheduler` so other schedulers get them.
