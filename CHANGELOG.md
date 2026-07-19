# Changelog

## [0.1.5](https://github.com/eventb-rossi/rossi/compare/v0.1.4...v0.1.5) - 2026-07-19

### Added
- *(parse)* Add Rodin-canonical formula spacing
- *(parse)* Expose shared syntax snapshot queries

### Changed
- *(lsp)* Unify component cursor resolution
- *(lint)* Reuse dependency graph identities
- *(parse)* Shift spans through AST visitors
- *(xml)* Share ZIP entry parsing
- *(parse)* Encode single-argument applications
- Remove dead code
- *(build)* Emit canonical formulas during printing
- *(build)* Group event check state
- *(lsp)* Share candidate dependency environments
- *(lsp)* Extract component occurrence service

### Fixed
- *(build)* Reject ill-typed expressions before emission
- *(cli)* Reject colliding fmt output paths
- *(parse)* Enforce parallel assignment arity
- *(cli)* Handle structured output failures
- *(cli)* Show errors in quiet continue mode
- *(cli)* Raw-copy retained formatted ZIP entries
- *(cli)* Contain loose build output paths
- *(cli)* Propagate recursive input scan errors
- *(build)* Reject inherited event label conflicts
- *(parse)* Reset prefix depth across connective operands
- *(lsp)* Make component rename and references syntax-aware
- *(parse)* Shift sliced errors to absolute locations
- *(lsp)* Scope keyword completions structurally
- *(lsp)* Centralize dependency environments
- *(parse)* Align recovery identifier validation
- *(xml)* Parse project names with quick-xml
- *(xml)* Emit Rodin operator glyphs in source files
- *(build)* Retain enriched machine formula ASTs
- *(build)* Journal insert-if-absent bindings
- *(build)* Enrich predicates inside bool expressions
- *(build)* Raw-copy retained ZIP entries
- *(build)* Propagate project directory scan errors
- *(build)* Reject conflicting event parameters
- *(build)* Mask outer types during binder inference
- *(build)* Preserve Rodin checked element identities
- *(lsp)* Derive signature help from shared syntax
- *(lsp)* Batch selection ranges on shared syntax
- *(lsp)* Index workspace symbols from disk
- *(lsp)* Make document analysis snapshots atomic
- *(lsp)* Offload blocking workspace operations
- *(lsp)* Keep failed workspace scans incomplete

## [0.1.4](https://github.com/eventb-rossi/rossi/compare/v0.1.3...v0.1.4) - 2026-07-06

### Added
- *(build)* Enforce EB021/EB022 in the static checker
- *(cli)* Report EB026 assignment-in-predicate in validate
- *(parse)* Detect misplaced assignment in predicate (EB026)
- *(build)* Register EB026 assignment-in-predicate rule
- *(lsp)* Surface EB026 as a diagnostic with an operator quick-fix

### Changed
- *(build)* Extract duplicate-name detection into a shared module

### Fixed
- *(build)* Align EB006/EB009 severities with Rodin
- *(cli)* Fail rossi build on any error diagnostic
- *(lint)* Repartition the variable-usage lints (EB011/EB012)
- *(build)* Fail the build on duplicate component names (EB019)
- *(lint)* Remove the EB013 dead-constant lint
- *(lint)* Report duplicate component names as errors (EB019)
- *(parse)* Drop the misleading strict error when recovery reports EB026
- *(build)* Closure and closure1 are not Event-B built-ins
- *(lint)* Skip variable reference lints on unresolvable refinement chains
- *(lint)* Key the cross-component index by component, not name
- *(lint)* Count inherited invariants as references (EB011/EB012)
- *(lint)* Check extended INITIALISATION against the inherited chain (EB014)
- *(lint)* Count extended events' inherited clauses in EB011/EB012

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
