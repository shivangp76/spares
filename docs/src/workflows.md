# Workflows

## Live notes

Live notes let you write notes in a source file and keep them in sync with the database across repeated imports. Each "live sync group" is identified by a `live-sync-name` set in the file's global settings. Notes with the same `live-sync-name` are matched to the database by their position in the file (block 0, block 1, …), so edits to existing notes are updated rather than duplicated.

Note boundaries are determined by file position. You can freely add, delete, or reorder notes in the file — each note is matched to the database by `(live_sync_name, block_order)`, where `block_order` is its 0-based index within that sync group. Content outside note boundaries (headings, comments, prose) is preserved verbatim across imports.

### 1. Mark notes as live

Add `live-sync-name` to the global settings of your file, *before* any notes:

```md
<!--- spares: start --->
<!--- # live-sync-name: lecture_notes_501 --->

# Oceans

<!--- spares: note start --->
The Pacific Ocean's deepest point is {{the Mariana Trench}}.
<!--- spares: note end --->

Some geographers split the Pacific into the North and South Pacific.

<!--- spares: note start --->
The Atlantic Ocean's deepest point is {{the Puerto Rico Trench}}.
<!--- spares: note end --->

<!--- spares: end --->
```

Notes after a `live-sync-name` setting belong to that sync group and are assigned `live_block_order` 0, 1, 2, … in file order. The headings (`# Oceans`) and prose (`Some geographers split...`) sit outside note boundaries and are not part of any note — they are preserved across imports without being parsed. You can switch to a different sync group by setting `live-sync-name` again later in the file.

### 2. Import

```sh
spares import --parser markdown ./oceans.md
```

On import, spares:
- Mints unique `id:` keys on each cloze (e.g. `{{[id:a1b2c3d4e5f6] the Mariana Trench}}`) for stable card matching.
- Sets `live_block_order` (the note's position) and `live:<name>` tag.
- Creates a new note in the DB, or updates an existing one matched by `(live_sync_name, block_order)`.

The file is rewritten in place with the new `id:` keys, so subsequent imports see the same IDs:

```md
<!--- spares: start --->
<!--- # live-sync-name: lecture_notes_501 --->

# Oceans

<!--- spares: note start --->
The Pacific Ocean's deepest point is {{[id:a1b2c3d4e5f6] the Mariana Trench}}.
<!--- spares: note end --->

Some geographers split the Pacific into the North and South Pacific.

<!--- spares: note start --->
The Atlantic Ocean's deepest point is {{[id:f6e5d4c3b2a1] the Puerto Rico Trench}}.
<!--- spares: note end --->

<!--- spares: end --->
```

### 3. Edit and re-import

Edit the source file freely and re-run the same import command:

```sh
spares import --parser markdown ./oceans.md
```

Only notes whose content actually changed are updated. Existing cards keep their review history.

Matching is **positional**: the first note in the file matches DB entry `(live_sync_name, 0)`, the second matches `(live_sync_name, 1)`, and so on. This means you can freely adjust note boundaries and content between them — what matters is each note's ordinal position within its sync group.

Here is the file from step 2 after some heavy editing — the Pacific note was deleted, a new Indian Ocean note was inserted at the top, and non-note content was added. Crucially, the Atlantic note is **unchanged**:

```md
<!--- spares: start --->
<!--- # live-sync-name: lecture_notes_501 --->

# Deepest Ocean Points

The five oceans are Pacific, Atlantic, Indian, Southern, and Arctic.

<!--- spares: note start --->
The Indian Ocean's deepest point is {{the Java Trench}}, which reaches over 7,000 meters.
<!--- spares: note end --->

Contrast this with the Atlantic:

<!--- spares: note start --->
The Atlantic Ocean's deepest point is {{[id:f6e5d4c3b2a1] the Puerto Rico Trench}}.
<!--- spares: note end --->

The Atlantic also contains the Romanche Trench near the equator.

<!--- spares: end --->
```

On re-import:
- The heading changed to `# Deepest Ocean Points` — non-note content, preserved as-is.
- The **new Indian Ocean note** at position 0 gets `block_order = 0` → matches the old Pacific DB entry (`a1b2c3d4e5f6`) → **updates** it (the DB entry now stores Indian Ocean content). Its cloze has no `id:` yet, so a fresh one is minted.
- The **Atlantic note** stays at position 1 with `id:f6e5d4c3b2a1` intact → gets `block_order = 1` → matches its own old DB entry → **updates** it. Because the `id:` key and note content are the same, the existing card keeps its review history unchanged.

The text outside note boundaries (`Contrast this with the Atlantic`, `The Atlantic also contains...`) is **not parsed** — only content inside `note start`/`note end` is.

> **Key insight:** Because matching is by `(live_sync_name, block_order)` and block_order is the note's file position, deleting or inserting notes shifts the positions of everything that follows. Existing clozes keep their review history as long as their `id:` key survives (the Atlantic cloze `f6e5d4c3b2a1` is untouched). When you insert a new note, its clozes get fresh `id:` keys. The non-note content (headings, commentary) stays untouched.

### 4. Strip liveness (finalize)

Once the notes are stable, remove all live-sync machinery:

```sh
spares import --parser markdown --strip-liveness ./oceans.md
```

This:
- Removes `live_sync_name`, `live_block_order` from custom data.
- Removes the `live:<name>` tag from each note.
- Removes `id:` keys from every cloze in the file (the file is rewritten).
- Sets the action to `Update` (matched by `note-id`) so the existing database note is patched instead of re-created.

After stripping, the notes are ordinary notes and can no longer be matched by `live_sync_name` on subsequent imports.

> **Note:** `--strip-liveness` requires the notes to already exist in the database. Import them first without the flag.

## Inheriting SRS data from another card

New cards normally start with fresh scheduling. If you create a card that tests the same material as an existing card — for example when splitting one note into several, or when importing material you already know — you can copy the source card's scheduling and review history onto the new card with the `inh:` cloze setting.

On creation (or update), the new card inherits the source card's `stability`, `difficulty`, `desired_retention`, `state`, `due`, and `special_state`, along with its full review history. It will be scheduled as if it had been reviewed before.

### Syntax

Add `inh:` to a cloze's settings:

```
{{[inh:NOTE_ID/ORDER] content }}
```

- `NOTE_ID` is the id of the note containing the source card.
- `ORDER` is the 1-based order of the source card within its note (the first card of a note has order 1).

### 1. Find the source card

List the source note's cards to find the right `order` (the output JSON has an `order` field per card):

```sh
spares card get -n 5
```

### 2. Add `inh:` to the destination cloze

In a markdown file, the destination card inherits from note 5's first card:

```md
<!--- spares: start --->

<!--- # tags: geology, oceans --->
<!--- spares: note start --->
# Deep Sea Trenches

The Mariana Trench is the deepest point in the Pacific Ocean {{[inh:5/1] at about 10,935 meters}}.
<!--- spares: note end --->

<!--- spares: end --->
```

Import:

```sh
spares import --parser markdown ./trenches.md
```

The new card is created with the same scheduling and review history as note 5's first card, instead of starting from scratch.

### Notes

- The source card must already exist in the database; otherwise the import fails with an error.
- `ORDER` must be `>= 1`.
- For clozes that produce both a forward and backward card (via `r:`), only the forward card inherits — the backward card starts fresh.
- Each cloze in a note can carry its own `inh:` pointing at a different source card.
- `inh:` is ephemeral: it is consumed when the note is created or updated and is stripped from the stored note data, so it will not be re-applied on subsequent imports.

## CLI cards

A CLI card is reviewed by running a command rather than by flipping a rendered document. Mark one
with a `spares: cli` block, written in the host parser's comment syntax:

```md
Have the tests been kept green?
<!--- spares: cli start --->
<!--- exec = "pytest tests/" --->
<!--- spares: cli end --->
```

At review time the text outside the block is printed in the terminal, then `exec` runs under
`sh -c`. The command owns stdin, so it may prompt you. It must finish by writing a single JSON
object to stdout — `{"score": 0.87}`, a float in `[0, 1]` — which is converted to a scheduler
rating.

A note may hold several blocks; each one is a separate card. A note cannot mix CLI blocks with
clozes or image occlusions — give the CLI block its own note.

### Block identity

Blocks carry an `id`, which spares mints for you the first time it writes the note:

```md
<!--- spares: cli start --->
<!--- exec = "pytest tests/" --->
<!--- id = "a1b2c3d4e5f6" --->
<!--- spares: cli end --->
```

It does the same job as a cloze's `id:` key. A card is bound to the `id` of its block, not to the
block's position, so you can add, remove, and reorder blocks in a note and every remaining card
keeps its own command and review history.

## Fast note creation

Using snippets, such as through [LuaSnip](https://github.com/L3MON4D3/LuaSnip), can speed up note creation.

## Converting notes between parsers

For example,
```sh
spares import --to-parser="markdown" 0001.tex 0002.tex
```

With [notes in a git or jj repository](#notes-as-a-git-or-jj-repository), converting cannot be done by pushing (it shows up as a deleted note and a new one). Run the import above on the files in your clone instead: spares moves the notes' files to the new parser's directory and commits that, so fetch afterwards.

## Unbury cards matching query

```sh
spares card edit -q 'QUERY and c.user_buried' --special-state none
```
where QUERY is replaced with your query

## Bulk note actions

spares ships with fzf support which can be used to perform bulk note actions, such as tagging. This selector can also be used to find all notes with a certain tag by typing `tag: .*tag1`. For more than 1 tag, see `spares_query` for advanced querying.

Examples:
```sh
spares note edit --tags-to-add tag1 tag2 --files 0001.tex 0002.tex
spares note edit --tags-to-remove tag1 tag2 --files 0001.tex 0002.tex
```

## Getting notes created `n` days before a note

You might be reviewing an old note and realize you forgot the note. Let's say this note was taken during a lecture in class. Then, you may need to understand the notes created a few days before that note. For example, this note may be a theorem that depends on a definition introduced a few days earlier in class. This utility allows you to find all notes created `n` days before a note, so that you can perform an action on them, such as marking them as forgotten. You may also wish to filter the notes by the tag.

```sh
# Get notes created $NUM_DAYS days before the note with id $ID, where $NUM_DAYS defaults to 5: `get_notes_before $ID $NUM_DAYS`
get_notes_before() {
  local note_id=$1
  local days_before=${2:-5}

  # Get the creation date of the input note
  local note_date=$(spares note get "$note_id" | jq -r '.created_at')

  # Check if we successfully got a date
  if [[ -z "$note_date" || "$note_date" == "null" ]]; then
    echo "Error: Could not retrieve note $note_id or it has no creation date" >&2
    return 1
  fi

  # Calculate the date N days before
  local start_date=$(date -j -v-${days_before}d -f "%Y-%m-%dT%H:%M:%SZ" "$note_date" "+%Y-%m-%dT%H:%M:%SZ")

  # Search for notes in the date range
  spares note search "created_at<=$note_date and created_at>=$start_date"
}
```

## Visualizations

Print tags as a tree:
```sh
spares tag list --limit=9999 --tree
```

Print notes as graph:
```sh
spares note list --limit=9999 --graph
```

## Notes as a git or jj repository

Spares can keep your note files in a git repository, so you can branch them, let an LLM edit them, review the result with any tool, and keep or discard it. The database acts as the remote:

- `notes-spares/` in the data directory is spares' copy. Only the spares server writes to it, and it commits every change, so its `main` describes the database.
- `notes/` (and the `~/notes` symlink) becomes your clone of it.
- **Fetch** to get changes made in spares: web edits, `spares note edit`, undo, tag renames, renders.
- **Push to `main`** to import your changes into spares.

### Setup

The server must be running.

```sh
spares note init-vcs        # git
spares note init-vcs --jj   # jj, colocated with git (jj 0.30 or newer)
```

Your old notes directory is kept as `notes.pre-vcs-<time>`. Notes you had edited without syncing are put in their own commit (the `pre-vcs-local-edits` branch, or a jj change), which is printed. A file can also differ because spares changed the note without rewriting it (an undo, a tag rename), and pushing such a file would revert that change, so review them before pushing them. Other files from the old directory are copied into the clone as untracked files. Delete the old directory once you are done.

If the setup is interrupted, run it again: it continues where it stopped.

### With jj

`init-vcs --jj` adds two aliases to the clone's jj config:

```sh
jj spares-pull                  # get spares' changes, rebase your work onto them
jj new main@origin              # start a change, e.g. for an LLM to edit
# …edit…
jj diff                         # review
jj abandon                      # dislike it
jj describe -m "…" && jj spares-push   # like it: imports it into spares
```

`jj spares-push [REV]` fetches, then refuses if spares has newer notes (run `jj spares-pull` first) or if the revision or its unpushed ancestors have conflicts. Otherwise it moves your local `main` to the revision (default `@`, or `@-` if `@` is empty) and pushes it. `jj spares-pull [REV]` rebases the branch of `@` (or of `REV`) onto spares' latest `main`. jj itself refuses to push a stale `main`, conflicted commits, and commits without a description.

New files are not tracked automatically in the clone (`snapshot.auto-track = "none()"`), since only existing notes can be pushed; use `jj file track` for a `README.md`.

Use jj commands only in a jj clone, rather than mixing in `git pull` and `git push`.

### With git

```sh
git pull --rebase                       # get spares' changes
git switch -c llm-edit origin/main      # start a branch, e.g. for an LLM to edit
# …edit…
git diff origin/main                    # review
git switch main && git branch -D llm-edit        # dislike it
git commit -am "…" && git push origin HEAD:main  # like it: imports it into spares
```

### What a push can change

- Only existing notes, at their canonical paths (`<parser>/<id>.<ext>`, such as `typst/0042.typ`), and a `README.md` at the root. Each file must still hold exactly that one note.
- Create notes with `spares note add` or `spares import`, and delete them with `spares note delete`. A note's parser cannot be changed by pushing.
- The linked-notes section is regenerated by spares, so edits to it are ignored.
- Only `main` can be pushed.
- Never edit `notes-spares/` by hand (a PDF viewer's inverse search may open files there). Spares rewrites such files from the database, saving what it overwrites to `notes-spares/.git/spares-quarantine/`.

A push is imported completely or not at all. After a push, spares may commit the notes as it writes them (for example with renumbered clozes) and render them, so pull before editing the same notes again. If what you pushed was already written the way spares writes it, there is nothing to pull.

### Rejected pushes

The reason a push was rejected is printed on lines starting with `spares:` (prefixed with `git:` or `remote:`), just above git's or jj's own error. jj follows them with `The remote rejected the following updates … (reason: push-to-checkout hook declined)` and a hint to check your permissions, which you can ignore; the `spares:` lines say what went wrong. Nothing is imported from a rejected push, and your commits stay in your clone.

| Message | What to do |
|---|---|
| spares has newer notes than your push is based on | Pull and rebase (`jj spares-pull` / `git pull --rebase`), resolve conflicts, push again |
| notes … changed in spares since the commit your push is based on | Spares committed their current version: pull and rebase, then push again |
| a path is not a note file, adds or deletes a note, has a conflict marker or fails to parse | Fix the listed files and push again |
| the spares server is not running; start it to push | Start the spares server (a version with `spares note init-vcs`) and push again |
| this repository … does not belong to the spares server at … | Run `spares note init-vcs --repair-hooks` with the settings of the instance the repository belongs to |
| only `main` can be pushed | Move `main` to your commit and push it (`jj spares-push`, or `git push origin HEAD:main`) |

While a push is being imported, other changes to notes (the web UI, the CLI) wait for it, for at most 30 seconds, and are then refused with a message to retry. Nothing is lost.

`spares note vcs-check` compares the repository with the database, and `spares note vcs-check --fix` rewrites what differs. After reinstalling spares or changing its address, run `spares note init-vcs --repair-hooks`.

## Syncing notes with Anki

### Option 1: Interactive Mode
```sh
spares sync --from anki --to spares --dry-run
```

This will walk you through syncing notes between spares and Anki. Use `--individual` to review changes one at a time instead of in bulk.

To sync specific notes by ID:
```sh
spares sync --from anki --to spares --ids 5 12 23
```

### Option 2: Batch selection with fzf

Use `--print-files` to output changed note file paths non-interactively, then pipe them to fzf for multi-selection:

```sh
spares sync --from anki --to spares --print-files \
  | fzf -m --preview 'bat --color=always {}' \
  | xargs spares sync --from anki --to spares --files @-
```

The `--print-files` output is cache-rendered paths, and `--files` accepts them directly.

## Latex

### Compilation

Tools
- Neovim with [Vimtex](https://github.com/lervag/vimtex)
- `latexmk`

`.latexmkrc`
```perl
# $out_dir can be a directory with a large number of files. However, $aux_dir must be a directory with a relatively small number of files. Otherwise, latexmk will take significantly longer to compile (sometimes 5x the time).
# NOTE: $XDG_CACHE_HOME needs to be fully expanded and replace here
$out_dir = '$XDG_CACHE_HOME/vimtex';
# $aux_dir is not specified here since this value would override the one supplied in `nvim/init.lua` for `vimtex`. We want each note file to create its own directory, so we can control the number of files in the `$aux_dir`.

# Enable shell escape for packages listed here: <https://tex.stackexchange.com/questions/598818/how-can-i-enable-shell-escape>
$pdflatex = 'pdflatex --shell-escape %O %S';
```

Config for `vimtex`:
```vim
vim.g.vimtex_compiler_latexmk = { aux_dir = function()
  return os.getenv("LATEX_OUT_DIR") .. "/aux/" .. vim.fn.expand("%:t:r")
end }
```
where `$LATEX_OUT_DIR` is set in your shell to the appropriate directory.

With [notes in a git or jj repository](#notes-as-a-git-or-jj-repository):
- spares compiles notes from its own copy (`notes-spares/`), while vimtex compiles the file in your clone. Both write the same PDF in `$LATEX_OUT_DIR`, so after compiling an unpushed change (for example on an LLM's branch), reviews show that version until spares renders the note again (`spares note generate`).
- Inverse search from a PDF spares compiled opens the file in `notes-spares/`. Edit the note in your clone instead: spares overwrites edits made in `notes-spares/`, saving them to `notes-spares/.git/spares-quarantine/`.
