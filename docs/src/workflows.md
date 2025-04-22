# Workflows

## Fast note creation

Using snippets, such as through [LuaSnip](https://github.com/L3MON4D3/LuaSnip), can speed up note creation.

## Converting notes between parsers

For example,
```sh
spares_cli import --to-parser="markdown" 0001.tex 0002.tex
```

## Bulk note actions

spares ships with fzf support which can be used to perform bulk note actions, such as tagging. This selector can also be used to find all notes with a certain tag by typing `tag: .*tag1`. For more than 1 tag, see `spares_query` for advanced querying.

Examples:
```sh
spares_cli edit note --tags-to-add tag1 tag2 --files 0001.tex 0002.tex
spares_cli edit note --tags-to-remove tag1 tag2 --files 0001.tex 0002.tex
```

## Visualizations

Print tags as a tree:
```sh
spares_cli list tag --limit=9999 --tree
```

Print notes as graph:
```sh
spares_cli list note --limit=9999 --graph
```

## Syncing notes between sources

Available sources:
1. spares
2. spares-local-files
3. anki

### Option 1: Interactive Mode
```sh
spares_cli sync interactive --from {source1} --to {source2} --run
```
where `{source1}` and `{source2}` are from the list above.

This will walk you through syncing notes between these sources

### Option 2: Render Diffs
```sh
spares_cli sync render-diffs --from {source1} --to {source2}
```
where `{source1}` and `{source2}` are from the list above.

This will:
- Render notes in `/tmp/spares_cli/{from_source}/notes/{parser_name}/` and `/tmp/spares_cli/{to_source}/notes/{parser_name}/`.
- Render diffs in `/tmp/spares_cli/{from_source}/diffs/{parser_name}/`.
- Output the path to the directory containing the diffs.

A suggested workflow is to use `fzf` to select diffs from the outputted path and use `sed` to transform them into the corresponding note path. For example:
```sh
diff-selector-widget() {
  print -z "$(eval "fd --absolute-path --ignore --hidden --no-require-git --type f --type l . --exec-batch ls -t" |
    sort --reverse |
    fzf --multi \
      --prompt="sync notes> " \
      --preview 'bat --color=always {}' \
      --preview-window 'up,60%,wrap,border-bottom,+{2}+3/3,~3' \
      --bind 'enter:become:sort -u {+f1} | sed "s|/diffs/|/notes/|g" | sed "s/.diff//g" | tr "\n" " "')"
  zle accept-line
  preexec # End with beam cursor
}
zle -N diff-selector-widget
bindkey -M viins '^d' diff-selector-widget
```
Thus, the final workflow to sync from spares-local-files to spares looks like:
1. Run `cd $(spares_cli sync render-diffs --from spares --to spares-local-files) | diff-selector-widget`
1. Press `Ctrl+D`
1. Select the notes you would like to sync
1. Run `spares_cli import --adapter spares-local-files --run {FILES}` where `{FILES}` is the selected notes

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
