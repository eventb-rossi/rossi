# Changelog

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
