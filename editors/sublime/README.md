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
(Homebrew, Scoop, Gentoo, or Fedora COPR — each installs it alongside the `rossi`
CLI) or with `cargo install eventb-lsp`; see the
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
        "selector": "source.eventb"
    }
}
```

This assumes `eventb-language-server` is on your `PATH`. Once configured, all
standard LSP features become available: diagnostics, completion, hover, go-to-
definition, find references, rename, formatting, semantic highlighting, code
actions, folding, smart selection, signature help, and document links.

Code **folding** and **smart selection expand/shrink** are available via the
Command Palette as `LSP: Expand Selection` and `LSP: Shrink Selection`; the
editor's native fold UI also uses the server's folding ranges.

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
