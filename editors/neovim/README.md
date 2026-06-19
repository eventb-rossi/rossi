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

### ⌨️ Symbol Input (type ASCII, get Unicode)
- **Eager combos** convert on the fly: `=>` → ⇒, `<=>` → ⇔, `&` → ∧, `|->` → ↦, `<:` → ⊆
- **`\name` leader** expands any operator on a boundary: `\and` → ∧, `\to` → →, `\forall` → ∀, `\nat` → ℕ
- Maximal munch handles ambiguous prefixes (`<=` → ≤ but `<=>` → ⇔)
- Toggle with `rossi.input.enabled`; disable only the eager combos with `rossi.input.eager`

### ✂️ Snippets (LuaSnip)
- Ready-made scaffolds for machines, contexts, events, and clauses
- Expand by prefix: `mch`, `ctx`, `evt`, `inv`, `grd`, `act`, and more
- Generated from the canonical Rossi snippet table, shared with the VS Code extension

### 🔍 LSP Features (via Language Server)
- **Real-time Diagnostics**: Instant feedback on syntax errors
- **Document Symbols**: Hierarchical outline and quick navigation
- **Code Formatting**: Auto-format with Unicode or ASCII operators
- **Code Completion**: Context-aware suggestions
- **Semantic Tokens**: Precise, parser-driven highlighting (applied automatically)
- **Selection Range**: Smart expand/shrink selection along the syntax tree
- **Hover Documentation**: Operator and symbol documentation
- **Go-to-Definition**: Jump to symbol definitions (across files!)
- **Find References**: Find all symbol usages
- **Rename Symbol**: Rename symbols across your workspace
- **Workspace Symbols**: Search for symbols across files
- **Document Links**: Click SEES/REFINES/EXTENDS to navigate
- **Signature Help**: Parameter hints for quantifiers and lambda
- **Code Actions**: Quick fixes and refactorings
- **Folding Ranges**: Collapse/expand code sections

## Quick Start

### 1. Install the Language Server

```bash
# Clone the repository (if you haven't already)
git clone https://github.com/eventb-rossi/rossi
cd rossi

# Build and install the language server
cargo install --path crates/eventb-lsp

# Verify installation
eventb-language-server --version
```

The server will be installed to `~/.cargo/bin/eventb-language-server`.

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
  cmd = { '/path/to/eventb-language-server' },
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

### Semantic Tokens

Highlighting is refined by the language server as you edit:
- Parser-driven token types disambiguate keywords, labels, and operators
- Applied automatically once the LSP client attaches — no extra configuration
- Falls back to the bundled `syntax/eventb.vim` rules before the server is ready

### Selection Range

Grow and shrink the selection along the syntax tree:
- Expands from the symbol under the cursor outward (term → clause → section)
- Drives `vim.lsp.buf.selection_range` and treesitter-style incremental selection

## Symbol Input

Type ASCII and get Unicode without leaving the keyboard. Enable it for Event-B
buffers:

```lua
-- Set up the input method once, then enable it on the eventb filetype.
require('eventb.input').setup{}

vim.api.nvim_create_autocmd('FileType', {
  pattern = 'eventb',
  callback = function()
    require('eventb.input').enable(0) -- enable for the current buffer
  end,
})
```

Two ways to enter symbols, both on by default:

- **Eager combos** — symbolic operators convert as soon as they are unambiguous:
  - `=>` → ⇒, `<=>` → ⇔, `&` → ∧, `|->` → ↦, `:` → ∈, `<:` → ⊆, `..` → ‥
  - Longest-match wins: `<=` becomes ≤ only once you type a non-`>` character,
    while `<=>` becomes ⇔.
- **`\name` leader** — type a backslash, an operator name, then a space or any
  boundary character:
  - `\and` → ∧, `\or` → ∨, `\not` → ¬, `\to` → →, `\forall` → ∀, `\exists` → ∃,
    `\in` → ∈, `\nat` → ℕ, `\int` → ℤ, `\pow` → ℙ
  - The leader is also how you enter alphabetic operators (`NAT`, `or`, …) —
    these are never converted eagerly so they don't interfere with ordinary text.

Toggle the whole feature with `rossi.input.enabled`, or keep only the `\name`
leader by setting `rossi.input.eager` to `false`:

```lua
require('eventb.input').setup{
  enabled = true, -- master switch (rossi.input.enabled)
  eager = true,   -- eager combos; false keeps only the \name leader
}
```

This complements the whole-file `:RossiConvertCurrentFileToUnicode` /
`:RossiConvertCurrentFileToAscii` commands.

## Snippets

Snippets are distributed in the VS Code JSON format and load through LuaSnip's
`from_vscode` loader. Point the loader at the bundled `snippets` directory (it
contains the generated `package.json` and `eventb.json`):

```lua
require('luasnip.loaders.from_vscode').lazy_load({
  paths = { '/path/to/rossi/editors/neovim/snippets' },
})
```

Type a prefix in an Event-B buffer and expand it (the key depends on your
LuaSnip mapping, commonly `<Tab>` or `<C-k>`). A few examples:

| Prefix | Inserts                          |
|--------|----------------------------------|
| `mch`  | A MACHINE skeleton               |
| `ctx`  | A CONTEXT skeleton               |
| `evt`  | An EVENT with guards and actions |
| `inv`  | An invariant line                |
| `grd`  | A guard line                     |
| `act`  | An action (assignment) line      |

The snippet table is shared with the VS Code extension, so prefixes and bodies
stay identical across editors.

## Editor Commands

The plugin exposes the same actions as the VS Code extension, as `:Rossi*`
user commands:

- `:RossiImportRodinProject`
- `:RossiExportCurrentFileToRodinZip`
- `:RossiExportWorkspaceToRodinZip`
- `:RossiOpenInRodin`
- `:RossiBuildCheckedRodinZip`
- `:RossiValidateCurrentFile`
- `:RossiValidateWorkspace`
- `:RossiConvertCurrentFileToUnicode`
- `:RossiConvertCurrentFileToAscii`
- `:RossiCheckToolchain`

Suggested keymaps (set them in your `on_attach` or an `eventb` FileType
autocommand):

```lua
local opts = { noremap = true, silent = true, buffer = bufnr }
vim.keymap.set('n', '<leader>pu', '<Cmd>RossiConvertCurrentFileToUnicode<CR>', opts)
vim.keymap.set('n', '<leader>pa', '<Cmd>RossiConvertCurrentFileToAscii<CR>', opts)
vim.keymap.set('n', '<leader>pv', '<Cmd>RossiValidateCurrentFile<CR>', opts)
```

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

### Rossi Commands
| Command | Action |
|---------|--------|
| `:RossiConvertCurrentFileToUnicode` | Convert operators to Unicode |
| `:RossiConvertCurrentFileToAscii` | Convert operators to ASCII |
| `:RossiValidateCurrentFile` | Validate the current file |

See [Editor Commands](#editor-commands) for the full list and suggested keymaps.

## Troubleshooting

### Server not starting

Check if the server is in your PATH:
```bash
which eventb-language-server
```

If not found, specify the full path:
```lua
require('lspconfig').eventb.setup{
  cmd = { vim.fn.expand('~/.cargo/bin/eventb-language-server') },
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
