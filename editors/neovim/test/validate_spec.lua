-- Headless test: `:RossiValidate` results skip findings the language server
-- already shows, so a buffer never lists one twice.
--
-- Run from the repository root:
--   nvim --headless --noplugin -u NONE -l editors/neovim/test/validate_spec.lua

vim.opt.runtimepath:prepend("editors/neovim")
local commands = require("eventb.commands")

local dir = vim.fn.tempname()
vim.fn.mkdir(dir, "p")
local file = dir .. "/M.eventb"
vim.fn.writefile({
  "MACHINE M",
  "VARIABLE",
  "    x",
  "INVARIANTS",
  "    @inv1 x ∈ ℕ",
  "    @inv1 x ≥ 0",
  "END",
}, file)
local bufnr = vim.fn.bufadd(file)
vim.fn.bufload(bufnr)

-- What the language server shows: the duplicate label, and a syntax error
-- without a code.
local server = vim.api.nvim_create_namespace("rossi-validate-spec-server")
vim.diagnostic.set(server, bufnr, {
  {
    lnum = 5,
    col = 4,
    message = "duplicate invariant label `inv1` in machine `M` (used 2 times)",
    severity = vim.diagnostic.severity.ERROR,
    source = "rossi",
    code = "EB022",
  },
  {
    lnum = 1,
    col = 0,
    message = "Syntax error: expected VARIABLES",
    severity = vim.diagnostic.severity.ERROR,
    source = "rossi",
  },
})

local function row(rule, line, message)
  return {
    file = dir,
    success = false,
    inner_filename = "M.eventb",
    rule_id = rule,
    error = message,
    severity = "error",
    region = line and { start_line = line, start_column = 5, end_line = line, end_column = 10 } or nil,
  }
end

commands._apply_validation(
  vim.json.encode({
    -- Shown by the server: the same rule on the same line, and the CLI's
    -- EB004 for the server's uncoded syntax error.
    row("EB022", 6, "duplicate invariant label `inv1` in machine `M` (used 2 times)"),
    row("EB004", 2, "Pest parsing error: expected VARIABLES"),
    -- Not shown: the same rule on another line, a rule the server does not
    -- report, and a finding with no region.
    row("EB022", 5, "duplicate invariant label `inv1` in machine `M` (used 2 times)"),
    row("EB012", 3, "variable `x` is never assigned outside INITIALISATION"),
    row("EB014", nil, "INITIALISATION does not assign `x`"),
  }),
  dir
)

local validate = vim.api.nvim_get_namespaces()["rossi-validate"]
local kept = {}
for _, d in ipairs(vim.diagnostic.get(bufnr, { namespace = validate })) do
  table.insert(kept, string.format("%s@%d", d.code, d.lnum))
end
table.sort(kept)
assert(
  vim.deep_equal(kept, { "EB012@2", "EB014@0", "EB022@4" }),
  "validate kept " .. vim.inspect(kept)
)

vim.fn.delete(dir, "rf")
print("Validate dedupe passed")
