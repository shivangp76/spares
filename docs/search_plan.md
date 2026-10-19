# Search plan

Work left to bring spares' search up to Anki's. What is done is documented on the code: the
syntax and fields in `spares_core::search` (`cargo doc --workspace --no-deps
--document-private-items --open`), and `spares note search --fields` and `--examples`.

Anki searches that spares can already do with another query are left out:

- `tag:none`: `-tag~re:"."`.
- Wildcards (`d*g`, `d_g`), whole words (`w:dog`) and case-insensitive regexes: `data~re:"..."`,
  with `(?i)` to ignore case.
- `introduced:7`: `c.rated[>=today-7d] -c.rated[<today-7d]`.
- `prop:pos`: spares keeps no position for new cards. `new_order` works from the note's
  `created_at`, so `sort_by_asc=created_at` is the same order.

Each improvement below says what is missing, when it matters (use case), a small example, and how
it could be built.

## Leeches by card

A `c.leech` field, true for the cards `get_leeches` lists: those with at least
`lapses_threshold` lapses.

- **Use case.** `tag=leech` finds leeches' notes, but the tag is on the note, so it matches every
  card of a note with one leech, and it is not there at all with `action = "none"`, or after you
  remove it while fixing the note. Finding the leech cards themselves, among other conditions,
  takes `spares card leeches` and matching ids by hand.
- **Example.** A note with 3 clozes, where only `c2` has 9 lapses. `tag=leech c.suspended=false`
  finds all 3 cards, while `c.leech -c.suspended` finds just `c2`. With `action = "none"`,
  `c.leech tag=spanish` finds the Spanish leeches, which no query finds today.
- **How.** A boolean field in the evaluator that compiles to `c.lapses`'s subquery (which counts
  rows matching `schedulers::lapse_sql`, as `get_leeches` does) compared with `>=` the
  `lapses_threshold` read from the config, the way `c.waiting_on_links` reads `QueueConfig`.
  `c.lapses>=8` already gives the same result; the field just saves knowing the threshold.

## Postponed cards

Find the cards whose due date was moved without a review: today, only `spares card postpone`. This
is Anki's `resched:n`.

- **Use case.** After postponing reviews for a holiday, checking which cards were moved, or
  bringing back the ones in one tag early, needs to know which ones were postponed.
- **Example.** `spares card postpone 50` moves 50 reviews on 2026-10-18. `c.postponed[today]`
  finds those 50, and `c.postponed[today] tag=exam` the exam cards among them.
- **How.** `move_cards` changes `due` and logs nothing, so this first needs a `ReviewLogKind`
  for a moved due date (`Reschedule`), like `Forget`: no rating, the old and new due date, and
  skipped by replay, statistics and `c.rated`. Then `c.postponed[<date>]` works like
  `c.rated[<date>]` against that kind. It also lets card info show the postponement, and could
  carry Anki's `Manual` rows that are dropped on import (see `adapters/impls/anki/database.rs`)
  when they can be told apart from forgets.
