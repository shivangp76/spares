# Two-way sync for spares: design plan

Status: in progress. Section 4 (ids, including `spares migrate ids` and `next_ids` for the other tables) sections 5–10 (change tracking, the sync engine, endpoints, CLI, frontend, auto-sync and deployment) are implemented. Last updated 2026-10-14. Problems found in a review on 2026-10-03 (checked against the code and the real DB, read-only) are listed under **Open issues** at the end of the section they affect.

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
- **SSH:** port 22 stays open on the public IP until `deploy_vps.sh init` closes it (its last step). Updates and the `remote_host` rsync use Tailscale SSH.
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

## 5. Schema changes (migration `20261014130000_sync`, implemented)
- **Change tracking:** `usn INTEGER NOT NULL DEFAULT -1` on `parser`, `tag`, `note`, `card` and `review_log` (`-1` = changed and not yet synced). The default makes new rows dirty without a trigger.
- **`grave(oid, kind TEXT, usn, PRIMARY KEY(oid, kind))`:** IDs deleted on this side and not yet synced. `kind` is the table name. Triggers fill it, so app code never touches it.
  - **Why it's needed:** the sync only exchanges changed rows, and a deleted row no longer exists. Without a grave, "deleted over there" looks the same as "never synced yet", and the deleted row would come back.
  - Alternatives were rejected. Soft deletes would mean `deleted_at IS NULL` in every query. Diffing all IDs still needs the same "was it ever synced" information.
  - Inserting a row under an ID that has a grave (undo of a delete, redo of a create) removes the grave.
- **`sync_base(kind, oid, snapshot JSON, PRIMARY KEY(kind, oid))`:** the last synced version of a row, taken on its first change or delete after a sync. It holds:
  - for a note: its data, custom data, parser, tag IDs and the keywords not embedded in the text;
  - for a card: its fields and tag IDs.
  - The views `sync_note_snapshot` and `sync_card_snapshot` build the same JSON for the current rows.
- **`sync_conflict(id, note_id, base, local, remote, created_at)`:** unresolved conflicts.
- **`sync_client(replica, last_usn, last_sync_at)`** (server only): the last sync of each client, to catch a client restored from a backup.
- **`sync_applying`:** holds one row while sync applies remote changes, inside its own transaction, which turns every trigger off (see `sync::set_applying`). A trigger can't read a TEMP table, so this is a regular table; the row is never committed.
- **Triggers,** so no write path can be missed:
  - `AFTER UPDATE` on parser, tag, note and card, when a synced column changed (not `updated_at` alone) and the row isn't dirty yet: snapshot the old row into `sync_base`, then set `usn = -1`. They don't touch `updated_at`, which review snapshot tags use to decide whether to rebuild.
  - `BEFORE INSERT`/`DELETE` on `note_tag`, `card_tag` and the non-embedded rows of `note_keyword`: snapshot the parent while it still has the old children, then mark it dirty.
    - When the parent or the tag is being deleted, the cascade runs after that row is gone (tested with `sqlite3`), so the triggers skip it: deleting a tag doesn't mark every note that had it.
  - `note_link` and embedded keywords aren't tracked. They are derived from the text and rebuilt where a note is applied.
  - `BEFORE DELETE` on parser, tag, note and card: snapshot the row (with its tags, which a cascade would remove) if it was synced. `AFTER DELETE`: insert a grave. Keeping the base of deleted rows lets the merge know which card each order in a note's synced text named.
- **`app_state` keys:**
  - `sync_usn`: on the server, its counter; on the client, the server's counter at the last sync.
  - `last_sync_at`, `last_sync_event_id`, `needs_full_sync`.

The open issues found in the review (child snapshots taken after the change, no way to turn the triggers off, cascades marking every note, `parser` having no `updated_at`) are fixed as described above.

## 6. Sync engine: `spares_core/src/sync/` (implemented)

| File | What it does |
|---|---|
| `records.rs` | The rows sync carries (`Changes`): parsers, tags, notes (with tag IDs and non-embedded keywords), cards (with tag IDs), review logs, graves. A record has the same JSON shape as its `sync_base` snapshot. |
| `merge.rs` | Pure three-way merges: values, sets, JSON objects key by key, and note text (`diffy`). |
| `orders.rs` | Replaces the card orders in a note's text with card IDs for the text merge (see below). |
| `client.rs` | `merge_pulled`: merges the server's changes into the local DB. |
| `apply.rs` | Upserts and deletes for records, parents first; the server applies a push with `apply_changes`. |
| `server.rs` | `meta`, `pull`, `push`, `full_download`, `full_upload`. |
| `full.rs` | Snapshot (`VACUUM INTO`) and restore. |
| `run.rs` | `sync`: the client's side, deciding between a normal and a full sync, through a `SyncRemote`. |

### What gets synced
- Parsers, tags, notes, cards and review logs, with graves.
- `note_link` and embedded keywords are derived from the text, so they aren't sent. The receiving side rebuilds them whenever a note's text changes.
- `render_asset` and `render_package` rows: the client sends all of them on every push, since assets are only added on the laptop. (Their files still go by rsync.)
- Not synced: `event` (undo history), `offline_action`, `app_state`, `id_counter` (except as below).

### Merge rules (`client.rs`)
*Base* means the `sync_base` row, or the local row if it isn't dirty. A dirty row with no base is one this side already pushed in a sync that didn't finish, so the remote version is taken as the base and the local version wins.

| Situation | Result |
|---|---|
| Changed on one side only | That side's version, whatever its timestamp. |
| Note text changed on both sides | Line-based three-way merge (`diffy`) of the texts with their card orders replaced by card IDs (below). If that conflicts, the plain texts are tried too. |
| Same lines edited on both sides, or a merged text that no longer parses | Keep the local text. Store base, local and remote in `sync_conflict` and tag the note `sync-conflict`. |
| Note tags, non-embedded keywords, card tags | Set merge: additions from either side kept, removals from either side applied. |
| Note `custom_data`, card `custom_data` | Key by key. A key both sides changed: local for notes, the newer change for cards. |
| Note `parser_id`, `created_at` | Three-way; local wins a conflict. |
| review_log | Union. A log keeps its card and tag where either side still has them. |
| Card scheduling (`due`, `stability`, `difficulty`, `state`) | As a whole from the side that changed it. Both changed: the side with the newer review, else the newer change. If both reviewed, the memory state is replayed from the merged logs (`compute_memory_state`). |
| Card `special_state`, `desired_retention` | Three-way; the newer change wins a conflict. |
| Card `order`, `back_type`, `note_id` | Follow the note text. |
| Note deleted on one side, edited on the other | The edit wins: the note is kept (its cards and reviews are sent back) and tagged `sync-conflict`, with a `sync_conflict` row. A review doesn't count as an edit. |
| Note deleted on one side, not edited on the other | Delete it. |
| Card deleted on one side | Delete it, even if the other side reviewed it (its reviews stay, unlinked). A card's existence follows its note's text. |
| Tag or parser deleted on one side, changed on the other | Keep it, and send back what it is on. A parser still used by notes is kept too. |
| Tags, parsers (metadata) | Per-field three-way merge; the newer change wins a tag conflict. |
| Both sides have a tag with the same name (review snapshot tags, `sync-conflict`, or the same new tag) | The local tag is folded into the server's: its notes, cards, review logs and filtered-tag scheduler data move over, and it's deleted. |
| Both sides have a parser with the same name | Sync stops with a message. |

**Card IDs when both sides changed a note's text.** A cloze names its cards by position (`o:1,2`), and adding a cloze renumbers every later one, so two edits that each add a cloze would conflict on every cloze line. Before merging, each side's orders are replaced with the IDs of the cards they name on that side (the base's from the cards at the last sync, which the triggers keep in `sync_base` even when deleted). After the merge:
- `reconcile_cards` gives each cloze the card its merged text names, else the card with its cloze uid, else (when the text names no cards) the card in its place;
- cards nothing names are deleted (with graves), and clozes without a card get new ones;
- the text is written back with sequential orders.

The client does all of this and pushes the result, so the server never re-parses, and card IDs are the same on both sides.

### Protocol (no chunking with two replicas)
1. `meta` → `{usn, epoch, server_time, schema_version, replica, role, empty, client_last_usn}`. The client refuses to sync if:
   - the clocks differ by more than 5 minutes;
   - the schema versions differ;
   - both sides have the same replica number;
   - the roles aren't one `client` and one `server`.

   It asks for a full sync (`FullSyncRequired`, with the reason) when any of these hold:
   - it has never synced;
   - `needs_full_sync` is set;
   - the server's epoch changed (it was replaced by a full upload);
   - its last usn is ahead of the server's (the server was restored from a backup);
   - its last usn is behind what the server recorded for it in `sync_client` (the client was restored from a backup).
2. `pull {since}`: the server first stamps everything changed on it since its last stamp (`usn = -1`, phone edits included) with `usn + 1`, then returns rows with `usn > since`, plus every card of each changed note. It also sets its undo guard.
3. The client opens one local transaction, merges (`merge_pulled`), and pushes everything dirty afterwards.
4. `push {pull_usn, changes, render assets}`: the server answers 409 (`SyncRetry`) if its usn moved or anything changed on it since the pull (a phone edit while the client merged). The client rolls back and starts over, up to 3 times. Otherwise the server applies the changes as given in one transaction (foreign keys deferred), stamps them with `usn + 1`, records the client in `sync_client`, and returns the new usn and row counts.
5. The client stamps its dirty rows with the new usn, clears its graves and bases, records `sync_usn`, `last_sync_at` and its undo guard, compares counts, and commits.
   - If the counts differ, it rolls back and sets `needs_full_sync`. With the 409 check this only happens if the replicas drifted apart some other way.
   - **Crash safety:** if the client fails after the server committed a push, the next pull returns the client's own changes, which match its still-dirty rows and merge to themselves.
- **Live notes:** before writing notes, the incoming notes' `live_block_order` is cleared, so two notes swapping block orders in one sync don't trip the unique index.

### Full sync (escape hatch, and the first sync)
- `full.rs` copies the incoming database (a `VACUUM INTO` snapshot) table by table from an `ATTACH`ed file in one transaction, instead of swapping the file, so the server can do it with the database open. It checks `integrity_check` and the schema version first.
- Afterwards every row has `usn = 0`, and graves, bases and undo history are cleared.
- `id_counter` rows are merged by taking the larger value, so a replica never lowers its own counter.
- **Upload:** the client first marks everything synced and sets `needs_full_sync` until the upload succeeds. Edits made during the upload stay dirty, with what was uploaded as their base. The server gets a new epoch, so any other client must do a full sync too.
- **Download:** the client continues from the `sync_usn` and epoch in the server's snapshot. Rows that were dirty on the server get stamped and sent again on the next pull, where they merge to themselves.
- A full download reports every note as changed, and every note that's gone as deleted, so the caller can regenerate files (section 7).

#### Open issues
- **Request size.** axum limits request bodies to 2 MB by default. `PUT /api/sync/full` and large pushes need a higher limit (section 9).
- **Before a full sync, show how many changes each side would lose.**

## 7. After a sync
- **Files** (`sync/files.rs`, implemented): after a sync, on whichever side the changes landed:
  - the files of every changed note are regenerated with `render_notes` (notes and cards, rendered where the config renders);
  - the files of deleted notes are removed (`delete_note_files`, `reconcile_after` for the notes repository);
  - card files left over from cards a sync removed (orders past the note's last card) are deleted.

  The server does this after a push or a full upload; the client after `sync_now` (which `POST /api/sync/run` calls).
- **Change number** (implemented): the nvim plugin refreshes its keyword cache when `spares event latest` changes, and sync writes create no events. So that number is the newest note event id plus `app_state.notes_version_offset`, which every sync that changes notes moves on. A full sync, which clears `event`, adds more than the newest event id, so the number never repeats.
- **Undo:** record `last_sync_event_id`. `/api/undo` refuses events at or below it (Anki also clears undo on sync). The server sets it when it answers a pull or a push, the client when it commits a sync.

### Open issues
- **Regenerating files after a sync and edits not imported yet.**
  - With the notes repository (`notes_vcs`), regenerated files are committed to the server copy (`notes-spares`). Edits not pushed yet live in the user's clone in `notes/`, so they are never overwritten. They meet the sync's commit when the clone is rebased, as with any other server-side change.
  - Without it, files under `<data_dir>/notes/` are rewritten in place. `spares sync cloud` imports pending file edits (`spares sync`) first (section 8). An edit made during the sync could still be overwritten.

## 8. Fewer conflicts to begin with
- **Note file edits.** With the notes repository, edits to note files reach the database by `jj git push` from `notes/`. Push them before `spares sync cloud` so they're in this sync; otherwise they go in the next one. Nothing is lost either way, since the sync's own file changes are commits that the clone rebases onto.
  - `spares sync` (the Anki adapter's sync) is not involved. Section 2's description of it as files → DB was wrong.
  - Without the notes repository, `spares sync cloud` warns that file edits not imported yet are overwritten if the sync changes those notes.
- **Optional auto-sync** (`spares_server/src/auto_sync.rs`, implemented): with `sync_url` and `sync_interval_minutes` set, a `client` server syncs on startup and every that many minutes. Failures (the server being unreachable, for one) are logged, not fatal. A required full sync pauses it with one warning until `spares sync cloud` does it. It doesn't run the rsync of rendered files; `spares sync cloud` does.
- Live notes are read-only on the web.

## 9. Interfaces
- **Server** (`spares_server/src/handlers/sync.rs`, implemented), all behind `require_api_key`:
  - `GET /api/sync/meta?replica=N`, `POST /api/sync/pull`, `POST /api/sync/push`, `GET`/`PUT /api/sync/full?replica=N`. These are answered only by `SPARES_SYNC_ROLE=server`. Push and full upload accept up to 1 GiB.
  - Error statuses: 409 when the server changed during the sync (the client starts over), 412 when a full sync is required (with the reason), 400 for other sync errors, 403 for the wrong role.
  - `POST /api/sync/run {mode: normal | full_upload | full_download}`: makes a `client` sync with `sync_url`, using `SPARES_SYNC_API_KEY`. Returns the report (counts, conflicts, changed and deleted notes).
  - `GET /api/sync/status`: role, replica, `sync_url`, last sync, `needs_full_sync`, and the number of conflicts.
  - `GET /api/sync/conflicts` and `POST /api/sync/conflicts/{id}` with `"keep"`, `"delete"` or `{"text": "..."}`.
- **CLI** (implemented):
  - `spares sync cloud [--full-upload|--full-download]`. With `sync_url` set, it runs a two-way sync through the local server. If a full sync is needed, it says why and asks which direction. Then, with `remote_host`, it copies rendered files, templates and render dependencies to the server by rsync (the database is not copied). Without `sync_url`, it pushes the database as before.
  - `spares sync conflicts` resolves conflicts one at a time. It shows a `git diff` of this side's text against the other's, with card orders shown as card IDs (`o:c40`) so a cloze that only moved reads the same. The choices are:
    - keep this side's text;
    - take the other side's;
    - edit a merge with conflict markers in `$EDITOR`;
    - for a note deleted on one side: keep it or delete it.

    The chosen text's card IDs are turned back into this side's orders, so each cloze keeps its card and reviews.
  - `spares migrate ids [--dry-run]` (section 4).
- **Frontend** (implemented):
  - a Sync button in the navbar with the last-synced time and conflict count. It is shown only on a `client` with `sync_url` set, i.e. the laptop's own web app; a full sync is left to `spares sync cloud`;
  - a `sync-conflict` badge on notes in the list and in the note view, pointing to `spares sync conflicts`;
  - live notes read-only (their text; tags and keywords stay editable);
  - IDs shown in letter form.
- **Config** (`SparesExternalConfig`, `spares_core/src/config.rs`): `sync_url` and `sync_interval_minutes` (0 = off), next to `remote_host`.
- **Env:** `SPARES_REPLICA` (`0`–`7`: `0` = laptop, `1` = server), `SPARES_SYNC_ROLE` (`server`/`client`), `SPARES_SYNC_API_KEY` (the server's `SPARES_API_KEY`, for the client).

### Open issues
- **Conflicts only exist on the laptop,** because the merge runs there. The phone would see the `sync-conflict` badge, but its server has no conflict rows to show.
- **rsync order:** the asset rsync of `cards/` runs after the laptop regenerates its files (`spares sync cloud` copies files after the sync). It can still overwrite cards the server rendered itself for a note edited on the phone since the sync.

## 10. Deployment (`scripts/deploy_vps.sh` and `scripts/setup_vps.sh`, implemented)
The repo already had `scripts/setup_vps.sh`, an idempotent script that installs the build tools, pandoc and LaTeX, builds spares from source on the server, writes `/etc/spares/spares.env` and a systemd unit, and optionally serves HTTPS with Caddy (`--domain`). It was extended rather than duplicated in a new `deploy/` directory:
- the env file sets `SPARES_REPLICA=1` and `SPARES_SYNC_ROLE=server` (the server refuses to start without them);
- the unit restarts always;
- `--tailscale` installs Tailscale, joins the tailnet with Tailscale SSH (`tailscale up --ssh`), and serves the app privately with `tailscale serve --bg 8080`. No port is opened to the internet;
- `--user` lets root run it: a missing user is created with sudo and root's SSH keys, and asked for a password, since updates go through sudo;
- it saves its arguments in `/etc/spares/setup.conf`, so updating is `sudo bash setup_vps.sh` with none, and the URL in `/etc/spares/url`.

`scripts/deploy_vps.sh` runs on the laptop and does the rest (it replaces the planned `spares_deploy` zsh function):
- `init <IP> [--user <name>] [--domain <domain>]`:
  1. checks that GitHub's `main` has the checkout, since the server builds from it;
  2. backs up the database (`VACUUM INTO`);
  3. copies `setup_vps.sh` to the server and runs it as root;
  4. stores the API key in the Keychain (service `spares-sync`) and sets `remote_host`, `sync_url` and `sync_interval_minutes` in `config.toml`, keeping a `.bak`;
  5. adds `SPARES_REPLICA`, `SPARES_SYNC_ROLE` and `SPARES_SYNC_API_KEY` (read from the Keychain, so the key is never in a file) to `spares.zsh` if missing;
  6. waits until the restarted local server reports the client role, the `sync_url` and the key (`sync_api_key_set` in `/api/sync/status`);
  7. runs `spares sync cloud --full-upload` and compares note, card and review counts on both sides;
  8. with Tailscale, closes the public ports with `ufw` (only `tailscale0` and Tailscale's UDP port stay open). This runs only after the laptop has reached the server over Tailscale SSH, and the provider's web console is the way back in.
- `update`: copies `setup_vps.sh` to the server and runs it with sudo over Tailscale SSH.

`spares sync cloud` copies the laptop's `config.toml` to the server (`server_config`): the same settings, so reviews on the phone are scheduled alike, without `remote_host`, `sync_url`, `sync_interval_minutes` and `remote_rendered_dir`, and with `render_browser_parsers = false`.

**When the VPS arrives:**
1. Install Tailscale on the laptop and the phone, logged in to one account.
2. Push to GitHub's `main`.
3. Create a Debian 13 VPS with your SSH key, then run `scripts/deploy_vps.sh init <IP>`.
4. On the phone, open the URL it prints and log in with the API key.

### Open issues
- **Hetzner:** the CX22's name and price may have changed. Check when buying.
- **pandoc/latexmk:** the script installs them, so markdown and LaTeX notes edited on the phone render on the server. The server builds from source, which needs about 2 GB of memory (the script adds swap).

## 11. Delivery order (one commit each)
1. ID newtype, allocator and `spares migrate ids`. Done, then `next_ids` for the other tables.
2. Sync migration and triggers. Done.
3. Merge engine and tests. Done.
4. Protocol endpoints and the client. Done, together with file regeneration (part of 6), so sync was never usable without it.
5. Undo guard (the rest of 6). Done.
6. CLI, including `sync conflicts`. Done.
7. Frontend. Done.
8. Auto-sync. Done.
9. Deploy files. Done (in `scripts/setup_vps.sh`).

## 12. Verification (no VPS needed)
Implemented as tests in `spares_core` (`ids`, `sync::tests`, `sync::tracking_tests`, `sync::merge`, `sync::orders`, `api::undo::e2e_tests::ids`), and run by hand with two local servers (below) on 2026-10-14: a full upload, edits to different lines of one note on both sides, a new note and a review on the server, a conflict resolved with the other side's text, and automatic sync on startup all gave the same database on both sides, with the files regenerated. The phone step was not run.


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
