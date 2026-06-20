# rossi-cli

The `rossi` command-line tool for the Event-B formal modeling language —
part of the
[Rossi](https://github.com/eventb-rossi/rossi) toolchain. It wraps the
[`rossi`](https://crates.io/crates/rossi) parser, the
[`rossi-build`](https://crates.io/crates/rossi-build) static checker, and the
[`eventb-lsp`](https://crates.io/crates/eventb-lsp) language server behind a
single binary named `rossi`.

## Installation

```bash
cargo install rossi-cli
```

This installs a `rossi` executable.

## Subcommands

| Subcommand | Purpose |
|------------|---------|
| `validate` | Validate `.eventb` files, Rodin `.zip` archives, or unzipped Rodin project directories. |
| `import`   | Import a Rodin `.zip` / `.buc` / `.bum` / directory into `.eventb` text. |
| `export`   | Export `.eventb` / `.txt` / directory into a Rodin `.zip` archive. |
| `fmt`      | Reformat Event-B in place (operator convention, indentation). |
| `build`    | Static-check a Rodin project and emit `.bcc` / `.bcm` checked XML. |
| `lsp`      | Run the Rossi language server over stdio (equivalent to the `eventb-language-server` binary). |

```bash
rossi validate model.eventb
rossi fmt --ascii model.eventb
rossi build project.zip
```

Run `rossi --help` (or `rossi <subcommand> --help`) for the full set of
options. See the [project README](https://github.com/eventb-rossi/rossi) for
the complete toolchain and editor integrations.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
