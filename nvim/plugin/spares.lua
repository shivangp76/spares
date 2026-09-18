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

-- Completion is opt-in: a file participates only if it contained its parser's
-- `spares: start` comment when it was opened.
vim.api.nvim_create_autocmd({ 'BufReadPost', 'FileType' }, {
  group = augroup,
  callback = function(args)
    require('spares').detect_buffer(args.buf)
  end,
})

-- Detect buffers that were already open when the plugin loaded: under
-- lazy-loading the autocmd above is registered after BufReadPost/FileType have
-- already fired for them.
for _, buf in ipairs(vim.api.nvim_list_bufs()) do
  if vim.api.nvim_buf_is_loaded(buf) then
    require('spares').detect_buffer(buf)
  end
end

vim.api.nvim_create_autocmd('InsertCharPre', {
  group = augroup,
  callback = function()
    if not vim.b.spares_parser then return end
    local c = vim.v.char
    if not c or c == '' then return end
    if not (c:match('[%w\']') or c == ' ') then return end
    vim.schedule(function()
      require('spares').complete_keywords({ auto = true })
    end)
  end,
})

-- Refresh keyword cache when note mutations are detected (poll every 2 min).
-- Spares notes are not updated frequently, so a long poll interval is fine.
local function poll_event_id()
  local ok, spares = pcall(require, 'spares')
  if ok and spares.has_enabled_buffer() then
    spares.check_event_id()
  end
end

local timer = vim.uv.new_timer()
timer:start(120000, 120000, vim.schedule_wrap(poll_event_id))
