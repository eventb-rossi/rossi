# Installing the Rossi Event-B extension for Zed

## 1. Install the language server

```bash
cargo install --path crates/eventb-lsp
```

Confirm `eventb-language-server` is on your `PATH`:

```bash
eventb-language-server --help    # or: which eventb-language-server
```

(Alternatively, pin an absolute path later via
`lsp."eventb-language-server".binary.path` in Zed `settings.json`.)

## 2. Make the tree-sitter grammar loadable

Zed fetches grammars from a git repository pinned to a revision; it cannot load
a grammar from a plain local directory. The grammar is the standalone
`tree-sitter-eventb` repository, developed in this monorepo under
`editors/tree-sitter-eventb/` (with `src/parser.c` checked in, so no Node
toolchain is needed to *use* it).

**For a published release**, pin the published repository in `extension.toml`:

```toml
[grammars.eventb]
repository = "https://github.com/eventb-rossi/tree-sitter-eventb"
rev = "<commit-sha>"
```

**For local development** before that repository is published, point
`extension.toml` at the local grammar repo. Zed fetches the pinned rev from the
repo's git history, so everything must be committed first — uncommitted files
are invisible to Zed:

```bash
cd editors/tree-sitter-eventb
git add -A && git commit -m "wip"   # make sure the grammar is actually at HEAD
git rev-parse HEAD                  # copy this SHA
```

Then edit `editors/zed/extension.toml`:

```toml
[grammars.eventb]
repository = "file:///ABSOLUTE/PATH/TO/editors/tree-sitter-eventb"
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
   `document_symbols: "on"` (outline), `document_folding_ranges: "on"`.

## Troubleshooting

- **No language server / "binary not found".** Ensure `eventb-language-server`
  is on `PATH`, or set `lsp."eventb-language-server".binary.path`. Check
  command palette → **zed: open log** for startup errors.
- **No highlighting.** The grammar reference in `extension.toml` must resolve
  (step 2). Reinstall the dev extension after editing `extension.toml`.
- **Grammar out of date after editing the token tables.** Re-run
  `cargo run -p rossi-cli -- gen-grammars` and `npx tree-sitter generate` in the
  grammar directory (see [README.md](README.md#regenerating-the-grammar)).
