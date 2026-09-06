# Event-B Language Support for Sublime Text, bat, and delta

## Intro

`EventB.sublime-syntax` is a Sublime Text syntax definition. It is read by the
[`syntect`](https://github.com/trishume/syntect) library, so the same file gives
Event-B highlighting in **Sublime Text**, **[bat](https://github.com/sharkdp/bat)**
(`cat` with wings), and **[delta](https://github.com/dandavison/delta)** (the git
pager). The last two do not support the Rossi language server, only syntax highlighting.

> **`EventB/EventB.sublime-syntax` and `EventB/operators.py` are generated files —
> do not edit by hand.** Both are produced from the canonical token tables by
> `cargo xtask gen-grammars` and checked in CI. Change the tables
> in `crates/rossi/src/{operators,keywords,builtins}.rs` and regenerate.

## Sublime Text

### Installation

Copy the entire `EventB/` directory into Sublime Text's `Packages/` folder
(`Preferences → Browse Packages…`). You need all three files in one directory:

```
Packages/
└── EventB/
    ├── .python-version         ← tells ST4 to use Python 3.8
    ├── EventB.sublime-syntax   ← syntax highlighting
    ├── EventB.py               ← input method plugin (requires ST4)
    └── operators.py            ← generated operator table (loaded by EventB.py)
```

Files ending in `.eventb` highlight automatically once the syntax file is present.
`EventB.py` loads automatically; no restart is needed after the copy.

### Language Server (hover, completion, go-to-definition, …)

First install the `eventb-language-server` binary via your package manager
(Homebrew, APT, Scoop, Gentoo, or Fedora COPR — each installs it alongside the
`rossi` CLI) or with `cargo install eventb-lsp`; see the
[main Installation guide](../../README.md#installation).

Then install [Package Control](https://docs.sublimetext.io/guide/package-control/usage.html)
and the [LSP package](https://packages.sublimetext.io/packages/LSP).

Open the Command Palette:

```
Windows/Linux: Ctrl + Shift + P
macOS:         Cmd  + Shift + P
```

Type and select `Preferences: LSP Server Configurations`. In the right-hand user
settings pane add:

```json
{
    "eventb-language-server": {
        "enabled": true,
        "command": ["eventb-language-server"],
        "selector": "source.eventb",
        "settings": {
            "rossi": {
                "rodin": { "path": "", "workspace": "" }
            }
        }
    }
}
```

This assumes `eventb-language-server` is on your `PATH`. Once configured, all
standard LSP features become available: diagnostics, completion, hover, go-to-
definition, find references, rename, formatting, semantic highlighting, code
actions, code lenses, folding, smart selection, signature help, and document
links.

Code **folding** and **smart selection expand/shrink** are available via the
Command Palette as `LSP: Expand Selection` and `LSP: Shrink Selection`; the
editor's native fold UI also uses the server's folding ranges.

**Inlay hints** — inferred declaration types after machine variables, event
parameters, and context constants, plus `WD` markers on formulas with a
non-trivial well-definedness condition — render when `"show_inlay_hints": true`
is set in the LSP package settings. Which hints the server emits is configured
under `rossi.inlayHints` (`enabled`, `wellDefinedness`, `maxLength`) in the
`settings` block above.

An **Open in Rodin** code lens appears on MACHINE/CONTEXT declarations (set
`"show_code_lens": "annotation"` in the LSP package settings if lenses don't
render). Running it builds the file's directory into a persistent Rodin
workspace — `.rossi/rodin` next to your sources by default — and launches the
Rodin IDE on it; proofs made in Rodin survive rebuilds. While Rodin is open
the sync is live (`rossi.rodin.sync`, on by default): saving a file rebuilds
the project in the background and Rodin picks it up within a few seconds,
while edits saved in Rodin flow back into the `.eventb` sources automatically
(three-way merge, git-style conflict markers if both sides changed the same
lines). Set
`rossi.rodin.path` in the `settings` block above if Rodin is not at the
platform default location (`/Applications/Rodin.app`, `rodin.exe`, `rodin`).

### The Rodin math font

Four Event-B operators have no standard Unicode code point, so Rodin encodes
them in the Unicode Private Use Area: `<<->` (U+E100), `<->>` (U+E101),
`<<->>` (U+E102) and `<+` (U+E103). Rossi writes them in ASCII, which renders
in any font, so **most users need nothing here**. Read on only if you set
`"privateUseGlyphs": true` under `rossi.format` in the `settings` block above,
to exchange files with a tool that reads only Rodin's spelling.

The glyphs live in one font, Brave Sans Mono Roman, which ships with this
repository at [editors/vscode/fonts/](../vscode/fonts/) — its licence is in
[LICENSE-BraveSansMono.txt](../vscode/LICENSE-BraveSansMono.txt). Install it
into your own font directory (no administrator rights needed): double-click it
on macOS and press *Install Font*, right-click → *Install* on Windows, or copy
it into `~/.local/share/fonts/` and run `fc-cache -f` on Linux.

Sublime Text has no per-range font mapping and no user-configurable fallback
list — it uses the platform's own fallback, which has no way to guess a font
for an unassigned private-use code point. So unlike Emacs or kitty, Sublime
cannot keep your font for everything else: set `font_face` to the math font in
your Event-B syntax-specific settings to see the glyphs.

```json
// Packages/User/EventB.sublime-settings — named after EventB.sublime-syntax
{
    "font_face": "Brave Sans Mono"
}
```

### Symbol input (eager mode and leader mode)

`EventB.py` provides as-you-type ASCII→Unicode substitution for Event-B operators,
matching the behaviour of the VS Code and Neovim plugins.

**Eager mode** — symbolic combos convert automatically via maximal munch:

| You type | You get |
|----------|---------|
| `=>`     | `⇒`    |
| `<=>`    | `⇔`    |
| `\|->`   | `↦`    |
| `<:`     | `⊆`    |
| `/=`     | `≠`    |
| `<=`     | `≤`    |

Multi-character operators wait for the next character before committing, so
`<=` converts to `≤` only when a character that cannot extend it to `<=>` is
typed — allowing `<=>` → `⇔` to win when the third character is `>`.

**Leader mode** — type `\name` then any non-letter boundary character:

| You type      | You get |
|---------------|---------|
| `\implies `   | `⇒ `   |
| `\forall `    | `∀ `   |
| `\in `        | `∈ `   |
| `\nat `       | `ℕ `   |
| `\or `        | `∨ `   |

The leader character `\` is reserved and never starts an eager run. Alphabetic
operator names (`NAT`, `or`, `dom`, …) also work as leader names (`\NAT`,
`\or`, `\dom`).

Every substitution is a single undo step (`Ctrl+Z` / `Cmd+Z` restores the
ASCII). Input works everywhere in Event-B files, including inside comments.

## bat and delta

```sh
mkdir -p "$(bat --config-dir)/syntaxes"
cp EventB/EventB.sublime-syntax "$(bat --config-dir)/syntaxes/"
bat cache --build
bat --list-languages | grep -i event-b   # confirm it registered
```

`delta` reuses bat's syntax set, so once bat knows Event-B, `git diff` of an
`.eventb` file through delta is highlighted too. Verify with:

```sh
bat sample.eventb
```

`bat` and `delta` use only `EventB.sublime-syntax`; `EventB.py` and
`operators.py` are not needed for them.
