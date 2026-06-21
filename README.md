# Rossi

A Rust toolchain for the Event-B formal modeling language: parser,
static checker, command-line tool, and Language Server Protocol
implementation.

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
- **`rossi-cli`** — the `rossi` command-line tool wrapping the
  parser, checker, and language server.
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

**Command-line workflows**
- `validate`, `import`, `export`, `fmt`, and `build` subcommands
- Text, JSON, and SARIF 2.1.0 diagnostic output for CI and IDE integration

**Editor integration (LSP)**
- Diagnostics, completion, hover, go-to-definition, find references, rename
- Formatting, semantic highlighting, code actions, folding, and signature help
- Extensions for VS Code, Neovim, Emacs, Sublime Text, and Zed

## Installation

Build the `rossi` command-line tool from source:

```bash
git clone https://github.com/eventb-rossi/rossi
cd rossi
cargo build --release -p rossi-cli
```

The binary is then available at `target/release/rossi`. The standalone
language server (`eventb-language-server`) builds the same way with
`-p eventb-lsp`.

To use Rossi as a library, depend on the `rossi` crate — the parser, typed
AST, pretty-printer, and Rodin XML/ZIP conversion. Run `cargo doc -p rossi
--open` for the API documentation.

## CLI Tool

The project ships a `rossi` command-line tool that wraps the parser,
the `rossi-build` static checker, and the language server:

| Subcommand | Purpose |
|------------|---------|
| `validate` | Validate `.eventb` files, Rodin `.zip` archives, or unzipped Rodin project directories. |
| `import`   | Import Rodin `.zip`/`.buc`/`.bum`/dir into `.eventb` text. |
| `export`   | Export `.eventb`/`.txt`/dir into a Rodin `.zip` archive. |
| `fmt`      | Reformat Event-B in place (operator convention, indentation). |
| `build`    | Static-check a Rodin project and emit `.bcc` / `.bcm` checked XML. |
| `lsp`      | Run the Rossi language server over stdio (equivalent to the `eventb-language-server` binary). |

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
    "success": true,
    "component_type": "Context",
    "component_name": "counter_ctx"
  }
]
```

For `.eventb` files, `validate` parses the text and reports component results.
For `.zip` archives, it also runs rossi-build semantic checks and advisory
lints unless `--no-semantic` is set; `--no-lints` keeps semantic checks but
drops advisory lint rows. Directory inputs are treated as unzipped Rodin
projects and require semantic checks, so `--no-semantic` is rejected for them.

### Import (Rodin → Event-B text)

```bash
# Convert a Rodin .zip archive into .eventb text files (one per component)
rossi import project.zip --output ./project

# Use ASCII operators (and a custom indent) in the emitted text
rossi import project.zip --output ./project --ascii --indent="  "

# Merge all components into a single file, optionally specifying order
rossi import project.zip --output project.eventb --merge=M0,C0
```

### Export (Event-B text → Rodin project)

```bash
# Pack a directory of .eventb files into a Rodin .zip archive
rossi export ./project --output project.zip

# Or emit a loose Rodin project directory
rossi export ./project --output ./rodin-project
```

`export` writes a complete Rodin project: a `.project` descriptor (named after
the output path) plus each component's native XML. Use a `.zip` output path for
an importable archive, or a directory output path for loose project files. The
archive always uses Unicode operators, which is what Rodin expects, so `export`
has no operator-convention option — use `rossi fmt` to change the convention of
text files.

### Format (`fmt`)

`fmt` reformats Event-B *without* crossing the Rodin↔text boundary: it
normalizes the operator convention (`--ascii`/`--unicode`, default Unicode) and
indentation (`--indent`).

```bash
# Convert ASCII-operator text to Unicode (default), printing to stdout
rossi fmt model.eventb

# Reformat files in place; pick the operator convention explicitly
rossi fmt -i ./project --ascii
rossi fmt -i model.eventb --indent="  "

# CI gate: exit non-zero if anything is not already formatted
rossi fmt --check ./project

# Normalize a Rodin archive to canonical Unicode XML (other entries preserved)
rossi fmt project.zip -o normalized.zip
```

Editors using the language server format on save with the same engine; `rossi
fmt` is its command-line and CI counterpart. (Rodin archives must stay Unicode,
so `--ascii` is rejected for `.zip`/`.buc`/`.bum` inputs.)

### Build (static check + Rodin checked XML)

```bash
# Static-check and emit .bcc / .bcm into a checked Rodin .zip
rossi build project.zip --output project-checked.zip

# Or emit loose files into a directory
rossi build project.zip --output ./out
```

### LSP

```bash
# Start the language server over stdio
rossi lsp
```

This is identical to running the standalone `eventb-language-server`
binary; editor extensions may invoke either form.

## Development

```bash
# Enable the pre-commit hook (runs cargo fmt, clippy, doc, and tests)
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
- **Cross-file resolution** — transitive SEES / REFINES / EXTENDS traversal

### Editor Extensions

Extensions are available in the `editors/` directory:

- **VS Code** (`editors/vscode/`) — syntax highlighting, LSP integration, snippets, and as-you-type ASCII→Unicode symbol input
- **Neovim** (`editors/neovim/`) — file detection, syntax highlighting, LSP config
- **Sublime Text** (`editors/sublime/`) — syntax highlighting, LSP integration, and as-you-type ASCII→Unicode symbol input (also used by `bat` and `delta` for syntax only)
- **Emacs** (`editors/emacs/`) — major mode for Event-B
- **Zed** (`editors/zed/`) — LSP integration plus a tree-sitter grammar for highlighting; semantic-token overlay

See each editor's README and INSTALL files for setup instructions.

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all checks pass: `cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test`
5. Submit a pull request

## Related Projects

- [Rodin Platform](https://www.event-b.org/) - Eclipse-based IDE for Event-B
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
