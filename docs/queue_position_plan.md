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
  limit (of the top level or of a preset), buried, or suspended. "Where is this card?" is often
  "why is it missing?"

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
   - Run `ordered_queue` with `learn_ahead = true`, the order live review shows cards in. Then run
     it with `learn_ahead = false` to find the held-back learning cards: those in the second result
     and not in the first.
   - Evaluate the match query with output type `Cards`, so card-level filters such as `c.state=0`
     work, and intersect it with the filter's cards in SQL (`card_id IN (...)` over the review's
     `where_clause`). This avoids pulling every matching card id when the match is broad.
   - For matching cards that are not in the queue, work out the reason from the card (suspended,
     buried, `due` after the cutoff) or from the cap that was full when the card's turn came. The
     cap loop already knows that last reason, so have it record skipped card ids as it goes.
   - Do not render anything.
3. **Filters with `limit`.** A query with `limit` is snapshotted to a filtered tag once per day. Use
   today's snapshot if there is one. Otherwise return an error saying to start the review first,
   rather than creating the snapshot as a side effect of looking.
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
   }
   ```

5. **Tests** in `api/queue.rs`:
   - Positions agree with the index of each card in `get_review_queue`.
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

## Order of work

1. Core refactor (`ordered_queue`), with `get_review_queue`'s tests still passing.
2. `queue_positions`, its schema and tests.
3. Endpoint.
4. CLI command.
5. Web app panel.

Each step is its own commit.
