-- Headless unit tests for the Event-B Neovim input method.
--
-- Run with Neovim:
--   nvim --headless --noplugin -u NONE -l editors/neovim/test/input_spec.lua
-- or with any Lua interpreter (luajit / lua5.1+); a minimal `vim` shim is
-- installed below when running outside Neovim, since the pure matcher layer
-- this spec exercises only touches a couple of `vim.*` helpers.
--
-- These cases are a faithful translation of editors/vscode/src/symbolMatcher
-- .test.ts. They drive the ported `step_eager` through the SAME oracle as the
-- TS `simulateEagerTyping` (including that a `convertHeld` boundary clears the
-- run rather than re-seeding), so this is the reference oracle for the Lua port.
-- Exits non-zero on the first batch of failures so it can gate CI / pre-commit.

-- Resolve the module path relative to this script so the spec runs from any cwd.
local function script_dir()
  local src = debug.getinfo(1, "S").source:gsub("^@", "")
  return src:match("^(.*)/[^/]*$") or "."
end
local dir = script_dir()
-- .../editors/neovim/test -> .../editors/neovim/lua
package.path = dir .. "/../lua/?.lua;" .. package.path

-- Minimal `vim` shim for running under a bare Lua interpreter. Only the symbols
-- touched while *requiring* eventb.input are needed; the editor glue functions
-- (enable/setup/handlers) are not invoked by this pure spec.
if not _G.vim then
  _G.vim = {
    api = setmetatable({}, {
      __index = function()
        return function() end
      end,
    }),
    v = { char = "" },
    bo = setmetatable({}, { __index = function() return {} end }),
    fn = {},
  }
end

local input = require("eventb.input")
local Matcher = input._Matcher
local step_eager = input._step_eager
local leader_token_before = input._leader_token_before

-- A representative slice of the real `rossi/operatorTable` payload, copied
-- verbatim from symbolMatcher.test.ts. Covers the ambiguous prefixes, an
-- alphabetic op, and a backslash op (leader-only). The Lua matcher ignores
-- the `description` field (kept here only to mirror the TS fixture 1:1).
local ROWS = {
  { ascii = "&",    unicode = "∧",  description = "and",       aliases = { "and" },          symbolic = true,  eager = true },
  { ascii = "=>",   unicode = "⇒",  description = "implies",   aliases = { "implies", "imp" }, symbolic = true,  eager = true },
  { ascii = "<=>",  unicode = "⇔",  description = "iff",       aliases = { "iff" },          symbolic = true,  eager = true },
  { ascii = "<=",   unicode = "≤",  description = "le",        aliases = { "le", "leq" },    symbolic = true,  eager = true },
  { ascii = "|->",  unicode = "↦",  description = "maplet",    aliases = { "maplet" },       symbolic = true,  eager = true },
  { ascii = "|>",   unicode = "▷",  description = "ranres",    aliases = {},                 symbolic = true,  eager = true },
  { ascii = "|>>",  unicode = "⩥",  description = "ransub",    aliases = {},                 symbolic = true,  eager = true },
  { ascii = ":=",   unicode = "≔",  description = "assign",    aliases = { "assign" },       symbolic = true,  eager = true },
  { ascii = ":",    unicode = "∈",  description = "in",        aliases = { "in" },           symbolic = true,  eager = true },
  { ascii = "::",   unicode = ":∈", description = "becomesin", aliases = {},                 symbolic = true,  eager = true },
  { ascii = "<:",   unicode = "⊆",  description = "subseteq",  aliases = { "subseteq" },     symbolic = true,  eager = true },
  { ascii = "/=",   unicode = "≠",  description = "neq",       aliases = { "neq" },          symbolic = true,  eager = true },
  -- Symbolic but NOT eager (server policy): bare `/` and backslash ops.
  { ascii = "/",    unicode = "÷",  description = "divide",    aliases = { "div" },          symbolic = true,  eager = false },
  { ascii = "..",   unicode = "‥",  description = "range",     aliases = {},                 symbolic = true,  eager = true },
  { ascii = "\\/",  unicode = "∪",  description = "union",     aliases = { "union", "cup" }, symbolic = true,  eager = false },
  { ascii = "NAT",  unicode = "ℕ",  description = "nat",       aliases = { "nat" },          symbolic = false, eager = false },
  { ascii = "or",   unicode = "∨",  description = "or",        aliases = { "or" },           symbolic = false, eager = false },
}

local matcher = Matcher.new(ROWS)

-- Iterate the UTF-8 chars of `s` (the ASCII input here is single-byte, but the
-- oracle below treats `pending`/`out` as byte strings exactly like the TS code
-- treats them as JS UTF-16 strings, so plain byte iteration matches).
local function chars(s)
  local out = {}
  for i = 1, #s do
    out[#out + 1] = s:sub(i, i)
  end
  return out
end

-- Reference oracle: simulate left-to-right typing through the eager state
-- machine and return the resulting text. A 1:1 port of simulateEagerTyping in
-- symbolMatcher.ts, so the ported step_eager is checked against the SAME logic
-- the VS Code matcher is tested against.
local function simulate_eager_typing(m, str)
  local out = ""
  local pend = ""
  for _, ch in ipairs(chars(str)) do
    local action = step_eager(m, pend, ch)
    if action.type == "wait" or action.type == "reset" then
      -- Both keep the typed char and adopt the new run.
      out = out .. ch
      pend = action.pending
    elseif action.type == "convertWithChar" then
      -- `pend` is already in `out`; drop it and the (unwritten) char, appending
      -- the glyph that subsumes both.
      out = out:sub(1, #out - #pend) .. action.unicode
      pend = ""
    elseif action.type == "convertHeld" then
      -- Drop the held run from `out`, append the glyph, then the char.
      out = out:sub(1, #out - action.held_len) .. action.unicode .. ch
      pend = ""
    end
  end
  return out
end

--------------------------------------------------------------------------------

local failures = 0
local passes = 0

local function check(label, got, want)
  if got == want then
    passes = passes + 1
    print("ok   " .. label)
  else
    failures = failures + 1
    io.stderr:write(("not ok %s\n    got:  %s\n    want: %s\n"):format(
      label, tostring(got), tostring(want)))
  end
end

-- --- Eager substitution ------------------------------------------------------

-- Single-char complete op converts immediately on keystroke.
check("& -> and", simulate_eager_typing(matcher, "a&b"), "a∧b")
-- Two-char op completes on its final char.
check("=> -> implies", simulate_eager_typing(matcher, "x=>y"), "x⇒y")
-- Prefix ambiguity: <= is held until a boundary proves no `>` follows.
check("<= waits then converts on boundary", simulate_eager_typing(matcher, "a<=b"), "a≤b")
check("<=> beats <=", simulate_eager_typing(matcher, "p<=>q"), "p⇔q")
-- Maplet is built up across three chars (maximal munch).
check("|-> -> maplet", simulate_eager_typing(matcher, "f|->x"), "f↦x")
-- |> held (because of |>>) then converts on boundary; |>> wins when completed.
check("|> waits then converts", simulate_eager_typing(matcher, "a|>b"), "a▷b")
check("|>> beats |>", simulate_eager_typing(matcher, "a|>>b"), "a⩥b")
-- Assignment vs membership disambiguation.
check(":= -> assign", simulate_eager_typing(matcher, "x:=y"), "x≔y")
check(": -> in on boundary", simulate_eager_typing(matcher, "x:y"), "x∈y")
check(":: -> becomes-in", simulate_eager_typing(matcher, "x::y"), "x:∈y")
check("<: -> subseteq", simulate_eager_typing(matcher, "S<:T"), "S⊆T")

-- Blocklisted single chars stay literal; their extensions still convert.
check("// stays a comment", simulate_eager_typing(matcher, "// a=>b"), "// a⇒b")
check("lone / stays", simulate_eager_typing(matcher, "a/b"), "a/b")
check("/= still converts", simulate_eager_typing(matcher, "a/=b"), "a≠b")
check(".. still converts", simulate_eager_typing(matcher, "1..5"), "1‥5")
check("lone . stays", simulate_eager_typing(matcher, "a.b"), "a.b")

-- Backslash is reserved for the leader: \/ (union) is NOT eager, and a
-- backslash always ends the current run without converting it.
check("\\ does not eager-convert", simulate_eager_typing(matcher, "a\\/b"), "a\\/b")
check("\\ cancels a held run", simulate_eager_typing(matcher, "<\\x"), "<\\x")

-- Alphabetic ops are never eager.
check("NAT stays literal", simulate_eager_typing(matcher, "x:NAT"), "x∈NAT")
check("or stays literal", simulate_eager_typing(matcher, "p or q"), "p or q")

-- --- Leader resolution -------------------------------------------------------

check("\\and", matcher:resolve_leader("and"), "∧")
check("\\to is absent here", matcher:resolve_leader("to"), nil)
check("\\nat (alias)", matcher:resolve_leader("nat"), "ℕ")
check("\\NAT (ascii word)", matcher:resolve_leader("NAT"), "ℕ")
check("\\or (ascii word)", matcher:resolve_leader("or"), "∨")
check("\\union", matcher:resolve_leader("union"), "∪")
check("unknown leader", matcher:resolve_leader("nope"), nil)

-- --- Leader token scanning ---------------------------------------------------

local function tok_eq(t, name, start)
  return t ~= nil and t.name == name and t.start == start
end

check("token before cursor", tok_eq(leader_token_before("foo \\foral"), "foral", 4), true)
check("no token (space after)", leader_token_before("\\and ") == nil, true)
check("no backslash", leader_token_before("and") == nil, true)
check("token at start", tok_eq(leader_token_before("\\in"), "in", 0), true)

-- --- The real generated operator table ---------------------------------------
-- Drive the same maximal-munch / leader cases the cell brief calls out against
-- the ACTUAL canonical table, proving input.lua + operators.lua agree end-to-
-- end (not just against the hand-copied fixture above).

do
  local ok, ops = pcall(require, "eventb.operators")
  if ok and ops and ops.rows then
    local real = Matcher.new(ops.rows)
    check("real: => implies", simulate_eager_typing(real, "x=>y"), "x⇒y")
    check("real: |-> maplet", simulate_eager_typing(real, "f|->x"), "f↦x")
    check("real: <=> iff", simulate_eager_typing(real, "p<=>q"), "p⇔q")
    check("real: <= maximal munch", simulate_eager_typing(real, "a<=b"), "a≤b")
    -- Leader expansion targets present in the canonical table.
    check("real: \\forall", real:resolve_leader("forall"), "∀")
    check("real: \\to", real:resolve_leader("to"), "→")
    check("real: \\lambda", real:resolve_leader("lambda"), "λ")
    check("real: \\nat (alias)", real:resolve_leader("nat"), "ℕ")
    check("real: \\NAT (ascii word)", real:resolve_leader("NAT"), "ℕ")
  else
    -- Not fatal: the pure fixture cases above are the contract. Note the skip.
    print("# skipped real-table cases: eventb.operators not requireable")
  end
end

-- --- Live editor glue (real Neovim only) -------------------------------------
-- Exercise the actual `on_bytes` handler end-to-end: enable the input method on
-- a real Event-B buffer, "type" one byte at a time, and assert the buffer text.
-- This catches glue-level bugs the pure oracle cannot (offset math, textlock,
-- the contiguity check, config gating). Skipped automatically under a bare Lua
-- interpreter, where there is no buffer API to drive.

local function running_in_neovim()
  -- The shim installs no-op `vim.api.*`; the real API returns a buffer handle.
  if type(vim) ~= "table" or type(vim.api) ~= "table" then
    return false
  end
  local ok, handle = pcall(vim.api.nvim_create_buf, false, true)
  if not ok or type(handle) ~= "number" or handle == 0 then
    return false
  end
  pcall(vim.api.nvim_buf_delete, handle, { force = true })
  return true
end

if running_in_neovim() then
  local buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_set_current_buf(buf)
  vim.bo[buf].filetype = "eventb"
  input.setup({})

  -- "Type" `s` one byte at a time, the only thing that fires `on_bytes` per
  -- char. The window cursor is unreliable in headless, so we always append at
  -- end-of-line and resync our column to the line length after each keystroke
  -- (which also absorbs the length change when a conversion fires).
  local function type_text(s)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "" })
    local col = 0
    for i = 1, #s do
      vim.api.nvim_buf_set_text(buf, 0, col, 0, col, { s:sub(i, i) })
      vim.wait(15, function() return false end) -- drain the scheduled edit
      col = #(vim.api.nvim_buf_get_lines(buf, 0, 1, false)[1] or "")
    end
  end
  local function line()
    return vim.api.nvim_buf_get_lines(buf, 0, 1, false)[1]
  end
  local function live(label, typed, want)
    type_text(typed)
    check("live: " .. label, line(), want)
  end

  -- Eager conversions through the real handler (incl. maximal munch & resets).
  live("=> implies", "x=>y", "x⇒y")
  live("|-> maplet", "f|->x", "f↦x")
  live("<=> iff", "p<=>q", "p⇔q")
  live("<= munch on boundary", "a<=b", "a≤b")
  live("& and", "a&b", "a∧b")
  live(":= assign", "x:=y", "x≔y")
  live(": in", "x:y", "x∈y")
  live(":: becomes-in", "x::y", "x:∈y")
  live("<: subseteq", "S<:T", "S⊆T")
  live("/= neq", "a/=b", "a≠b")
  live(".. range", "1..5", "1‥5")
  live("lone / stays", "a/b", "a/b")
  live("lone . stays", "a.b", "a.b")
  live("alphabetic 'or' literal", "p or q", "p or q")
  live("\\/ reserved (literal)", "a\\/b", "a\\/b")
  live("chained =>", "a=>b=>c", "a⇒b⇒c")

  -- Leader expansion commits on the boundary char (space kept after the glyph).
  live("\\forall<space>", "\\forall ", "∀ ")
  live("\\to<space>", "\\to ", "→ ")
  live("\\lambda<space>", "\\lambda ", "λ ")
  live("\\nat alias", "\\nat ", "ℕ ")
  live("\\NAT ascii word", "\\NAT ", "ℕ ")
  live("\\nope unknown (literal)", "\\nope ", "\\nope ")

  -- Config gating mirrors rossi.input.enabled / rossi.input.eager.
  input.setup({ enabled = false })
  live("disabled: no conversion", "x=>y", "x=>y")
  input.setup({ enabled = true, eager = false })
  live("eager=off: no eager", "x=>y", "x=>y")
  live("eager=off: leader still works", "\\to ", "→ ")
  input.setup({}) -- restore defaults
else
  print("# skipped live on_bytes cases: not running under Neovim")
end

--------------------------------------------------------------------------------

print(("\n1..%d  (%d passed, %d failed)"):format(passes + failures, passes, failures))
if failures > 0 then
  io.stderr:write(("\n%d assertion(s) failed\n"):format(failures))
  os.exit(1)
end
print("All Event-B input method tests passed")
