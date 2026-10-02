# eventb-mcp

Model Context Protocol server for the [Rossi](https://github.com/eventb-rossi/rossi)
Event-B toolchain. It serves one project directory to an LLM host (Claude
Code and its kind) as typed tools: validation with located findings, the
build with its proof obligations and their status, one obligation's
sequent, and ProB model checking through `eventb-animate`. Every call
returns one JSON document; the host edits the `.eventb` files with its own
tools and the server only reads them.

The server is started by the `rossi mcp` subcommand of `rossi-cli`:

```json
{
  "mcpServers": {
    "rossi": {
      "type": "stdio",
      "command": "rossi",
      "args": ["mcp", "--root", "${CLAUDE_PROJECT_DIR}"],
      "timeout": 600000
    }
  }
}
```

As a library, `RossiServer::new(root)` builds the handler and
`serve_stdio` runs it over standard input and output. The tool surface
(names, descriptions, schemas) is pinned by `tests/fixtures/tools.json`.
