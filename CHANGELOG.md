# Changelog

## [0.2.0](https://github.com/eventb-rossi/rossi/compare/v0.1.9...v0.2.0) - 2026-08-22

### Added
- *(cli)* Wrap at 120 columns by default
- *(fmt)* Wrap formulas at a configurable maximum line width
- *(fmt)* Make camille the default formatting style
- *(cli)* Expose the formatter style preset and toggles
- *(fmt)* Add the camille formatting style to the pretty printer
- *(build)* Expose a static-check-only entry point with the typed model
- *(lsp)* Mark well-definedness conditions with inlay hints
- *(lsp)* Serve declaration type inlay hints
- *(lsp)* Map rossi.format.maxLineWidth to the formatter
- *(lsp)* Map rossi.format style settings to the formatter

### Changed
- *(build)* Return structured well-definedness conditions
- *(lsp)* Collect document-link source lines once

### Fixed
- *(cli)* End formatted text with exactly one newline

## [0.1.9](https://github.com/eventb-rossi/rossi/compare/v0.1.8...v0.1.9) - 2026-08-18

### Added
- *(build)* Stamp a .project descriptor into text-built source archives
- *(cli)* Import proof files next to the generated text
- *(cli)* Add export --build and --proofs
- *(cli)* Preserve proof state in directory and flat-archive builds
- *(build)* Emit proof-obligation files from the build
- *(validate)* Surface proof status in text, JSON and SARIF output
- *(validate)* Surface EB010 with --show-info
- *(parse)* Accept multiple event refinement targets
- *(parse)* Accept labeled and repeatable machine variants
- *(formula)* Derive before-after and feasibility predicates for assignments
- *(wd)* Emit EB010 from checked formulas
- *(lsp)* Sync Rodin model edits back into the sources automatically
- *(build)* Preserve proofs and carry stamps when repacking archives
- *(pog)* Reconcile regenerated obligations with previous proof state
- *(pog)* Parse obligation stamps and expose semantic equality in the po view
- *(pog)* Emit merged guard-strengthening obligations
- *(build)* Check merged events statically
- *(pog)* Emit lexicographic variant obligations
- *(build)* Check every labeled variant
- *(pog)* Emit convergence obligations
- *(pog)* Emit guard strengthening and simulation obligations
- *(pog)* Emit witness obligations and refined-event invariants
- *(pog)* Emit action and invariant preservation obligations
- *(pog)* Build event hypotheses and guard obligations
- *(pog)* Build machine hypotheses, invariant and variant obligations
- *(pog)* Emit context proof obligations
- *(pog)* Model proof-obligation files and hypothesis chains
- *(build)* Add typed closure lookups to the checked model
- *(build)* Report proof status from Rodin proof files
- *(lsp)* Mirror workspace proofs back beside the sources when Rodin exits
- *(lsp)* Seed text-adjacent proof files into the Rodin project at lens start
- *(lsp)* Catch up on Rodin model edits at watcher startup
- *(lsp)* Rebuild the shared Rodin project on save
- *(lsp)* Watch the Rodin workspace and surface proof status live
- *(lsp)* Serve Open in Rodin as CodeLens + executeCommand
- *(lsp)* Add the Rodin workspace build, launch, and lock core

### Changed
- *(lsp)* Host the .rossi/rodin reverse lookup beside its naming convention
- *(build)* Route export --build through the text-to-project doors
- *(cli)* Hoist the shared build pipeline into build_common
- *(build)* Share the text-to-project assembly between the CLI and the LSP
- *(parse)* Tighten the AST layer after review
- *(build)* Simplify the checker and generator internals
- *(lsp)* Derive the pretty-printer from one FormatConfig mapping

### Fixed
- *(cli,lsp)* Skip dot-directories when collecting source files
- *(parse)* Reject nonstandard set declarations
- *(build)* Keep theorem guards over variables disappearing here
- *(lsp)* Hold the Open in Rodin single-flight until Rodin takes the lock
- *(lsp)* Seed Rodin workspace preferences without clobbering foreign keys
- *(lsp)* Harden and streamline the Rodin sync loop
- *(lsp)* Create the Rodin workspace watcher off the request path

### Documentation
- *(cli)* Document the proof round-trip through import and export

## [0.1.8](https://github.com/eventb-rossi/rossi/compare/v0.1.7...v0.1.8) - 2026-08-11

### Added
- *(pretty)* Render the new formula model
- *(validate)* Expose joined path in JSON output
- *(ast)* Lower parsed formulas onto the typed model
- *(ast)* Scope-aware occurrence walker for the new formula model
- *(ast)* Add datatype extensions with constructors and destructors
- *(ast)* Add operator extension mechanism with dynamic tags
- *(ast)* Simplify well-definedness lemmas by antecedent subsumption
- *(ast)* Add well-definedness lemma computation
- *(ast)* Add two-pass type checking with unification
- *(ast)* Add path-based subformula positions
- *(ast)* Add fresh-name resolution for bound identifiers
- *(ast)* Add formula rewriter and substitution infrastructure
- *(ast)* Synthesize node types bottom-up at construction
- *(ast)* Add immutable formula nodes with factory construction
- *(ast)* Add mathematical type hierarchy and type environments
- *(ast)* Add formula tag constants and operator kind enums
- *(build)* Expose typed formulas through the checked model
- *(build)* Gate formulas through the two-pass checker
- *(build)* Type declarations with the two-pass checker

### Changed
- *(parser)* Build the formula model directly and drop the legacy tree
- *(ast)* Make the legacy formula tree parser-internal
- *(ast)* Route all formulas through the typed model
- *(build)* Render canonical text from typed formulas
- *(ast)* Move the legacy formula types behind an ast::legacy module
- *(ast)* Allow multiple targets in membership assignments
- *(build)* Type extended-event scopes from inherited decls
- *(build)* Derive canonical formula text at render time
- *(build)* Adopt the core mathematical types throughout
- *(build)* Drop the superseded inference and enrichment passes
- *(build)* Retire the well-typedness façade
- *(build)* Funnel identifier typing through a single seam

### Fixed
- *(validate)* Retain ruleless failures in SARIF
- *(validate)* Reject directories without components
- *(ast)* Scope implicit comprehension binders everywhere they bind

## [0.1.7](https://github.com/eventb-rossi/rossi/compare/v0.1.6...v0.1.7) - 2026-07-27

### Added
- *(validate)* Fail on advisory lints with --deny-warnings
- *(validate)* Tag SARIF runs with --sarif-category
- *(validate)* Write the report to --output

### Fixed
- *(validate)* Report directory parse failures per component file
- *(validate)* Render directory members as real paths
- *(build)* Validate inherited event render state

## [0.1.6](https://github.com/eventb-rossi/rossi/compare/v0.1.5...v0.1.6) - 2026-07-21

### Added
- *(build)* Expose project archive entry classification

### Fixed
- *(build)* Decode checked project names

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
