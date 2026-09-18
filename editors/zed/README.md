# Rossi Event-B for Zed

[Zed](https://zed.dev) support for [Event-B](https://www.event-b.org/) formal
models, powered by the Rossi language server (`eventb-language-server`) and a
tree-sitter grammar generated from the same canonical token tables as every
other Rossi editor integration.

## Features

Syntax highlighting comes from the standalone
[`tree-sitter-eventb`](https://github.com/eventb-rossi/tree-sitter-eventb)
grammar (see the Grammar note below); everything else is provided by
`eventb-language-server` over LSP:

- Real-time diagnostics
- Code completion (including the `\name` leader, e.g. `\and` → ∧, `\to` → →)
- Hover documentation, go-to-definition, find references
- Rename, document/workspace symbols, document links
- Formatting (Unicode or ASCII operators), code actions (quick fixes, refactors)
- Folding ranges, selection ranges, signature help
- Document highlight, type hierarchy (REFINES/EXTENDS), go-to-implementation,
  go-to-type-definition, pull diagnostics, range formatting
- The project static check (type errors over the dependency closure) and, as
  hint diagnostics, each file's open proof obligations

Zed can also overlay the server's **semantic tokens** on top of the tree-sitter
highlighting for richer, meaning-aware colors — see Configuration.

> **Not available in Zed:** the **Open in Rodin** feature other editors get as
> a CodeLens on MACHINE/CONTEXT declarations. Zed does not render LSP CodeLens,
> so the lens (and with it the one-click Rodin launch) has no surface here. The
> `rossi.rodin.*` settings below are still forwarded to the server, so the
> feature lights up without reconfiguration once Zed gains CodeLens support.
> The same applies to the `n/m proof obligations discharged` lens. The proof
> obligation tree, gutter marks and sequent document exist only in the VS Code
> extension; the `rossi/proofObligations` and `rossi/proofState` requests they
> use are documented in the language server's README for another client to
> adopt.

## Prerequisites

The extension expects the language server (`eventb-language-server`) on your
`PATH`. Install it via your package manager (Homebrew, APT, Scoop, Gentoo, or
Fedora COPR — each installs it alongside the `rossi` CLI) or with `cargo install
eventb-lsp`; see the [main Installation guide](../../README.md#installation).
From a clone, build it with:

```bash
cargo install --path crates/eventb-lsp   # installs `eventb-language-server`
```

## Installing the extension

This extension is developed in-tree under `editors/zed/`. Install it as a dev
extension:

1. Open Zed → command palette → **zed: install dev extension**.
2. Select the `editors/zed/` directory.

Opening any `.eventb` file then activates highlighting and the language server.

> **Grammar note.** Zed loads tree-sitter grammars from a git repository pinned
> to a revision — it has no local-path option. Highlighting uses the standalone
> [`tree-sitter-eventb`](https://github.com/eventb-rossi/tree-sitter-eventb)
> grammar (hand-maintained in its own repository, vendored in this monorepo as
> the `editors/tree-sitter-eventb/` submodule), pinned in `extension.toml` to
> the same commit as that submodule.
> Until that repository is published, follow the local-development workflow in
> [INSTALL.md](INSTALL.md) to point `extension.toml` at a local `file://` repo.

## Configuration

Add to your Zed `settings.json` (per-language keys live under
`languages."Event-B"`):

```json
{
  "languages": {
    "Event-B": {
      "tab_size": 4,
      "semantic_tokens": "combined",
      "document_symbols": "on",
      "document_folding_ranges": "on",
      "inlay_hints": { "enabled": true }
    }
  },
  "lsp": {
    "eventb-language-server": {
      "settings": {
        "rossi": {
          "format": { "style": "camille", "useUnicode": true, "maxLineWidth": 120 },
          "diagnostics": { "enabled": true },
          "completion": { "enabled": true },
          "inlayHints": { "enabled": true, "wellDefinedness": true, "maxLength": 32 },
          "rodin": { "path": "", "workspace": "" }
        }
      }
    }
  }
}
```

What each per-language key buys you (all default to `"off"` in Zed):

| Key | Effect |
| --- | --- |
| `semantic_tokens` | `"combined"` overlays the server's semantic tokens on the tree-sitter base; `"full"` uses the server's tokens exclusively. |
| `document_symbols` | `"on"` sources the outline and breadcrumbs from the server's `textDocument/documentSymbol` (the grammar ships no `outline.scm`). |
| `document_folding_ranges` | `"on"` uses the server's folding ranges instead of indentation/tree-sitter. |
| `inlay_hints` | `{ "enabled": true }` renders the server's inlay hints: inferred declaration types and `WD` well-definedness markers (which hints the server emits is set under `rossi.inlayHints`). |

### Server binary and settings

- **Binary discovery.** The extension uses `eventb-language-server` from your
  `PATH`. To pin a specific build, set
  `lsp."eventb-language-server".binary.path` to an absolute path (and optionally
  `binary.arguments`).
- **Server options.** Everything under `lsp."eventb-language-server".settings`
  is forwarded to the server. Nest options under `rossi` as shown above; the
  available options (`format`, `diagnostics`, `completion`, `inlayHints`,
  `rodin`, `trace`) and their defaults match the Neovim integration
  (`editors/neovim/lua/lspconfig/eventb.lua`).

## ASCII → Unicode input

Zed extensions cannot install a live keystroke input method (unlike the VS Code
extension's `=>` → ⇒ substitution). Instead:

- Type the **`\name` leader** and accept the completion: `\and` → ∧, `\to` → →,
  `\nat` → ℕ, `\forall` → ∀, etc. (served by the language server).
- Convert a whole file from the terminal: `rossi fmt --in-place file.eventb`
  (to Unicode) or `rossi fmt --in-place --ascii file.eventb` (to ASCII).

## Tasks for the Rossi CLI

Zed extensions cannot contribute editor commands or menus, so the VS Code
extension's Rodin/validation commands are run via the `rossi` CLI. Drop this in
your project's `.zed/tasks.json`:

```json
[
  {
    "label": "Rossi: validate current file",
    "command": "rossi",
    "args": ["validate", "$ZED_FILE"]
  },
  {
    "label": "Rossi: format to Unicode",
    "command": "rossi",
    "args": ["fmt", "--in-place", "$ZED_FILE"]
  },
  {
    "label": "Rossi: convert to ASCII",
    "command": "rossi",
    "args": ["fmt", "--in-place", "--ascii", "$ZED_FILE"]
  },
  {
    "label": "Rossi: export to Rodin .zip",
    "command": "rossi",
    "args": ["export", "--output", "$ZED_DIRNAME/rodin-export.zip", "$ZED_FILE"]
  }
]
```

Run them with **task: spawn** from the command palette.

## Snippets

`snippets/event-b.json` ships common Event-B scaffolds (VS Code snippet format,
generated from the canonical snippet table). Zed loads snippets per language;
copy it to `~/.config/zed/snippets/event-b.json` to enable them. Prefixes
include `ctx`, `mch`, `evt`, `init`, `axm`, `inv`, `grd`, `act`, `forall`,
`exists`, `lambda`.

## Regenerating the grammar

The grammar and its highlight queries are hand-maintained in the standalone
`tree-sitter-eventb` repository; nothing here generates them. What this
repository generates is the token *manifest* the grammar's own test checks
against. After changing `crates/rossi/src/{keywords,operators,builtins}.rs`:

```bash
cargo xtask gen-grammars        # updates tokens.json, this extension's highlights copy, snippets
```

Then teach the grammar the new spelling in `editors/tree-sitter-eventb/` and
verify it — see the grammar repo's
[Development](../tree-sitter-eventb/README.md#development) section for the
`tree-sitter generate` / test sequence, whose token-contract test reads that
manifest.

`cargo xtask gen-grammars --check` (run in CI) fails if the generated files
drift from the tables. The extension's bundled `languages/eventb/highlights.scm`
is written verbatim from the grammar repo's `queries/highlights.scm` — edit only
the latter, and move `extension.toml`'s `rev` with the submodule when it
changes.

## Limitations

Zed's extension API has no equivalent for the VS Code extension's editor
commands, context menus, keybindings, walkthroughs, or live ASCII→Unicode input
method. Those are replaced by the documented substitutes above (LSP completions,
snippets, and `rossi` CLI tasks).
