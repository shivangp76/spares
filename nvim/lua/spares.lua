local M = {}

function M.setup(opts)
  opts = opts or {}
  if opts.binary then vim.g.spares_binary = opts.binary end
  if opts.notes_dir then vim.g.spares_notes_dir = opts.notes_dir end
end

local parser_ext = {
  markdown       = 'md',
  ['latex-note'] = 'tex',
  typst          = 'typ',
}

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

  local ext = parser_ext[pname] or pname
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

  local current_line = vim.api.nvim_get_current_line()
  local cursor_col = vim.api.nvim_win_get_cursor(0)[2]
  local current_word = string.match(current_line:sub(1, cursor_col), '%w+$') or ''

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
        local row, col = unpack(vim.api.nvim_win_get_cursor(0))
        vim.fn.complete(col + 1, tags)
      end
    end,
    on_stderr = function(_, data)
      if data then
        vim.notify('Error running spares: ' .. table.concat(data, '\n'), vim.log.levels.ERROR)
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
      M._keywords_loading = false
    end,
    on_stderr = function(_, data)
      if data then
        vim.notify(
          'Error loading spares keywords: ' .. table.concat(data, '\n'),
          vim.log.levels.ERROR
        )
      end
      M._keywords_loading = false
    end,
  })
  if job <= 0 then
    M._keywords_loading = false
    vim.notify('Failed to start spares job for keywords', vim.log.levels.ERROR)
  end
end

-- Returns (start_col, partial) when cursor is inside a #key[...] or #lin[...]
-- block on the current line. start_col is the 1-indexed column of the first
-- character after the opening `[`. partial is the text typed so far.
-- Only scans the current line (single-line blocks), so multi-line references
-- get completion only on the first line. The manual <Plug> is the fallback.
local function get_keyword_block()
  local line = vim.api.nvim_get_current_line()
  local cur_0col = vim.api.nvim_win_get_cursor(0)[2]
  if cur_0col < 1 then return nil end

  local best_start = nil
  local i = 1
  while true do
    local s = line:find('#key%[', i) or line:find('#lin%[', i)
    if not s then break end

    local bracket_open_1 = s + 5

    local depth = 1
    local j = s + 6
    while j <= #line and depth > 0 do
      local ch = line:sub(j, j)
      if ch == '[' then
        depth = depth + 1
      elseif ch == ']' then
        depth = depth - 1
      end
      j = j + 1
    end

    if depth > 0 then
      -- unclosed on this line; cursor is inside if past the '['
      if cur_0col + 1 >= bracket_open_1 then
        best_start = bracket_open_1
      end
    else
      local close_1 = j - 1
      if (cur_0col + 1) >= bracket_open_1 and (cur_0col + 1) <= close_1 then
        best_start = bracket_open_1
      end
    end

    i = s + 1
  end

  if not best_start then return nil end

  local partial = line:sub(best_start, cur_0col)
  return best_start, partial
end

function M.check_event_id()
  local binary = find_binary()
  if not binary then return end
  if M._event_job then return end

  M._event_job = vim.fn.jobstart({ binary, 'event', 'latest' }, {
    stdout_buffered = true,
    on_stdout = function(_, data)
      M._event_job = nil
      if not data then return end
      local combined = table.concat(data, ''):match('^%s*(%d+)%s*$')
      if not combined then return end
      local latest = tonumber(combined)
      if latest and latest ~= M._last_event_id then
        M._last_event_id = latest
        M.refresh_keywords()
      end
    end,
    on_stderr = function(_, _data)
      M._event_job = nil
    end,
  })
  if M._event_job <= 0 then
    M._event_job = nil
  end
end

function M.complete_keywords()
  if not M._keywords then
    M.check_event_id()
    return
  end

  if vim.api.nvim_get_mode().mode ~= 'i' then return end

  local start_col, partial = get_keyword_block()
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
