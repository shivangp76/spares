# Design of sync and note ids

How to set up and use two-way sync is in [Getting Started](./getting_started.md#two-way-sync) and [Deploying a sync server](./deploying.md). This page explains why it works the way it does, for anyone changing it. The code is in `spares_core/src/ids/` and `spares_core/src/sync/`.

## Goal

Review and edit notes from the phone anywhere, even when the laptop is off. The laptop and a small server (reached over Tailscale) both hold the whole database and sync like Anki and AnkiWeb. Notes can be edited on the laptop (CLI, note files, live notes) or on the web.

**Unsynced laptop edits must never be silently overwritten by later phone edits.** Laptop edits often sit unsynced for a while.

## Decisions

1. **Stay on SQLite.**
   - A central Postgres would need the network for everything.
   - Two-way Postgres replication (pgEdge Spock) uses last-writer-wins and long snowflake ids, and it would mean porting all the SQLite-specific SQL (`strftime`, `VACUUM INTO`, JSON, sqlx types).
   - Turso's offline sync is last-push-wins per row by default.
   - Merging note text is application logic whatever the database.
2. **What we take from Anki:** a per-row `usn` with the server's counter as the "since" cursor, tombstones (graves), review-log union, clock, schema and count checks, and a full upload or download as the escape hatch.
3. **What we don't take from Anki: last-writer-wins.** Rows changed on both sides get a three-way merge against the last synced version. A real conflict is stored and flagged, never discarded.
4. **An edit beats a delete**, the opposite of Anki.
5. **Live notes are read-only on the web**, because their source file is on the laptop. Their tags and keywords stay editable.
6. **A replica's identity comes from the environment, never the database.** A full download replaces the database, and the laptop must not take on the server's identity. There are two settings, both required with no default (tests default to replica `0`, role `client`):
   - `SPARES_REPLICA`, `0`–`7`: which id form the replica writes and its residue for the other tables (below). `0` is the laptop, `1` the server.
   - `SPARES_SYNC_ROLE`, `server` or `client`: whether this instance answers `/api/sync/*` or runs the client side, including the merge.

   Keeping them apart means a new replica only takes a new number, and code never infers the role from the number.

## Note ids

### Requirements

1. **Unique without coordination**, so an offline laptop can still create notes.
2. **Short.** Laptop-created notes get 3 letters for roughly the next 6,000 notes (4–7 years at the current rate), then 4 letters for decades. Notes created on the web get one more letter. No id is longer than 8 letters. No scheme can keep 3 letters much longer: there are only 17,576 of them, and the 4,117 notes that existed at the migration take a quarter.
3. **Letters only.** macOS Finder sorts runs of digits as numbers, so mixed letters and digits would sort wrongly, and the phone needs no number keyboard.
4. **Lowercase only**, which is safe on case-insensitive file systems. Input is accepted in any case, because phone keyboards capitalize the first letter.
5. **Notes imported together sort next to each other**, even when the other replica creates notes at the same time, so related notes can be browsed with the arrow keys.
6. **Ids follow creation order across devices.** A note created after a sync sorts after everything either side created before that sync. Notes created on both sides while out of sync are ordered by position rather than time.
7. **Never reused**, even after a delete or an undo.
8. **Derived from the data, not the clock**, so clock changes and full syncs can't break it.

### Format

```
laptop:  <block><slot>          e.g. block "gi" + slot "r"        → "gir"
server:  <block>z<tail>         e.g. block "gi" + "z" + tail "a"  → "giza"
```

- **Blocks** are consecutive letter runs: `aa`, `ab`, …, `az`, `ba`, …. Each block has 25 laptop slots, `a`–`y`. **`z` is reserved for the server**, so the server's notes in a block sort after the laptop's.
- **The first letter decides the block's length**, so blocks grow instead of running out:

  | First letter | Block | Laptop id | Server id | Laptop slots |
  |---|---|---|---|---|
  | `a`–`s` | 2 letters | 3 | 4 | 12,350 |
  | `t`–`x` | 3 letters | 4 | 5 | 84,500 |
  | `y` | 4 letters | 5 | 6 | 439,400 |
  | `z` | 5 letters | 6 | 7 | 11.4 million |

- **The tail** (server only) is 1 letter (`a`–`y`) for the server's first 25 notes in a block, then `z` + 2 letters for the next 650.
- **No id is the start of another.** No block is the start of another block, and after its block a laptop id has `a`–`y` where a server id has `z`. So sorting the strings sorts by block, then slot, then tail, in both byte order and Finder's order (`localizedStandardCompare`, checked on names like `giy.typ`, `giza.typ`, `gizzaa.typ`, `gja.typ`, `taaa.typ`).
- Creation time stays in `created_at`.

### Allocation

Each replica keeps a cursor, the last note id it handed out (see "The high-water mark"). **M** is the larger of the cursor and the largest note id in the table: the newest note this replica has created or seen.

- **The laptop takes the next free slots after M:** `gir`, `gis`, …, `giy`, `gja`, ….
  - If M is a server note (`giza`), the next slot is the start of the next block (`gja`).
  - **A call that doesn't fit in what's left of the current block starts at the next block**, then fills as many blocks as it needs. An import of 60 notes takes `gja`–`gjy`, `gka`–`gky`, `gla`–`glj`, all 3 letters.
- **The server takes the next tail ids after M.** Its first note goes after the newest note it has seen (`giq` → `giza`), later ones follow (`gizb`, …) until it sees a newer laptop note. The server never writes into a block it hasn't seen a note in.

Why this meets the requirements:
- **Unique:** each side writes only its own form, and its cursor never goes back.
- **Imports stay together:** the only foreign ids that can sort inside a laptop import are server tails of the blocks it fills. The server writes only into the tail of the newest block it has seen. An import that crosses a block boundary starts in a fresh block, so the server sees none of its blocks until it has all of them, and then writes after the last one. A server call is one run of tail ids.
- **Creation order:** both sides always allocate above M, and after a sync M covers everything the other side created before it.

**Cost.** Skipped slots are lost for good: the rest of a block when a call doesn't fit (about 15% of slots on the real history), and the rest of the laptop's block each time it resumes after seeing a web-created note (about 12 slots each). Replaying the 2025–2026 creation history at 1,000–1,500 notes a year with up to 25 web-created notes a year, laptop ids stay 3 letters for 4–7 more years, then 4 letters for about 45–75 years. At 100 web-created notes a year, 3 letters last 2.5–3.5 years.

`allocate_note_ids(conn, n)` gets all of a call's notes at once, so an import gets one run of ids. Undo and redo replay the ids stored in the event and don't allocate. Allocation returns an error rather than produce a 9th letter, which only a server creating hundreds of notes in one block without seeing a newer laptop note could reach.

### The high-water mark

"Largest id in the table + 1" hands out a deleted note's id again once the newest notes are deleted. Two unrelated notes would then share one file path in the notes repository's history, "create, undo, create, redo" would fail on the primary key, and sync would carry a grave and a new note with the same id.

So `id_counter(replica, tbl, last)` stores, per replica and table, the largest id that replica ever handed out. Deletes never lower it. It's keyed by replica because full syncs copy the whole database, including the other side's rows. The next id starts from `max(last, largest in the table)`, which counts ids that arrived from the other replica.

- **Locking.** The allocation's first statement is a no-op write on its `id_counter` row, so the transaction takes SQLite's write lock before it reads the table's maximum. With sqlx's default deferred transaction, reading first would let two concurrent creates read the same maximum.
- **A full sync never lowers the receiving replica's own counter.** `id_counter` rows are merged by taking the larger value. Otherwise a full download could make the laptop hand out again an id it allocated and deleted before the sync.

### Storage

Ids stay `INTEGER PRIMARY KEY`. The letters are stored as a base-27 number left-aligned to 8 places: a letter is its position + 1 (`a` = 1 … `z` = 26), and an empty place is 0. So `gir` is 7·27⁷ + 9·27⁶ + 18·27⁵.

- Comparing the integers compares the strings, whatever their lengths, and the database schema and JSON wire format keep integers.
- The packing doesn't depend on blocks or tails, so the allocation rules can change without touching stored ids, and decoding (frontend, nvim) needs no knowledge of them.
- The largest value, `zzzzzzzz`, is 27⁸ − 1 (39 bits), exact as a JS or Lua number. The frontend must decode with division and `Math.floor`, since JS `>>` works on 32 bits.
- The smallest id, `a`, is 27⁷, far above the old numeric ids, so a note id below 27⁷ can only be unmigrated. No valid id packs to `0`, so there are no sentinel ids.
- `NoteId` is a newtype; other ids stay `i64`. Any letter string parses, whether or not the allocator could produce it. Digits are rejected, with a hint to look the old id up in `id-map.tsv`.

### Other tables

Cards, review logs, tags and parsers have no human-facing requirement, but still need ids unique across replicas. New rows take the smallest id above `max(high-water marks, MAX(id))` that is congruent to `SPARES_REPLICA` mod 8 (`next_ids`).

- The modulus is fixed at 8, not the current replica count, so adding a replica never changes the rule for existing ones. Wasting 7/8 of the numbers costs nothing, since these ids are never shown.
- Existing rows kept their ids when note ids were migrated, so review logs, the phone's offline queue and undo payloads still refer to the right cards.
- `note_tag`, `card_tag`, `note_link` and `note_keyword` keep SQLite's own row ids. They are identified by their parent row and contents.

### A third replica

Not implemented: allocation refuses replicas other than `0` and `1`. A second laptop (`SPARES_REPLICA=2`, `SPARES_SYNC_ROLE=client`) would take `y` as its tail letter in blocks opened after it joins, with the laptop keeping `a`–`x`. Add it while all replicas are synced. Nothing already allocated changes, because the packing doesn't depend on these rules.

### Alternatives considered

- **Batches of 26** (the scheme this replaced): every existing note and every new single note was 4 letters, imports over 26 notes (half of the notes created since 2025) were 5, every call took a whole batch even for one note, and parity reserved half the batches for a server that rarely creates notes.
- **Keeping the laptop in its block after it sees a web note** (`giq`, then phone `giza`, then laptop `gir` rather than `gja`). It would remove the ~12 slots lost per web note (a year more of 3-letter ids at 25 web notes a year), but the web note would sort after laptop notes created later, breaking requirement 6.
- **No vowels (base 21)**, so ids never spell words, including rude ones. It makes ids longer. If some ids turn out annoying, the allocator can skip a denylist of slots at any time; a skipped slot costs capacity, never correctness.

### The migration

`spares migrate ids` renumbered the existing notes once, before a second replica existed, giving them consecutive laptop slots in creation order (`aaa` to `giq`). It rewrote every reference (cards, tags, keywords, links, `inh:` references, tag queries), emptied the undo history, regenerated note and card files (with `git mv` in the notes repository, so `git log --follow` works), rewrote the `spares sync` hash index, cleared render caches and wrote `id-map.tsv`. The server refuses to start on an unmigrated database. The Anki collection keeps the old numeric ids, so re-importing from Anki would add those notes as new ones.

## Change tracking

Migration `20261014130000_sync`:

- **`usn`** on `parser`, `tag`, `note`, `card` and `review_log`, defaulting to `-1` (changed and not yet synced), so new rows are dirty without a trigger.
- **`grave(oid, kind, usn)`**: ids deleted here and not synced yet. Without it, "deleted over there" looks the same as "never synced", and the deleted row would come back. Soft deletes would mean `deleted_at IS NULL` in every query. Inserting a row under an id with a grave (undo of a delete) removes the grave.
- **`sync_base(kind, oid, snapshot)`**: a row's last synced version, saved on its first change or delete after a sync, for the three-way merge. For a note: its data, custom data, parser, tag ids and non-embedded keywords; for a card: its fields and tag ids. The views `sync_note_snapshot` and `sync_card_snapshot` build the same JSON for current rows.
- **`sync_conflict`**: unresolved conflicts. **`sync_client`** (server only): each client's last sync, to catch a client restored from a backup.
- **Triggers maintain all of this**, so no write path can be missed:
  - after an update that changed a synced column (not `updated_at` alone, which review snapshot tags rely on), snapshot the old row if it isn't dirty yet and set `usn = -1`;
  - before inserts and deletes on `note_tag`, `card_tag` and non-embedded `note_keyword` rows, snapshot and dirty the parent. When the parent or the tag is being deleted, the cascade runs after that row is gone, so the triggers skip it: deleting a tag doesn't mark every note that had it;
  - before a delete, snapshot the row with its tags; after it, insert a grave. Keeping the base of deleted cards lets the merge know which card each order in a note's synced text named.
- **`sync_applying`** holds one row while sync writes the other side's changes, which turns every trigger off. A trigger can't read a TEMP table, so it is a regular table whose row is never committed.
- `note_link` and embedded keywords are derived from the text, so they aren't tracked or sent; they are rebuilt wherever a note is applied. `event`, `offline_action` and `app_state` aren't synced. `render_asset` and `render_package` rows are sent in full on every push, since assets are only added on the laptop.

## Merge rules

The client merges (`sync/client.rs`) and pushes the result, so the server never re-parses notes and card ids are the same on both sides. *Base* means the `sync_base` row, or the local row if it isn't dirty. A dirty row with no base was already pushed in a sync that didn't finish, so the remote version is the base and the local version wins.

| Situation | Result |
|---|---|
| Changed on one side only | That side's version, whatever its timestamp. |
| Note text changed on both sides | Line-based three-way merge (`diffy`) with card orders replaced by card ids (below). If that conflicts, the plain texts are tried too. |
| Same lines edited on both sides, or a merged text that no longer parses | Keep the local text. Store base, local and remote in `sync_conflict` and tag the note `sync-conflict`. |
| Note tags, non-embedded keywords, card tags | Set merge: additions and removals from either side applied. |
| Note and card `custom_data` | Key by key. A key both sides changed: local for notes, the newer change for cards. |
| Note `parser_id`, `created_at` | Three-way; local wins a conflict. |
| review_log | Union. A log keeps its card and tag where either side still has them. |
| Card scheduling (`due`, `stability`, `difficulty`, `state`) | As a whole from the side that changed it. Both changed: the side with the newer review, else the newer change. If both reviewed, the memory state is replayed from the merged logs. |
| Card `special_state`, `desired_retention` | Three-way; the newer change wins a conflict. |
| Card `order`, `back_type`, `note_id` | Follow the note text. |
| Note deleted on one side, edited on the other | The edit wins: the note is kept (its cards and reviews are sent back), tagged `sync-conflict`, with a `sync_conflict` row. A review doesn't count as an edit. |
| Note deleted on one side, not edited on the other | Deleted. |
| Card deleted on one side | Deleted, even if the other side reviewed it (its reviews stay, unlinked). A card's existence follows its note's text. |
| Tag or parser deleted on one side, changed on the other | Kept, and sent back. A parser still used by notes is kept too. |
| Tag and parser fields | Three-way per field; the newer change wins a tag conflict. |
| Both sides have a tag with the same name | The local tag is folded into the server's (notes, cards, review logs, filtered-tag scheduler data) and deleted. |
| Both sides have a parser with the same name | Sync stops with a message. |

**Card ids in the text merge.** A cloze names its cards by position (`o:1,2`), and adding a cloze renumbers every later one, so two edits that each add a cloze would conflict on every cloze line. Before merging, each side's orders are replaced with the ids of the cards they name on that side (the base's from `sync_base`). After the merge, `reconcile_cards` gives each cloze the card its merged text names, else the card with its cloze uid, else the card in its place; cards nothing names are deleted, clozes without a card get new ones, and the text is written back with sequential orders. `spares sync conflicts` shows diffs the same way (`o:c40`), so a cloze that only moved reads the same.

## Protocol

There is no chunking, since there are only two replicas.

1. **`meta`**. The client refuses to sync if the clocks differ by more than 5 minutes, the schema versions differ, both sides have the same replica number, or the roles aren't one `client` and one `server`. It requires a full sync (412, with the reason) if it has never synced, `needs_full_sync` is set, the server's epoch changed (another full upload replaced it), its last usn is ahead of the server's (server restored from a backup), or behind what `sync_client` recorded for it (client restored from a backup).
2. **`pull {since}`**. The server stamps everything changed on it since its last stamp (phone edits included) with `usn + 1`, and returns rows with `usn > since`, plus every card of each changed note.
3. The client merges in one local transaction, then **`push`**es everything dirty. The server answers 409 if anything changed on it since the pull (a phone edit while the client merged); the client rolls back and starts over, up to 3 times. Otherwise the server applies the changes in one transaction (foreign keys deferred), stamps them, records the client in `sync_client`, and returns the new usn and row counts.
4. The client stamps its dirty rows, clears its graves and bases, compares counts and commits. If the counts differ, it rolls back and sets `needs_full_sync`.

**Crash safety:** if the client fails after the server committed a push, the next pull returns the client's own changes, which match its still-dirty rows and merge to themselves.

**Live notes:** incoming notes' `live_block_order` is cleared before notes are written, so two notes swapping block orders in one sync don't trip the unique index.

### Full sync

The escape hatch, and the first sync. The incoming database (a `VACUUM INTO` snapshot) is copied table by table from an `ATTACH`ed file in one transaction, rather than swapping the file, so the server can do it with the database open. Its integrity and schema version are checked first. Afterwards every row has `usn = 0`, and graves, bases and undo history are cleared.

- **Upload:** the client marks everything synced and sets `needs_full_sync` until the upload succeeds. Edits made during the upload stay dirty, with what was uploaded as their base. The server gets a new epoch, so any other client must full sync too.
- **Download:** the client continues from the `sync_usn` and epoch in the server's snapshot. Rows that were dirty on the server are stamped and sent again on the next pull, where they merge to themselves.

## After a sync

- **Files** (`sync/files.rs`), on whichever side the changes landed: changed notes' files are regenerated, deleted notes' files removed, and card files past a note's last card deleted. With the notes repository, regenerated files are committed to the server copy (`notes-spares`), so unpushed edits in the clone in `notes/` are never overwritten; they meet the sync's commit when the clone is rebased.
- **Change number:** the nvim plugin refreshes its keyword cache when `spares event latest` changes, but sync writes create no events. So that number is the newest note event id plus `app_state.notes_version_offset`, which every sync that changes notes moves on. A full sync, which clears `event`, adds more than the newest event id, so the number never repeats.
- **Undo:** `/api/undo` refuses events at or below `last_sync_event_id`, as Anki clears undo on sync. The server sets it when it answers a pull or push, the client when it commits.

## Known limitations and open work

- **The rsync of rendered cards can overwrite the server's.** `spares sync cloud` copies `cards/` of parsers rendered on the server (not Typst) after the sync. A note edited on the phone between the sync and the copy has its newly rendered cards replaced by the laptop's older ones.
- **Without the notes repository, a note file edited during a sync can be overwritten** when the sync changes that note. `spares sync cloud` warns about edits not imported yet.
- **Conflicts can only be resolved on the laptop**, where the merge runs. The phone shows the `sync-conflict` tag, but its server has no conflict rows.
- **A full sync doesn't say how many changes each side would lose** before asking which direction.
- **No test checks that every synced row gets its id from `allocate_note_ids` or `next_ids`.**
- **Untested:** the end-to-end steps from a phone browser. The rest was run with two local servers (a second `spares_server` on `:8090` with `SPARES_REPLICA=1`, `SPARES_SYNC_ROLE=server`, its own data and cache directories, and every `*_OUT_DIR` unset, or real rendered files get overwritten).

## Sources

- Anki's sync: <https://github.com/ankitects/anki/tree/main/rslib/src/sync> (`normal.rs`, `chunks.rs`, `graves.rs`, `meta.rs`); <https://docs.ankiweb.net/syncing.html>, <https://docs.ankiweb.net/sync-server.html>
- pgEdge Spock: <https://github.com/pgEdge/spock>
- Turso offline writes: <https://turso.tech/blog/introducing-offline-writes-for-turso>
