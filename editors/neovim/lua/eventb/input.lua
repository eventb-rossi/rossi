-- Editor-side Unicode input method for Event-B (Neovim port).
--
-- Converts ASCII operator combos to Unicode as the user types -- the layer the
-- language server deliberately does NOT own (per-keystroke substitution must be
-- synchronous, local, and undo/cursor aware). The matching logic is a faithful
-- port of the VS Code `symbolMatcher.ts` module; this file is the Neovim glue
-- (the `symbolInput.ts` analogue), driven by an `nvim_buf_attach` `on_bytes`
-- listener. `on_bytes` fires AFTER the inserted char lands (and, unlike
-- `InsertCharPre`, is not under textlock, so it can edit the buffer), which
-- matches the post-insertion model of the VS Code `onDidChangeTextDocument`
-- handler exactly -- the same offset arithmetic carries over directly.
--
-- The operator data is the canonical generated table (`eventb.operators`,
-- itself emitted from `eventb_lsp::server::operator_rows()`), so the
-- ASCII<->Unicode mapping is never duplicated here and can never disagree with
-- the LSP `rossi/operatorTable` request that the VS Code extension consumes.
--
-- Two trigger modes (both on by default, per the pin-defaults convention):
--   - eager: symbolic combos (`=>`, `|->`, `<=>`) convert via maximal munch
--     with a one-character lookahead; alphabetic ops (`NAT`, `or`, `dom`) are
--     excluded so converting them eagerly cannot block typing ordinary words.
--   - leader: a `\name` abbreviation (`\and`, `\to`, `\nat`) expands to Unicode
--     on a boundary character. The leader is `\` (pinned, matching Lean/Agda);
--     it is reserved and can never start an eager run.
--
-- v1 scope: single primary cursor, Insert mode typing. Paste / multi-cursor /
-- programmatic edits reset state and never corrupt the buffer. Conversion
-- happens everywhere, including comments (matches Lean); comment-awareness is a
-- future refinement.

local M = {}

--------------------------------------------------------------------------------
-- Pure matcher (port of symbolMatcher.ts)
--------------------------------------------------------------------------------

--- The leader character that begins a `\name` abbreviation.
local LEADER = "\\"

--- A character that can appear inside a leader name (`\forall`, `\nat1`).
local function is_name_char(ch)
  return ch:match("^[A-Za-z0-9]$") ~= nil
end

-- SymbolMatcher, built once from the operator rows.
--   exact_by_ascii : eager op ASCII -> Unicode (O(1) exact-match lookup)
--   prefixes       : every strict prefix of an eager op ASCII (O(1) extend)
--   leader         : leader name -> Unicode (aliases + alphabetic ASCII words)
--   max_leader_len : longest leader-name length (sizes the `\name` lookback)
local Matcher = {}
Matcher.__index = Matcher

function Matcher.new(rows)
  local self = setmetatable({
    exact_by_ascii = {},
    prefixes = {},
    leader = {},
    max_leader_len = 0,
  }, Matcher)
  for _, r in ipairs(rows) do
    if r.eager then
      self.exact_by_ascii[r.ascii] = r.unicode
      for i = 1, #r.ascii - 1 do
        self.prefixes[r.ascii:sub(1, i)] = true
      end
    end
    for _, alias in ipairs(r.aliases) do
      self.leader[alias] = r.unicode
    end
    -- Alphabetic ASCII (NAT, or, POW, UNION, ...) doubles as a leader name so
    -- `\NAT` / `\or` work alongside the curated aliases.
    if not r.symbolic then
      self.leader[r.ascii] = r.unicode
    end
  end
  for name in pairs(self.leader) do
    if #name > self.max_leader_len then
      self.max_leader_len = #name
    end
  end
  return self
end

--- True if some eager op is strictly longer than `s` and starts with `s`.
function Matcher:can_extend(s)
  return self.prefixes[s] == true
end

--- The Unicode glyph if `s` is exactly an eager op, else nil.
function Matcher:exact(s)
  return self.exact_by_ascii[s]
end

--- True if `s` could begin or be an eager op (worth holding as a run).
function Matcher:is_eager_start(s)
  return self.prefixes[s] == true or self.exact_by_ascii[s] ~= nil
end

--- The Unicode glyph for a leader name (`forall`, `nat`, `to`), else nil.
function Matcher:resolve_leader(name)
  return self.leader[name]
end

-- The decision for one typed character in the eager state machine, expressed
-- relative to the cursor (mirrors EagerAction in symbolMatcher.ts):
--   { type = "wait",  pending = <str> }              -- grow the run; no edit
--   { type = "reset", pending = <str> }              -- start fresh; no edit
--   { type = "convertWithChar", unicode = <str> }    -- char completed an op
--   { type = "convertHeld", unicode = <str>, held_len = <n> } -- char broke one

--- Advance the eager state machine by one typed character `ch`, given the
--- current uncommitted `pending` run. Pure: returns the action to apply.
local function step_eager(matcher, pending, ch)
  -- The leader char is reserved; it never participates in an eager run.
  if ch == LEADER then
    return { type = "reset", pending = "" }
  end

  local candidate = pending .. ch

  if matcher:can_extend(candidate) then
    return { type = "wait", pending = candidate }
  end

  local exact_candidate = matcher:exact(candidate)
  if exact_candidate ~= nil then
    return { type = "convertWithChar", unicode = exact_candidate }
  end

  -- `candidate` cannot extend and is not itself an op. If the run we were
  -- holding (without `ch`) was a complete op, `ch` is its boundary: convert.
  local held_glyph = matcher:exact(pending)
  if held_glyph ~= nil then
    return { type = "convertHeld", unicode = held_glyph, held_len = #pending }
  end

  -- Otherwise restart, seeding the new run with `ch` if it could begin an op.
  return { type = "reset", pending = matcher:is_eager_start(ch) and ch or "" }
end

--- Find a `\name` leader token ending exactly at the end of `text` (i.e.
--- immediately before the cursor). Returns { name = <str>, start = <0-based> }
--- or nil. Used both for committing on a boundary and for the underline decor.
local function leader_token_before(text)
  -- Mirrors /\\([A-Za-z][A-Za-z0-9]*)$/ : a backslash, an alphabetic first
  -- char, then name chars, anchored at the end.
  local name = text:match("\\([A-Za-z][A-Za-z0-9]*)$")
  if not name then
    return nil
  end
  -- The backslash sits one char before the name; `start` is its 0-based offset.
  return { name = name, start = #text - #name - 1 }
end

--- Find the in-progress leader prefix ending at the cursor: a backslash plus
--- zero or more name chars (so a lone `\` matches too). Returns
--- { start = <0-based>, length = <n> } or nil. Used for the underline decor.
local function leader_prefix_before(text)
  -- Mirrors /\\[A-Za-z0-9]*$/.
  local m = text:match("\\[A-Za-z0-9]*$")
  if not m then
    return nil
  end
  return { start = #text - #m, length = #m }
end

-- Exposed for the headless spec (the simulateEagerTyping oracle) and for any
-- callers that want the pure layer without the editor glue.
M._Matcher = Matcher
M._step_eager = step_eager
M._leader_token_before = leader_token_before
M._leader_prefix_before = leader_prefix_before
M._is_name_char = is_name_char
M._LEADER = LEADER

--------------------------------------------------------------------------------
-- Editor glue (port of symbolInput.ts)
--------------------------------------------------------------------------------

local api = vim.api

-- Configuration, mirroring `rossi.input.enabled` / `rossi.input.eager`. Both
-- default on (pin-defaults convention).
local config = {
  enabled = true,
  eager = true,
}

-- The shared matcher, built lazily from the generated operator table.
local matcher = nil

-- The augroup owning all our autocommands (recreated by setup()).
local augroup = nil

-- Buffers we have already attached an `on_bytes` listener to (the listener
-- detaches itself when the buffer unloads), keyed by bufnr.
local attached = {}

-- An extmark namespace for the optional `\name` underline decoration.
local ns = api.nvim_create_namespace("eventb_input_leader")

-- The single open `\name` underline extmark per buffer, keyed by bufnr, so a
-- refresh deletes just that mark by id instead of clearing the whole namespace
-- on every cursor move. nil/absent when nothing is underlined.
local deco_mark = {}

-- Per-buffer eager run state, keyed by bufnr. Each entry is the uncommitted
-- ASCII run sitting in the document just before the cursor:
--   { text = <str>, row = <0-based>, col = <0-based byte col just after run> }
-- `row`/`col` is the buffer position recorded right after the run was extended;
-- the next keystroke keeps the run only if the new insertion is exactly there
-- (the contiguity check, mirroring symbolInput.ts pendingRun.end).
local pending = {}

--- True while we apply our own edit, so the resulting `on_bytes` notification
--- is ignored (the `applying` guard from symbolInput.ts).
local applying = false

local function get_matcher()
  if matcher == nil then
    local ok, ops = pcall(require, "eventb.operators")
    if not ok then
      return nil
    end
    matcher = Matcher.new(ops.rows)
  end
  return matcher
end

local function reset_pending(bufnr)
  pending[bufnr] = nil
end

--- Lookback window for `\name` scanning: the backslash plus the longest leader
--- name. Only meaningful once the operator table (and matcher) has loaded.
local function leader_lookback(m)
  return m.max_leader_len + 1
end

--- The byte text of line `row` up to (but not including) byte column `col`.
local function line_before(bufnr, row, col)
  local line = api.nvim_buf_get_lines(bufnr, row, row + 1, false)[1] or ""
  return line:sub(1, col)
end

--- Replace the byte range [start_col, end_col) on `row` with `text` as a single
--- self-contained undo step (so `u` restores the ASCII), suppressing the
--- resulting `on_bytes` via the `applying` guard. If the cursor sat at the end
--- of the replaced range, move it to the end of `text` so typing continues
--- seamlessly. `on_bytes` runs under textlock, so callers schedule this.
local function replace(bufnr, row, start_col, end_col, text)
  if not api.nvim_buf_is_valid(bufnr) then
    return false
  end
  applying = true
  local ok = pcall(api.nvim_buf_set_text, bufnr, row, start_col, row, end_col, { text })
  applying = false
  if not ok then
    return false
  end
  -- Keep the cursor glued to the end of the substitution when it was there.
  local win = api.nvim_get_current_win()
  if api.nvim_win_get_buf(win) == bufnr then
    local cur = api.nvim_win_get_cursor(win)
    if cur[1] - 1 == row and cur[2] == end_col then
      api.nvim_win_set_cursor(win, { row + 1, start_col + #text })
    end
  end
  return true
end

--- Resolve and replace a `\name` ending at byte column `col` on `row`.
--- `before` is line text up to `col`. Returns true if an abbreviation committed.
local function try_leader_commit(bufnr, m, before, row, col)
  local from = math.max(0, #before - leader_lookback(m))
  local window = before:sub(from + 1) -- Lua is 1-based; `from` is a 0-based offset
  local tok = leader_token_before(window)
  if not tok then
    return false
  end
  local glyph = m:resolve_leader(tok.name)
  if glyph == nil then
    return false
  end
  local start = from + tok.start -- 0-based byte col of the backslash
  return replace(bufnr, row, start, col, glyph)
end

--- Eager state machine for one inserted char `ch` (already in the buffer at
--- [col - 1, col) on `row`). `before` is the line text up to `col` INCLUDING
--- `ch`. `insert_col = col - 1` is where `ch` landed. Mirrors symbolInput.ts
--- handleEager: the char is already in the document, like VS Code's change
--- event, so offsets follow the same model. Schedules any buffer edit.
local function handle_eager(bufnr, m, ch, before, row, col)
  local insert_col = col - #ch -- 0-based byte col where `ch` was inserted

  -- Keep the run only if this insertion is exactly contiguous with the previous
  -- run end on the same buffer line; otherwise start fresh.
  local run = pending[bufnr]
  local cur = ""
  if run and run.row == row and run.col == insert_col then
    cur = run.text
  end

  local action = step_eager(m, cur, ch)
  if action.type == "wait" or action.type == "reset" then
    -- No edit: `ch` stays; record the run end at the post-insert cursor col.
    pending[bufnr] = { text = action.pending, row = row, col = col }
  elseif action.type == "convertWithChar" then
    -- `cur .. ch` occupies [insert_col - #cur, col); replace with the glyph.
    local start = insert_col - #cur
    reset_pending(bufnr)
    vim.schedule(function()
      replace(bufnr, row, start, col, action.unicode)
    end)
  elseif action.type == "convertHeld" then
    -- Replace the held run BEFORE `ch` ([insert_col - held_len, insert_col));
    -- keep `ch`.
    local start = insert_col - action.held_len
    reset_pending(bufnr)
    vim.schedule(function()
      replace(bufnr, row, start, insert_col, action.unicode)
    end)
  end
end

--- The `on_bytes` listener: the single entry point that drives both modes,
--- mirroring symbolInput.ts onChange. Only a single-byte ASCII insertion on one
--- line drives input; anything else (paste, deletion, multi-line, Unicode)
--- resets the run. Returns true to detach when the buffer is gone.
local function on_bytes(bufnr, start_row, start_col, old_end_row, old_end_col, new_end_row, new_end_col)
  if applying or not config.enabled then
    return
  end
  if not api.nvim_buf_is_valid(bufnr) then
    return true -- detach
  end
  local m = get_matcher()
  if not m then
    return
  end

  -- Single-character insertion: no deletion, exactly one new byte on one line.
  local single_insert =
    old_end_row == 0 and old_end_col == 0
    and new_end_row == 0 and new_end_col == 1
  if not single_insert then
    reset_pending(bufnr)
    return
  end

  local row = start_row
  -- The inserted byte landed at [start_col, start_col + 1); the cursor is now
  -- just after it. `before` includes the just-inserted char (post-insertion
  -- model, exactly like the VS Code change event).
  local col = start_col + 1
  local before = line_before(bufnr, row, col)
  local ch = before:sub(col, col)
  -- Guard against a non-ASCII inserted byte (e.g. a multibyte glyph fragment).
  if #ch ~= 1 or ch:match("^[\32-\126]$") == nil then
    reset_pending(bufnr)
    return
  end

  -- 1) Leader commit: a boundary char (not a name char, not the leader) right
  --    after a resolvable `\name`. The boundary `ch` stays after the glyph.
  if not is_name_char(ch) and ch ~= LEADER then
    -- The `\name` precedes `ch`, i.e. ends at `insert_col = col - 1`.
    local insert_col = col - 1
    local lead_before = before:sub(1, insert_col)
    if config.enabled then
      local from = math.max(0, #lead_before - leader_lookback(m))
      local window = lead_before:sub(from + 1)
      local tok = leader_token_before(window)
      if tok and m:resolve_leader(tok.name) ~= nil then
        reset_pending(bufnr)
        vim.schedule(function()
          try_leader_commit(bufnr, m, line_before(bufnr, row, insert_col), row, insert_col)
        end)
        return
      end
    end
  end

  -- 2) Eager substitution.
  if config.eager then
    handle_eager(bufnr, m, ch, before, row, col)
  else
    reset_pending(bufnr)
  end
end

--- Refresh the `\name` underline extmark on the cursor line of `bufnr`.
--- Optional polish; a no-op when input is disabled or no abbreviation is open.
local function update_decoration(bufnr)
  -- Drop the previous underline (if any) by id -- O(1), no buffer-wide scan.
  local prev = deco_mark[bufnr]
  if prev then
    api.nvim_buf_del_extmark(bufnr, ns, prev)
    deco_mark[bufnr] = nil
  end
  if not config.enabled then
    return
  end
  local m = get_matcher()
  if not m then
    return
  end
  local win = api.nvim_get_current_win()
  if api.nvim_win_get_buf(win) ~= bufnr then
    return
  end
  local cursor = api.nvim_win_get_cursor(win) -- { 1-based row, 0-based col }
  local row = cursor[1] - 1
  local col = cursor[2]
  local before = line_before(bufnr, row, col)
  local from = math.max(0, #before - leader_lookback(m))
  local window = before:sub(from + 1)
  local prefix = leader_prefix_before(window)
  if not prefix then
    return
  end
  local start = col - prefix.length
  deco_mark[bufnr] = api.nvim_buf_set_extmark(bufnr, ns, row, start, {
    end_row = row,
    end_col = col,
    hl_group = "Underlined",
  })
end

--- Enable the input method for `bufnr` (defaults to the current buffer).
--- Idempotent: attaches the `on_bytes` listener once per buffer and refreshes
--- the buffer-local decoration autocommands.
function M.enable(bufnr)
  bufnr = bufnr or api.nvim_get_current_buf()
  if augroup == nil then
    augroup = api.nvim_create_augroup("EventBInput", { clear = true })
  end
  -- Clear any previous buffer-local autocommands for this buffer in our group.
  api.nvim_clear_autocmds({ group = augroup, buffer = bufnr })

  -- Attach the byte listener once. It stays attached for the buffer's life;
  -- `config.enabled` gates it dynamically so toggling the setting needs no
  -- reattach, and there is no way to detach a stale closure twice.
  if not attached[bufnr] then
    attached[bufnr] = true
    api.nvim_buf_attach(bufnr, false, {
      on_bytes = function(_, buf, _changedtick, start_row, start_col, _byte_offset,
                          old_end_row, old_end_col, _old_end_byte,
                          new_end_row, new_end_col, _new_end_byte)
        return on_bytes(buf, start_row, start_col, old_end_row, old_end_col, new_end_row, new_end_col)
      end,
      on_detach = function(_, buf)
        attached[buf] = nil
        pending[buf] = nil
      end,
    })
  end

  -- Refresh the leader underline on cursor moves and on leaving insert mode,
  -- and drop the pending eager run when insert mode ends.
  api.nvim_create_autocmd({ "CursorMovedI", "InsertEnter" }, {
    group = augroup,
    buffer = bufnr,
    desc = "Event-B: refresh leader-abbreviation underline",
    callback = function()
      update_decoration(bufnr)
    end,
  })

  api.nvim_create_autocmd({ "InsertLeave", "BufLeave" }, {
    group = augroup,
    buffer = bufnr,
    desc = "Event-B: drop the pending eager run and clear decoration",
    callback = function()
      reset_pending(bufnr)
      api.nvim_buf_clear_namespace(bufnr, ns, 0, -1)
    end,
  })
end

--- Disable the input method for `bufnr` (defaults to the current buffer).
--- The `on_bytes` listener stays attached but goes inert (it re-checks
--- `config.enabled` and the buffer's filetype is unchanged); buffer-local
--- decoration autocommands are removed.
function M.disable(bufnr)
  bufnr = bufnr or api.nvim_get_current_buf()
  if augroup ~= nil then
    api.nvim_clear_autocmds({ group = augroup, buffer = bufnr })
  end
  reset_pending(bufnr)
  api.nvim_buf_clear_namespace(bufnr, ns, 0, -1)
end

--- Configure the input method and auto-enable it for Event-B buffers.
--- `opts` mirrors the VS Code `rossi.input` config:
---   { enabled = <bool>, eager = <bool> }   (both default true)
function M.setup(opts)
  opts = opts or {}
  if opts.enabled ~= nil then
    config.enabled = opts.enabled
  end
  if opts.eager ~= nil then
    config.eager = opts.eager
  end

  augroup = api.nvim_create_augroup("EventBInput", { clear = true })

  -- Auto-enable for every Event-B buffer (now and in the future).
  api.nvim_create_autocmd("FileType", {
    group = augroup,
    pattern = "eventb",
    desc = "Event-B: enable ASCII -> Unicode input",
    callback = function(ev)
      M.enable(ev.buf)
    end,
  })

  -- Cover any Event-B buffers already open when setup() runs.
  if config.enabled then
    for _, buf in ipairs(api.nvim_list_bufs()) do
      if api.nvim_buf_is_loaded(buf) and vim.bo[buf].filetype == "eventb" then
        M.enable(buf)
      end
    end
  end
end

return M
