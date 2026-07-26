Example setup with Lazy.nvim:
```lua
{
  name = "spares",
  dir = os.getenv("SPARES_DIR") .. "/nvim",
  cmd = "SparesKeyword",
  keys = {
    { "<localleader>sk", function() require("spares").open() end,          desc = "Spares: Open spares note" },
    { "<C-x><C-t>",      function() require("spares").complete_tags() end, desc = "Spares: Complete tag",    mode = "i" },
  },
},
```
