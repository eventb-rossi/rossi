# eventb-lsp

[![crates.io](https://img.shields.io/crates/v/eventb-lsp?label=crates.io)](https://crates.io/crates/eventb-lsp)

Language Server Protocol (LSP) implementation for Event-B formal modeling language.

## Overview

This crate provides a Language Server Protocol implementation for Event-B
models. It is built on the `rossi` parser and is intended to be used from
VS Code, Neovim, Emacs, or any editor with LSP support.

### ✅ Current Features

- **Real-time diagnostics** - Syntax errors as you type, using parser recovery
- **Document synchronization** - Efficient incremental text updates using rope data structure
- **Document symbols** - Navigate your Event-B models with hierarchical outline
- **Code formatting** - Auto-format documents with Unicode or ASCII operators
- **Completion** - Keywords, operators, snippets, and identifiers from the current document and workspace
- **Hover** - Documentation for Event-B keywords, operators, built-ins, and known identifiers
- **Go-to-definition** - Local and cross-file navigation for declarations and `SEES` / `REFINES` / `EXTENDS`
- **Find references** - Identifier and component references, including workspace references
- **Workspace symbols** - Search indexed contexts, machines, events, variables, constants, and sets
- **Rename refactoring** - Rename identifiers and components across indexed documents
- **Semantic highlighting** - LSP semantic tokens for Event-B constructs
- **Document links** - Clickable links for `SEES`, `REFINES`, and `EXTENDS` targets
- **Code actions** - ASCII/Unicode operator conversion, missing-clause fixes, missing `END`, sorting, and rename hints
- **Folding** - Folding ranges for components, events, initialisation, and clauses
- **Signature help** - Parameter hints for quantifiers, lambdas, and set comprehensions

## Installation

The `eventb-language-server` binary ships **alongside the `rossi` CLI**: the VS
Code / VSCodium extension downloads it on first activation, and every
package-manager install of `rossi` (Homebrew, Scoop, Gentoo, Fedora COPR)
includes it — see the
[project README](https://github.com/eventb-rossi/rossi#installation).

### From crates.io

```bash
cargo install eventb-lsp
```

This installs `eventb-language-server` to `~/.cargo/bin/`.

### From source

```bash
cd crates/eventb-lsp
cargo install --path .
```

### From workspace root

```bash
cargo build --release --bin eventb-language-server
# Binary available at: target/release/eventb-language-server
```

## Usage

### VS Code

The easiest way to use the language server is with the VS Code extension (see `editors/vscode/`).

Alternatively, configure manually in `.vscode/settings.json`:

```json
{
  "rossi.languageServer.path": "/path/to/eventb-language-server"
}
```

### Neovim

Add to your `nvim-lspconfig` setup:

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

-- Define eventb_ls if not already defined
if not configs.eventb_ls then
  configs.eventb_ls = {
    default_config = {
      cmd = {'eventb-language-server'},
      filetypes = {'eventb'},
      root_dir = lspconfig.util.root_pattern('.git', 'eventb.toml') or lspconfig.util.path.dirname,
      settings = {},
    },
  }
end

-- Setup the language server
lspconfig.eventb_ls.setup{
  on_attach = on_attach,  -- Your custom on_attach function
  capabilities = capabilities,  -- Your capabilities
}
```

Create an autocommand to detect `.eventb` files:

```lua
vim.api.nvim_create_autocmd({'BufRead', 'BufNewFile'}, {
  pattern = '*.eventb',
  callback = function()
    vim.bo.filetype = 'eventb'
  end,
})
```

### Emacs

Use `lsp-mode`:

```elisp
(use-package lsp-mode
  :hook (eventb-mode . lsp)
  :config
  (add-to-list 'lsp-language-id-configuration '(eventb-mode . "eventb"))
  (lsp-register-client
   (make-lsp-client
    :new-connection (lsp-stdio-connection "eventb-language-server")
    :major-modes '(eventb-mode)
    :server-id 'eventb-ls)))

;; Define eventb-mode if not already defined
(define-derived-mode eventb-mode prog-mode "Event-B"
  "Major mode for editing Event-B files.")

(add-to-list 'auto-mode-alist '("\\.eventb\\'" . eventb-mode))
```

## Features in Detail

### Real-time Diagnostics

The server reports syntax errors as you edit. On-type diagnostics are debounced
by `rossi.diagnostics.debounceMs` (default 500 ms); `didOpen` and `didSave`
analyze immediately, and `0` disables debouncing:

```eventb
CONTEXT test
SETS
    STATUS
CONSTANS  <- Error: unknown keyword
    max
END
```

Errors appear with:
- Precise location (line and column)
- Clear error messages
- Error recovery (continues parsing after errors)

Semantic diagnostics from the static checker are not wired into the LSP yet.
See [Semantic Analysis Reuse](#semantic-analysis-reuse) for the intended
integration path.

### Document Symbols

Navigate your Event-B models with a hierarchical outline:

- **Contexts**: Sets, Constants, Axioms, Theorems
- **Machines**: Variables, Invariants, Events, Variant
- **Events**: Parameters, Guards, Witnesses, Actions

Use in VS Code:
- Outline view in sidebar
- Breadcrumb navigation
- Symbol search (`Ctrl+Shift+O` / `Cmd+Shift+O`)

### Code Formatting

Format Event-B documents with consistent style:

```bash
# Unicode operators (default): ∧, ∨, ⇒, ∈, ∀, ∃
# ASCII operators: /\, \/, =>, :, !, #
```

Use in VS Code:
- Format Document: `Shift+Alt+F` (Windows/Linux) or `Shift+Option+F` (Mac)
- Format on Save: Enable in settings

Configuration options:
```json
{
  "rossi.format.useUnicode": true,
  "rossi.format.indentation": "    "
}
```

### Completion and Hover

Completion includes Event-B keywords, operators, snippets, built-ins, local
identifiers, and symbols discovered through the workspace index. Operator
completion follows the configured Unicode/ASCII preference.

Hover provides compact documentation for keywords, operators, built-ins, and
known identifiers. For identifiers, the provider uses parsed document context
and cross-file information where available.

### Navigation and Refactoring

The server supports go-to-definition, find-references, workspace symbols, and
rename. Navigation resolves local declarations and cross-file references through
`SEES`, `REFINES`, and `EXTENDS` chains.

Rename works for identifiers and indexed components. It updates all references
that the workspace index can resolve.

### Display Features

Semantic tokens provide syntax-aware highlighting beyond TextMate grammar
highlighting. Folding ranges cover `CONTEXT`, `MACHINE`, `EVENT`,
`INITIALISATION`, and major clause sections.

Document links make `SEES`, `REFINES`, and `EXTENDS` targets clickable when the
referenced component is known to the workspace index.

### Code Actions

Implemented code actions include:

- Convert ASCII operators to Unicode and Unicode operators to ASCII
- Convert only the current selection between operator styles
- Add a missing `END` keyword when diagnostics indicate that shape
- Add common missing clauses for contexts and machines
- Sort `VARIABLES` and `CONSTANTS` clauses alphabetically
- Show a rename hint when the cursor is on an event name

### Signature Help

Signature help is available for universal and existential quantifiers, lambda
expressions, and set comprehensions. It supports both Unicode and ASCII forms.

## Semantic Analysis Reuse

The LSP currently reports parser diagnostics only. The `rossi-build` crate
already contains most of the static-checking machinery needed for semantic
diagnostics, and it should be reused rather than reimplemented in the LSP.

Reusable `rossi-build` surfaces:

- `Project`, `ProjectComponent`, and `build(Project)` for project-level static checking
- `BuildResult::diagnostics` and `Severity` for checker findings
- `TypeEnv` and `Type` for scoped type environments
- `infer::*` for type inference over expressions, predicates, constants, variables, and event parameters
- `checked_predicate::*` for free-identifier checks plus Rodin-canonical predicate, expression, and action forms
- `wellformed::*` for conservative type-shape checks over predicates, expressions, and actions
- `enrich::*` for inferred binder type stamping and set-comprehension lowering
- `normalize::*` for Rodin-style canonical formatting of checked formulas

Recommended integration path:

1. Add `rossi-build` as an `eventb-lsp` dependency.
2. Add a small LSP semantic diagnostics adapter instead of calling
   `rossi-build` internals directly from `server.rs`.
3. Build an in-memory `Project` from open `.eventb` documents and workspace
   components using `ProjectComponent` with default `RodinIds`.
4. Run `rossi_build::build(&project)` after successful parsing.
5. Map `rossi-build` diagnostics to LSP diagnostics by resolving diagnostic
   origins such as `Machine`, `Machine.inv1`, or `Machine.Event.grd1` back to
   labels and component names in source text.
6. Use exact label/name ranges when available; otherwise fall back to the
   component declaration or file-level range.

This reuse should cover scope errors, many type inference errors, conservative
well-typedness checks, missing `SEES` / `REFINES` / `EXTENDS` targets, circular
dependency diagnostics, and Rodin-style static-check drop/accuracy behavior.

It should not be presented as full proof support. `rossi-build` does not yet
generate proof obligations, prove invariants, or implement complete Event-B
well-definedness and refinement proof checks.

## Known Limitations

- LSP diagnostics are syntax-only until the semantic diagnostics adapter is added.
- Find-references and rename for variables, constants, sets, and parameters resolve from AST identifier spans and are scope-aware: a quantifier / lambda / comprehension / parameter binder of the same name is not confused with the symbol, and the after-state form `x'` is handled at its base. Component-name references and rename remain structural (whole-word) lookups, and the semantic-token recovery path still scans text for declarations in regions the parser could not recover.
- Semantic tokens are AST-driven: declarations, keywords, labels, comments, and identifier *usages* inside formula bodies (variables / constants / sets keep their declared kind; quantifier, lambda, and comprehension binders and event parameters are coloured as parameters).
- Workspace indexing is eager/basic; there is no LRU eviction, cancellation support, or parallel indexing yet.

## Development

### Building

```bash
cargo build
```

### Running

The server communicates via stdin/stdout using the LSP JSON-RPC protocol:

```bash
cargo run
```

Or with logging:

```bash
RUST_LOG=debug cargo run
```

### Testing

Run all tests:

```bash
cargo test
```

Run specific test module:

```bash
cargo test -p eventb-lsp formatting
```

### Logging

Control logging level with the `RUST_LOG` environment variable:

```bash
# Info level (default)
RUST_LOG=info eventb-language-server

# Debug level (verbose)
RUST_LOG=debug eventb-language-server

# Module-specific
RUST_LOG=eventb_lsp::server=debug eventb-language-server
```

Logs are written to stderr and include:
- Server lifecycle events
- Document operations (open/change/close)
- LSP requests and responses
- Parse errors and diagnostics

## Architecture

The server is organized into focused modules (principal ones shown):

```
eventb-lsp/src/
├── server.rs            # LSP protocol implementation (tower-lsp)
├── document.rs          # Document management (ropey, dashmap)
├── analysis.rs          # Document symbol extraction
├── cross_references.rs  # Workspace component index and dependency graph
├── completion.rs        # Completion provider
├── hover.rs             # Hover provider
├── definition.rs        # Go-to-definition provider
├── references.rs        # Find-references provider
├── rename.rs            # Rename provider
├── semantic_tokens.rs   # Semantic token provider
├── code_actions.rs      # Quick fixes and refactorings
├── folding.rs           # Folding range provider
├── signature_help.rs    # Signature help provider
├── document_links.rs    # Document links provider
├── selection_range.rs   # Selection range provider
├── symbols.rs           # Cursor symbol resolution (definition / references)
├── formula_walk.rs      # Formula AST walker (binder scope)
├── formatting.rs        # Formatting via the rossi pretty printer
├── config.rs            # Server configuration (the `rossi` settings section)
└── main.rs              # Entry point and initialization
```

### Components

1. **Server** (`server.rs`)
   - Implements LSP protocol using tower-lsp
   - Handles client communication
   - Manages LSP capabilities and requests

2. **Document Manager** (`document.rs`)
   - In-memory document storage with DashMap
   - Efficient text operations using ropey rope data structure
   - Incremental synchronization
   - Position ↔ offset conversion

3. **Analysis** (`analysis.rs`)
   - Extracts symbols from Event-B AST
   - Builds hierarchical symbol tree
   - Maps Event-B constructs to LSP symbol kinds

4. **Cross-reference Manager** (`cross_references.rs`)
   - Indexes workspace Event-B files
   - Tracks `SEES`, `REFINES`, and `EXTENDS`
   - Provides dependency and visibility information to navigation providers

5. **Feature Providers**
   - Completion, hover, definition, references, rename, workspace symbols,
     semantic tokens, document links, code actions, folding, and signature help
   - Each provider owns a narrow LSP feature and reuses shared document and
     cross-reference state where needed

6. **Formatting** (`formatting.rs`)
   - Integrates with rossi pretty printer
   - Configurable Unicode/ASCII operators
   - Custom indentation support

## Configuration

The server supports the following configuration options (passed via LSP `workspace/configuration`):

```typescript
interface RossiConfig {
  format: {
    useUnicode: boolean;      // Use Unicode operators (default: true)
    indentation: string;      // Indentation string (default: "    ")
    maxLineLength: number;    // Parsed for future wrapping behavior (default: 100)
  };
  diagnostics: {
    enabled: boolean;         // Enable diagnostics (default: true)
    debounceMs: number;       // On-type debounce window in ms (default: 500; 0 = inline)
  };
  completion: {
    enabled: boolean;         // Enable completion (default: true)
    triggerCharacters: string[];
  };
  trace: {
    server: "off" | "messages" | "verbose";
  };
}
```

## Performance

The LSP server is designed for responsive editing:

- **Parser**: Fast PEG-based parsing with pest
- **Text operations**: Efficient rope data structure for large files
- **Concurrency**: Uses tokio for async operations
- **Caching**: Maintains provider-specific caches for workspace and document state

Current limitations are listed in [Known Limitations](#known-limitations).

## Troubleshooting

### Server not starting

1. Check the server is installed:
   ```bash
   which eventb-language-server
   ```

2. Test manually:
   ```bash
   echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | eventb-language-server
   ```

3. Check logs:
   ```bash
   RUST_LOG=debug eventb-language-server 2> lsp.log
   ```

### Diagnostics not appearing

- Ensure the file has `.eventb` extension
- Check the language ID is set to "eventb"
- Verify the document opened successfully (check logs)

### Formatting not working

- Ensure document has valid Event-B syntax
- Check format settings in editor configuration
- Formatting requires successful parse (syntax errors prevent formatting)

## Contributing

Contributions are welcome! See the [main repository](https://github.com/eventb-rossi/rossi) for development guidelines.

### Code Quality

Before submitting changes:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](../../LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Resources

- **LSP Specification**: https://microsoft.github.io/language-server-protocol/
- **Event-B Language**: https://wiki.event-b.org/index.php/Event-B_Language
- **tower-lsp Documentation**: https://docs.rs/tower-lsp
- **Rodin Platform**: https://www.event-b.org/

