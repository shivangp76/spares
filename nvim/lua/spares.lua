local M = {}

function M.setup(opts)
  opts = opts or {}
  if opts.binary then vim.g.spares_binary = opts.binary end
  if opts.notes_dir then vim.g.spares_notes_dir = opts.notes_dir end
end

-- Syntax of each spares parser, keyed by parser name.
-- `marker` is the parser's `spares: start` comment: a file must contain it when
-- opened to opt into keyword completion. `blocks` are the delimiters completion
-- triggers inside; `suffix` (markdown reference links) is a pattern that must
-- follow the closing delimiter for the block to count as a note reference.
local parsers = {
  markdown = {
    ext = 'md',
    filetypes = { 'markdown', 'pandoc' },
    marker = '<!--- spares: start --->',
    blocks = {
      { prefix = '[', open = '[', close = ']', suffix = '^%[li' },
    },
  },
  ['latex-note'] = {
    ext = 'tex',
    filetypes = { 'tex', 'latex', 'plaintex' },
    marker = '% spares: start',
    blocks = {
      { prefix = '\\key{', open = '{', close = '}' },
      { prefix = '\\li{', open = '{', close = '}' },
    },
  },
  typst = {
    ext = 'typ',
    filetypes = { 'typst' },
    marker = '// spares: start',
    blocks = {
      { prefix = '#key[', open = '[', close = ']' },
      { prefix = '#lin[', open = '[', close = ']' },
    },
  },
}

local parser_by_filetype = {}
local parser_by_ext = {}
for name, parser in pairs(parsers) do
  parser_by_ext[parser.ext] = name
  for _, ft in ipairs(parser.filetypes) do
    parser_by_filetype[ft] = name
  end
end

local function find_binary()
  local g = vim.g.spares_binary
  if g and type(g) == 'string' and g ~= '' then
    return vim.fn.expand(g)
  end
  if vim.fn.executable('spares') == 1 then
    return 'spares'
  end
  local debug = '/Users/shivang/spares/target/debug/spares'
  if vim.fn.filereadable(debug) == 1 then
    return debug
  end
  return nil
end

local function find_notes_dir()
  local g = vim.g.spares_notes_dir
  if g and type(g) == 'string' and g ~= '' then
    return vim.fn.expand(g)
  end
  return (vim.env.HOME or vim.fn.expand('~')) .. '/.local/share/spares/notes'
end

local function get_keyword_under_cursor()
  local cursor = vim.fn.getpos('.')
  local cur_lnum, cur_1col = cursor[2], cursor[3]
  if cur_lnum < 1 then return nil end

  local lines = vim.api.nvim_buf_get_lines(0, 0, -1, false)
  local bufstr = table.concat(lines, '\n')

  local line_off = {}
  local pos = 0
  for i = 1, #lines do
    line_off[i] = pos
    pos = pos + #lines[i] + 1
  end

  local cur_1idx = (line_off[cur_lnum] or 0) + cur_1col

  local search = 1
  while true do
    local s = bufstr:find('#lin%[', search)
    if not s then break end

    local depth = 1
    local i = s + 5
    while i <= #bufstr and depth > 0 do
      local ch = bufstr:sub(i, i)
      if ch == '[' then
        depth = depth + 1
      elseif ch == ']' then
        depth = depth - 1
      end
      i = i + 1
    end
    if depth > 0 then break end

    local kw_start = s + 5
    local kw_end = i - 2
    if cur_1idx >= kw_start and cur_1idx <= kw_end then
      return bufstr:sub(kw_start, kw_end)
    end
    search = i
  end

  return nil
end

local function run(args)
  local binary = find_binary()
  if not binary then return nil end
  local out = vim.fn.system({ binary, unpack(args) })
  if vim.v.shell_error ~= 0 then return nil end
  if not out or out == '' then return nil end
  local trimmed = out:match('^%s*(.-)%s*$')
  if trimmed == '' or trimmed == 'null' then return nil end
  return trimmed
end

local function resolve_path(note_id)
  local raw = run({ 'note', 'get', tostring(note_id) })
  if not raw then return nil end
  local ok, note = pcall(vim.fn.json_decode, raw)
  if not ok or type(note) ~= 'table' or not note.parser_id then return nil end

  local raw2 = run({ 'parser', 'list' })
  if not raw2 then return nil end
  local ok2, parsers = pcall(vim.fn.json_decode, raw2)
  if not ok2 or type(parsers) ~= 'table' then return nil end

  local pname
  for _, p in ipairs(parsers) do
    if type(p) == 'table' and p.id == note.parser_id then
      pname = p.name
      break
    end
  end
  if not pname then return nil end

  local ext = parsers[pname] and parsers[pname].ext or pname
  local notes_dir = find_notes_dir():gsub('/+$', '')
  return string.format('%s/%s/%04d.%s', notes_dir, pname, note_id, ext)
end

local function find_line(path, keyword)
  local out = vim.fn.system({ 'rg', '-F', '-n', '--', keyword, path })
  if vim.v.shell_error == 0 and out and out ~= '' then
    return tonumber(out:match('^(%d+):'))
  end
  return nil
end

function M.open()
  local binary = find_binary()
  if not binary then
    vim.notify(
      'spares: binary not found — set vim.g.spares_binary or add spares to PATH',
      vim.log.levels.ERROR
    )
    return
  end

  local kw = get_keyword_under_cursor()
  if not kw then
    vim.notify('spares: no #lin[...] enclosing cursor', vim.log.levels.WARN)
    return
  end
  kw = kw:match('^%s*(.-)%s*$')
  if not kw or kw == '' then
    vim.notify('spares: empty keyword in #key[]', vim.log.levels.WARN)
    return
  end

  local raw = run({ 'keyword', 'search', kw })
  if not raw then
    vim.notify('spares: no match for "' .. kw .. '"', vim.log.levels.WARN)
    return
  end

  local ok, res = pcall(vim.fn.json_decode, raw)
  if not ok or type(res) ~= 'table' or not res.matched_keyword or not res.note_id then
    vim.notify(
      'spares: unexpected search result for "' .. kw .. '"',
      vim.log.levels.ERROR
    )
    return
  end

  local path = resolve_path(res.note_id)
  if not path then
    vim.notify(
      'spares: could not resolve file path for note ' .. res.note_id,
      vim.log.levels.ERROR
    )
    return
  end
  if vim.fn.filereadable(path) == 0 then
    vim.notify('spares: note file not found: ' .. path, vim.log.levels.ERROR)
    return
  end

  local lineno = find_line(path, res.matched_keyword)
  vim.cmd('edit ' .. vim.fn.fnameescape(path))
  if lineno then
    vim.api.nvim_win_set_cursor(0, { lineno, 0 })
    vim.cmd('normal! zz')
  end
end

function M.complete_tags()
  local binary = find_binary()
  if not binary then
    vim.notify(
      'spares: binary not found — set vim.g.spares_binary or add spares to PATH',
      vim.log.levels.ERROR
    )
    return
  end

  -- Tag names are kebab/colon separated (e.g. `physics:e-and-m`), so the
  -- partial word cannot stop at `%w`: doing so would leave the already-typed
  -- text in place and the completion would be appended to it.
  local current_line = vim.api.nvim_get_current_line()
  local cursor_col = vim.api.nvim_win_get_cursor(0)[2]
  local current_word = current_line:sub(1, cursor_col):match('[%w%-_:%.]+$') or ''
  -- 1-indexed column of the first character of the partial; `vim.fn.complete`
  -- replaces from here to the cursor.
  local start_col = cursor_col - #current_word + 1

  local job = vim.fn.jobstart({ binary, 'tag', 'list', '--short' }, {
    stdout_buffered = true,
    on_stdout = function(_, data)
      if data then
        local tags = {}
        for _, tag in ipairs(data) do
          if tag ~= '' and tag:find(current_word, 1, true) then
            table.insert(tags, tag)
          end
        end
        if vim.api.nvim_get_mode().mode ~= 'i' then return end
        vim.fn.complete(start_col, tags)
      end
    end,
    on_stderr = function(_, data)
      local msg = data and table.concat(data, '\n'):match('^%s*(.-)%s*$') or ''
      if msg ~= '' then
        vim.notify('Error running spares: ' .. msg, vim.log.levels.ERROR)
      end
    end,
  })
  if job <= 0 then
    vim.notify('Failed to start spares job', vim.log.levels.ERROR)
  end
end

-- Keyword completion ---------------------------------------------------------

M._keywords = nil
M._keywords_loading = false
M._last_event_id = nil
M._event_job = nil
-- Set when a completion request arrives before the keyword cache has loaded.
-- The cache loads asynchronously, so without this the characters typed during
-- that first load produce no popup at all.
M._pending_complete = false

-- Replay a completion request that was dropped while the cache was loading.
local function replay_pending_complete()
  if not M._pending_complete then return end
  M._pending_complete = false
  vim.schedule(function()
    if vim.api.nvim_get_mode().mode ~= 'i' then return end
    M.complete_keywords({ auto = true })
  end)
end

function M.refresh_keywords()
  local binary = find_binary()
  if not binary then
    vim.notify(
      'spares: binary not found — set vim.g.spares_binary or add spares to PATH',
      vim.log.levels.ERROR
    )
    return
  end

  if M._keywords_loading then return end
  M._keywords_loading = true

  local job = vim.fn.jobstart({ binary, 'keyword', 'list', '--short' }, {
    stdout_buffered = true,
    on_stdout = function(_, data)
      if data then
        local keywords = {}
        for _, kw in ipairs(data) do
          if kw ~= '' then
            table.insert(keywords, kw)
          end
        end
        M._keywords = keywords
      end
    end,
    on_stderr = function(_, data)
      local msg = data and table.concat(data, '\n') or ''
      if msg ~= '' then
        vim.notify('Error loading spares keywords: ' .. msg, vim.log.levels.ERROR)
      end
    end,
    on_exit = function()
      M._keywords_loading = false
      if M._keywords then
        replay_pending_complete()
      else
        M._pending_complete = false
      end
    end,
  })
  if job <= 0 then
    M._keywords_loading = false
    M._pending_complete = false
    vim.notify('Failed to start spares job for keywords', vim.log.levels.ERROR)
  end
end

-- Returns (start_col, partial) when the cursor is inside a keyword block of the
-- given parser on the current line. start_col is the 1-indexed column of the
-- first character after the opening delimiter. partial is the text typed so far.
-- Only scans the current line (single-line blocks), so multi-line references
-- get completion only on the first line. The manual <Plug> is the fallback.
local function get_keyword_block(parser)
  local line = vim.api.nvim_get_current_line()
  local cur_0col = vim.api.nvim_win_get_cursor(0)[2]
  if cur_0col < 1 then return nil end

  local best_start = nil
  for _, block in ipairs(parser.blocks) do
    local i = 1
    while true do
      local s = line:find(block.prefix, i, true)
      if not s then break end

      local content_start = s + #block.prefix

      local depth = 1
      local j = content_start
      while j <= #line and depth > 0 do
        local ch = line:sub(j, j)
        if ch == block.open then
          depth = depth + 1
        elseif ch == block.close then
          depth = depth - 1
        end
        j = j + 1
      end

      local inside
      if depth > 0 then
        -- Unclosed on this line; the cursor is inside if past the delimiter.
        -- A block needing a suffix cannot be recognized until it is closed.
        inside = not block.suffix and cur_0col + 1 >= content_start
      else
        local close_1 = j - 1
        inside = (cur_0col + 1) >= content_start
          and (cur_0col + 1) <= close_1
          and (not block.suffix or line:sub(close_1 + 1):find(block.suffix) ~= nil)
      end

      -- Keep the innermost match, whichever block type it came from.
      if inside and (not best_start or content_start > best_start) then
        best_start = content_start
      end

      i = s + 1
    end
  end

  if not best_start then return nil end

  local partial = line:sub(best_start, cur_0col)
  return best_start, partial
end

-- Keyword completion is opt-in per file: the buffer must have contained its
-- parser's `spares: start` comment when it was opened.
local function buffer_parser()
  -- Detect on demand. When the plugin is lazy-loaded by its own keymap, the
  -- BufReadPost/FileType autocmds are registered only once the key is pressed,
  -- which is too late to have seen the buffer that triggered the load.
  if not vim.b.spares_detected then M.detect_buffer() end
  local name = vim.b.spares_parser
  return name and parsers[name] or nil
end

local function parser_name_for_buf(bufnr)
  local name = parser_by_filetype[vim.bo[bufnr].filetype]
  if name then return name end
  local ext = vim.fn.fnamemodify(vim.api.nvim_buf_get_name(bufnr), ':e')
  return parser_by_ext[ext]
end

function M.detect_buffer(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_loaded(bufnr) then return end

  local name = parser_name_for_buf(bufnr)
  local enabled = false
  if name then
    local marker = parsers[name].marker
    for _, line in ipairs(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)) do
      if line:find(marker, 1, true) then
        enabled = true
        break
      end
    end
  end

  vim.b[bufnr].spares_parser = enabled and name or nil
  vim.b[bufnr].spares_detected = true

  -- Warm the cache as soon as a participating file is opened, so the first
  -- keyword block typed in it completes immediately.
  if enabled and not M._keywords then
    M.check_event_id()
  end
end

function M.has_enabled_buffer()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(buf) and vim.b[buf].spares_parser then
      return true
    end
  end
  return false
end

function M.check_event_id()
  local binary = find_binary()
  if not binary then return end
  if M._event_job then return end

  M._event_job = vim.fn.jobstart({ binary, 'event', 'latest' }, {
    stdout_buffered = true,
    on_stdout = function(_, data)
      if not data then return end
      local combined = table.concat(data, ''):match('^%s*(%d+)%s*$')
      if not combined then return end
      local latest = tonumber(combined)
      if not latest then return end
      -- Also refresh when the cache is empty: an earlier load may have failed
      -- after the event id was already recorded, which would otherwise wedge
      -- completion off until the next note mutation.
      if latest ~= M._last_event_id or not M._keywords then
        M._last_event_id = latest
        M.refresh_keywords()
      end
    end,
    on_exit = function()
      M._event_job = nil
      if not M._keywords_loading and not M._keywords then
        M._pending_complete = false
      end
    end,
  })
  if M._event_job <= 0 then
    M._event_job = nil
    M._pending_complete = false
  end
end

function M.complete_keywords(opts)
  local parser = buffer_parser()
  if not parser then
    if not (opts and opts.auto) then
      vim.notify(
        'spares: keyword completion is off for this buffer — add the parser\'s '
          .. '"spares: start" comment and reopen the file',
        vim.log.levels.WARN
      )
    end
    return
  end

  if not M._keywords then
    M._pending_complete = true
    M.check_event_id()
    return
  end

  if vim.api.nvim_get_mode().mode ~= 'i' then return end

  local start_col, partial = get_keyword_block(parser)
  if not start_col or not partial or partial == '' then return end

  local matches = vim.fn.matchfuzzy(M._keywords, partial)
  if #matches > 50 then
    matches = { unpack(matches, 1, 50) }
  end
  if #matches > 0 then
    vim.fn.complete(start_col, matches)
  end
end

return M
