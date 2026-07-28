Example setup with Lazy.nvim:
```lua
{
  name = "spares",
  dir = os.getenv("SPARES_DIR") .. "/nvim",
  cmd = "SparesKeyword",
  keys = {
    { "<localleader>sk", function() require("spares").open() end,              desc = "Spares: Open spares note" },
    { "<C-x><C-t>",      function() require("spares").complete_tags() end,     desc = "Spares: Complete tag",     mode = "i" },
    { "<C-x><C-k>",      function() require("spares").complete_keywords() end, desc = "Spares: Complete keyword", mode = "i" },
  },
},
```

## Keyword completion

Keywords are automatically completed inside `#key[...]` and `#lin[...]`
blocks in **any** buffer (not just spares note files): as you type,
matching keywords are shown in a popup menu. The matching uses fuzzy
subsequence matching, so typing `Le` will suggest `Levy's Continuity
Theorem`.

The keyword list is cached on first use and refreshed in the background
(checks for note mutations every 2 minutes). You can also manually refresh
with `:SparesRefreshKeywords` or trigger completion with
`<Plug>(spares-complete-keyword)` mapped to `<C-x><C-k>` (as shown above).
