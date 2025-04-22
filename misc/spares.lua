-- Spares

-- Installation: Add the following to your init.lua:
-- ```lua
-- -- Spares
-- local spares_file = os.getenv("SPARES_DIR") .. '/misc/spares.lua'
-- dofile(figures_manager_file)
--- ```

-- Complete tags
function complete_tags()
  local current_line = vim.api.nvim_get_current_line()
  local cursor_col = vim.api.nvim_win_get_cursor(0)[2]
  -- Extract the current word before the cursor
  -- local current_word_start, current_word_end = current_line:sub(1, cursor_col):find("%w+$")
  -- local current_word = current_word_start and current_line:sub(current_word_start, current_word_end) or ""
  local current_word = string.match(current_line:sub(1, cursor_col), "%w+$") or ""
  local job = vim.fn.jobstart('spares_cli list tag', {
    stdout_buffered = true,
    on_stdout = function(_, data)
      if data then
        -- Clean up the output and remove empty entries
        local tags = {}
        for _, tag in ipairs(data) do
          if tag ~= "" and tag:find(current_word, 1, true) then
            table.insert(tags, tag)
          end
        end

        -- Insert the filtered tags as completion items
        local row, col = unpack(vim.api.nvim_win_get_cursor(0))
        -- local start_col = current_word_start and current_word_start - 1 or col
        -- vim.fn.complete(start_col, tags)
        vim.fn.complete(col + 1, tags)
      end
    end,
    on_stderr = function(_, data)
      if data then
        vim.notify("Error running spares_cli: " .. table.concat(data, "\n"), vim.log.levels.ERROR)
      end
    end,
  })
  if job <= 0 then
    vim.notify("Failed to start spares_cli job", vim.log.levels.ERROR)
  end
end
