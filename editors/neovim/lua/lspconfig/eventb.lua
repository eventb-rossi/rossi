-- Rossi Language Server configuration for nvim-lspconfig
-- https://github.com/eventb-rossi/rossi

local util = require('lspconfig.util')

-- Pinned on_attach defaults shared by every Event-B buffer. Kept as a named
-- function so user configs can compose with it (see the docs block below):
--   local eventb = require('lspconfig.configs.eventb') -- or read this file
--   require('lspconfig').eventb.setup{ on_attach = function(c, b)
--     -- your bindings; the pinned defaults already ran via default_config
--   end }
-- nvim-lspconfig merges a user-supplied on_attach *after* default_config's, so
-- both run and nothing here is lost.
local function default_on_attach(client, bufnr)
  -- Keep CodeLens ("Open in Rodin" on MACHINE/CONTEXT declarations) fresh.
  -- Guarded so we never poke a server build that lacks the capability.
  if client.server_capabilities.codeLensProvider then
    local group = vim.api.nvim_create_augroup('RossiCodeLens', { clear = false })
    vim.api.nvim_create_autocmd({ 'BufEnter', 'CursorHold', 'InsertLeave' }, {
      group = group,
      buffer = bufnr,
      callback = function()
        vim.lsp.codelens.refresh({ bufnr = bufnr })
      end,
    })
    -- Populate lenses immediately on attach rather than waiting for the first event.
    vim.lsp.codelens.refresh({ bufnr = bufnr })
  end

  -- Show the server's inlay hints (inferred declaration types, WD markers).
  -- vim.lsp.inlay_hint ships with Neovim 0.10+; older versions simply skip
  -- this, like the selection_range fallback below.
  if client.server_capabilities.inlayHintProvider and vim.lsp.inlay_hint then
    vim.lsp.inlay_hint.enable(true, { bufnr = bufnr })
  end

  local opts = { noremap = true, silent = true, buffer = bufnr }

  -- Run the CodeLens under the cursor (e.g. open the model in Rodin).
  vim.keymap.set('n', '<leader>cl', vim.lsp.codelens.run, opts)

  -- Expand the LSP selection range (textDocument/selectionRange, smart
  -- expand/shrink). Use the built-in when this Neovim ships it, otherwise fall
  -- back to a request that selects the first returned range in visual mode.
  vim.keymap.set({ 'n', 'x' }, '<leader>v', function()
    if vim.lsp.buf.selection_range then
      vim.lsp.buf.selection_range(1)
      return
    end
    local params = vim.lsp.util.make_position_params(0, client.offset_encoding)
    params.positions = { params.position }
    params.position = nil
    client.request('textDocument/selectionRange', params, function(err, result)
      if err or not result or not result[1] then
        return
      end
      local range = result[1].range
      vim.api.nvim_win_set_cursor(0, { range.start.line + 1, range.start.character })
      vim.cmd('normal! v')
      vim.api.nvim_win_set_cursor(0, { range['end'].line + 1, math.max(range['end'].character - 1, 0) })
    end, bufnr)
  end, opts)

  -- :Rossi* command keymaps (commands defined in lua/eventb/commands.lua).
  require('eventb.commands').setup()
  vim.keymap.set('n', '<leader>ru', '<cmd>RossiConvertUnicode<cr>', opts)
  vim.keymap.set('n', '<leader>ra', '<cmd>RossiConvertAscii<cr>', opts)
  vim.keymap.set('n', '<leader>rv', '<cmd>RossiValidate<cr>', opts)
end

return {
  default_config = {
    -- Command to start the language server
    cmd = { 'eventb-language-server' },

    -- File types that this server handles
    filetypes = { 'eventb' },

    -- Pinned defaults: CodeLens refresh autocmd + selection-range expand +
    -- Rossi keymaps. A user `on_attach` passed to setup{} composes with
    -- (runs after) this one.
    on_attach = default_on_attach,

    -- Root directory detection
    -- Looks for .git directory or eventb.toml file
    root_dir = function(fname)
      return util.root_pattern('.git', 'eventb.toml')(fname)
        or util.find_git_ancestor(fname)
        or util.path.dirname(fname)
    end,

    -- Single file support for files outside of a workspace
    single_file_support = true,

    -- Server settings
    settings = {
      rossi = {
        -- Formatting configuration
        format = {
          -- Style preset: "camille" or "rossi"; "" follows the server default
          style = "",
          -- Use Unicode operators (∧, ∨, ⇒, ∈) instead of ASCII (/\, \/, =>, :)
          useUnicode = true,
          -- Indentation string (spaces or tabs); "" follows the style preset
          indentation = "",
          -- Maximum line width when formatting; 0 disables wrapping
          maxLineWidth = 120,
        },

        -- Diagnostics configuration
        diagnostics = {
          -- Enable/disable diagnostics
          enabled = true,
          -- Parsed for future debouncing; diagnostics are immediate
          debounceMs = 500,
        },

        -- Completion configuration
        completion = {
          -- Enable/disable completion
          enabled = true,
        },

        -- Inlay hints configuration
        inlayHints = {
          -- Show inferred declaration types after variables/parameters/constants
          enabled = true,
          -- Mark formulas with a non-trivial well-definedness condition ("WD")
          wellDefinedness = true,
          -- Truncate type hints longer than this many characters; 0 disables
          maxLength = 32,
        },

        -- Rodin integration ("Open in Rodin" code lens)
        rodin = {
          -- Rodin executable, macOS .app bundle, or app name; "" = platform default
          path = "",
          -- Shared Rodin workspace directory; "" = <root>/.rossi/rodin
          workspace = "",
          -- Mutual live sync with a running Rodin: saves rebuild the
          -- project while Rodin is open, and edits saved in Rodin flow
          -- back into the .eventb sources
          sync = true,
          -- Use the bridge plug-in a running Rodin publishes, when there
          -- is one: it registers the project in the live instance rather
          -- than asking you to use File > Import
          bridge = true,
          -- Merge Rodin's unsaved edits into the buffer as they are typed,
          -- rather than waiting for Ctrl+S in Rodin. Needs the bridge plug-in.
          liveSync = false,
        },
      },
    },
  },

  docs = {
    description = [[
https://github.com/eventb-rossi/rossi

Rossi Language Server provides language support for Event-B formal modeling:
- Real-time diagnostics
- Document symbols and navigation
- Code formatting (Unicode/ASCII operators)
- Code completion
- Hover documentation
- Go-to-definition
- Find references
- Rename symbol
- Workspace symbols
- Document links
- Signature help
- Code actions (quick fixes and refactorings)
- Folding ranges
- Selection range (smart expand/shrink, `<leader>v`)
- Semantic tokens
- Inlay hints (inferred declaration types, well-definedness markers)
- "Open in Rodin" code lens (builds into a persistent `.rossi/rodin` Rodin
  workspace and launches the Rodin IDE; proofs made there survive rebuilds)

**Pinned defaults (default_config.on_attach):**

When this config is set up, every Event-B buffer automatically gets:
- A CodeLens refresh autocmd (BufEnter/CursorHold/InsertLeave), guarded on the
  server advertising codeLensProvider, plus `<leader>cl` to run the lens under
  the cursor (e.g. open the model in Rodin).
- `<leader>v` to expand the LSP selection range (textDocument/selectionRange).
- Inlay hints enabled (vim.lsp.inlay_hint, Neovim 0.10+), guarded on the
  server advertising inlayHintProvider.
- The :Rossi* user commands (from lua/eventb/commands.lua) and `<leader>ru` /
  `<leader>ra` / `<leader>rv` for convert-to-unicode / convert-to-ascii /
  validate.

A user `on_attach` passed to setup{} runs *after* these pinned defaults
(nvim-lspconfig composes them), so it never disables them.

**Syntax highlighting:** the server's semantic tokens attach automatically and
overlay the bundled regex syntax (syntax/eventb.vim). The regex grammar remains
the no-LSP fallback, so buffers stay highlighted even before the server attaches
or when it is unavailable.

**Installation:**

Build and install the language server:
```bash
cargo install --path crates/eventb-lsp
```

**Configuration:**

Add to your Neovim configuration (init.lua):
```lua
require('lspconfig').eventb.setup{
  on_attach = function(client, bufnr)
    -- Your keybindings here
  end,
  capabilities = require('cmp_nvim_lsp').default_capabilities(),
}
```

Or with custom settings:
```lua
require('lspconfig').eventb.setup{
  settings = {
    rossi = {
      format = {
        style = "camille",
        useUnicode = true,
        indentation = "  ",
      },
      diagnostics = {
        enabled = true,
      },
    },
  },
  on_attach = function(client, bufnr)
    -- Your keybindings here
  end,
}
```
]],
  },
}
