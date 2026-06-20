# rossi

Core Event-B parser, typed AST, pretty-printer, and Rodin XML/ZIP conversion
library — the foundation of the [Rossi](https://github.com/eventb-rossi/rossi)
toolchain for the Event-B formal modeling language.

## What it does

- Parses the full Event-B syntax (contexts, machines, events, refinement,
  witnesses) into a typed AST.
- Round-trips between `.eventb` text and the native Rodin `.buc` / `.bum` /
  `.zip` XML formats.
- Pretty-prints the AST with configurable indentation and either Unicode or
  ASCII operator conventions (Rodin Keyboard mapping), so you can
  parse → transform → print.
- Optional `serde` feature for JSON serialization of the AST.

## Usage

```toml
[dependencies]
rossi = "0.1"
```

```rust
use rossi::{parse, to_string, Component};

let component = parse("CONTEXT C SETS S END").unwrap();
if let Component::Context(ctx) = &component {
    println!("context {}", ctx.name);
}

// Pretty-print the AST back to `.eventb` text.
println!("{}", to_string(&component));
```

See the [API documentation](https://docs.rs/rossi) for the parser, AST, XML/ZIP
conversion, and pretty-printer, and the
[project README](https://github.com/eventb-rossi/rossi) for the rest of the
toolchain — the `rossi-build` static checker, the `rossi` command-line tool,
and the `eventb-lsp` language server.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
