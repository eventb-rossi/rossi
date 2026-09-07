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
package-manager install of `rossi` (Homebrew, APT, Scoop, Gentoo, Fedora COPR)
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

The response is always the hierarchical `DocumentSymbol[]` form, never the flat
`SymbolInformation[]` one, with one root per component — a file holding several
components yields several roots. Every row carries a `detail` string naming the
Event-B construct it came from; those strings are a stable contract, listed
under [Document symbol vocabulary](#document-symbol-vocabulary).

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

### Operator Convention

`rossi.format.useUnicode` names the project's operator convention, and
formatting normalizes to it along with everything else. To keep only the
operators in line without reformatting, the server offers a
`source.fixAll.rossi` code action that rewrites every operator spelling to the
convention and changes nothing else: layout, comments, and labels stay as they
are. Being a `source.*` kind, editors can run it on save — in VS Code:

```json
{
  "[eventb]": {
    "editor.codeActionsOnSave": { "source.fixAll.rossi": "explicit" }
  }
}
```

To see deviations before saving, set `rossi.format.enforceUnicode`: every
ASCII operator spelling in code (comments and labels excluded) gets an advisory
diagnostic, code `ascii-operator`, naming its Unicode form, with a quick fix
for that one token; the fix-all action clears them all at once. The four
operators whose only Unicode glyph is a Rodin private-use character (`<+`,
`<<->`, `<->>`, `<<->>`) stay ASCII under either convention and are never
flagged. It is an opt-in for projects that keep their sources in Unicode and
has no effect with `rossi.format.useUnicode` off, where formatting and the
fix-all action would reintroduce what it flags.

The CI counterpart is `rossi fmt --check`, which fails on any file the
formatter would change, operator spellings included.

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
- Normalize every operator to the configured convention and nothing else
  (`source.fixAll.rossi`, for editors' run-on-save hooks)
- Add a missing `END` keyword when diagnostics indicate that shape
- Add common missing clauses for contexts and machines
- Sort `VARIABLES` and `CONSTANTS` clauses alphabetically
- Show a rename hint when the cursor is on an event name

### Signature Help

Signature help is available for universal and existential quantifiers, lambda
expressions, and set comprehensions. It supports both Unicode and ASCII forms.

## Integration Contract

Everything below is what a third-party extension or tool may rely on. The
document symbol vocabulary and the custom request are pinned by tests, and the
language id and wire behaviour by the wire-level suite, so none of it drifts
silently. Anything not listed here is an implementation detail.

### Language identity

| | |
|---|---|
| Language id | `eventb` |
| File extension | `.eventb` |
| Settings section | `rossi` — see [Configuration](#configuration) |
| Executable | `eventb-language-server`, speaking LSP over stdio |
| Position encoding | UTF-16 code units, the LSP default |

### Document symbol vocabulary

`textDocument/documentSymbol` is the supported way to enumerate the components
in a file: every root of the response *is* a component, so no bespoke request
is needed and none is offered. Scanning the source with a regular expression is
not equivalent — component keywords are case-insensitive, `rossi fmt --style
rossi` writes `MACHINE` in upper case, and a comment or an identifier that
merely contains `machine` defeats the match.

For this machine:

```eventb
MACHINE m
VARIABLES
    v
INVARIANTS
    @inv1 v ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 v := 0
    END

    EVENT step
    ANY p
    WHERE
        @grd1 p ∈ ℕ
    THEN
        @act1 v := p
    END
END
```

the outline is:

```text
m                    MODULE          Machine
├─ v                 VARIABLE        Variable
├─ inv1              PROPERTY        Invariant
├─ INITIALISATION    CONSTRUCTOR     Event
│  └─ act1           PROPERTY        Action
└─ step              FUNCTION        Event
   ├─ p              TYPE_PARAMETER  Parameter
   ├─ grd1           PROPERTY        Guard
   └─ act1           PROPERTY        Action
```

The vocabulary is closed — these are all the values `detail` ever takes:

| `detail` | `kind` | Produced for |
|---|---|---|
| `Context` | `MODULE` | context root |
| `Machine` | `MODULE` | machine root |
| `Set` | `ENUM` | a `SETS` entry |
| `Constant` | `CONSTANT` | a `CONSTANTS` entry |
| `Axiom` | `PROPERTY` | an axiom |
| `Theorem` | `PROPERTY` | a `theorem` axiom or a `theorem` invariant |
| `Variable` | `VARIABLE` | a `VARIABLES` entry |
| `Invariant` | `PROPERTY` | an invariant |
| `Variant` | `NUMBER` | present when the machine has a `VARIANT` |
| `Event` | `CONSTRUCTOR` | `INITIALISATION` |
| `Event` | `FUNCTION` | an event with no `STATUS` clause |
| `Event (ordinary)` | `FUNCTION` | `STATUS ordinary` |
| `Event (convergent)` | `FUNCTION` | `STATUS convergent` |
| `Event (anticipated)` | `FUNCTION` | `STATUS anticipated` |
| `Parameter` | `TYPE_PARAMETER` | an `ANY` entry |
| `Guard` | `PROPERTY` | a `WHERE` / `WHEN` predicate |
| `With` | `PROPERTY` | a `WITH` predicate |
| `Witness` | `PROPERTY` | a `WITNESS` predicate |
| `Action` | `PROPERTY` | a `THEN` / `BEGIN` action |

Consuming it correctly means knowing five things:

- **Every row points at its own declaration**, so any of them can be navigated
  to. A row falls back to `(0,0)-(0,0)` only where there is genuinely no source
  text to point at — a component imported from Rodin XML, or a region the
  parser could not recover.
- **The `variant` row collapses the clause.** It is a single lower-case
  literal, emitted once even when the machine declares more than one `VARIANT`,
  and it points at the first of them.
- **`Theorem` is ambiguous** between a theorem axiom and a theorem invariant.
  The parent component tells them apart.
- **Bare `Event` is ambiguous** between `INITIALISATION` and a status-less
  event. The `kind` tells them apart, as does the name.
- **Unlabeled predicates are all named `unlabeled`**, so names are not unique
  within a parent.

`SymbolKind` is presentational and is recorded here as it stands today,
including one wart: `workspace/symbol` reports events as `SymbolKind::EVENT`,
while `documentSymbol` reports them as `FUNCTION` and `INITIALISATION` as
`CONSTRUCTOR`. Match on `detail`, not on `kind`, when the two must agree.

### Custom requests

`rossi/operatorTable` is the only non-standard method the server implements,
and it exists solely because the operator table has no standard equivalent —
it lets an editor build an input method without duplicating the mapping in its
own source. It takes **no** `params`, and returns one row per operator
spelling. Everything else the server offers is a standard LSP method.

Three `workspace/executeCommand` commands are also registered —
`rossi.rodin.open`, `rossi.animate.check` and `rossi.animate.po`. They drive
external tools and are meant for the bundled extensions rather than for
general integration.

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

Settings live in the `rossi` section and reach the server over LSP
`workspace/configuration`. Clients that send the section on its own
(`{"format": ...}`) and clients that send the whole settings object
(`{"rossi": {"format": ...}}`) are both accepted.

These are the keys the server itself reads. An empty string or `null` means
"follow the formatting style preset" rather than a literal value, and an
unparseable object is rejected as a whole, leaving the previous configuration
in place.

```typescript
interface RossiConfig {
  format: {
    style: "" | "camille" | "rossi";       // preset; "" selects the default
    useUnicode: boolean;                   // default: true
    enforceUnicode: boolean;               // flag ASCII spellings; false
    indentation: string;                   // default: "" (2 camille, 4 rossi)
    keywordCase: "" | "lower" | "upper";   // default: ""
    declLists: "" | "inline" | "one-per-line";  // default: ""
    blankBetweenClauses: boolean | null;   // default: null (follow preset)
    maxLineWidth: number;                  // default: 120; 0 disables wrapping
  };
  diagnostics: {
    enabled: boolean;                      // default: true
    debounceMs: number;                    // default: 500; 0 = inline
  };
  completion: {
    enabled: boolean;                      // default: true
  };
  inlayHints: {
    enabled: boolean;                      // inferred types; default: true
    wellDefinedness: boolean;              // "WD" markers; default: true
    maxLength: number;                     // default: 32; 0 disables truncation
  };
  rodin: {
    path: string;                          // default: "" (platform default)
    workspace: string;                     // default: "" (<root>/.rossi/rodin)
    sync: boolean;                         // save-driven sync; default: true
    bridge: boolean;                       // use the plug-in; default: true
    liveSync: boolean;                     // merge unsaved edits; default: false
    mirrorProofs: boolean;                 // default: true
  };
  animate: {
    path: string;                          // default: "" (resolve on PATH)
    timeLimitSecs: number;                 // default: 120; 0 selects it
    disproveTimeoutMs: number;             // default: 1000; 0 selects it
  };
}
```

Other `rossi.*` keys — `rossi.tool.path`, `rossi.languageServer.path`,
`rossi.validate.onSave`, `rossi.input.*` and `rossi.trace.server` — belong to
the editor extension rather than to the server, which never sees them. The VS
Code extension declares the full set with descriptions in
[`editors/vscode/package.json`](../../editors/vscode/package.json).

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
