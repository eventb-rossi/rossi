# Changelog

## [0.1.3](https://github.com/eventb-rossi/rossi/compare/v0.1.2...v0.1.3) - 2026-06-29

### Added
- *(cli)* Add completions subcommand for shell completion scripts
- *(build)* Reject reading a disappeared variable in guards/actions (EB025)
- *(build)* Reject assigning a disappeared variable (EB025)
- *(build)* Register EB025 disappeared-variable rule
- *(validate)* Flag new events assigning inherited variables
- *(validate)* Register EB024 rule
- *(lsp)* Flag duplicate component names across files
- *(lsp)* Flag unknown SEES/EXTENDS/REFINES targets
- *(lsp)* Flag circular EXTENDS/REFINES as you type
- *(lsp)* Add workspace queries for cross-component diagnostics
- *(lsp)* Colour constants as read-only variables, not numbers
- *(lsp)* Surface duplicate-name and shadowed-name lints as you type

### Changed
- *(lsp)* Extract diagnostics_for from publish_diagnostics
- *(lsp)* Emit semantic-token indices from ALL, not the discriminant
- *(lsp)* Classify declared symbols through one source of truth
- *(lsp)* Drop never-emitted semantic-token legend entries
- *(lsp)* Derive semantic-token legend from a single source
- *(lsp)* Extract diagnostic conversion into a diagnostics module

### Fixed
- *(lsp)* Improve cross-component diagnostics

### Documentation
- *(cli)* Document shell completion generation

## [0.1.2](https://github.com/eventb-rossi/rossi/compare/v0.1.1...v0.1.2) - 2026-06-26

### Added
- *(validate)* Check each project of a multi-project archive on its own
- *(import)* Split a multi-project archive into per-project subdirectories
- *(export)* Export a directory of subprojects as a multi-project archive
- *(build)* Support multi-project Rodin archives
- *(rossi)* Add multi-project Rodin zip/dir writers

## [0.1.1](https://github.com/eventb-rossi/rossi/compare/v0.1.0...v0.1.1) - 2026-06-23

### Added
- *(operators)* Accept +->> and -->> surjection input spellings

### Changed
- *(rossi)* Host the operator input-method table in rossi::operators

### Fixed
- *(parser)* Guard predicate lists against section keywords
- *(parser)* Treat Unicode space separators as whitespace (Rodin parity)
- *(infer)* Type identifiers buried in operand expressions (Rodin parity)

### Documentation
- Document the Ubuntu APT install channel
- Document package-manager and extension installs
- Point CI and editor docs at `cargo xtask gen-grammars`

## [0.1.0] - 2026-06-20

Initial release of the Rossi toolchain for the Event-B formal modeling language —
parser, static checker, language server, and CLI — published to crates.io:

- [`rossi`](https://crates.io/crates/rossi) — parser, typed AST, pretty-printer, and Rodin XML/ZIP round-trip.
- [`rossi-build`](https://crates.io/crates/rossi-build) — static checker and Rodin `.bcc` / `.bcm` builder.
- [`eventb-lsp`](https://crates.io/crates/eventb-lsp) — Language Server Protocol implementation.
- [`rossi-cli`](https://crates.io/crates/rossi-cli) — the `rossi` command-line tool.

See the [v0.1.0 release](https://github.com/eventb-rossi/rossi/releases/tag/v0.1.0) for details.
