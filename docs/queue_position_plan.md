# Queue position plan

Show where the cards matching a second query sit in a review's queue, from the CLI and the web
app. For example: in the review of `tag="probability:grad:2" or
linked_from_note=(tag="probability:grad:2")`, at which positions are the cards of notes matching
`data~re:"key.Uni"`?

Today this takes calling `/api/review/queue` and `/api/notes/search` with `curl` and lining the two
up with `jq`. That route also renders every card in the queue, though all it needs is ids.

## What it shows

For a review filter (a query, or a filtered tag by name or id) and a match query:

- How many cards the review would show today, and where each matching card is among them, from 1.
  For each matching card, show its note, card order, state and due date.
- Learning and relearning cards held back by `learn_ahead_limit`, listed after the others with the
  time they can be shown. They have no fixed position, since review shows them once nothing else is
  left and their time has come.
- Matching cards the review would not show today, with the reason: not due, over the new or review
  limit (of the top level or of a preset), waiting on a linked note (`new_after_linked_notes`,
  `reviews_after_linked_notes`), buried, or suspended. "Where is this card?" is often "why is it
  missing?"
- How many matching cards are not in the review at all, so a typo in the filter does not look the
  same as "nothing matches".

A filtered tag has no limits and no learn ahead limit, so for it nothing is held back and the only
reasons are buried and suspended.

Leaving out the match query lists the whole queue, which is also a preview of a session.

A position holds only until the next card is rated. Rating brings learning cards back, changes the
caps the new/review mix spreads over, and may bury siblings. Both clients should say so in one
line, not leave it to a footnote.

## Core (`spares_core`)

1. **Share the ordered queue.** Move the part of `get_review_queue` that runs the ordered query and
   skips cards over the limits into a function returning the kept rows (`ReviewCard`, `due`,
   preset), e.g. `ordered_queue(db, filter, days_ahead, learn_ahead, now)`. `get_review_queue`
   then renders those rows as it does today. This keeps a single copy of the ordering and cap
   logic.
2. **`queue_positions`** in `api/queue.rs` (or `offline_review.rs`, next to what it reuses):
   - Run `ordered_queue` with `learn_ahead = true`, the order live review shows cards in. The
     held-back learning cards are those of `ReviewFilterQuery::held_back_clause`, shown from
     `due - learn_ahead_limit`, as in `get_review_queue` and `next_review_card_available_at`.
   - Evaluate the match query with output type `Cards`, so card-level filters such as `c.state=0`
     work, and intersect it with the filter's cards in SQL (`card_id IN (...)` over the review's
     `where_clause`). This avoids pulling every matching card id when the match is broad.
   - For matching cards that are not in the queue, work out the reason from the card (suspended,
     buried, `due` after the cutoff), from `waiting_on_links`, or from the cap that was full when
     the card's turn came. The cap loop already knows that last reason, so have it record skipped
     card ids as it goes. A cap that is already 0 never reaches the loop, since
     `daily_limits_clause` drops its cards in the `WHERE`, so those are checked against the caps
     at the start.
   - Do not render anything.
3. **Filters with `limit`.** A query with `limit` is snapshotted to a filtered tag once per day. Use
   today's snapshot if there is one, found by `review_snapshot_tag_name`, with the "built today"
   check of `create_review_snapshot_tag` moved to a helper both use. Otherwise return an error
   saying to start the review first, rather than creating the snapshot as a side effect of
   looking.
4. **Schema** (`schema::review`):

   ```rust
   pub struct QueuePositionRequest {
       pub filter: Option<GetReviewCardFilterRequest>,
       /// Cards to locate. `None` lists the whole queue.
       pub match_query: Option<String>,
   }

   pub struct QueuePositionResponse {
       /// Cards the review would show now.
       pub total: u32,
       pub positions: Vec<QueuePosition>, // position from 1, card_id, note_id, card_order, card_state, due
       pub held_back: Vec<HeldBackCard>,  // card fields + shown_from
       pub not_shown: Vec<NotShownCard>,  // card fields + reason
       /// Matching cards that are not in the review's filter at all.
       pub not_in_review: u32,
   }
   ```

5. **Tests** in `api/queue.rs`:
   - Positions agree with the index of each card in `get_review_queue`.
   - The card at position 1 is the one `get_review_card` returns, with a preset limit and the
     interleaved mix, since live review applies caps in the `WHERE` and the queue in its loop.
   - A card waiting on a linked note is `not_shown` with that reason.
   - Matching cards outside the filter are counted in `not_in_review`.
   - A card over the new limit is reported as `not_shown` with that reason.
   - A learning card beyond `learn_ahead_limit` is in `held_back`.
   - A match query that is a card filter (`c.state=0`) locates only the cards it matches.
   - A query with `limit` and no snapshot returns an error and creates no tag.

Document the behaviour in rustdoc on `queue_positions` and the request struct, not on a separate
page.

## Server (`spares_server`)

`POST /api/review/queue/position` in `review_routes`, behind the API key like the rest. It unburies
cards when the day has rolled over, as opening Review does, so its positions are the ones review
would use.

## CLI (`spares_cli`)

`spares review` takes `ReviewArgs` directly, so a subcommand under it would clash with
`spares review -q ...`. Add a top-level command instead:

```text
spares queue [-q QUERY | -t TAG_NAME | --tag-id ID] [-m MATCH] [--limit N] [--json]
```

- It flattens the same `FilterArgs` as `spares review`, so a filter can be reused by changing only
  the subcommand.
- Output is a table: position, note id, card order, state, due, and the note's first line (from the
  search response's text, cut to the terminal width). After the table come the held-back and
  not-shown sections, then the one-line caveat.
- `--limit` caps the rows when there is no `--match` (default 50).
- `--json` prints the response as is, for scripts.
- Example:

  ```text
  $ spares queue -q 'tag="probability:grad:2" or linked_from_note=(tag="probability:grad:2")' -m 'data~re:"key.Uni"'
  147 cards in this review. 14 match:
    pos  note         card  state     due
      3  64598779314     3  learning  2026-04-30
      4  64598779314     4  learning  2026-04-30
    ...
  Positions change as you rate cards.
  ```

- The command lives in `spares_cli/src/queue.rs`. It is wired up in `args.rs` and `main.rs`, and
  the shell completions pick it up from clap.

## Web app (`spares_frontend/frontend`)

On `ReviewPage.tsx`'s landing view, below the filter box:

- A **Find in queue** `SearchInput` for the match query, with the same search help and highlighting
  of query errors as the filter box. It is prefilled from and written back to `?match=` so a lookup
  can be bookmarked.
- Results in a compact list, styled like the Cards page rows:
  - Each row shows the position, note, card order, state and due date.
  - Clicking a note opens it in the existing side panel (`NoteDetail`).
  - The held-back and not-shown cards are listed after it, in muted sections. Each not-shown card
    gives its reason.
- With the match box empty, a **Preview queue** button lists the first 50 cards.
- It refreshes when the filter changes, using the same debounce as the filter box, and on returning
  to the landing view after a session.
- Offline it shows "Needs the server". The cached offline queue holds at most 200 cards, and the
  client cannot evaluate a match query.
- Types go in the API types file next to `ReviewQueueResponse`, and the call goes in the API client
  next to the other review calls.

During a session, a "Find in queue" entry in `CardTools` could open the same panel for the current
filter. Leave it for later unless the landing view turns out to be too far from where it is
wanted.

Test it with headless Playwright against a scratch server: two notes, one matching. Check that the
positions in the list match those `spares queue` prints for the same filter.

## Search in queue order

Ordering search results by queue position is not a sort key of the query language
(`sort_by_asc=queue_position`): a position depends on today's counts, the cap loop and learn ahead,
which are not SQL; it needs a review to be a position in; a query's sort already orders the review
of that query, so the key would refer to itself; and search returns notes. The endpoint already is
"these results in queue order", since the match query is the search:

- On the Cards page, an "Order: queue position" option sends the search as `match_query` with no
  filter (the default review) and shows the cards in that order, with the held-back and not-shown
  cards after them and their reasons. It reuses the rows of the Review panel.
- In the CLI, `spares queue -m QUERY` without a filter does the same.

## Order of work

1. Core refactor (`ordered_queue`), with `get_review_queue`'s tests still passing.
2. `queue_positions`, its schema and tests.
3. Endpoint.
4. CLI command.
5. Web app panel.
6. Queue order on the Cards page.

Each step is its own commit.
