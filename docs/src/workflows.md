# Workflows

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
