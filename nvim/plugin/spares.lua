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
