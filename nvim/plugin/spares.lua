if vim.g.loaded_spares_plugin then return end
vim.g.loaded_spares_plugin = true

vim.api.nvim_create_user_command('SparesKeyword', function()
  require('spares').open()
end, { desc = 'Open spares note for #key[...] under cursor' })

vim.api.nvim_create_user_command('SparesCompleteTag', function()
  require('spares').complete_tags()
end, { desc = 'Complete spares tag name under cursor' })

vim.keymap.set('i', '<Plug>(spares-complete-tag)', function()
  require('spares').complete_tags()
end, { desc = 'spares: complete tag (insert mode)' })

-- Keyword completion
vim.api.nvim_create_user_command('SparesCompleteKeyword', function()
  require('spares').complete_keywords()
end, { desc = 'Complete spares keyword under cursor' })

vim.api.nvim_create_user_command('SparesRefreshKeywords', function()
  require('spares').refresh_keywords()
end, { desc = 'Refresh cached spares keywords' })

vim.keymap.set('i', '<Plug>(spares-complete-keyword)', function()
  require('spares').complete_keywords()
end, { desc = 'spares: complete keyword (insert mode)' })

local augroup = vim.api.nvim_create_augroup('SparesKeyword', { clear = true })

vim.api.nvim_create_autocmd('InsertCharPre', {
  group = augroup,
  callback = function()
    local c = vim.v.char
    if not c or c == '' then return end
    if not (c:match('[%w\']') or c == ' ') then return end
    local line = vim.api.nvim_get_current_line()
    if not (line:find('#key%[') or line:find('#lin%[')) then return end
    vim.schedule(function()
      require('spares').complete_keywords()
    end)
  end,
})

-- Refresh keyword cache when note mutations are detected (poll every 2 min).
-- Spares notes are not updated frequently, so a long poll interval is fine.
local function poll_event_id()
  local ok, spares = pcall(require, 'spares')
  if ok then
    spares.check_event_id()
  end
end

local timer = vim.uv.new_timer()
timer:start(120000, 120000, vim.schedule_wrap(poll_event_id))
