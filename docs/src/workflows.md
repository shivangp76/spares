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

## Fast note creation

Using snippets, such as through [LuaSnip](https://github.com/L3MON4D3/LuaSnip), can speed up note creation.

## Converting notes between parsers

For example,
```sh
spares import --to-parser="markdown" 0001.tex 0002.tex
```

## Unbury cards matching query

```sh
spares edit card -q 'QUERY and c.user_buried' --special-state none
```
where QUERY is replaced with your query

## Bulk note actions

spares ships with fzf support which can be used to perform bulk note actions, such as tagging. This selector can also be used to find all notes with a certain tag by typing `tag: .*tag1`. For more than 1 tag, see `spares_query` for advanced querying.

Examples:
```sh
spares edit note --tags-to-add tag1 tag2 --files 0001.tex 0002.tex
spares edit note --tags-to-remove tag1 tag2 --files 0001.tex 0002.tex
```

## Getting notes created `n` days before a note

You might be reviewing an old note and realize you forgot the note. Let's say this note was taken during a lecture in class. Then, you may need to understand the notes created a few days before that note. For example, this note may be a theorem that depends on a definition introduced a few days earlier in class. This utility allows you to find all notes created `n` days before a note, so that you can perform an action on them, such as marking them as forgotten. You may also wish to filter the notes by the tag.

```sh
# Get notes created $NUM_DAYS days before the note with id $ID, where $NUM_DAYS defaults to 5: `get_notes_before $ID $NUM_DAYS`
get_notes_before() {
  local note_id=$1
  local days_before=${2:-5}

  # Get the creation date of the input note
  local note_date=$(spares get note "$note_id" | jq -r '.created_at')

  # Check if we successfully got a date
  if [[ -z "$note_date" || "$note_date" == "null" ]]; then
    echo "Error: Could not retrieve note $note_id or it has no creation date" >&2
    return 1
  fi

  # Calculate the date N days before
  local start_date=$(date -j -v-${days_before}d -f "%Y-%m-%dT%H:%M:%SZ" "$note_date" "+%Y-%m-%dT%H:%M:%SZ")

  # Search for notes in the date range
  spares search "created_at<=$note_date and created_at>=$start_date"
}
```

## Visualizations

Print tags as a tree:
```sh
spares list tag --limit=9999 --tree
```

Print notes as graph:
```sh
spares list note --limit=9999 --graph
```

## Syncing notes between sources

Available sources:
1. spares
2. spares-local-files
3. anki

### Option 1: Interactive Mode
```sh
spares sync --from {source1} --to {source2} --dry-run
```
where `{source1}` and `{source2}` are from the list above.

This will walk you through syncing notes between these sources. Use `--individual` to review changes one at a time instead of in bulk.

To sync a specific note by ID:
```sh
spares sync --from spares-local-files --to spares --ids 5 12 23
```

### Option 2: Batch selection with fzf

Use `--print-files` to output changed note file paths non-interactively, then pipe them to fzf for multi-selection:

```sh
spares sync --from spares --to spares-local-files --print-files \
  | fzf -m --preview 'bat --color=always {}' \
  | xargs spares sync --from spares-local-files --to spares --files @-
```

This works the same in reverse (from spares-local-files to spares). The `--print-files` output is cache-rendered paths, and `--files` accepts them directly.

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
