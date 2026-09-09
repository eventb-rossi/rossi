[![crates.io](https://img.shields.io/crates/v/rossi-cli?label=crates.io)](https://crates.io/crates/rossi-cli)
[![VS Code Marketplace](https://vsmarketplacebadges.dev/version-short/rossi.event-b.svg)](https://marketplace.visualstudio.com/items?itemName=rossi.event-b)
[![Open VSX](https://img.shields.io/open-vsx/v/rossi/event-b?label=Open%20VSX)](https://open-vsx.org/extension/rossi/event-b)

# Event-B Rossi

A Rust toolchain for the Event-B formal modeling language: parser,
static checker, proof checker, command-line tool, and Language Server
Protocol implementation.

## Overview

Event-B is a formal method for system-level modeling and analysis.
Rossi covers the full author-to-Rodin path:

- **`rossi`** — pest-based parser and typed AST with a pretty-printer
  that round-trips between `.eventb` text and the native Rodin
  `.buc` / `.bum` / `.zip` XML formats.
- **`rossi-build`** — static checker that layers type inference and
  well-formedness checks on the AST and emits Rodin-compatible
  `.bcc` / `.bcm` checked XML, so models authored in text round-trip
  through the Rodin toolchain.
- **`rossi-prove`** — sequent-prover kernel that checks the proofs a
  Rodin project stores (`.bpr` / `.bps`) against its proof obligations,
  by reuse and by replay.
- **`rossi-cli`** — the `rossi` command-line tool wrapping the
  parser, checker, prover, and language server.
- **`eventb-lsp`** — Language Server Protocol implementation powering
  editor extensions for VS Code, Neovim, Emacs, Sublime Text, and Zed.

## Features

**Parse & round-trip**
- Full Event-B syntax: contexts, machines, events, refinement, witnesses
- Text ↔ native Rodin XML (`.buc` / `.bum` / `.zip`)
- Unicode and ASCII operator conventions (Rodin Keyboard mapping)
- Pretty-printer with configurable indentation; parse → transform → print
- Optional serde support for JSON serialization of the AST

**Static checking & type inference**
- Type inference with unification (integers, booleans, given sets, power sets, products)
- Well-formedness checks for guards, actions, invariants, and axioms
- Cross-reference resolution across SEES / EXTENDS / REFINES, with circular-dependency detection
- `EB0xx` diagnostics plus advisory lints (dead or unmodified variables, incomplete INITIALISATION, …)
- Rodin-compatible `.bcc` / `.bcm` checked output

**Proof checking**
- Stored Rodin proofs checked against regenerated obligations by reuse (re-applying the recorded rules) and by replay (re-running the reasoners)
- Per-obligation verdicts — discharged, reviewed, pending, unattempted, broken — with the proof's recorded confidence
- Proof-store maintenance without Rodin: drop the proofs whose obligation is gone, empty the ones that no longer open

**Command-line workflows**
- `validate`, `import`, `export`, `fmt`, `build`, `prove`, and `clean` subcommands
- Text, JSON, and SARIF 2.1.0 diagnostic output for CI and IDE integration

**Editor integration (LSP)**
- Diagnostics, completion, hover, go-to-definition, find references, rename
- Formatting, semantic highlighting, code actions, folding, and signature help
- Extensions for VS Code, Neovim, Emacs, Sublime Text, and Zed

## Installation

### Editor extension (VS Code / VSCodium)

Install **Event-B (Rossi)** from your editor's marketplace. The extension bundles
syntax highlighting and the language server, and downloads the prebuilt `rossi` /
`eventb-language-server` binaries for your platform on first activation — so for
most users this is the only step.

- **VS Code** — [Marketplace](https://marketplace.visualstudio.com/items?itemName=rossi.event-b)
  or `code --install-extension rossi.event-b`
- **VSCodium** — [Open VSX](https://open-vsx.org/extension/rossi/event-b)
  or `codium --install-extension rossi.event-b`

### CLI — package managers

The `rossi` command-line tool (each package also installs the
`eventb-language-server`) is available from the major package managers:

```bash
# Homebrew (macOS / Linux)
brew tap eventb-rossi/tap
brew install rossi

# APT (Ubuntu 26.04 "Resolute" or later)
curl -fsSL https://eventb-rossi.github.io/apt/KEY.gpg \
  | sudo gpg --dearmor -o /etc/apt/keyrings/eventb.gpg
echo "deb [signed-by=/etc/apt/keyrings/eventb.gpg] https://eventb-rossi.github.io/apt resolute main" \
  | sudo tee /etc/apt/sources.list.d/eventb.list
sudo apt update
sudo apt install rossi

# Scoop (Windows)
scoop bucket add eventb https://github.com/eventb-rossi/scoop-eventb
scoop install eventb/rossi

# Gentoo
eselect repository eventb-rossi
emaint sync -r eventb-rossi
emerge -av rossi

# Fedora (COPR)
sudo dnf copr enable @eventb-rossi/eventb-copr
sudo dnf install rossi
```

### CLI — prebuilt archives

Every release also attaches a standalone archive per platform to the
[Releases page](https://github.com/eventb-rossi/rossi/releases), each holding
both binaries. Check a download against the release's `SHA256SUMS`, then unpack
it somewhere on your `PATH`:

```bash
tar -xzf rossi-aarch64-apple-darwin.tar.gz

# macOS, and only for an archive fetched with a browser: browsers tag downloads
# with `com.apple.quarantine`, and Gatekeeper then refuses to run binaries that
# Rossi does not notarize. Installs from a package manager are unaffected.
xattr -dr com.apple.quarantine rossi eventb-language-server
```

### CLI — from crates.io

```bash
cargo install rossi-cli   # the `rossi` CLI
cargo install eventb-lsp  # the standalone `eventb-language-server`
```

### CLI — from source

```bash
git clone https://github.com/eventb-rossi/rossi
cd rossi
cargo build --release -p rossi-cli
```

The binary is then available at `target/release/rossi`. The standalone
language server (`eventb-language-server`) builds the same way with
`-p eventb-lsp`.

### As a library

To use Rossi as a library, depend on the `rossi` crate — the parser, typed
AST, pretty-printer, and Rodin XML/ZIP conversion. Run `cargo doc -p rossi
--open` for the API documentation.

## CLI Tool

The project ships a `rossi` command-line tool that wraps the parser and
the `rossi-build` static checker (the language server is the separate
`eventb-language-server` binary):

| Subcommand | Purpose |
|------------|---------|
| `validate` | Validate `.eventb` files, Rodin `.zip` archives, or unzipped Rodin project directories. |
| `import`   | Import Rodin `.zip`/`.buc`/`.bum`/dir into `.eventb` text. |
| `export`   | Export `.eventb`/`.txt`/dir into a Rodin `.zip` archive. |
| `fmt`      | Reformat Event-B in place (style, operator convention, indentation). |
| `build`    | Static-check a Rodin project and emit `.bcc` / `.bcm` checked XML. |
| `dump`     | Write a checked project as a `rossi-model` JSON document: the typed formula trees in Rodin's vocabulary. |
| `prove`    | Check a project's stored proofs against its proof obligations. |
| `clean`    | Drop the stored proofs whose obligation no longer exists, and empty the ones that no longer apply. |
| `completions` | Print a shell completion script to stdout (run `rossi completions --help` for the supported shells). |

### Validate

```bash
# Validate a single file
rossi validate crates/rossi/examples/counter.eventb

# Validate multiple files
rossi validate crates/rossi/examples/*.eventb

# JSON output for tooling integration
rossi validate --format json crates/rossi/examples/counter.eventb

# SARIF output for IDEs and code-scanning tools
rossi validate --format sarif crates/rossi/examples/base-model.zip

# Quiet mode (only show errors)
rossi validate --quiet crates/rossi/examples/*.eventb

# Continue past failures
rossi validate --continue-on-error crates/rossi/examples/*.eventb

# Skip semantic checks for .zip inputs, or skip advisory lints
rossi validate --no-semantic crates/rossi/examples/base-model.zip
rossi validate --no-lints crates/rossi/examples/base-model.zip

# Write the report to a file instead of the terminal (any format)
rossi validate --format sarif --output rossi.sarif ./my-project

# Fail on advisory lints too, not just errors
rossi validate --deny-warnings ./my-project

# Name the analysis a SARIF run belongs to
rossi validate --format sarif --sarif-category rossi ./my-project
```

**Text output:**
```
✓ crates/rossi/examples/counter.eventb - Valid Context 'counter_ctx'
✓ crates/rossi/examples/counter_machine.eventb - Valid Machine 'counter'

==================================================
Summary:
  Total:  2
  Passed: 2 ✓
  Failed: 0 ✗
==================================================
```

**JSON output:**
```json
[
  {
    "file": "crates/rossi/examples/counter.eventb",
    "input": "file",
    "success": true,
    "component_type": "Context",
    "component_name": "counter_ctx",
    "path": "crates/rossi/examples/counter.eventb"
  }
]
```

For `.eventb` files, `validate` parses the text and reports component results.
For `.zip` archives, it also runs rossi-build semantic checks and advisory
lints unless `--no-semantic` is set; `--no-lints` keeps semantic checks but
drops advisory lint rows. Directory inputs are treated as unzipped Rodin
projects and require semantic checks, so `--no-semantic` is rejected for them.

#### Locating a diagnostic

Every JSON row includes the ready-to-use location in `path`. A `directory`
member is a real path (`my-project/M.eventb`), while an `archive` member uses
the archive separator (`model.zip!/M.bum`) because no such file exists on
disk. SARIF reports the same value in `artifactLocation.uri`.

The component fields remain available separately: `file` names the input,
`inner_filename` names a member when there is one, and `input` says whether
the input is a `file`, `directory`, or `archive`.

#### Using it in CI

`--format sarif` always emits exactly **one** `runs[]` entry, however many
files, directories and archives it was given. Code scanning
[rejects an upload whose runs share a category][sarif-runs], so this is what
lets a whole workspace be uploaded in a single SARIF file. `--sarif-category`
names that run, and `--output` writes it where an upload step can find it:

```yaml
- run: rossi validate --format sarif --sarif-category rossi --output rossi.sarif models/
  continue-on-error: true          # upload the findings even when the gate fails
- uses: github/codeql-action/upload-sarif@v4
  with:
    sarif_file: rossi.sarif
    category: rossi
```

Advisory lints exit 0 by default; add `--deny-warnings` to gate on them too.
It changes only the exit code — a warning is still reported as a warning in
every format, so code scanning records the severity the rule actually has.

[sarif-runs]: https://github.blog/changelog/2025-07-21-code-scanning-will-stop-combining-multiple-sarif-runs-uploaded-in-the-same-sarif-file/

### Import (Rodin → Event-B text)

```bash
# Convert a Rodin .zip archive into .eventb text files (one per component)
rossi import project.zip --output ./project

# Use ASCII operators (and a custom indent) in the emitted text
rossi import project.zip --output ./project --ascii --indent="  "

# Merge all components into a single file, in dependency order: every
# component follows the ones it needs, and each context is introduced just
# before the first component that EXTENDS or SEES it
rossi import project.zip --output project.eventb --merge

# ...or lead with an explicit order; the rest follows in dependency order
rossi import project.zip --output project.eventb --merge=M0,C0
```

The input's proof state (`.bpr`/`.bps`/`.bpo`) is copied byte-exact into the
same directory as the generated text — exactly where a later bare
`rossi export --proofs` looks, so import → edit → export round-trips proofs
without extra flags. Pass `--no-proofs` to skip the copies.

### Export (Event-B text → Rodin project)

```bash
# Pack a directory of .eventb files into a Rodin .zip archive
rossi export ./project --output project.zip

# Or emit a loose Rodin project directory
rossi export ./project --output ./rodin-project

# Also static-check and generate proof obligations (.bcc/.bcm + .bpo/.bps)
rossi export ./project --output project.zip --build

# Attach local proof state too (implies --build)
rossi export ./project --output project.zip --proofs
rossi export ./project --output project.zip --proofs=./rodin-project
```

`export` writes a complete Rodin project: a `.project` descriptor (named after
the output path) plus each component's native XML. Use a `.zip` output path for
an importable archive, or a directory output path for loose project files. The
archive always uses Unicode operators, which is what Rodin expects, so `export`
has no operator-convention option — use `rossi fmt` to change the convention of
text files.

With `--build`, the export also runs the static checker and proof-obligation
generator (the same pipeline and exit semantics as `rossi build`: error
diagnostics still write the output, then fail the command). With
`--proofs[=PATH]` — which implies `--build` — local proof state joins the
output: `.bpr` proofs are carried byte-exact, and the generated `.bpo`/`.bps`
are reconciled against the local ones, so unchanged obligations keep their
stamps and recorded statuses while changed ones are re-stamped for Rodin to
re-check. `PATH` is a Rodin project directory or `.zip` (for a multi-project
export, sub-projects match `PATH/<name>/`). The bare form looks next to the
text inputs first and then in the LSP's shared Rodin workspace
(`<root>/.rossi/rodin/<project>`); a custom `rossi.rodin.workspace` setting
lives in editor configuration and is not visible to the CLI, so those setups
pass the location explicitly with `--proofs=PATH`. When the LSP's Open in
Rodin lens is in use, its proof mirror copies the workspace's proof files
back next to the sources whenever Rodin exits, so the bare form normally
finds current state right next to the text. Proof sources are
read-only. Note that a directory output is not itself a proof source: to
carry stamps across repeated loose exports, point `--proofs=<outdir>` at the
previous output.

### Format (`fmt`)

`fmt` reformats Event-B *without* crossing the Rodin↔text boundary: it
normalizes the layout (`--style camille|rossi`, default `camille` — the style
Rodin's Camille editor prints: lowercase keywords, inline declaration lists,
2-space indent), the operator convention (`--ascii`/`--unicode`, default
Unicode), and indentation (`--indent`, default the preset's). The preset's
keyword case, declaration-list layout, and blank-line policy can be overridden
individually (`--keyword-case`, `--decl-lists`, `--blank-between-clauses`).

Lines longer than `--max-width` (default 120 characters, `0` disables) wrap at
the outermost operator, one operand per line, operator-leading, with
continuation lines hanging-aligned under the formula's start; argument lists
fill greedily after commas. Comment text is never wrapped:

```eventb
machine m

invariants
  @inv1 request ∈ dom(pending)
        ∧ status(request) = active
        ∧ (owner(request) = user
           ∨ user ∈ admins)
end
```

```bash
# Convert ASCII-operator text to Unicode (default), printing to stdout
rossi fmt model.eventb

# Reformat files in place; pick the operator convention explicitly
rossi fmt -i ./project --ascii
rossi fmt -i model.eventb --indent="  "

# The original uppercase 4-space layout; add --max-width 0 to reproduce
# the old default output byte-for-byte (wrapping is on in every style)
rossi fmt -i model.eventb --style rossi

# Wrap at 100 columns instead of 120; 0 disables wrapping
rossi fmt -i model.eventb --max-width 100

# CI gate: exit non-zero if anything is not already formatted
rossi fmt --check ./project

# Normalize a Rodin archive to canonical Unicode XML (other entries preserved)
rossi fmt project.zip -o normalized.zip
```

Editors using the language server format on save with the same engine, or
normalize just the operator spellings on save through the `source.fixAll.rossi`
code action; `rossi fmt` is their command-line and CI counterpart. (Rodin
archives must stay Unicode, so `--ascii` is rejected for `.zip`/`.buc`/`.bum`
inputs.)

### Build (static check + Rodin checked XML)

```bash
# Static-check and emit .bcc / .bcm into a checked Rodin .zip
rossi build project.zip --output project-checked.zip

# Or emit loose files into a directory
rossi build project.zip --output ./out
```

### Dump (checked model as JSON)

Rodin has no tree export of its typed syntax: every file it writes turns a
formula back into text and stores the types beside it, so a tool that wants
the tree has to parse and type-check the text again. `rossi dump` writes the
checked model with its formulas still trees, in Rodin's own vocabulary.

```bash
# Write the document to stdout
rossi dump project.zip

# Or to a file
rossi dump ./src --output model.json

# Choose a project from an archive holding several
rossi dump many.zip --project inventory

# Restrict the document to one machine and what it depends on
rossi dump ./src --component controller

# Indent it for reading
rossi dump counter.eventb --pretty

# Print the schema the documents conform to
rossi dump --schema
```

A document is written on one line, because it is written for a tool rather
than for a reader and indenting one costs several times the bytes. `--pretty`
indents it for reading a small model or inspecting the format.

Inputs are the same as `build`: a Rodin `.zip`, a directory (a Rodin project
or a folder of Event-B text), or a single `.eventb` / `.txt` / `.buc` / `.bum`
file.

**JSON output** (trimmed to one invariant):

```json
{
  "format": "rossi-model",
  "version": 1,
  "types": { "ℤ": { "kind": "INT" } },
  "machines": [
    {
      "name": "counter",
      "invariants": [
        {
          "label": "inv1",
          "theorem": false,
          "source": "/counter/counter.bum|org.eventb.core.machineFile#counter|org.eventb.core.invariant#inv1",
          "span": { "file": "counter.bum", "start": 62, "end": 76, "line": 6, "col": 5, "end_line": 6, "end_col": 19 },
          "text": "count\u2208\u2115",
          "predicate": {
            "tag": 107, "op": "IN",
            "children": [
              { "tag": 1, "op": "FREE_IDENT", "type": "\u2124", "name": "count" },
              { "tag": 402, "op": "NATURAL", "type": "\u2119(\u2124)" }
            ]
          }
        }
      ]
    }
  ]
}
```

Each node carries Rodin's numeric `tag` and its constant name as `op`, and its
children follow Rodin's child positions, so a path through `children` is the
same path through the formula in Rodin. Bound variables are de Bruijn indices,
as in Rodin, and each occurrence also names the declaration it resolves to.
Every action additionally carries `ba`, the same action written as a predicate
relating the before and after states.

A machine carries the closure of what is visible in it, as Rodin's checked
files do: inherited invariants, the whole chain of an extended event, and the
contexts it sees transitively. Anything inherited names the machine that wrote
it, so a consumer wanting only one machine's own declarations can filter on
that.

Spans are absent for Rodin XML input, which carries no source text.

`--component` narrows the document to the named components and everything they
depend on: the contexts that declare the names their formulas use, and the
machines they refine, which their elements cite as the origin of what they
inherit. Every name a restricted document mentions therefore still resolves
inside it. Findings about components left out are left out too, so the exit
code reports only what the document contains. The flag is repeatable, and a
name matching no component is an error listing the ones that exist.

#### Exit codes

`0` when the document has no error diagnostics, `1` when it does, and `2` for
a usage mistake such as an archive holding several projects with no
`--project`. A document is written even when the exit code is `1`: what the
checker rejected is absent from the elements, and `diagnostics` says why. When
the input cannot be read at all nothing reaches stdout, so a consumer never
parses a truncated document.

### Shell completions

`rossi completions <shell>` prints a completion script to stdout, generated
from the CLI's own command tree so it always matches the installed version:

```bash
# zsh — a directory on your $fpath
rossi completions zsh > ~/.zsh/completions/_rossi

# bash (system-wide), fish, or load it for this session only
rossi completions bash | sudo tee /etc/bash_completion.d/rossi >/dev/null
rossi completions fish > ~/.config/fish/completions/rossi.fish
eval "$(rossi completions bash)"
```

## Language Server & IDE Support

The `eventb-lsp` Language Server Protocol implementation provides modern
IDE features for Event-B development:

- **Real-time diagnostics** — syntax and semantic errors with error recovery
- **Completion & hover** — context-aware keywords, operators, identifiers, snippets
- **Navigation** — go-to-definition, find references, and document/workspace symbols
- **Rename refactoring** — safe identifier renaming with validation
- **Formatting & semantic highlighting** — Unicode/ASCII operators, AST-based tokens
- **Code actions** — Unicode/ASCII conversion, extract constant, sort clauses
- **Code folding, smart selection, signature help, and document links**
- **Inlay hints** — inferred declaration types after variables, event
  parameters, and constants, plus `WD` markers on formulas carrying a
  non-trivial well-definedness condition (switchable via `rossi.inlayHints.*`)
- **Cross-file resolution** — transitive SEES / REFINES / EXTENDS traversal

## Development

```bash
# Enable the pre-commit hook (runs cargo fmt, clippy, and doc)
git config core.hooksPath .githooks

# Build
cargo build

# Run the tests (all, a specific suite, or with output)
cargo test
cargo test --test full_models_test
cargo test -- --nocapture

# Generate API documentation
cargo doc --open
```

## Related Projects

- [Rodin Platform](https://eventb-soton.github.io/en-us/) - Eclipse-based IDE for Event-B
- [ProB](https://prob.de/) - Animator and model checker for Event-B
- [Event-B Documentation](https://wiki.event-b.org/)

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

## References

- [Event-B Language Summary](https://wiki.event-b.org/index.php/Event-B_Language)
- [Event-B Notation Guide](https://wiki.event-b.org/index.php/Mathematical_Notation)
- [Rodin Keyboard User Guide](https://wiki.event-b.org/index.php/Rodin_Keyboard_User_Guide)
- [Rodin User Manual](https://wiki.event-b.org/index.php/Rodin_User_Manual)

## Authors

Rossi Contributors
