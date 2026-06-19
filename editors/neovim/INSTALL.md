# Event-B Neovim Installation Guide

This guide provides detailed installation instructions for setting up Event-B language support in Neovim.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Installation Methods](#installation-methods)
  - [Method 1: Using a Plugin Manager (Recommended)](#method-1-using-a-plugin-manager-recommended)
  - [Method 2: Manual Installation](#method-2-manual-installation)
- [LSP Configuration](#lsp-configuration)
- [Symbol Input](#symbol-input)
- [Snippets](#snippets)
- [Verification](#verification)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

### Required

1. **Neovim 0.8 or later**
   ```bash
   nvim --version
   ```
   If you need to install or upgrade Neovim:
   - Ubuntu/Debian: `sudo apt install neovim`
   - Arch Linux: `sudo pacman -S neovim`
   - macOS: `brew install neovim`
   - Or download from: https://github.com/neovim/neovim/releases

2. **Rust toolchain** (to build the language server)
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source $HOME/.cargo/env
   ```

3. **Rossi Language Server**
   ```bash
   # Clone the repository
   git clone https://github.com/eventb-rossi/rossi
   cd rossi

   # Build and install the language server
   cargo install --path crates/eventb-lsp

   # Verify installation
   eventb-language-server --version
   ```

   The server will be installed to `~/.cargo/bin/eventb-language-server`.

### Recommended

1. **nvim-lspconfig** - LSP configuration framework
   ```bash
   # Will be installed via plugin manager in the next steps
   ```

2. **nvim-cmp** - Completion engine
   ```bash
   # Will be installed via plugin manager in the next steps
   ```

---

## Installation Methods

### Method 1: Using a Plugin Manager (Recommended)

Choose your plugin manager and follow the corresponding instructions:

#### Using lazy.nvim (Recommended)

Add to `~/.config/nvim/lua/plugins/eventb.lua`:

```lua
return {
  -- Event-B language support
  {
    'eventb-rossi/rossi',
    ft = 'eventb',
    config = function()
      -- Syntax and filetype detection are loaded automatically
    end,
  },

  -- LSP configuration (if not already installed)
  {
    'neovim/nvim-lspconfig',
    config = function()
      require('lspconfig').eventb.setup{
        on_attach = function(client, bufnr)
          -- Enable format on save
          vim.api.nvim_create_autocmd("BufWritePre", {
            buffer = bufnr,
            callback = function()
              vim.lsp.buf.format { async = false }
            end,
          })

          -- Keybindings
          local opts = { noremap=true, silent=true, buffer=bufnr }
          vim.keymap.set('n', 'gd', vim.lsp.buf.definition, opts)
          vim.keymap.set('n', 'gr', vim.lsp.buf.references, opts)
          vim.keymap.set('n', 'K', vim.lsp.buf.hover, opts)
          vim.keymap.set('n', '<leader>ca', vim.lsp.buf.code_action, opts)
          vim.keymap.set('n', '<leader>rn', vim.lsp.buf.rename, opts)
          vim.keymap.set('n', '<leader>f', function()
            vim.lsp.buf.format { async = true }
          end, opts)
        end,
      }
    end,
  },

  -- Completion (if not already installed)
  {
    'hrsh7th/nvim-cmp',
    dependencies = {
      'hrsh7th/cmp-nvim-lsp',
      'L3MON4D3/LuaSnip',
    },
    config = function()
      local cmp = require('cmp')
      cmp.setup{
        sources = {
          { name = 'nvim_lsp' },
        },
        mapping = cmp.mapping.preset.insert({
          ['<C-b>'] = cmp.mapping.scroll_docs(-4),
          ['<C-f>'] = cmp.mapping.scroll_docs(4),
          ['<C-Space>'] = cmp.mapping.complete(),
          ['<C-e>'] = cmp.mapping.abort(),
          ['<CR>'] = cmp.mapping.confirm({ select = true }),
        }),
      }
    end,
  },
}
```

Then restart Neovim or run:
```vim
:Lazy sync
```

#### Using packer.nvim

Add to `~/.config/nvim/lua/plugins.lua` or your packer configuration:

```lua
use {
  'eventb-rossi/rossi',
  ft = 'eventb',
}

use {
  'neovim/nvim-lspconfig',
  config = function()
    require('lspconfig').eventb.setup{}
  end
}

use {
  'hrsh7th/nvim-cmp',
  requires = {
    'hrsh7th/cmp-nvim-lsp',
  },
  config = function()
    local cmp = require('cmp')
    cmp.setup{
      sources = {
        { name = 'nvim_lsp' },
      },
    }
  end
}
```

Then run:
```vim
:PackerSync
```

#### Using vim-plug

Add to `~/.config/nvim/init.vim`:

```vim
call plug#begin()

" Event-B support
Plug 'eventb-rossi/rossi', { 'for': 'eventb' }

" LSP
Plug 'neovim/nvim-lspconfig'

" Completion
Plug 'hrsh7th/nvim-cmp'
Plug 'hrsh7th/cmp-nvim-lsp'

call plug#end()

" Configure LSP
lua << EOF
require('lspconfig').eventb.setup{}
EOF
```

Then run:
```vim
:PlugInstall
```

---

### Method 2: Manual Installation

If you prefer not to use a plugin manager or want to contribute to development:

#### Step 1: Copy Syntax Files

```bash
cd rossi/editors/neovim

# Create directories if they don't exist
mkdir -p ~/.config/nvim/syntax
mkdir -p ~/.config/nvim/ftdetect

# Copy syntax highlighting
cp syntax/eventb.vim ~/.config/nvim/syntax/

# Copy filetype detection
cp ftdetect/eventb.vim ~/.config/nvim/ftdetect/
```

#### Step 2: Copy LSP Configuration

**If using nvim-lspconfig:**

```bash
# Find your nvim-lspconfig installation directory
# It's usually in one of these locations:
#   ~/.local/share/nvim/site/pack/*/start/nvim-lspconfig/
#   ~/.local/share/nvim/lazy/nvim-lspconfig/

# Copy the LSP configuration
cp lua/lspconfig/eventb.lua ~/.local/share/nvim/site/pack/*/start/nvim-lspconfig/lua/lspconfig/

# Or if using lazy.nvim:
cp lua/lspconfig/eventb.lua ~/.local/share/nvim/lazy/nvim-lspconfig/lua/lspconfig/
```

**If NOT using nvim-lspconfig:**

Add to your `~/.config/nvim/init.lua`:

```lua
-- Define Rossi LSP client
vim.api.nvim_create_autocmd('FileType', {
  pattern = 'eventb',
  callback = function()
    vim.lsp.start({
      name = 'eventb-language-server',
      cmd = {'eventb-language-server'},
      root_dir = vim.fs.dirname(vim.fs.find({'.git', 'eventb.toml'}, { upward = true })[1]),
      settings = {
        rossi = {
          format = {
            useUnicode = true,
            indentation = "    ",
          },
        },
      },
    })
  end,
})
```

#### Step 3: Restart Neovim

```bash
nvim
```

---

## LSP Configuration

### Basic Configuration

Add to `~/.config/nvim/init.lua`:

```lua
-- Ensure nvim-lspconfig is loaded
require('lspconfig').eventb.setup{}
```

### Recommended Configuration with Keybindings

Add to `~/.config/nvim/init.lua`:

```lua
local lspconfig = require('lspconfig')

lspconfig.eventb.setup{
  -- Settings
  settings = {
    rossi = {
      format = {
        useUnicode = true,
        indentation = "    ",
        maxLineLength = 100, -- Parsed for future wrapping; not applied yet
      },
      diagnostics = {
        enabled = true,
        debounceMs = 500, -- Parsed for future debouncing; diagnostics are immediate
      },
    },
  },

  -- Keybindings and autocommands
  on_attach = function(client, bufnr)
    local opts = { noremap=true, silent=true, buffer=bufnr }

    -- Navigation
    vim.keymap.set('n', 'gd', vim.lsp.buf.definition, opts)
    vim.keymap.set('n', 'gD', vim.lsp.buf.declaration, opts)
    vim.keymap.set('n', 'gr', vim.lsp.buf.references, opts)
    vim.keymap.set('n', 'gi', vim.lsp.buf.implementation, opts)
    vim.keymap.set('n', 'K', vim.lsp.buf.hover, opts)
    vim.keymap.set('n', '<C-k>', vim.lsp.buf.signature_help, opts)

    -- Code actions
    vim.keymap.set('n', '<leader>ca', vim.lsp.buf.code_action, opts)
    vim.keymap.set('n', '<leader>rn', vim.lsp.buf.rename, opts)

    -- Formatting
    vim.keymap.set('n', '<leader>f', function()
      vim.lsp.buf.format { async = true }
    end, opts)

    -- Format on save
    vim.api.nvim_create_autocmd("BufWritePre", {
      buffer = bufnr,
      callback = function()
        vim.lsp.buf.format { async = false }
      end,
    })

    -- Diagnostics
    vim.keymap.set('n', '[d', vim.diagnostic.goto_prev, opts)
    vim.keymap.set('n', ']d', vim.diagnostic.goto_next, opts)
    vim.keymap.set('n', '<leader>e', vim.diagnostic.open_float, opts)
    vim.keymap.set('n', '<leader>q', vim.diagnostic.setloclist, opts)
  end,

  -- Capabilities (for nvim-cmp)
  capabilities = require('cmp_nvim_lsp').default_capabilities(),
}
```

### Custom Server Path

If `eventb-language-server` is not in your PATH:

```lua
require('lspconfig').eventb.setup{
  cmd = { '/full/path/to/eventb-language-server' },
  -- Or use:
  cmd = { vim.fn.expand('~/.cargo/bin/eventb-language-server') },
}
```

The language server pins sensible defaults for the newer LSP features, so no
extra setup is required to get them:

- **Semantic tokens** — applied automatically once the client attaches.
- **Selection range** — smart expand/shrink; bind `vim.lsp.buf.selection_range`.

---

## Symbol Input

Type ASCII and get Unicode as you type. The input method ships with the plugin
under `lua/eventb/`. Set it up once and enable it for Event-B buffers:

```lua
require('eventb.input').setup{
  enabled = true, -- master switch (rossi.input.enabled)
  eager = true,   -- eager combos; set false to keep only the \name leader
}

vim.api.nvim_create_autocmd('FileType', {
  pattern = 'eventb',
  callback = function()
    require('eventb.input').enable(0)
  end,
})
```

- **Eager combos**: `=>` → ⇒, `<=>` → ⇔, `&` → ∧, `|->` → ↦, `<:` → ⊆.
- **`\name` leader**: `\and` → ∧, `\to` → →, `\forall` → ∀, `\nat` → ℕ.

See the [README](README.md#symbol-input) for the full behavior and toggles.

---

## Snippets

Snippets are bundled in the VS Code JSON format under `snippets/` and load
through LuaSnip. Install [LuaSnip](https://github.com/L3MON4D3/LuaSnip) (it is
already a dependency of the nvim-cmp setup above), then point its `from_vscode`
loader at the bundled `snippets` directory:

```lua
require('luasnip.loaders.from_vscode').lazy_load({
  paths = { '/path/to/rossi/editors/neovim/snippets' },
})
```

If you installed the plugin with a plugin manager, use the plugin's runtime
path instead of a hard-coded path, for example:

```lua
require('luasnip.loaders.from_vscode').lazy_load({
  paths = { vim.fn.expand('~/.local/share/nvim/lazy/rossi/editors/neovim/snippets') },
})
```

Expand a prefix (`mch`, `ctx`, `evt`, `inv`, `grd`, `act`, …) in an
Event-B buffer with your LuaSnip expand key. See the
[README](README.md#snippets) for the prefix list.

---

## Verification

### Check LSP Status

```vim
:LspInfo
```

Should show:
```
Language client log: ~/.local/state/nvim/lsp.log
Detected filetype: eventb

1 client(s) attached to this buffer:
  Client: eventb-language-server (id: 1, bufnr: [1])
    filetypes: eventb
    cmd: eventb-language-server
```

### View LSP Logs

If something isn't working:

```vim
:lua vim.cmd('e'..vim.lsp.get_log_path())
```

---

## Troubleshooting

### Server Not Found

**Problem**: `:LspInfo` shows "No clients attached"

**Solution**:
```bash
# Check if server is in PATH
which eventb-language-server

# If not found, add to PATH or specify full path
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### Server Crashes on Startup

**Problem**: LSP starts but immediately crashes

**Solution**:
```bash
# Test the server manually
eventb-language-server

# Check Rust installation
cargo --version

# Rebuild the server
cd rossi
cargo clean
cargo install --path crates/eventb-lsp --force
```

### No Syntax Highlighting

**Problem**: Event-B file has no syntax highlighting

**Solution**:
```vim
" Check filetype
:set filetype?

" Should output: filetype=eventb
" If not, manually set it:
:set filetype=eventb

" If still no highlighting, check if syntax file exists:
:echo globpath(&rtp, 'syntax/eventb.vim')
```

### No Completions

**Problem**: `<C-Space>` doesn't trigger completions

**Solution**:
1. Ensure nvim-cmp is installed and configured
2. Check if LSP client is attached: `:LspInfo`
3. Verify cmp sources include LSP:
   ```lua
   require('cmp').setup{
     sources = {
       { name = 'nvim_lsp' },
     },
   }
   ```

### Unicode Characters Not Displaying

**Problem**: Operators show as �� or boxes

**Solution**:
1. Ensure UTF-8 encoding:
   ```vim
   :set encoding=utf-8
   :set fileencoding=utf-8
   ```

2. Install a Nerd Font:
   ```bash
   # Download and install from:
   # https://www.nerdfonts.com/
   ```

3. Configure your terminal to use the font

### Permission Denied

**Problem**: Cannot execute `eventb-language-server`

**Solution**:
```bash
# Make the binary executable
chmod +x ~/.cargo/bin/eventb-language-server

# Verify
ls -l ~/.cargo/bin/eventb-language-server
```

### LSP Configuration Not Found

**Problem**: `require('lspconfig').eventb` fails

**Solution**:

**If using nvim-lspconfig from a plugin manager**, copy the config file to the right location:

```bash
# Find your nvim-lspconfig directory
find ~/.local/share/nvim -name "nvim-lspconfig" -type d

# Copy the eventb.lua to the lua/lspconfig directory
cp editors/neovim/lua/lspconfig/eventb.lua <found-path>/lua/lspconfig/
```

**Alternatively**, define the LSP manually without nvim-lspconfig (see Method 2, Step 2 above).

---

## Next Steps

After successful installation:

1. **Customize**: Adjust keybindings and settings to your preference

## Resources

- **Main Repository**: https://github.com/eventb-rossi/rossi
- **Neovim LSP Documentation**: `:help lsp`
- **nvim-lspconfig**: https://github.com/neovim/nvim-lspconfig
- **nvim-cmp**: https://github.com/hrsh7th/nvim-cmp

## Getting Help

- **Issues**: https://github.com/eventb-rossi/rossi/issues
- **Discussions**: https://github.com/eventb-rossi/rossi/discussions
- **Neovim Help**: `:help lsp`, `:help lspconfig`
