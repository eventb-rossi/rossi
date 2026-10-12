# eventb-animate-driver

The contract of the [Rossi](https://github.com/eventb-rossi/rossi) toolchain
with [`eventb-animate`](https://github.com/eventb-rossi/eventb-animate), the
ProB-backed model checker: which binary to spawn, the command line of each
run (a model check, the constraint-based invariant check, the
well-definedness prover, the disprover over proof obligations), the
watchdog that keeps a hung JVM from wedging its caller, and the verdicts
read out of the tool's format-4 JSON report.

Both the language server (`eventb-lsp`) and the MCP server (`eventb-mcp`)
drive the tool through this crate, so the contract is written down once.
