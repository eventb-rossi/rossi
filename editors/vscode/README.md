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
- Semantic highlighting from the language server is on by default, so
  identifiers, carrier sets, constants, labels, and event parameters render
  distinctly under any theme — no configuration required
- The ambiguous / non-basic-ASCII Unicode warnings are turned off for `.eventb`
  files, so math operators (`∀ ∃ ⇒ ∈ ↦ ℕ`) don't trigger spurious warnings
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
- Format on save support, or normalize only the operators on save
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
(APT, Gentoo, Fedora COPR, …), verification, and troubleshooting.

## Extension Settings

This extension contributes the following settings. The four that name an
executable — `rossi.languageServer.path`, `rossi.tool.path`,
`rossi.rodin.path` and `rossi.animate.path` — are user/machine settings: a
workspace's own `.vscode/settings.json` cannot supply them, so cloning a
repository can never make the extension launch a program of that
repository's choosing. `rossi.rodin.workspace` may still be set per
workspace, but only in a trusted one.

- `rossi.languageServer.path`: Path to the Event-B language server executable (default: searches in PATH)
- `rossi.tool.path`: Path to the Rossi CLI executable used for import, export, build, validation, and conversion commands (default: `rossi`)
- `rossi.rodin.path`: Path to the Rodin IDE executable, macOS `.app` bundle, or app name used by the `Open in Rodin` code lens (defaults: `/Applications/Rodin.app` on macOS, `rodin.exe` on Windows, `rodin` on Linux)
- `rossi.rodin.workspace`: Directory used as the shared Rodin workspace by the `Open in Rodin` code lens; proofs made in Rodin persist there (default: `.rossi/rodin` inside the workspace folder)
- `rossi.rodin.sync`: Mutual live synchronization with a running Rodin — saves rebuild the project while Rodin is open, and edits saved in Rodin flow back into the sources (default: `true`)
- `rossi.rodin.mirrorProofs`: Bridge proof files (`.bpr`/`.bps`/`.bpo`) between the checkout and the Rodin workspace at `Open in Rodin` session boundaries — seed the project from files next to the sources when the lens runs, mirror the project's files back when Rodin exits (default: `true`)
- `rossi.format.style`: Formatting style preset — `"camille"` (lowercase keywords, inline declaration lists, 2-space indent) or `"rossi"` (uppercase keywords, one-per-line lists, 4-space indent); empty follows the language server's default preset (default: `""`)
- `rossi.format.useUnicode`: Use Unicode operators (∧, ∨, ⇒, ∈) instead of ASCII (/\, \/, =>, :) when formatting (default: `true`)
- `rossi.format.enforceUnicode`: Flag ASCII operator spellings (`/\`, `:`, `NAT`, …) outside comments and labels with an advisory diagnostic and a quick fix, for a project that keeps its sources in Unicode (no effect with `rossi.format.useUnicode` off; the private-use operators `<+`, `<<->`, `<->>`, `<<->>` stay ASCII and are not flagged unless `rossi.format.privateUseGlyphs` is on); pair with the `source.fixAll.rossi` action on save and `rossi fmt --check` in CI (default: `false`)
- `rossi.format.privateUseGlyphs`: Spell the four relation/override operators with Rodin's private-use glyphs (U+E100..E103) instead of ASCII `<<->`, `<->>`, `<<->>` and `<+`, for exchanging files with a tool that reads only Rodin's spelling — formatting, completion and hover follow immediately, the as-you-type input method after a window reload (it loads the operator table once, at activation); no effect with `rossi.format.useUnicode` off; the glyphs need Rodin's Brave Sans Mono font, which `Rossi: Install the Rodin Math Font` installs for you (default: `false`)
- `rossi.format.indentation`: Indentation string (spaces or tabs) to use when formatting; empty follows the style preset (default: `""`)
- `rossi.format.keywordCase`: Keyword-case override — `"lower"` or `"upper"`; empty follows the style preset (default: `""`)
- `rossi.format.declLists`: Declaration-list layout override — `"inline"` or `"one-per-line"`; empty follows the style preset (default: `""`)
- `rossi.format.blankBetweenClauses`: Blank line before each top-level clause keyword; unset follows the style preset (default: `null`)
- `rossi.format.maxLineWidth`: Maximum line width when formatting, in characters; long formulas wrap onto operator-leading continuation lines, `0` disables wrapping (default: `120`)
- `rossi.inlayHints.enabled`: Show inferred declaration types as inlay hints after machine variables, event parameters, and context constants; rendering also honours VS Code's `editor.inlayHints.enabled` master switch (default: `true`)
- `rossi.inlayHints.wellDefinedness`: Mark formulas carrying a non-trivial well-definedness condition with a `WD` inlay hint whose tooltip shows the condition (default: `true`)
- `rossi.inlayHints.maxLength`: Maximum rendered length of a type hint in characters; longer types are truncated with `…` and shown in full in the hint tooltip, `0` disables truncation (default: `32`)
- `rossi.diagnostics.enabled`: Enable real-time diagnostics for syntax errors (default: `true`)
- `rossi.diagnostics.debounceMs`: Reserved for future diagnostic debouncing; diagnostics currently run immediately after typing (default: `500`)
- `rossi.completion.enabled`: Enable Event-B code completion (default: `true`)
- `rossi.input.enabled`: Convert ASCII to Unicode math symbols as you type — eager combos and the `\name` leader (default: `true`)
- `rossi.input.eager`: Eagerly substitute symbolic combos (`=>`, `<=>`, `|->`, `:=`) while typing; when `false`, only the `\name` leader converts (default: `true`)
- `rossi.trace.server`: Traces communication between VS Code and the language server (default: `"off"`)

### Example Configuration

The executable paths belong in your **user** `settings.json`
(`Ctrl+Shift+P` → *Preferences: Open User Settings (JSON)*):

```json
{
  "rossi.languageServer.path": "/path/to/eventb-language-server", // only if not in PATH
  "rossi.tool.path": "/path/to/rossi", // only if not in PATH
  "rossi.rodin.path": "/Applications/Rodin.app" // only if Rodin isn't at the platform default
}
```

Everything else can live in the project's `.vscode/settings.json`:

```json
{
  "rossi.format.style": "camille",
  "rossi.format.useUnicode": true,
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
        @act1 count := 0
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
- **Normalize operators on save** without reformatting: the `source.fixAll.rossi`
  code action rewrites every operator spelling to `rossi.format.useUnicode` and
  leaves layout, comments, and labels alone. Enable it per language in the
  project's `.vscode/settings.json`:

  ```json
  {
    "[eventb]": {
      "editor.codeActionsOnSave": { "source.fixAll.rossi": "explicit" }
    }
  }
  ```

  `rossi fmt --check` is the CI counterpart: it fails on any file the formatter
  would change, operator spellings included.
- **See the convention while editing**: set `rossi.format.enforceUnicode` to
  `true` to flag every ASCII operator spelling with an advisory diagnostic and a
  quick fix before the save rewrites it

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
- `Rossi: Build Checked Rodin ZIP`
- `Rossi: Validate Current File`
- `Rossi: Validate Workspace`
- `Rossi: Convert Current File to Unicode`
- `Rossi: Convert Current File to ASCII`
- `Rossi: Check Toolchain`
- `Rossi: Install the Rodin Math Font`

Rodin and conversion commands shell out to the configured `rossi.tool.path`.
`Install the Rodin Math Font` needs no toolchain: it copies the bundled Brave
Sans Mono Roman into your own font directory (no administrator rights on any
platform), then offers to list it as a fallback for Event-B files, so
`rossi.format.privateUseGlyphs` renders. The extension never sets your editor
font on its own. See [INSTALL.md](INSTALL.md#the-rodin-math-font).

### Open in Rodin

An **Open in Rodin** code lens appears above every `MACHINE`/`CONTEXT` header
(provided by the language server, so it works the same in other editors). It
builds the file's directory into a persistent Rodin workspace — `.rossi/rodin`
next to your sources by default (add `.rossi/` to `.gitignore`); override with
`rossi.rodin.workspace` — and launches the Rodin IDE configured via
`rossi.rodin.path` on it. Because the workspace persists, proofs made in Rodin
live alongside the generated proof obligations and survive rebuilds: clicking
the lens again after editing the model reconciles the regenerated obligations
with the recorded proof state, so unchanged obligations keep their proofs.

While Rodin stays open, the two tools keep each other current
(`rossi.rodin.sync`, on by default). Saving an `.eventb` file rebuilds the
Rodin project in the background, and Rodin picks the files up within a few
seconds — its builder, proof obligations, and Explorer all update, but Rodin
editors already open on a component keep showing the old content until you
reopen them (or press F5 inside the editor). In the other direction, saving
a machine or context in Rodin updates the corresponding `.eventb` file — or
your open buffer — automatically via a three-way merge; when both sides
changed the same lines, the conflict lands in the source with git-style
markers and a warning.

When Rodin is already open on the workspace, clicking the lens can only rebuild
the project: registering a new one needs the same Eclipse instance area Rodin is
holding, so you are told to use `File > Import`. The **Rodin bridge** removes
that step. It is a plug-in shipped with the
[eventb-rossi Rodin bundle](https://github.com/eventb-rossi/Rodin-Bundle) that
opens a loopback socket inside Rodin; when it is there, the lens registers the
project in the running instance and brings it forward instead. Nothing is
required of you: with stock Rodin, or with `rossi.rodin.bridge` turned off,
everything above behaves exactly as described.

Editing in Rodin can reach your buffer before you save there
(`rossi.rodin.liveSync`, off by default). The Rodin Editor writes every
keystroke through to its database rather than holding it in the widget, so the
bridge can report a rename the moment it is typed and the language server
merges it into the `.eventb` file. Where you and Rodin have changed the same
lines it leaves the buffer alone and tries again on the next edit, so conflict
markers never appear while you type.

Proof files travel with the sources too (`rossi.rodin.mirrorProofs`, on by
default). When the lens runs, `.bpr`/`.bps`/`.bpo` files sitting next to the
`.eventb` sources — placed there by `rossi import` or a `git pull` — are
copied into the Rodin project before Rodin opens; when Rodin exits, the
project's proof files are copied back next to the sources, so proof work
lands in version control without a manual `rossi export --proofs`. The
checkout wins at session start and the workspace at session end: a proof
deleted in Rodin is deleted next to the sources too, while deleting a proof
file only in git does not stick — it returns from the workspace when the
session ends. Commit all three extensions. The exit mirror relies on the
Eclipse workspace lock probe and is unavailable on Windows.

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
