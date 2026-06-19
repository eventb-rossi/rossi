# Check your toolchain

The Rossi extension drives two executables:

- **`rossi`** — the CLI used for import, export, build, validation, and formatting.
- **`eventb-language-server`** — powers live diagnostics, completion, and symbols.

Run **Rossi: Check Toolchain** to confirm both are on your `PATH`.

If they are missing, build them from the Rossi source tree:

```sh
cargo build --release --bin rossi --bin eventb-language-server
```

then point `rossi.tool.path` and `rossi.languageServer.path` at the binaries in
**Settings**.
