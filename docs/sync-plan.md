# Two-way sync for spares: design plan

Status: in progress. Section 4 (ids, including `spares migrate ids` and `next_ids` for the other tables) is implemented. Last updated 2026-10-14. Problems found in a review on 2026-10-03 (checked against the code and the real DB, read-only) are listed under **Open issues** at the end of the section they affect.

## 1. Goal and context

**Problem.** `spares_serve` (in `~/.config/spares/spares.zsh`) runs `spares_server` on the laptop at `0.0.0.0:8080`. The phone can only reach it on the same Wi-Fi.

**Goal.** Review and edit notes from the phone anywhere, even when the laptop is off.

**Hosting (to be bought later).**
- A cheap VPS (Hetzner CX22, about €4/mo).
- Reached privately over Tailscale (free plan), using `tailscale serve` for HTTPS. No public port is opened.
- Until then, build everything so that setting up the VPS only means pointing a URL at it.

**Requirements.**
- **Two-way sync, like Anki/AnkiWeb.** Notes can be edited on the laptop (CLI, `notes/` files, live notes) or on the web (phone).
- **Unsynced laptop edits must never be silently overwritten by later phone edits.** Laptop edits often sit unsynced for a while.
- **Note IDs are short (3 letters for the next several years, then 4), typeable, grouped by import batch and in creation order across devices** (see section 4).

### Open issues
- **SSH:** "No public port is opened" isn't true while SSH (used by the `remote_host` rsync) listens on the public IP. Firewall it, or use Tailscale SSH.
- **Hetzner:** the CX22's name and price may have changed. Check when buying.

## 2. Current state of the codebase (relevant facts)

### IDs
- Every table uses a local `INTEGER PRIMARY KEY` with no AUTOINCREMENT, so new IDs are `max+1`. Two replicas would hand out the same IDs.
- `NoteId`/`CardId` are `pub type … = i64` (`spares_core/src/model.rs:14-15`). `NoteId` appears in about 200 places in Rust and about 56 in the frontend.

### File names
- Note IDs appear in file names, zero-padded to 4 digits: `0001.typ`, `0001-1-front.typ` (`spares_core/src/parsers/mod.rs:416`, `get_output_filename`).
- Card files use `<note_id>-<card_order>`, so **card IDs never appear in file names**.
- Cloze tags embed the note ID through `cloze_tag_str(note_id, card_order)`.
- There are currently about 4,117 notes.

### Change tracking
- There's no per-row change tracking and no tombstones.
- `event` is an undo log, not a full change log. Not logged: `log=false` paths, cards deleted implicitly when their cloze is removed, tag rebuilds, `note_link` rescoring, `app_state`, `render_asset`.
- `review_log` rows are referenced from events only by ID.

### Source of truth
- **The DB (`note.data`) is the source of truth.** Creating or updating a note regenerates its files under `<data_dir>/notes/<parser>/` and `<data_dir>/cards/<parser>/` (`api/note/create.rs`, `api/note/update/mod.rs`). This happens wherever the write runs, including the server.
- **`spares sync`** (`spares_cli/src/sync/mod.rs`, `interactive.rs`) diffs the editable `notes/` mirror against the DB version and imports changes (files → DB), with a `git diff` confirmation step.
- **Live notes** have a second source of truth: your own source files on the laptop (`spares_cli/src/import.rs`, keyed by `(live_sync_name, block_order)`).

### Other facts
- **Cards come from parsing note text.** Changing a note's data creates and deletes cards.
- **The web UI already edits.** It creates, updates and deletes notes, tags, parsers and cards, submits reviews, and calls undo (`spares_frontend/frontend/src/api/client.ts`).
- **The current `spares sync cloud`** (`spares_cli/src/sync/cloud.rs`) is push-only:
  1. takes a `VACUUM INTO` snapshot and rsyncs it;
  2. runs `sqlite3 .restore` over ssh, which replaces the remote DB;
  3. rsyncs the data dir, excluding `notes/` and the `cards/` of browser-rendered parsers.
  - Any edits or reviews made on the remote are lost on the next push.
- **The CLI talks to the local server over HTTP** (`base_url` in `spares_cli/src/main.rs`).
- **Auth:** the API uses a bearer key (`require_api_key`), but `/files` is outside that layer. This is fine behind Tailscale.

## 3. Decisions made

1. **Stay on SQLite.**
   - A central Postgres would need the network for everything.
   - Two-way Postgres replication (pgEdge Spock) uses last-writer-wins and long snowflake IDs, and it would mean porting all the SQLite-specific SQL (`strftime`, `VACUUM INTO`, JSON, sqlx types).
   - Turso's offline sync is last-push-wins per row by default.
   - **Merging note text is application logic whatever the database.**
2. **What we take from Anki:**
   - a per-row `usn` with the server's counter as the "since" cursor;
   - tombstones (graves);
   - review-log union;
   - clock, schema and count checks;
   - full upload or download as the escape hatch.
3. **What we don't take from Anki: last-writer-wins.** Use a three-way merge against the last synced version instead. A real conflict is stored and flagged, never discarded.
4. **An edit beats a delete**, the opposite of Anki.
5. **Live notes are read-only on the web**, because their source file is on the laptop. Tags and flags stay editable.
6. **Note IDs are slots handed out in order from 2-letter blocks. The laptop fills slots `a`–`y` of each block, and the server writes its notes after them, under `z`** (section 4).

## 4. ID scheme

### Requirements
1. **Unique without coordination.** Each replica creates IDs on its own, so an offline laptop can still create notes.
2. **Short.** Laptop-created notes get 3 letters, whatever the import size, for roughly the next 6,000 notes (4–7 years at the current rate), then 4 letters for decades. Notes created on the web get one more letter. No ID is longer than 8 letters.
   - No scheme can keep 3 letters much longer: there are only 17,576 of them, and the 4,117 existing notes take a quarter.
3. **Letters only:**
   - macOS Finder sorts runs of digits as numbers, so mixed letters and digits would sort wrongly;
   - no switching to the number keyboard on the phone.
4. **Lowercase only**, which is safe on case-insensitive file systems. Input is accepted in any case, because phone keyboards capitalize the first letter.
5. **Notes imported together sort next to each other**, so you can browse related notes with the arrow keys. This holds even when the other replica creates notes at the same time.
6. **IDs follow creation order across devices.** A note created after a sync sorts after everything either side created before that sync. The only exception is notes created on both sides while out of sync, which are ordered by position rather than by time.
7. **Never reused**, even after a delete or an undo.
8. **Derived from the data, not the clock**, so clock changes and full syncs can't break it.
9. **Two replicas: the laptop (primary) and the server (secondary).** A third replica can be added later without changing existing IDs (see "A third replica").

### Format
```
laptop:  <block><slot>          e.g. block "gi" + slot "r"        → "gir"
server:  <block>z<tail>         e.g. block "gi" + "z" + tail "a"  → "giza"
```
- **Blocks** are consecutive letter pairs: `aa`, `ab`, …, `az`, `ba`, …. Each block has 25 laptop slots, `a`–`y`. **`z` is reserved for the server**, so the server's notes in a block sort after the laptop's.
- **The first letter decides the block's length**, so blocks grow instead of running out:

  | First letter | Block | Laptop ID | Server ID | Laptop slots |
  |---|---|---|---|---|
  | `a`–`s` | 2 letters | 3 | 4 | 12,350 (4,117 taken by the migration) |
  | `t`–`x` | 3 letters | 4 | 5 | 84,500 |
  | `y` | 4 letters | 5 | 6 | 439,400 |
  | `z` | 5 letters | 6 | 7 | 11.4 million |

- **The tail** (server only) is 1 letter (`a`–`y`) for the server's first 25 notes in a block, then `z` + 2 letters for the next 650.
- **No ID is the start of another.** No block is the start of another block, and after its block a laptop ID has `a`–`y` where a server ID has `z`. So sorting the strings sorts by block, then slot, then tail. Checked with `localizedStandardCompare` (Finder's order) and byte order on names like `giy.typ`, `giza.typ`, `gizzaa.typ`, `gja.typ`, `taaa.typ` and their card files.
- A replica's identity comes from **config/env, never the DB**. A full download replaces the DB file, and the laptop must not take on the server's identity. There are two separate settings. Both are required with no default, and the server refuses to start without them. Tests default to replica `0`, role `client`.
  - **`SPARES_REPLICA`**: the replica number, `0`–`7` (one per residue mod 8, see "Other tables"). `0` is the laptop and `1` is the server. A later replica takes the next unused number. The number decides which ID form the replica writes (see "Allocation" and "A third replica") and its residue for the other tables (see "Other tables").
  - **`SPARES_SYNC_ROLE`**: `server` or `client`. It decides whether this instance answers `/api/sync/*` or runs the client side, including the merge.
  - Keeping the two apart means adding a replica only takes a new number, and code never has to infer the role from the number.
- Creation time stays in `created_at`.

### Allocation
Each replica keeps a cursor: the last note ID it handed out (see "The high-water mark"). **M** is the larger of the cursor and the largest note ID in the table, i.e. the newest note this replica has created or seen.
- **The laptop takes the next free slots after M:** `gir`, `gis`, …, `giy`, `gja`, ….
  - If M is a server note (`giza`), the next slot is the start of the next block (`gja`).
  - **A call that doesn't fit in what's left of the current block starts at the next block**, then fills as many blocks as it needs. An import of 60 notes takes `gja`–`gjy`, `gka`–`gky`, `gla`–`glj`: all 3 letters.
- **The server takes the next tail IDs after M.** Its first note goes after the newest note it has seen (`giq` → `giza`). Later ones follow (`gizb`, …) until it sees a newer laptop note. The server never writes into a block it hasn't seen a note in.
- **Unique (requirement 1):** each side writes only its own form, and its cursor never goes back.
- **Imports stay together (requirement 5):** the only foreign IDs that can sort inside a laptop import are server tails of the blocks it fills. The server writes only into the tail of the newest block it has seen. An import that crosses a block boundary starts in a fresh block, so the server sees none of its blocks until it has all of them, and then it writes after the last one. A server call is one run of tail IDs, and nothing else can write between them.
- **Creation order (requirement 6):** both sides always allocate above M, and after a sync M covers everything the other side created before it.
- **Cost:** skipped slots are lost for good:
  - the rest of a block when a call doesn't fit (about 15% of slots on the real history);
  - the rest of the laptop's block each time it resumes after seeing a web-created note (about 12 slots each).
- **Expected lengths.** Simulated by replaying the 2025–2026 creation history at 1,000–1,500 notes a year with up to 25 web-created notes a year: laptop IDs stay 3 letters for 4–7 more years, then 4 letters for about 45–75 years. At 100 web-created notes a year, 3 letters last 2.5–3.5 years.
- **Compared with the batch scheme** this replaces:
  - every existing note and every new single note was 4 letters there, and imports over 26 notes (half of the notes created since 2025) were 5;
  - every call took a whole 26-note batch, even for one note, and parity reserved half the batches for a server that rarely creates notes.
- **A third replica** (for example a second laptop, with `SPARES_REPLICA=2` and `SPARES_SYNC_ROLE=client`) would take `y` as its tail letter in blocks opened after it joins, with the laptop keeping `a`–`x`. Add it at a point where all replicas are synced. Nothing already allocated changes, because the packing (below) doesn't depend on these rules.

### The high-water mark
- **The problem.** "Largest ID in the table + 1" hands out a deleted note's ID again once the newest notes are deleted. The real notes repo shows this: its last commit deleted notes 4207–4214, and the largest remaining ID is 4206. The results:
  - two unrelated notes share one file path in the notes repo;
  - "create, undo, create, redo" fails on the primary key;
  - sync would carry a grave and a new note with the same ID.
- **The counter table.** A new table `id_counter(replica INTEGER, tbl TEXT, last INTEGER NOT NULL, PRIMARY KEY (replica, tbl))` stores, per replica and table, the largest ID that replica ever handed out. Deletes never lower it.
  - It's keyed by replica because full syncs copy the whole DB, including the other side's rows.
- **The next ID** starts from `max(last, largest in the table)`. Taking the max with the table counts IDs that arrived from the other replica.
  - Notes: that's M in "Allocation".
  - Other tables: the smallest number above it that is congruent to the replica number mod 8 (see "Other tables").
- **Locking.** The allocation's first statement is a no-op write (`UPDATE id_counter SET last = last WHERE replica = ? AND tbl = ?`), so the transaction takes SQLite's write lock before it reads the table's maximum.
  - The slot arithmetic for notes is too involved for SQL, so it runs in Rust, and the new `last` is written in the same transaction.
  - With sqlx's default deferred transaction, reading first would let two concurrent creates read the same maximum, and one would fail with `SQLITE_BUSY`.
- **A full sync never lowers the receiving replica's own counter.** Read its `id_counter` rows before replacing the DB, and keep the larger value of each afterwards. Otherwise a full download could make the laptop hand out again an ID it allocated and deleted before the sync, which may already be in the notes repo's history.

### Storage: a packed i64 behind a newtype
- Don't switch to TEXT keys. Keep `INTEGER PRIMARY KEY` and store the letters as a base-27 number, left-aligned to 8 places: a letter is its position + 1 (`a` = 1 … `z` = 26), and an empty place is 0. `gir` is 7·27⁷ + 9·27⁶ + 18·27⁵ = 76,967,537,148.
- This keeps any two IDs in string order, whatever their lengths. Sorting the integers gives the same order as sorting the strings.
- The packing doesn't depend on blocks or tails, so the allocation rules can change later without touching stored IDs. Decoding (frontend, nvim) needs no knowledge of the allocation rules either.
- The largest value, `zzzzzzzz`, is 27⁸ − 1 ≈ 2.8·10¹¹ (39 bits), so it's exact as a JS number.
- **Old and new IDs can't be confused.** The smallest packed ID (`a`) is 27⁷ = 10,460,353,203, far above the old numeric IDs (at most 4,206). The migration can renumber in one pass, and any note ID below 27⁷ is unmigrated. No valid ID packs to `0`.
- **Make `NoteId` a newtype.**
  - Replace `pub type NoteId = i64` with `struct NoteId(i64)`, with `#[serde(transparent)]`, `#[sqlx(transparent)]`, `Copy`, `Ord` and `Hash`.
  - The JSON/API keeps the integer, so the DB schema and wire format don't change.
  - Only `NoteId` becomes a newtype. `CardId` and the others stay `i64`.
- **Don't implement `Display` at first.**
  - With `Display`, every `format!("{note_id}")` would still compile and silently switch to letters, including any that build SQL.
  - Start with an explicit `.letters()` and fix every compile error. Then add `Display`.
- **Parsing.** `FromStr` accepts 1–8 letters in any case and lowercases them. It rejects digits, with a hint to look the old ID up in `id-map.tsv` (see the migration).
  - Any letter string parses, whether or not the allocator could produce it. Looking up one that was never allocated just finds no note.
- **Allocation stops at 8 letters.** Past that it returns an error instead of producing an ID that can't be packed. Laptop IDs never get there (the tiers top out at 6 letters). Server IDs only would if the server created hundreds of notes in one block while never seeing a newer laptop note.
- **No sentinel IDs.** `0` and `-1` no longer decode to an ID, so a stray sentinel can't name a real note. Still, `NoteId` shouldn't carry them. Two places become `Option`:
  - `view/mod.rs:269` (`unwrap_or(0)`);
  - the Anki adapter's `-1` (`anki/database.rs:389`).
- **Frontend.** Add a `formatNoteId` to `utils.ts`. It must use division and `Math.floor`, because JS `>>` works on 32 bits and would corrupt 39-bit IDs.

### Where note IDs are shown or parsed
All of these use the letter form:
- **File names:**
  - `get_output_filename` (drop `{:0>4}`);
  - `parse_note_rel_path` (`notes_vcs/paths.rs`), whose all-digits check becomes all-letters;
  - rendered file names parsed in `api/browser_render.rs`;
  - the LaTeX aux dir (`parsers/impls/latex/mod.rs`, `{:0>4}`), which the vimtex config has to match.
- **Cloze tags:** `cloze_tag_str`.
- **Inside note text:** the `note-id:` setting (`parsers/settings.rs`) and `inh:NOTE_ID/ORDER` (`parsers/clozes/card_settings.rs`).
- **CLI args:** `spares_cli/src/args.rs`. Server path params stay integers.
- **Search** (`search/evaluator.rs`):
  - `id`, `linked_to_note` and `linked_from_note` accept letter IDs and reject numbers with an error, so `id>100` can't silently change meaning;
  - comparisons between letter IDs still work, because the order is preserved;
  - `c.id` stays numeric.
- **Web UI:** `NoteDetail.tsx`, `NotesPage.tsx`, `ReviewPage.tsx`, `NewNoteForm.tsx`.
- **nvim plugin** (`nvim/lua/spares.lua`, `%04d`): port the base-27 encoder and decoder to Lua. Lua numbers are doubles, which are exact for 39-bit IDs.

### Allocating note IDs
- Add `spares_core::ids::allocate_note_ids(conn, n) -> Vec<NoteId>`, which follows "Allocation" for this replica's role. Give it all of a call's notes at once, so an import gets one run of IDs. Use it at every fresh note insert:
  - `create_notes` (`api/note/create.rs`). Importers reach it too (the Anki adapter through the HTTP API).
  - The `INSERT INTO note` statements in `model.rs`, `schedulers/fsrs/reposition.rs`, `api/scheduler.rs`, `api/undo/invert_payload.rs`, `api/tag/mod.rs` and the undo e2e tests are test fixtures only. Switch them to the helper so tests produce real IDs.
- `create_notes_event` (same file) replays undo/redo with the IDs stored in the event, so it keeps them and doesn't allocate.

### Other tables (card, review_log, tag, parser)
- These have no human-facing requirement, but they still need IDs that are unique across replicas.
- **Existing rows keep their IDs.**
  - New rows take the smallest ID above `max(high-water marks, MAX(id))` that is congruent to the replica number mod 8 (`SPARES_REPLICA`: laptop `≡ 0`, server `≡ 1`, a third replica `≡ 2`). The high-water marks are this replica's and any other replica's a full sync brought along, so IDs keep increasing even after the newest rows are deleted.
  - The modulus is fixed at 8 from the start, not at the current replica count, so adding a replica never changes the rule for the existing ones.
  - These IDs are never shown, so wasting 7/8 of the numbers costs nothing. At a few thousand rows a year, i64 doesn't come close to running out.
  - New IDs are always above the old ones, so they can't collide.
  - Not re-keying keeps the card IDs that `review_log`, the phone's offline queue and undo payloads refer to.
  - All of this goes in one shared helper, `next_ids(conn, table, n)` (`spares_core/src/ids/mod.rs`). It is a single `INSERT … ON CONFLICT … RETURNING` on `id_counter`, so it is atomic without a surrounding transaction.
- Call sites that allocate new IDs (checked against the code on 2026-10-13):
  - card: `create_cards_on` (`api/note/create.rs`), also reached from `api/note/update/cards.rs`;
  - review_log: `rate_card` (`api/review.rs`), `copy_review_logs_on` (`api/note/create.rs`), `forget_card` (`api/card.rs`);
  - parser: `create_parser` (`api/parser.rs`);
  - tag: `create_tag_row` (`api/tag/mod.rs`).
- **Replays don't allocate.**
  - Undo/redo replays reuse the IDs stored in the event: `create_notes_event` (cards), `update_notes_event` (`api/note/update/event.rs`), `create_parser_event`, `create_tag_event`.
  - **`restore_review_log` (`api/undo/invert_payload.rs`) is the exception today:** it takes a new ID when a review is redone. Change it to reuse the original. The high-water mark guarantees that ID was never handed out again.
- `note_tag`, `card_tag`, `note_link` and `note_keyword` keep SQLite's own row IDs. They are identified by their parent row and their contents.
- Add a test that fails if any synced table gets a row without an ID from the helpers.

### Migrating existing data (`spares migrate ids [--dry-run]`, once, before a second replica exists)
**Preconditions.** The command stops unless:
- the server is stopped (it takes `BEGIN EXCLUSIVE` and the notes lock);
- the jj clone in `notes/` is clean and matches `notes-spares`'s `main`;
- `spares sync` has nothing to import;
- the DB isn't already migrated. If it is, the command does nothing.

Open the web app online beforehand, so the phone sends its offline queue.

**Numbering.**
1. Give notes consecutive laptop slots in current ID order (which is creation order): `aaa`, `aab`, …, `aay`, `aba`, ….
   - Every existing note gets a 3-letter ID, from `aaa` to `giq`, in today's order.
   - No grouping by import is needed: with one replica, nothing can land between notes of one import.
   - Slot `z` stays free in every block, so the server's tail rule works in every block.
2. After the migration:
   - the laptop's counter is `giq`, so new laptop notes continue at `gir`;
   - the server's first note goes in the tail of the newest block it has seen (`giza` if nothing new was created before the first full upload).

**Dry run** (on a copy of the DB). It prints:
- the first and last new IDs, and a sample of the old → new mapping;
- any `inh:` references or `id:` tag queries it would rewrite;
- the number of files to rename and regenerate;
- how long re-rendering takes.

**Real run.**
1. **Back up:**
   - `VACUUM INTO` a copy of the DB;
   - record the `notes-spares` HEAD;
   - write `id-map.tsv` (old number → new letters), for reading old commit messages and anything else that mentions old IDs.
2. **Rewrite the DB in one transaction**, with `foreign_keys = OFF` set before it starts:
   - Renumber `note.id` in one pass. Every new ID is at least 27⁷, above every old one, so they can't collide.
   - Rewrite `card.note_id`, `note_tag.note_id`, `note_keyword.note_id`, `note_link.parent_note_id` and `note_link.linked_note_id`.
   - Rewrite `inh:` references in `note.data` and note IDs in `tag.query`. The real DB has none, but the command handles them.
   - Empty `event`, which drops undo history.
   - Delete the `render_asset` rows for image-occlusion images, which are named by note ID.
   - Seed `id_counter` (the laptop's note counter is `giq`) and set `app_state.note_ids_migrated`.
     - The server refuses to start without this flag. As a backstop, it also refuses if any note ID is below 27⁷, which can only be an old numeric ID.
     - The new sqlx migration (`id_counter`) also stops an old binary from opening the migrated DB.
   - Before committing, check that:
     - row counts are unchanged;
     - `PRAGMA foreign_key_check` is empty;
     - every note ID decodes.
3. **Files.** The DB is the source of truth, so these are regenerated rather than edited.
   - **Notes repo:**
     - In `notes-spares`, `git mv` each `<parser>/NNNN.<ext>` to its new name and regenerate it (its `note-id:` line changes).
     - Make one commit, so git records renames and `git log --follow` keeps working.
     - Then `jj git fetch` and rebase in `notes/`.
     - Without the notes repo, regenerate `<data_dir>/notes/`.
   - **Cards:** delete and regenerate `cards/`. Card files embed the cloze tag and the image-occlusion image paths.
   - **Caches:**
     - rewrite the keys of `<cache_dir>/sync/hash_index.json`, which is keyed by file path; otherwise `spares sync` treats every file as new;
     - delete `<cache_dir>/note-hashes/`, `<cache_dir>/image_occlusions/` and the rendered outputs (`TYPST_OUT_DIR` and the other `*_OUT_DIR`s);
     - re-render everything. Rendered PDFs embed the cloze tag that the card open command searches for.
   - **Crashes:** progress is recorded in `app_state`, so rerunning the command after a crash finishes this step.
4. **Clear the phone's offline queues** (`clearOfflineQueues`), because the cached render paths changed.

**Rollback:**
- restore the DB backup;
- `git reset` `notes-spares` to the recorded HEAD;
- run the old binary.

**Not migrated:** the Anki collection (`collection.anki2`) keeps the old numeric spares IDs. They no longer parse, so re-importing from Anki would add those notes as new ones.

### Notes
- Random letter strings will sometimes spell real words, and 3-letter strings do so far more often than longer ones, including rude ones. Removing vowels (base 21) avoids that but makes IDs longer. We decided not to bother.
  - If some IDs turn out to be annoying, the allocator can skip a denylist of slots at any time. A skipped slot costs only capacity, never correctness.
- **Possible relaxation of requirement 6.** The laptop could keep filling its block after it sees a web-created note, instead of moving to the next block. The web note would then sort at the end of the block it was created in, after up to 24 laptop notes created later.
  - Example: the laptop has created notes up to `giq` and syncs. A note created on the phone gets `giza`. The laptop syncs again and creates 3 notes.
    - Current rule: they get `gja`, `gjb`, `gjc`. The order `giq` < `giza` < `gja` matches creation order, but slots `gir`–`giy` are never used.
    - Relaxed rule: they get `gir`, `gis`, `git`. The phone note now sorts after 3 notes created after it, and after up to 5 more as the laptop fills `giu`–`giy`.
  - The cost it removes: under the current rule, each time the laptop sees a new web note in its current block, it skips the rest of that block. That's 8 slots (`gir`–`giy`) in the example. The laptop is on average about halfway through a block's 25 slots, so this averages about 12 slots per web note (several web notes seen in one sync cost it only once).
  - Gain: that cost goes away. At 25 web notes a year that adds about a year of 3-letter IDs. At 100 a year it doubles them.
  - Not adopted, because it breaks requirement 6 as written.
- Live notes are matched by `(live_sync_name, block_order)`, not by ID, so the migration doesn't affect them.
- The real DB on 2026-10-13:
  - 4,117 notes, all Typst;
  - IDs increase with `created_at`;
  - 140 creation calls (distinct `created_at`). Since 2025: about 1,000 notes and 78 calls a year (about 1,500 notes a year in 2026). 9 of the 135 calls since 2025 had more than 26 notes, and they hold half of those notes;
  - no `inh:` references and no tag queries;
  - 45 events.

## 5. Schema changes (migration `2026100xxxxx_sync.up.sql`)
- **Change tracking:** add `usn INTEGER NOT NULL DEFAULT -1` to `parser`, `tag`, `note`, `card` and `review_log` (`-1` = changed and not yet synced).
- **`grave(oid INTEGER, kind INTEGER, usn INTEGER, PRIMARY KEY(oid, kind))`:** IDs deleted on this side and not yet synced. Triggers fill it, so app code never touches it.
  - **Why it's needed:** the sync only exchanges changed rows, and a deleted row no longer exists. Without a grave, "deleted over there" looks the same as "never synced yet", and the deleted row would come back.
  - Alternatives were rejected. Soft deletes would mean `deleted_at IS NULL` in every query. Diffing all IDs still needs the same "was it ever synced" information.
- **`sync_base(kind, oid, snapshot JSON, PRIMARY KEY(kind, oid))`:** the last synced version of a row. It holds:
  - for a note: its data, tags, keywords and links;
  - for a card: its fields and tags.
- **`sync_conflict(id, note_id, local JSON, remote JSON, created_at)`:** unresolved conflicts.
- **Triggers,** so no write path can be missed:
  - `AFTER UPDATE` on parser, tag, note and card when `NEW.usn = OLD.usn`. The sync code itself writes an explicit `usn`, which skips the trigger. Two steps:
    1. If `OLD.usn != -1` (first edit since the sync), snapshot the old row into `sync_base`.
    2. Set `usn = -1` and `updated_at = now`.
  - `AFTER INSERT`/`DELETE` on `note_tag`, `note_keyword`, `note_link` and `card_tag`: snapshot the base if needed, then mark the parent dirty.
  - `AFTER DELETE` on parser, tag, note and card: insert a grave.
- **`app_state` keys:**
  - `sync_usn`: on the server, its counter; on the client, the server's counter at the last sync.
  - `last_sync_at`, `last_sync_event_id`, `needs_full_sync`.

### Open issues: the triggers don't work as written
- **Child-table triggers snapshot the wrong version.** The base snapshot from the child-table triggers (`note_tag`, keywords, links) is taken in `AFTER INSERT/DELETE`, so it records the tags after the change, not the base. These need `BEFORE` triggers.
- **Sync can't turn the triggers off while it applies remote changes.**
  - The `NEW.usn = OLD.usn` trick only covers parent rows.
  - Child tables have no `usn`, so inserting remote tags marks the note dirty again and overwrites its `sync_base`.
  - Applying remote deletes creates local graves.
  - Fix: a sync-mode flag, for example a TEMP table checked in each trigger's `WHEN`.
- **Foreign-key cascades fire child triggers** (tested with `sqlite3`). Deleting one tag marks every note that has it as dirty.
- **`parser` and `review_log` have no `updated_at` column,** so the trigger as written fails on `parser`.

## 6. Sync engine: `spares_core/src/sync/` (pure DB logic)

- **`changes_since(usn)`** collects the graves, parsers, tags, notes (with tags, keywords and links), cards (with tags) and review_logs that changed since that usn or are dirty. Both pull and push use it.

### Open issues: what gets synced
- **`render_asset` and `render_package` stop reaching the server.**
  - Today they arrive because the whole DB is pushed.
  - rsync copies the asset files but not these table rows.
  - So Typst assets added after the first full upload would return 404 on the phone.

### Merge rules (`merge.rs`)
*Base* means the `sync_base` row, or the local row if the row isn't dirty.

| Situation | Result |
|---|---|
| Changed on one side only | That side's version, whatever its timestamp. This alone fixes the "old unsynced laptop edit" case when the two sides touched different things. |
| Note text changed on both sides | Line-based three-way merge (`diffy` crate). Non-overlapping hunks combine. |
| Same lines edited on both sides | Keep the local version live. Store the remote version in `sync_conflict` and tag the note `sync-conflict`. Nothing is discarded. No conflict markers go in the note, because they would break parsing and rendering. |
| Tags, keywords, links | Set merge: additions from either side kept, removals from either side applied |
| review_log | Union with `INSERT OR IGNORE`. Reviews are never dropped. |
| Card scheduling fields | From the side with the newer latest review |
| Other card fields (suspended, buried, due override) | Per-field three-way merge. If both sides changed the same field, the newer one wins. |
| Deleted on one side, edited on the other | The edit wins: keep the note and tag it `sync-conflict` |
| Deleted on one side, not edited on the other | Delete it |
| Parsers, tags (metadata) | Per-field three-way merge. Clashing unique names abort with a clear message. |

**Saving merged notes:** a merged note is saved through the normal `update_notes` path **on the client**, which re-parses it into cards. The resulting note and card rows are what get pushed. The server never re-parses on its own, which keeps card IDs the same on both sides.

#### Open issues
- **Derived data is synced as if it were user data.**
  - **Links and embedded keywords.** `note_link` is parsed from the note text (`get_linked_notes`, `api/note/update/links.rs`) and then resolved against keywords. Embedded keywords are also parsed from the text. Merging them as sets, separately from the text merge, can leave them out of step with the text. They should be rebuilt after the merge.
  - **Review snapshot tags** (`create_review_snapshot_tag`, `api/review.rs`) are a bigger problem.
    - Each replica creates its own `review-<hash>` tag with the same name and a different ID.
    - Each replica rebuilds `card_tag` and `card.custom_data` daily.
    - Under the rules above, sync aborts on the name clash the first time the same query is reviewed on both devices.
    - Fix: make these tags replica-local, or match tags by name.
  - **The `sync-conflict` tag** has the same name-clash problem when both sides create it.
- **How card IDs are kept during a merge isn't specified.**
  - If both sides added clozes, the client re-parses the merged text knowing only its local cards. It then creates its own card where the server already has one, possibly with reviews.
  - The result is duplicate cards on the server (with no grave), plus the foreign-key failure described under the protocol's open issues.
  - The client must apply remote cards first, then remove duplicates by `(note_id, order)` and re-point their review logs.
  - Handle both sides adding `c3` in different places.
  - A merged text that no longer parses should become a conflict, not fail the sync.
- **"Scheduling comes from the side with the newer review" isn't right.**
  - It ignores scheduling changes made without a review: manual `due`, reposition, reschedule, `desired_retention`.
  - If both sides reviewed offline, one side's effect on stability is lost. After merging the review logs, replay them with the existing `compute_memory_state` (`schedulers/mod.rs`) instead.
  - The field names in the table don't match the schema. The real fields are `special_state` (Suspended/UserBuried/SchedulerBuried), `due`, `desired_retention` and `custom_data`, and `due` falls into both of the table's card categories.
- **Rows missing from the merge table:**
  - `note.custom_data`;
  - `parser_id` changes.
  - It's also unclear whether a review counts as an "edit" for the rule that an edit beats a delete.

### Protocol (no chunking with two replicas; the client role runs on the laptop's local server)
1. `GET /api/sync/meta` → `{usn, migration_version, server_time, empty}`.
   - Refuse if the clocks differ by more than 5 minutes (the merge rules use `updated_at` to break ties) or the migration versions differ.
   - Decide between no-op, normal and full sync.
2. `POST /api/sync/pull {since}` → the server's changes since `since`: graves, parsers, tags, notes (with tags, keywords, links), cards (with tags), review_logs.
   - The client opens a local transaction, merges, and builds its push set: dirty rows, merged results, graves.
3. `POST /api/sync/push {expected_usn, changes}` → the server, in one transaction:
   - checks that its `usn` hasn't moved (if it has, return 409 and the client retries from step 2);
   - applies the changes as given (the client has already merged);
   - increments `usn` and returns `{new_usn, counts}`.
4. The client stamps the pushed rows with `new_usn`, clears its graves and the `sync_base` rows it pushed, compares counts, and commits.
   - If the counts don't match, it rolls back and sets `needs_full_sync`.

#### Open issues
- **Foreign-key errors will block sync for good.** `INSERT OR IGNORE` does not skip foreign-key failures (tested with `sqlite3`).
  - A phone review of a card the laptop deleted (cloze removed, or a live block removed) fails on every retry.
  - So does a `review_log.tag_id` pointing at a filtered tag deleted on the other side.
  - Fixes:
    - set `PRAGMA defer_foreign_keys`;
    - apply upserts from parent to child, and graves from child to parent;
    - null out dangling `card_id`/`tag_id`.
  - Never upsert with `INSERT OR REPLACE`: replacing a note cascade-deleted its cards (tested with `sqlite3`). Use `ON CONFLICT DO UPDATE`.
- **The 409 check doesn't catch phone edits made during a sync.**
  - The server's `sync_usn` only moves on push. A phone edit between the client's pull and push leaves it unchanged, so the push overwrites that edit without warning.
  - Nothing says who sets `usn` on the server's own dirty (`-1`) rows after a sync.
  - A phone review during a sync makes the counts differ, which sets `needs_full_sync`. Every full sync loses data on one side, so a count mismatch should retry instead.
  - The protocol should state that a re-sync after a crash is safe: identical changes on both sides are not a conflict.
- **Restoring the laptop from a backup, or a wrong `SPARES_REPLICA`, causes ID collisions between rows with different content.**
  - `SPARES_REPLICA` and `SPARES_SYNC_ROLE` are already required with no default (section 4).
  - `meta` should also exchange both sides' replica number and role. Sync is refused when both sides have the same `SPARES_REPLICA`, or when they aren't one `server` and one `client`.
  - The server should remember the last `usn` each replica synced at, and force a full sync when a client reports an older one.
- **Live notes:** the unique index on `(live_sync_name, live_block_order)` can fail partway through applying a push when a re-import has swapped block orders.

### Full sync (escape hatch, and the first sync)
- `GET /api/sync/full` streams a `VACUUM INTO` snapshot.
- `PUT /api/sync/full` accepts one: write to a temp file, run `PRAGMA integrity_check`, then restore with the backup API.
- Afterwards set every `usn = 0` and clear graves and bases.
- This reuses the snapshot logic from `cloud.rs` (`snapshot_database`) and replaces its `ssh` + `sqlite3` restore.

#### Open issues
- **A full download must regenerate every `notes/` and `cards/` file.** Otherwise `spares sync` treats the stale files as edits and reverts the server's changes.
- **Set `last_sync_event_id` after any full sync.**
- **Request size.** axum limits request bodies to 2 MB by default. `spares_server/src/route.rs` already raises the limit for image occlusions. Without a higher limit, both `PUT /api/sync/full` and a large push would be rejected.
- **Before a full sync, show how many changes each side would lose.**

## 7. After a sync
- **Files:** for each note changed or deleted by the sync, regenerate its `notes/` and `cards/` files with the existing path (`create_note_files_bulk` / `render_notes`, as `regenerate_notes` does), or move its files to the trash, as `api/note/delete.rs` does. This runs on both sides.
- **Undo:** record `last_sync_event_id`. `/api/undo` refuses events at or below it (Anki also clears undo on sync).

### Open issues
- **Regenerating files after a sync overwrites `notes/` edits that haven't been imported yet.**
  - Applying a phone edit regenerates `notes/<parser>/A.*`. Auto-sync can't run the interactive `spares sync` (files → DB) first, so any pending file edit to A is overwritten. That breaks the requirement that unsynced laptop edits are never silently overwritten (section 1).
  - The reverse happens too: a stale nvim buffer saved over the regenerated file makes the next `spares sync` undo the phone edit.
  - Even a manual `spares sync cloud` loses any hunks you decline in the `spares sync` confirmation.
  - Fix: before regenerating a file, check it against the hash index `spares sync` already keeps (`load_hash_index`, `spares_cli/src/sync/utils.rs`). If they differ, skip that note and flag it.
- **The undo guard must also run on the server.** `review_log` has no graves, so the server must set the guard when it answers a pull. Otherwise the phone can undo a review the laptop has already pulled.
- **The nvim keyword cache goes stale.** The plugin refreshes keywords when `event latest` changes. Sync writes don't create events, so completion goes stale after a sync.

## 8. Fewer conflicts to begin with
- `spares sync cloud` first runs the normal `spares sync` (files → DB), so edits that only exist in `notes/` files become dirty rows before the cloud sync.
- **Optional auto-sync:** the laptop's local server syncs on startup and every `sync_interval` minutes whenever the remote is reachable. Failures are logged, not fatal.
- Live notes are read-only on the web.

## 9. Interfaces
- **Server** (`spares_server/src/route.rs`, new `handlers/sync.rs`):
  - `/api/sync/{meta,pull,push,full}` behind `require_api_key`;
  - `POST /api/sync/run`, which makes this server act as the client against `sync_url` using `SPARES_SYNC_API_KEY`;
  - `GET /api/sync/status`;
  - `GET`/`POST /api/sync/conflicts`.
- **CLI:**
  - `spares sync cloud [--full-upload|--full-download]`. If a full sync is needed, the command explains why and asks which direction.
  - `spares sync conflicts` resolves conflicts one at a time using the `git diff` flow in `spares_cli/src/sync/interactive.rs`: keep local, keep remote, or edit the merged result.
  - `spares migrate ids [--dry-run]` (section 4).
  - Asset files (typst render assets, image-occlusion images, which are only created on the laptop) keep the rsync push from pass 2 of `cloud.rs` via `remote_host`.
- **Frontend:**
  - a Sync button, last-synced time and conflict count (only when `sync_url` is set);
  - a `sync-conflict` badge on notes;
  - live notes read-only;
  - IDs shown in letter form.
- **Config** (`SparesExternalConfig`, `spares_core/src/config.rs`): `sync_url` and `sync_interval`, next to `remote_host`.
- **Env:** `SPARES_REPLICA` (`0`–`7`: `0` = laptop, `1` = server), `SPARES_SYNC_ROLE` (`server`/`client`), `SPARES_SYNC_API_KEY`.

### Open issues
- **Conflicts only exist on the laptop,** because the merge runs there. The phone would see the `sync-conflict` badge, but its server has no conflict rows to show.
- **rsync order:** the asset rsync of `cards/` must run after the laptop regenerates its files. Otherwise it overwrites newer cards the server rendered.

## 10. Deployment, ready to use when the VPS arrives (`deploy/`)
- `deploy/spares.service`: a systemd unit with `SPARES_SOCKET_ADDRESS=127.0.0.1:8080`, `SPARES_FRONTEND_DIR`, `SPARES_REPLICA=1`, `SPARES_SYNC_ROLE=server`, the API key from an `EnvironmentFile`, and `Restart=always`.
- `deploy/setup-vps.sh`: an idempotent script that installs tailscale, rsync and typst (plus pandoc/latexmk only if server-side rendering of those formats is needed), creates the data dir at the same path as on the laptop, installs the unit, and runs `tailscale serve --bg 8080`.
- A `spares_deploy` zsh function next to `spares_serve` that builds the frontend and the linux binary (`cargo zigbuild --target x86_64-unknown-linux-gnu`), rsyncs them, and restarts the service.
- **When the VPS arrives:**
  1. Create the Debian 12 VPS and run `setup-vps.sh`.
  2. Install Tailscale on the phone.
  3. Set `sync_url`/`remote_host`.
  4. Run `spares sync cloud --full-upload` once.

### Open issues
- **Debian:** Debian 12's regular security support ended in June 2026, so use Debian 13.
- **pandoc/latexmk:**
  - Editing markdown or LaTeX notes from the phone regenerates their cards on the server, which runs pandoc or latexmk.
  - So the server needs them, unless those notes are read-only on the web.
  - Not tested: whether a missing tool fails the request or just leaves stale cards.

## 11. Delivery order (one commit each)
1. ID newtype, allocator and `spares migrate ids`.
2. Sync migration and triggers.
3. Merge engine and tests.
4. Protocol endpoints and the client.
5. CLI, including `sync conflicts`.
6. File regeneration and undo guard.
7. Frontend.
8. Auto-sync.
9. Deploy files.

### Open issues
- **Usable sync before file regeneration.** Commits 4–5 make sync usable before commit 6 adds file regeneration. Using sync in between gives the stale-file reverts described in the open issues of sections 6 and 7.

## 12. Verification (no VPS needed)
- **ID tests:**
  - the letter form and the packed integer round-trip for 1–8 letters;
  - sorting the strings matches sorting the integers for mixed lengths: laptop slots, server tails (`giy` < `giza` < `gizzaa` < `gja`) and tier boundaries (`szy` < `szza` < `taaa`);
  - Finder-style natural sort agrees (checked by hand with `localizedStandardCompare` on 2026-10-13);
  - property test with two replicas allocating in random interleavings with random syncs:
    - no ID repeats, and no ID is the start of another;
    - every note created after a sync sorts after everything either side created before it;
    - no laptop import has a server note sorting inside it, and no server call has a laptop note inside it;
  - a call that doesn't fit in the current block starts at the next one, and an import of 60 gets 60 three-letter IDs;
  - the laptop moves to the next block after it sees a server note in its current block;
  - no ID is handed out twice after a delete, after create, undo, create, redo, or after a full download;
  - allocation errors instead of producing a 9th letter;
  - card, review_log, tag and parser IDs are congruent to the replica number mod 8, and replicas `0`, `1` and `2` allocating in random interleavings never collide;
  - the server refuses to start when `SPARES_REPLICA` or `SPARES_SYNC_ROLE` is missing or invalid;
  - letter IDs parse in any case, and digits are rejected;
  - `migrate ids --dry-run` on a copy of the real DB gives `aaa`…`giq`.
- **Sync unit tests** (two temp DBs):
  - edits on different notes converge;
  - edits to different lines of one note merge;
  - edits to the same lines create a `sync_conflict` and keep both versions;
  - **an old unsynced laptop edit survives a newer phone edit to a different line or field**;
  - tag set merge;
  - review_log union, and scheduling taken from the latest review;
  - an edit beats a delete;
  - a plain delete propagates;
  - merged notes give the same card IDs on both sides;
  - a migration-version mismatch is refused;
  - a count mismatch sets `needs_full_sync`.
  - Also run the existing undo e2e tests and `cargo test`/`clippy`.
- **End to end with two local servers:**
  - Start a second `spares_server` on `:8090` with `SPARES_REPLICA=1` and `SPARES_SYNC_ROLE=server`, its own `SPARES_DATA_DIR`/`SPARES_CACHE_DIR`, and **all `*_OUT_DIR` unset**, or real rendered files get overwritten. Set `sync_url` to it and run `--full-upload`.
  - Then:
    1. edit note A locally (through its `notes/` file) without syncing;
    2. edit a different part of A and all of note B in the `:8090` web UI, and review some cards there;
    3. run `spares sync cloud`;
    4. check that A has both edits, B is updated, all review logs are present on both sides, and the `notes/` files were regenerated;
    5. repeat with an overlapping edit and resolve it with `spares sync conflicts`.
- **Phone:** open `http://<laptop-ip>:8090` on the same Wi-Fi and repeat the web steps from a phone browser.

## Sources
- Anki sync implementation: https://github.com/ankitects/anki/tree/main/rslib/src/sync (normal.rs, chunks.rs, graves.rs, meta.rs); manual: https://docs.ankiweb.net/syncing.html, https://docs.ankiweb.net/sync-server.html
- pgEdge Spock: https://github.com/pgEdge/spock, https://www.pgedge.com/solutions/benefit/multi-master
- Turso offline writes: https://turso.tech/blog/introducing-offline-writes-for-turso
