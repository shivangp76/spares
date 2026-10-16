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

## Prerequisite-aware order

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
