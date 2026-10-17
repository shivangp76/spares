# Prerequisite-aware order plan

How to build "Prerequisite-aware order" from `queue_plan.md`: hold back a note's cards until the
older notes it links to (`note_link`) are known.

## What is there already

- **`note_link`** has a row for each link: `parent_note_id` (B, the note that links),
  `linked_note_id` (A, the note it links to, or `NULL` if nothing matched) and `score`.
- **Matching** (`api/note/keyword_distance.rs`, `MATCHER_VERSION = 2`) aligns keywords word by
  word, so a link can no longer swap out a whole word (`Ziemer 6.20` → `Joshi 6.20`,
  `Spectral Theorem` → `Spectral Norm`). A keyword that does not match leaves `linked_note_id`
  `NULL`. The scores are distances:

  | Score     | Match                                         |
  |-----------|-----------------------------------------------|
  | 0.0       | Equal ignoring case                           |
  | 0.5       | Punctuation, accents or a possessive          |
  | 1.0       | A plural, a stop word, or words run together  |
  | 1.5, 2.5  | One or two typos in a long word               |
  | 2.5 and up | Words that only share a long stem            |

- **Links are matched when a render runs with `include_linked_notes`.** Links of a new note stay
  unmatched until then. A link whose matched keyword was renamed or removed is rematched on the
  next such render, and so is every link after a matcher change.
- **`note_link` is not synced**, and sync renders run with `include_linked_notes: false`
  (`sync/files.rs:137`). Each replica (laptop, VPS) holds cards back by its own links.
- **`build_review_filter_query`** (`api/review.rs:692`) builds the `where_clause` that the counts,
  `get_review_card`, the offline queue (`api/offline_review.rs:125`) and, through
  `QueueCaps::showable`, the interleaving caps all use. `review_eligibility_clause`
  (`api/review.rs:549`) repeats it for snapshot tags and must match it.
- `where_clause` is rebuilt on every `get_review_card`, so B's cards show up on the next fetch
  once A's card graduates, during the same session.

## Settings

New fields on `QueueConfig` (`spares_core/src/config.rs`):

```toml
[queue]
new_after_linked_notes = false      # new cards wait until the linked notes are learned
reviews_after_linked_notes = false  # reviews wait while a linked note's card is relearning
linked_note_max_score = 1.5         # only links scoring at most this count
```

- The default of `1.5` counts every match up to one typo, and leaves out matches on a shared stem
  only (`isometrically isomorphic` against `Isometric isomorphism`, 4.5), which are the most
  likely to be links to a related note rather than to the note itself.
- They are global, not per preset, like the other settings that shape a session. A preset version
  would need a `CASE` over `preset_sql` and can come later.
- Each field gets rustdoc with the chain rule example, like `bury_new_siblings`, plus defaults in
  `Default` and cases in the parse tests next to `documented_example_parses_in_order`.

## The clause

`pub(crate) fn linked_notes_clause(config: &QueueConfig, card_due_limit, card_id_scope: &str) ->
String` in `api/queue.rs`. It returns `""` when both settings are off, and otherwise:

```sql
AND NOT EXISTS (
  SELECT 1 FROM note_link nl
  JOIN note ln ON ln.id = nl.linked_note_id
  JOIN card lc ON lc.note_id = ln.id
  WHERE nl.parent_note_id = c.note_id
    AND nl.score <= {linked_note_max_score}
    AND (ln.created_at, ln.id) < (n.created_at, n.id)       -- only older notes hold back
    AND (lc.special_state IS NULL
         OR lc.special_state IN ({scheduler_buried}, {buried_until_later_today}))
    AND (   (c.state = {NEW}    AND lc.state IN ({NEW}, {LEARNING}))           -- new_after_…
         OR (c.state = {REVIEW} AND lc.state = {RELEARNING}
                                AND lc.due <= {card_due_limit}))               -- reviews_after_…
    {AND lc.id IN (<card ids of the query>)}                -- Query filters only
)
```

Only the half of the `OR` whose setting is on is included. Relearning cards of B are never held,
since that would break their steps.

Since these guarantees keep a card from being held back for good, there is no cap on how long it
waits:

| Risk                                   | Guarantee                                              |
|----------------------------------------|--------------------------------------------------------|
| A wrong link                           | Only links within `linked_note_max_score` count. Unmatched links have no `linked_note_id`, so the join drops them. |
| Cycles (A ↔ B, longer loops, self-links) | Only an older note holds back a newer one. That is a strict order, so it has no cycles, and it extends the "older notes first" rule to links. A link from an older note to a newer one holds nothing back. |
| A suspended or user buried, never learned | Only cards with no special state, or buried until later today or tomorrow, hold back. |
| A outside the query, never shown       | For `Query` filters, a card holding back must match the query too (`card_id_query_str`). |
| Reviews waiting for good               | Reviews only wait for a relearning card due today, so they are let through by the next day. |
| Filtered tags                          | Not applied, like the learn ahead limit: a filtered tag shows every card in it. |

## Wiring

- `build_review_filter_query`: append the clause to the unfiltered `where_clause`, and to
  `held_back_clause` so they agree.
- `review_eligibility_clause`: append it too, so snapshot tags match.
- `queue_order_by` is unchanged. This is a filter, not a sort key, so it works with every order
  and mix. The rustdoc says it matters most with `random`, `random_note`, `newest_first` and
  presets; with `new_order = "created"`, A already comes first.
- `note_link(parent_note_id)` is indexed, and `card(note_id)` is used by the joins. Check the plan
  with `EXPLAIN QUERY PLAN` on a real collection.

## Documented, not fixed

- The new count can go up during a session, as A graduates and lets B's cards through.
- Offline review only sees B's cards with the next queue download.
- A new note holds nothing back until a render with linked notes matches its links. A link to a
  renamed keyword keeps pointing to its old note until the next such render.
- On the VPS, holds follow the VPS's own `note_link`, which only changes when the VPS renders with
  linked notes. Review on the phone can differ from the laptop until links are regenerated there.
- `daily_limits_left_with` still counts a held review as due when working out the room left for
  new cards. Fixing this needs the clause in the limits query; the difference is small.

## Visibility (separate commit)

A search field, such as `c.waiting_on_links`, in `search/evaluator.rs` next to
`c.scheduler_buried`, built from the same clause, so `spares card list` can show why a card is not
coming up.

## Tests

In the `api/queue.rs` tests, with `create_notes` and `note_link` rows inserted directly:

1. A new card of B is held while A is new or learning, and shown once A is in review.
2. A link scoring above `linked_note_max_score`, or with no `linked_note_id`, holds nothing.
3. With links A → B and B → A, only the newer note is held.
4. A suspended A holds nothing back. A sibling-buried A does, until it is unburied.
5. With a query that leaves out A, B is not held.
6. A review of B is held while A is relearning and due today, not when A is due tomorrow.
7. The counts of `get_cards_left_by_state_and_time_estimate` match what the queue shows.
8. With both settings off, the `where_clause` is the same as before.

## Order of work

1. Settings with rustdoc.
2. `linked_notes_clause` and its wiring.
3. Tests.
4. The search field.
5. Remove "Prerequisite-aware order" from `queue_plan.md` and this file once implemented.
