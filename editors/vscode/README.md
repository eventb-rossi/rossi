[![VS Code Marketplace](https://vsmarketplacebadges.dev/version-short/rossi.event-b.svg)](https://marketplace.visualstudio.com/items?itemName=rossi.event-b)
[![Open VSX](https://img.shields.io/open-vsx/v/rossi/event-b?label=Open%20VSX)](https://open-vsx.org/extension/rossi/event-b)

# Event-B (Rossi) for VS Code

This extension provides comprehensive language support for Event-B formal modeling in Visual Studio Code, powered by the Rossi Language Server.

## Installation

Install **Event-B (Rossi)** from your editor's marketplace:

- **VS Code** — the [Marketplace](https://marketplace.visualstudio.com/items?itemName=rossi.event-b),
  the Extensions view (`Ctrl+Shift+X` → search "Event-B"), or
  `code --install-extension rossi.event-b`
- **VSCodium** — [Open VSX](https://open-vsx.org/extension/rossi/event-b) or
  `codium --install-extension rossi.event-b`

On first activation the extension downloads the prebuilt `eventb-language-server`
and `rossi` binaries for your platform (Linux/macOS/Windows, x86_64 or ARM64),
verifies them against the release `SHA256SUMS`, and caches them — so for most
users no further setup is needed. See [INSTALL.md](INSTALL.md) for step-by-step
setup, verification, and troubleshooting.

## Features

### 🎨 Syntax Highlighting
- Full syntax highlighting for Event-B constructs
- Support for both Unicode (∧, ∨, ⇒, ∈) and ASCII operators (/\, \/, =>, :)
- Syntax highlighting for:
  - Keywords (CONTEXT, MACHINE, EVENTS, etc.)
  - Logical operators
  - Set operators
  - Relation operators
  - Arithmetic operators

### 🔍 Real-time Diagnostics
- Instant feedback on syntax errors as you type
- Error recovery continues parsing after errors
- Clear error messages with precise locations

### 🗂️ Document Symbols & Navigation
- Hierarchical outline view in sidebar
- Breadcrumb navigation at the top of the editor
- Quick symbol search with `Ctrl+Shift+O` (Windows/Linux) or `Cmd+Shift+O` (Mac)
- Navigate through:
  - Contexts: Sets, Constants, Axioms, Theorems
  - Machines: Variables, Invariants, Events, Variant
  - Events: Parameters, Guards, Witnesses, Actions

### ✨ Code Formatting
- Auto-format documents with consistent style
- Choose between Unicode or ASCII operators
- Configurable indentation
- Format on save support
- Keyboard shortcuts:
  - Format Document: `Shift+Alt+F` (Windows/Linux) or `Shift+Option+F` (Mac)

### 📋 Code Snippets

Type a prefix and press Tab:

| Prefix | Description |
|--------|-------------|
| `ctx` | Context template |
| `mch` | Machine template |
| `evt` | Event with guards and actions |
| `init` | Initialisation event |
| `refines` | Event refining an abstract event |
| `axm` | Labeled axiom |
| `inv` | Labeled invariant |
| `grd` | Labeled guard |
| `act` | Labeled deterministic assignment |
| `actnd` | Labeled non-deterministic assignment (`:∈`) |
| `actst` | Labeled assignment with predicate (`:|`) |
| `forall` | Universal quantification |
| `exists` | Existential quantification |
| `lambda` | Lambda abstraction |
| `setcomp` | Set comprehension |

### ⌨️ Symbol Input (type ASCII, get Unicode)
- Convert ASCII to Unicode math symbols **as you type** — no special keyboard needed
- **Eager combos** convert on the fly: `=>` → ⇒, `<=>` → ⇔, `&` → ∧, `|->` → ↦, `:=` → ≔, `<:` → ⊆
- **`\name` leader** expands any operator on a boundary: `\and` → ∧, `\to` → →, `\forall` → ∀, `\nat` → ℕ
- Maximal munch handles ambiguous prefixes (`<=` → ≤ but `<=>` → ⇔)
- Each conversion is one undo step, so `Ctrl+Z` restores the ASCII you typed
- Toggle with `rossi.input.enabled`; disable only the eager combos with `rossi.input.eager`

### 🔁 Rodin Interoperability
- Import Rodin `.zip`, `.buc`, `.bum`, or XML project folders into `.eventb` files
- Export the current `.eventb` file or workspace to a Rodin `.zip`
- Open the current `.eventb` file or workspace in the Rodin IDE as a temporary one-way Rodin workspace
- Build checked Rodin `.zip` archives with generated `.bcc` / `.bcm` files
- Run on-demand validation and show results in VS Code Problems
- Convert the current `.eventb` file between Unicode and ASCII notation

## Requirements

The extension uses the Rossi Language Server (`eventb-language-server`) for
editor features and the `rossi` CLI for the Rodin import/export/build/validation
commands. **It downloads both for you on first activation** (see
[Installation](#installation)), so usually nothing else is required. `Open in
Rodin` additionally requires the Rodin IDE executable or macOS `.app` bundle.

### Installing the binaries yourself

For an unsupported platform, an offline machine, or a custom build, install the
binaries yourself and the extension will pick them up from your `PATH` (or point
`rossi.languageServer.path` / `rossi.tool.path` at them):

```bash
# Homebrew (macOS / Linux)
brew tap eventb-rossi/tap && brew install rossi

# Scoop (Windows)
scoop bucket add eventb https://github.com/eventb-rossi/scoop-eventb
scoop install eventb/rossi

# cargo
cargo install rossi-cli eventb-lsp
```

The package managers (and `cargo`) install both `rossi` and
`eventb-language-server`. To build from source instead, run
`cargo build --release --bin eventb-language-server --bin rossi` from the project
root and add `target/release/` to your `PATH`. See [INSTALL.md](INSTALL.md) and
the [main Installation guide](../../README.md#installation) for the full matrix
(Gentoo, Fedora COPR, …), verification, and troubleshooting.

## Extension Settings

This extension contributes the following settings:

- `rossi.languageServer.path`: Path to the Event-B language server executable (default: searches in PATH)
- `rossi.tool.path`: Path to the Rossi CLI executable used for import, export, build, validation, and conversion commands (default: `rossi`)
- `rossi.rodin.path`: Path to the Rodin IDE executable, macOS `.app` bundle, or app name used by `Open in Rodin` (defaults: `/Applications/Rodin.app` on macOS, `rodin.exe` on Windows, `rodin` on Linux)
- `rossi.format.useUnicode`: Use Unicode operators (∧, ∨, ⇒, ∈) instead of ASCII (/\, \/, =>, :) when formatting (default: `true`)
- `rossi.format.indentation`: Indentation string (spaces or tabs) to use when formatting (default: `"    "` - 4 spaces)
- `rossi.format.maxLineLength`: Maximum line length for future formatter wrapping behavior (default: `100`)
- `rossi.diagnostics.enabled`: Enable real-time diagnostics for syntax errors (default: `true`)
- `rossi.diagnostics.debounceMs`: Reserved for future diagnostic debouncing; diagnostics currently run immediately after typing (default: `500`)
- `rossi.completion.enabled`: Enable Event-B code completion (default: `true`)
- `rossi.input.enabled`: Convert ASCII to Unicode math symbols as you type — eager combos and the `\name` leader (default: `true`)
- `rossi.input.eager`: Eagerly substitute symbolic combos (`=>`, `<=>`, `|->`, `:=`) while typing; when `false`, only the `\name` leader converts (default: `true`)
- `rossi.trace.server`: Traces communication between VS Code and the language server (default: `"off"`)

### Example Configuration

Add to your `.vscode/settings.json`:

```json
{
  "rossi.languageServer.path": "/path/to/eventb-language-server", // only if not in PATH
  "rossi.tool.path": "/path/to/rossi", // only if not in PATH
  "rossi.rodin.path": "/Applications/Rodin.app", // only if Rodin isn't at the platform default
  "rossi.format.useUnicode": true,
  "rossi.format.indentation": "    ",
  "rossi.format.maxLineLength": 100,
  "rossi.diagnostics.enabled": true,
  "rossi.diagnostics.debounceMs": 500,
  "rossi.completion.enabled": true,
  "rossi.input.enabled": true,
  "rossi.input.eager": true,
  "editor.formatOnSave": true
}
```

## Usage

### Creating Event-B Files

1. Create a new file with `.eventb` extension
2. Start typing Event-B code
3. Enjoy syntax highlighting, diagnostics, and navigation

### Example: Simple Context

```eventb
CONTEXT counter_ctx
SETS
    STATUS
CONSTANTS
    max_value
AXIOMS
    @axm1 max_value = 100
    @axm2 max_value > 0
END
```

### Example: Simple Machine

```eventb
MACHINE counter
VARIABLES
    count
INVARIANTS
    @inv1 count >= 0
    @inv2 count <= 100
EVENTS
    EVENT INITIALISATION
    BEGIN
        count := 0
    END

    EVENT increment
    WHERE
        @grd1 count < 100
    THEN
        @act1 count := count + 1
    END
END
```

### Formatting

- **Format entire document**: `Shift+Alt+F` (Windows/Linux) or `Shift+Option+F` (Mac)
- **Format on save**: Enable `"editor.formatOnSave": true` in settings
- **Choose operator style**: Set `rossi.format.useUnicode` to `true` (Unicode) or `false` (ASCII)

### Symbol Input

Type ASCII and get Unicode without leaving the keyboard. Two ways, both on by default:

- **Eager combos** — symbolic operators convert as soon as they are unambiguous:
  - `=>` → ⇒, `<=>` → ⇔, `&` → ∧, `|->` → ↦, `:=` → ≔, `:` → ∈, `<:` → ⊆, `..` → ‥
  - Longest-match wins: `<=` becomes ≤ only once you type a non-`>` character, while `<=>` becomes ⇔.
- **`\name` leader** — type a backslash, an operator name, then a space or any boundary character:
  - `\and` → ∧, `\or` → ∨, `\not` → ¬, `\to` → →, `\forall` → ∀, `\exists` → ∃, `\in` → ∈, `\nat` → ℕ, `\int` → ℤ, `\pow` → ℙ
  - The leader is also how you enter alphabetic operators (`NAT`, `or`, …) — these are never converted eagerly so they don't interfere with ordinary text.

Press `Ctrl+Z` right after a conversion to restore your ASCII. Turn the feature off with `rossi.input.enabled`, or keep only the leader by setting `rossi.input.eager` to `false`. This complements the whole-file `Rossi: Convert Current File to Unicode/ASCII` commands.

### Symbol Navigation

- **Outline view**: Open the Outline panel in the sidebar (Explorer view)
- **Breadcrumbs**: Navigate using breadcrumbs at the top of the editor
- **Symbol search**: Press `Ctrl+Shift+O` (Windows/Linux) or `Cmd+Shift+O` (Mac) to search symbols in the current file

### Rodin Commands

Open the Command Palette and run:

- `Rossi: Import Rodin Project`
- `Rossi: Export Current File to Rodin ZIP`
- `Rossi: Export Workspace to Rodin ZIP`
- `Rossi: Open in Rodin`
- `Rossi: Build Checked Rodin ZIP`
- `Rossi: Validate Current File`
- `Rossi: Validate Workspace`
- `Rossi: Convert Current File to Unicode`
- `Rossi: Convert Current File to ASCII`
- `Rossi: Check Toolchain`

Rodin and conversion commands shell out to the configured `rossi.tool.path`. `Open in Rodin` exports a temporary Rodin project, registers it in a temporary workspace through Rodin’s headless Ant runner, and launches the configured `rossi.rodin.path`; edits made in Rodin are not synced back to `.eventb` files.

## Contributing

Contributions are welcome! See the [main repository](https://github.com/eventb-rossi/rossi) for development guidelines.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](../../LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Resources

- [Event-B Language](https://wiki.event-b.org/index.php/Event-B_Language)
- [Rodin Platform](https://www.event-b.org/)
- [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
