# Installing the Rossi Event-B extension for Zed

## 1. Install the language server

```bash
cargo install --path crates/rossi-lsp
```

Confirm `rossi-language-server` is on your `PATH`:

```bash
rossi-language-server --help    # or: which rossi-language-server
```

(Alternatively, pin an absolute path later via
`lsp."rossi-language-server".binary.path` in Zed `settings.json`.)

## 2. Make the tree-sitter grammar loadable

Zed fetches grammars from a git repository pinned to a revision; it cannot load
a grammar from a plain local directory. The grammar sources are committed here
under `grammars/tree-sitter-eventb/` (and `src/parser.c` is checked in, so no
Node toolchain is needed to *use* it).

**For a published release**, push `grammars/tree-sitter-eventb/` to a standalone
repository (e.g. `github.com/eventb-rossi/tree-sitter-eventb`) and pin it in
`extension.toml`:

```toml
[grammars.eventb]
repository = "https://github.com/eventb-rossi/tree-sitter-eventb"
rev = "<commit-sha>"
```

**For local development** before that repository exists, turn the grammar
directory into a local git repo and point `extension.toml` at it:

```bash
cd editors/zed/grammars/tree-sitter-eventb
git init && git add -A && git commit -m "tree-sitter-eventb"
git rev-parse HEAD          # copy this SHA
```

Then edit `editors/zed/extension.toml`:

```toml
[grammars.eventb]
repository = "file:///ABSOLUTE/PATH/TO/editors/zed/grammars/tree-sitter-eventb"
rev = "<the SHA you copied>"
```

## 3. Install the dev extension

1. Open Zed.
2. Command palette → **zed: install dev extension**.
3. Select the `editors/zed/` directory.

If you already have a published version installed, Zed uninstalls it first.

## 4. Verify

1. Open a `.eventb` file (e.g. one under `tests/fixtures/` or `examples/`).
2. You should see syntax highlighting (keywords, operators, constants,
   comments, strings, numbers).
3. The status bar should show the language server starting. Trigger completion
   (including `\and`, `\forall`), hover a symbol, format the document, and
   rename an identifier.
4. Enable the richer features in `settings.json` (see
   [README.md](README.md#configuration)): `semantic_tokens: "combined"`,
   `document_symbols: "on"` (outline), `document_folding_ranges: "on"`,
   `code_lens: "on"` (ProB lenses).

## Troubleshooting

- **No language server / "binary not found".** Ensure `rossi-language-server`
  is on `PATH`, or set `lsp."rossi-language-server".binary.path`. Check
  command palette → **zed: open log** for startup errors.
- **No highlighting.** The grammar reference in `extension.toml` must resolve
  (step 2). Reinstall the dev extension after editing `extension.toml`.
- **Grammar out of date after editing the token tables.** Re-run
  `cargo run -p rossi-cli -- gen-grammars` and `npx tree-sitter generate` in the
  grammar directory (see [README.md](README.md#regenerating-the-grammar)).
