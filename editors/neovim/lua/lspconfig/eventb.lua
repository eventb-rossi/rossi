-- Rossi Language Server configuration for nvim-lspconfig
-- https://github.com/eventb-rossi/rossi

local util = require('lspconfig.util')

return {
  default_config = {
    -- Command to start the language server
    cmd = { 'rossi-language-server' },

    -- File types that this server handles
    filetypes = { 'eventb' },

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
      eventb = {
        -- Formatting configuration
        format = {
          -- Use Unicode operators (∧, ∨, ⇒, ∈) instead of ASCII (/\, \/, =>, :)
          useUnicode = true,
          -- Indentation string (spaces or tabs)
          indentation = "    ",
          -- Maximum line length for formatting (optional)
          maxLineLength = 100,
        },

        -- Diagnostics configuration
        diagnostics = {
          -- Enable/disable diagnostics
          enabled = true,
          -- Debounce delay in milliseconds
          debounceMs = 500,
        },

        -- Completion configuration
        completion = {
          -- Enable/disable completion
          enabled = true,
          -- Trigger characters for completion
          triggerCharacters = { ":", ".", "(", "{" },
        },

        -- Trace configuration
        trace = {
          -- Server trace level: "off", "messages", "verbose"
          server = "off",
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
- ProB integration

**Installation:**

Build and install the language server:
```bash
cargo install --path crates/rossi-lsp
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
    eventb = {
      format = {
        useUnicode = true,
        indentation = "    ",
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
