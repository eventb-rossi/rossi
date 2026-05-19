# Event-B Language Support for Neovim

This directory contains Neovim configuration for Event-B formal modeling, providing comprehensive language support through the Rossi Language Server.

## Features

### 🎨 Syntax Highlighting
- Full syntax highlighting for Event-B constructs
- Support for both Unicode (∧, ∨, ⇒, ∈) and ASCII operators (/\, \/, =>, :)
- Highlighting for:
  - Keywords (CONTEXT, MACHINE, EVENTS, etc.)
  - Logical, set, relation, and arithmetic operators
  - Labels (axioms, invariants, guards, actions)
  - Comments and strings
  - Numbers and identifiers

### 🔍 LSP Features (via Language Server)
- **Real-time Diagnostics**: Instant feedback on syntax errors
- **Document Symbols**: Hierarchical outline and quick navigation
- **Code Formatting**: Auto-format with Unicode or ASCII operators
- **Code Completion**: Context-aware suggestions
- **Hover Documentation**: Operator and symbol documentation
- **Go-to-Definition**: Jump to symbol definitions (across files!)
- **Find References**: Find all symbol usages
- **Rename Symbol**: Rename symbols across your workspace
- **Workspace Symbols**: Search for symbols across files
- **Document Links**: Click SEES/REFINES/EXTENDS to navigate
- **Signature Help**: Parameter hints for quantifiers and lambda
- **Code Actions**: Quick fixes and refactorings
- **Folding Ranges**: Collapse/expand code sections
- **ProB Integration**: Run ProB animator and model checker

## Quick Start

### 1. Install the Language Server

```bash
# Clone the repository (if you haven't already)
git clone https://github.com/eventb-rossi/rossi
cd rossi

# Build and install the language server
cargo install --path crates/rossi-lsp

# Verify installation
rossi-language-server --version
```

The server will be installed to `~/.cargo/bin/rossi-language-server`.

### 2. Install Neovim Configuration

**Option A: Using a Plugin Manager (Recommended)**

Add to your Neovim config (using lazy.nvim, packer, etc.):

```lua
-- For lazy.nvim
{
  'eventb-rossi/rossi',
  ft = 'eventb',
  config = function()
    -- The syntax and ftdetect files will be loaded automatically
  end,
}
```

**Option B: Manual Installation**

Copy the files to your Neovim config directory:

```bash
cd rossi/editors/neovim

# Copy syntax highlighting
cp syntax/eventb.vim ~/.config/nvim/syntax/

# Copy filetype detection
cp ftdetect/eventb.vim ~/.config/nvim/ftdetect/

# For nvim-lspconfig integration (if using it)
cp lua/lspconfig/eventb.lua ~/.local/share/nvim/site/pack/*/start/nvim-lspconfig/lua/lspconfig/
```

### 3. Configure LSP

Add to your `init.lua` or `init.vim`:

#### Basic Configuration (init.lua)

```lua
-- Ensure nvim-lspconfig is installed
require('lspconfig').eventb.setup{}
```

#### With Custom Keybindings (init.lua)

```lua
require('lspconfig').eventb.setup{
  on_attach = function(client, bufnr)
    local opts = { noremap=true, silent=true, buffer=bufnr }

    -- Navigation
    vim.keymap.set('n', 'gd', vim.lsp.buf.definition, opts)
    vim.keymap.set('n', 'gD', vim.lsp.buf.declaration, opts)
    vim.keymap.set('n', 'gr', vim.lsp.buf.references, opts)
    vim.keymap.set('n', 'gi', vim.lsp.buf.implementation, opts)
    vim.keymap.set('n', 'K', vim.lsp.buf.hover, opts)
    vim.keymap.set('n', '<C-k>', vim.lsp.buf.signature_help, opts)

    -- Workspace
    vim.keymap.set('n', '<leader>wa', vim.lsp.buf.add_workspace_folder, opts)
    vim.keymap.set('n', '<leader>wr', vim.lsp.buf.remove_workspace_folder, opts)
    vim.keymap.set('n', '<leader>wl', function()
      print(vim.inspect(vim.lsp.buf.list_workspace_folders()))
    end, opts)

    -- Code actions
    vim.keymap.set('n', '<leader>rn', vim.lsp.buf.rename, opts)
    vim.keymap.set('n', '<leader>ca', vim.lsp.buf.code_action, opts)

    -- Formatting
    vim.keymap.set('n', '<leader>f', function()
      vim.lsp.buf.format { async = true }
    end, opts)

    -- Diagnostics
    vim.keymap.set('n', '[d', vim.diagnostic.goto_prev, opts)
    vim.keymap.set('n', ']d', vim.diagnostic.goto_next, opts)
    vim.keymap.set('n', '<leader>e', vim.diagnostic.open_float, opts)
    vim.keymap.set('n', '<leader>q', vim.diagnostic.setloclist, opts)
  end,
  capabilities = require('cmp_nvim_lsp').default_capabilities(),
}
```

#### With Custom Settings (init.lua)

```lua
require('lspconfig').eventb.setup{
  settings = {
    rossi = {
      format = {
        useUnicode = true,      -- Use Unicode operators
        indentation = "    ",   -- 4 spaces
        maxLineLength = 100,    -- Parsed for future wrapping; not applied yet
      },
      diagnostics = {
        enabled = true,
        debounceMs = 500, -- Parsed for future debouncing; diagnostics are immediate
      },
      completion = {
        enabled = true,
        triggerCharacters = { ":", ".", "(", "{" },
      },
    },
  },
  on_attach = function(client, bufnr)
    -- Your keybindings here
  end,
}
```

#### Vimscript Configuration (init.vim)

```vim
" Ensure nvim-lspconfig is installed
lua << EOF
require('lspconfig').eventb.setup{}
EOF
```

## Configuration Options

### Language Server Settings

```lua
settings = {
  rossi = {
    -- Formatting options
    format = {
      useUnicode = true,        -- Use Unicode (∧, ∨, ⇒) or ASCII (/\, \/, =>)
      indentation = "    ",     -- Indentation string (spaces or tabs)
      maxLineLength = 100,      -- Parsed for future wrapping; not applied yet
    },

    -- Diagnostics options
    diagnostics = {
      enabled = true,           -- Enable/disable diagnostics
      debounceMs = 500,         -- Parsed for future debouncing; diagnostics are immediate
    },

    -- Completion options
    completion = {
      enabled = true,           -- Enable/disable completion
      triggerCharacters = { ":", ".", "(", "{" },
    },

    -- Trace options
    trace = {
      server = "off",           -- "off", "messages", "verbose"
    },
  },
}
```

### Custom Server Path

If the server is not in your PATH:

```lua
require('lspconfig').eventb.setup{
  cmd = { '/path/to/rossi-language-server' },
  -- ... other settings
}
```

## Features Overview

### Code Completion

Type to trigger completion:
- Keywords: `CONTEXT`, `MACHINE`, `EVENTS`, etc.
- Operators: Type `:` to get `∈`, type `/\` to get `∧`
- Symbols: Variables, constants, parameters from context

### Hover Documentation

Hover over any operator or symbol to see:
- Operator documentation with examples
- Symbol types and definitions
- Cross-references

### Go-to-Definition

Press `gd` or `Ctrl+]` on:
- Variables → Jump to VARIABLES clause
- Constants → Jump to CONSTANTS or axiom definition
- Event names → Jump to EVENT declaration
- SEES references → Open the context file!
- REFINES references → Open the abstract machine!

### Find References

Press `gr` or `:LspReferences` to find all usages of:
- Variables across guards, actions, invariants
- Constants across axioms, guards, actions
- Events across refinement chains

### Rename Symbol

Press `<leader>rn` or `:LspRename` to rename:
- Variables, constants, parameters
- Updates all references across all files in workspace
- Safe refactoring with validation

### Code Actions

Press `<leader>ca` or `:LspCodeAction` for:
- **Convert operators**: ASCII ↔ Unicode
- **Add missing END**: Quick fix for parse errors
- **Add missing clauses**: INVARIANTS, AXIOMS, etc.
- **Sort clauses**: Alphabetically sort VARIABLES, CONSTANTS

### Document Symbols

Press `:LspDocumentSymbol` or `<leader>ds` to see:
- Hierarchical outline of your Event-B file
- Quick navigation to any section

### Formatting

Press `<leader>f` or `:lua vim.lsp.buf.format()` to:
- Consistent indentation
- Operator normalization (Unicode or ASCII)
- Clause ordering

### ProB Integration

Run ProB directly from Neovim:
- Code lens appears on MACHINE/CONTEXT declarations
- Commands: `:LspCodeLens` to see available actions
- Animate or model check your specifications
- Counterexamples shown as diagnostics

## Recommended Plugins

### Essential
- [nvim-lspconfig](https://github.com/neovim/nvim-lspconfig) - LSP configuration
- [nvim-cmp](https://github.com/hrsh7th/nvim-cmp) - Completion engine
- [cmp-nvim-lsp](https://github.com/hrsh7th/cmp-nvim-lsp) - LSP completion source

### Highly Recommended
- [telescope.nvim](https://github.com/nvim-telescope/telescope.nvim) - Fuzzy finder (workspace symbols)
- [trouble.nvim](https://github.com/folke/trouble.nvim) - Diagnostics list
- [nvim-navic](https://github.com/SmiteshP/nvim-navic) - Breadcrumb navigation

### Optional but Useful
- [fidget.nvim](https://github.com/j-hui/fidget.nvim) - LSP progress UI
- [lsp-colors.nvim](https://github.com/folke/lsp-colors.nvim) - LSP highlight colors
- [lspsaga.nvim](https://github.com/glepnir/lspsaga.nvim) - Enhanced LSP UI

## Example: Complete Configuration

Here's a complete example using lazy.nvim:

```lua
-- plugins/eventb.lua
return {
  -- Event-B syntax and LSP
  {
    'eventb-rossi/rossi',
    ft = 'eventb',
  },

  -- LSP configuration
  {
    'neovim/nvim-lspconfig',
    config = function()
      require('lspconfig').eventb.setup{
        settings = {
          rossi = {
            format = {
              useUnicode = true,
              indentation = "    ",
            },
          },
        },
        on_attach = function(client, bufnr)
          -- Enable format on save
          vim.api.nvim_create_autocmd("BufWritePre", {
            buffer = bufnr,
            callback = function()
              vim.lsp.buf.format { async = false }
            end,
          })
        end,
      }
    end,
  },

  -- Completion
  {
    'hrsh7th/nvim-cmp',
    dependencies = {
      'hrsh7th/cmp-nvim-lsp',
    },
    config = function()
      local cmp = require('cmp')
      cmp.setup{
        sources = {
          { name = 'nvim_lsp' },
        },
      }
    end,
  },
}
```

## Keyboard Shortcuts Reference

### Navigation
| Key | Action |
|-----|--------|
| `gd` | Go to definition |
| `gr` | Find references |
| `gi` | Go to implementation |
| `K` | Hover documentation |
| `<C-k>` | Signature help |

### Code Actions
| Key | Action |
|-----|--------|
| `<leader>ca` | Code actions menu |
| `<leader>rn` | Rename symbol |
| `<leader>f` | Format document |

### Diagnostics
| Key | Action |
|-----|--------|
| `[d` | Previous diagnostic |
| `]d` | Next diagnostic |
| `<leader>e` | Show diagnostic float |
| `<leader>q` | Diagnostic location list |

### Symbols
| Command | Action |
|---------|--------|
| `:LspDocumentSymbol` | Document outline |
| `:LspWorkspaceSymbol` | Workspace-wide symbol search |

## Troubleshooting

### Server not starting

Check if the server is in your PATH:
```bash
which rossi-language-server
```

If not found, specify the full path:
```lua
require('lspconfig').eventb.setup{
  cmd = { vim.fn.expand('~/.cargo/bin/rossi-language-server') },
}
```

### No syntax highlighting

Ensure the syntax file is loaded:
```vim
:set filetype=eventb
```

Check if the file was detected correctly:
```vim
:echo &filetype
```

Should output: `eventb`

### No completions

Check if the LSP client is attached:
```vim
:LspInfo
```

Ensure nvim-cmp is configured with the LSP source:
```lua
sources = {
  { name = 'nvim_lsp' },
}
```

### Unicode characters not displaying

Ensure your terminal and Neovim support UTF-8:
```vim
:set encoding=utf-8
```

Install a font that supports Unicode symbols:
- JetBrains Mono
- Fira Code
- Cascadia Code
- Nerd Fonts

### See server logs

Enable LSP logging:
```lua
vim.lsp.set_log_level("debug")
```

View logs:
```vim
:lua vim.cmd('e'..vim.lsp.get_log_path())
```

## Contributing

Contributions are welcome! Please see the main repository for contribution guidelines:
https://github.com/eventb-rossi/rossi

## License

Dual licensed under MIT or Apache-2.0, matching the main Rossi project.

## Resources

- **Main Repository**: https://github.com/eventb-rossi/rossi
- **Event-B Resources**:
  - [Event-B.org](https://www.event-b.org/)
  - [Event-B Wiki](https://wiki.event-b.org/)
  - [Rodin Platform](https://www.event-b.org/platform.html)
  - [ProB Model Checker](https://prob.hhu.de/)

## Support

- **Issues**: https://github.com/eventb-rossi/rossi/issues
- **Discussions**: https://github.com/eventb-rossi/rossi/discussions
