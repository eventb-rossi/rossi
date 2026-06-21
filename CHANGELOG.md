# Changelog

## [0.2.0](https://github.com/eventb-rossi/rossi/compare/v0.1.0...v0.2.0) - 2026-06-21

### Changed
- *(rossi)* Host the operator input-method table in rossi::operators

### Documentation
- Point CI and editor docs at `cargo xtask gen-grammars`

## [0.1.0] - 2026-06-20

Initial release of the Rossi toolchain for the Event-B formal modeling language —
parser, static checker, language server, and CLI — published to crates.io:

- [`rossi`](https://crates.io/crates/rossi) — parser, typed AST, pretty-printer, and Rodin XML/ZIP round-trip.
- [`rossi-build`](https://crates.io/crates/rossi-build) — static checker and Rodin `.bcc` / `.bcm` builder.
- [`eventb-lsp`](https://crates.io/crates/eventb-lsp) — Language Server Protocol implementation.
- [`rossi-cli`](https://crates.io/crates/rossi-cli) — the `rossi` command-line tool.

See the [v0.1.0 release](https://github.com/eventb-rossi/rossi/releases/tag/v0.1.0) for details.
