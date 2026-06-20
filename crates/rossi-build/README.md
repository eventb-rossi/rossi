# rossi-build

Static checker and builder for Event-B models —
part of the [Rossi](https://github.com/eventb-rossi/rossi) toolchain. It layers
type inference and well-formedness checking on top of the
[`rossi`](https://crates.io/crates/rossi) AST and emits Rodin-compatible
checked output.

## What it does

- Type inference with unification (integers, booleans, given sets, power sets,
  products).
- Well-formedness checks for guards, actions, invariants, and axioms.
- Cross-reference resolution across `SEES` / `EXTENDS` / `REFINES`, with
  circular-dependency detection.
- `EB0xx` diagnostics plus advisory lints (dead or unmodified variables,
  incomplete `INITIALISATION`, …).
- Reads Rodin `.buc` / `.bum` projects and emits Rodin-compatible `.bcc` /
  `.bcm` checked XML, so text-authored models round-trip through the Rodin
  toolchain.

## Usage

```toml
[dependencies]
rossi-build = "0.1"
```

See the [API documentation](https://docs.rs/rossi-build) for the checker entry
points. For command-line use, the same checker is exposed as `rossi build` in
the [`rossi-cli`](https://crates.io/crates/rossi-cli) tool.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
