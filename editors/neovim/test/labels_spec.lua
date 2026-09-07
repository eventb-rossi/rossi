-- Run from the repository root:
-- nvim --headless --noplugin -u NONE -l editors/neovim/test/labels_spec.lua

vim.cmd("source editors/neovim/syntax/eventb.vim")
vim.cmd("syntax sync fromstart")

local function group(column)
  return vim.fn.synIDattr(vim.fn.synID(1, column, 1), "name")
end

for _, label in ipairs({
  "a:", "a::", ":", "метка:", "😀:", "a@b", 'SAF5"', "inv//1", "inv/*1", "a" .. vim.fn.nr2char(0x85),
}) do
  for _, separator in ipairs({ " ", "\t", "\28", vim.fn.nr2char(0xa0), vim.fn.nr2char(0x2007) }) do
    vim.api.nvim_buf_set_lines(0, 0, -1, false, { "@" .. label .. separator .. "1 = 1", "end" })
    for column = 1, #label + 1 do
      assert(group(column) == "eventbLabel", label .. ": incomplete label at byte " .. column)
    end
    assert(group(#label + #separator + 2) == "eventbNumber", label .. ": label swallowed formula")
  end
end

for _, source in ipairs({ "// @label:", "/* @label: */", '"@label:"' }) do
  vim.api.nvim_buf_set_lines(0, 0, -1, false, { source })
  local actual = group(assert(source:find("@", 1, true)))
  assert(actual == "eventbComment" or actual == "eventbString", source .. ": label inside comment/string")
end

print("Camille label highlighting passed")
