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

## The Rodin math font

Four Event-B operators have no standard Unicode code point, so Rodin encodes
them in the Unicode Private Use Area: `<<->` (U+E100), `<->>` (U+E101),
`<<->>` (U+E102) and `<+` (U+E103). Rossi writes them in ASCII, which renders
in any font, so **most users need nothing here**. Read on only if you turn on
the language server's `format.privateUseGlyphs` setting to exchange files with
a tool that reads only Rodin's spelling.

The glyphs live in one font, Brave Sans Mono Roman, which ships with this
repository at [editors/vscode/fonts/](../vscode/fonts/) — its licence is in
[LICENSE-BraveSansMono.txt](../vscode/LICENSE-BraveSansMono.txt). Install it
into your own font directory; no administrator rights are needed:

| | Install to |
| --- | --- |
| macOS | `~/Library/Fonts/` (double-click the file and press *Install Font*) |
| Linux | `~/.local/share/fonts/`, then run `fc-cache -f` |
| Windows | right-click the file and choose *Install* |

Then, in `settings.json`, turn the setting on and list the font as a fallback
so it is consulted only for glyphs your own font lacks:

```json
{
  "buffer_font_family": "Your Font",
  "buffer_font_fallbacks": ["Brave Sans Mono"],
  "lsp": {
    "eventb-language-server": {
      "initialization_options": {
        "rossi": { "format": { "useUnicode": true, "privateUseGlyphs": true } }
      }
    }
  }
}
```

Two Zed caveats: `buffer_font_fallbacks` is documented for macOS and Windows,
and [does not work on Linux](https://github.com/zed-industries/zed/issues/17254)
— set `buffer_font_family` to the math font there instead. And Zed extensions
are sandboxed, so this extension [cannot install the font for
you](https://github.com/zed-industries/zed/issues/14522); the manual step above
is required.

## Troubleshooting

- **No language server / "binary not found".** Ensure `eventb-language-server`
  is on `PATH`, or set `lsp."eventb-language-server".binary.path`. Check
  command palette → **zed: open log** for startup errors.
- **No highlighting.** The grammar reference in `extension.toml` must resolve
  (step 2). Reinstall the dev extension after editing `extension.toml`.
- **Grammar out of date after editing the token tables.** Re-run
  `cargo xtask gen-grammars`, extend the grammar in `editors/tree-sitter-eventb/`
  and re-run `npx tree-sitter generate` there (see
  [README.md](README.md#regenerating-the-grammar)).
