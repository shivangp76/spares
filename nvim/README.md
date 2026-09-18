Example setup with Lazy.nvim:
```lua
{
  name = "spares",
  dir = os.getenv("SPARES_DIR") .. "/nvim",
  cmd = "SparesKeyword",
  -- `ft` matters: keyword completion only runs in buffers the plugin has seen
  -- opened. Without it the plugin stays unloaded until you press one of the
  -- keys below, and completion-as-you-type never starts in that file.
  ft = { "markdown", "pandoc", "tex", "latex", "plaintex", "typst" },
  keys = {
    { "<localleader>sk", function() require("spares").open() end,              desc = "Spares: Open spares note" },
    { "<C-x><C-t>",      function() require("spares").complete_tags() end,     desc = "Spares: Complete tag",     mode = "i" },
    { "<C-x><C-k>",      function() require("spares").complete_keywords() end, desc = "Spares: Complete keyword", mode = "i" },
  },
},
```

## Keyword completion

Keyword completion is opt-in per file: it only runs if the file contained
its parser's `spares: start` comment when it was opened. Add the comment
and reopen the file to enable it.

| Parser       | Filetypes            | Opt-in comment             | Completes inside      |
| ------------ | -------------------- | -------------------------- | --------------------- |
| `markdown`   | markdown, pandoc     | `<!--- spares: start --->` | `[...][li]`           |
| `latex-note` | tex, latex, plaintex | `% spares: start`          | `\key{...}`, `\li{...}` |
| `typst`      | typst                | `// spares: start`         | `#key[...]`, `#lin[...]` |

As you type inside one of those blocks, matching keywords are shown in a
popup menu. The matching uses fuzzy subsequence matching, so typing `Le`
will suggest `Levy's Continuity Theorem`.

Markdown links only complete once the `[li]` reference is present (type
`[][li]`, then move the cursor into the first pair of brackets), since a
bare `[` is not a spares note reference.

The keyword list is cached on first use and refreshed in the background
(checks for note mutations every 2 minutes). You can also manually refresh
with `:SparesRefreshKeywords` or trigger completion with
`<Plug>(spares-complete-keyword)` mapped to `<C-x><C-k>` (as shown above).
