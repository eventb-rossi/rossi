-- Run: nvim --headless --noplugin -u NONE -l editors/neovim/test/machine_commands_spec.lua

vim.opt.runtimepath:prepend("editors/neovim")
require("eventb.commands")

local file = vim.fn.tempname() .. ".eventb"
vim.fn.writefile({ "MACHINE M", "VARIABLES", "  x", "END" }, file)
vim.cmd.edit(file)
local bufnr = vim.api.nvim_get_current_buf()
vim.bo[bufnr].filetype = "eventb"
vim.api.nvim_win_set_cursor(0, { 3, 2 })

local calls = {}
vim.lsp.get_clients = function(opts)
  assert(opts.bufnr == bufnr and opts.name == "eventb")
  return {
    {
      offset_encoding = "utf-16",
      request = function(method, params, _, request_bufnr)
        assert(method == "workspace/executeCommand" and request_bufnr == bufnr)
        table.insert(calls, params)
      end,
    },
  }
end

vim.cmd.RossiOpenInRodin()
vim.cmd.RossiModelCheck()
vim.cmd.RossiDisprovePOs()

assert(vim.deep_equal(calls, {
  { command = "rossi.rodin.open", arguments = { vim.uri_from_bufnr(bufnr) } },
  { command = "rossi.animate.check", arguments = { vim.uri_from_bufnr(bufnr), { line = 2, character = 2 } } },
  { command = "rossi.animate.po", arguments = { vim.uri_from_bufnr(bufnr), { line = 2, character = 2 } } },
}))

vim.fn.delete(file)
print("machine commands passed")
vim.cmd.qa({ bang = true })
