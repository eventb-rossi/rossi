# Rossi Event-B for VS Code

This extension provides comprehensive language support for Event-B formal modeling in Visual Studio Code, powered by the Rossi Language Server.

## Features

### 🎨 Syntax Highlighting
- Full syntax highlighting for Event-B constructs
- Support for both Unicode (∧, ∨, ⇒, ∈) and ASCII operators (/\, \/, =>, :)
- Syntax highlighting for:
  - Keywords (CONTEXT, MACHINE, EVENTS, etc.)
  - Logical operators
  - Set operators
  - Relation operators
  - Arithmetic operators

### 🔍 Real-time Diagnostics
- Instant feedback on syntax errors as you type
- Error recovery continues parsing after errors
- Clear error messages with precise locations

### 🗂️ Document Symbols & Navigation
- Hierarchical outline view in sidebar
- Breadcrumb navigation at the top of the editor
- Quick symbol search with `Ctrl+Shift+O` (Windows/Linux) or `Cmd+Shift+O` (Mac)
- Navigate through:
  - Contexts: Sets, Constants, Axioms, Theorems
  - Machines: Variables, Invariants, Events, Variant
  - Events: Parameters, Guards, Witnesses, Actions

### ✨ Code Formatting
- Auto-format documents with consistent style
- Choose between Unicode or ASCII operators
- Configurable indentation
- Format on save support
- Keyboard shortcuts:
  - Format Document: `Shift+Alt+F` (Windows/Linux) or `Shift+Option+F` (Mac)

### 🔁 Rodin Interoperability
- Import Rodin `.zip`, `.buc`, `.bum`, or XML project folders into `.eventb` files
- Export the current `.eventb` file or workspace to a Rodin `.zip`
- Build checked Rodin `.zip` archives with generated `.bcc` / `.bcm` files
- Run on-demand validation and show results in VS Code Problems
- Convert the current `.eventb` file between Unicode and ASCII notation
- Trigger ProB animation and model checking through the language server

## Requirements

The Rossi Language Server must be installed and accessible in your PATH. Rodin import/export/build/validation commands also require the `rossi` CLI.

### Installation

From the project root:

```bash
cargo build --release --bin rossi-language-server
# Binary available at: target/release/rossi-language-server
```

Add the binary to your PATH or specify the full path in settings.

## Extension Settings

This extension contributes the following settings:

- `rossi.languageServer.path`: Path to the Event-B language server executable (default: searches in PATH)
- `rossi.tool.path`: Path to the Rossi CLI executable used for import, export, build, validation, and conversion commands (default: `rossi`)
- `rossi.format.useUnicode`: Use Unicode operators (∧, ∨, ⇒, ∈) instead of ASCII (/\, \/, =>, :) when formatting (default: `true`)
- `rossi.format.indentation`: Indentation string (spaces or tabs) to use when formatting (default: `"    "` - 4 spaces)
- `rossi.diagnostics.enabled`: Enable real-time diagnostics for syntax errors (default: `true`)
- `rossi.trace.server`: Traces communication between VS Code and the language server (default: `"off"`)

### Example Configuration

Add to your `.vscode/settings.json`:

```json
{
  "rossi.languageServer.path": "/path/to/rossi-language-server",
  "rossi.tool.path": "/path/to/rossi",
  "rossi.format.useUnicode": true,
  "rossi.format.indentation": "    ",
  "rossi.diagnostics.enabled": true,
  "editor.formatOnSave": true
}
```

## Usage

### Creating Event-B Files

1. Create a new file with `.eventb` extension
2. Start typing Event-B code
3. Enjoy syntax highlighting, diagnostics, and navigation

### Example: Simple Context

```eventb
CONTEXT counter_ctx
SETS
    STATUS
CONSTANTS
    max_value
AXIOMS
    axm1: max_value = 100
    axm2: max_value > 0
END
```

### Example: Simple Machine

```eventb
MACHINE counter
VARIABLES
    count
INVARIANTS
    inv1: count >= 0
    inv2: count <= 100
EVENTS
    INITIALISATION
    BEGIN
        count := 0
    END

    EVENT increment
    WHERE
        grd1: count < 100
    THEN
        act1: count := count + 1
    END
END
```

### Formatting

- **Format entire document**: `Shift+Alt+F` (Windows/Linux) or `Shift+Option+F` (Mac)
- **Format on save**: Enable `"editor.formatOnSave": true` in settings
- **Choose operator style**: Set `rossi.format.useUnicode` to `true` (Unicode) or `false` (ASCII)

### Symbol Navigation

- **Outline view**: Open the Outline panel in the sidebar (Explorer view)
- **Breadcrumbs**: Navigate using breadcrumbs at the top of the editor
- **Symbol search**: Press `Ctrl+Shift+O` (Windows/Linux) or `Cmd+Shift+O` (Mac) to search symbols in the current file

### Rodin Commands

Open the Command Palette and run:

- `Rossi: Import Rodin Project`
- `Rossi: Export Current File to Rodin ZIP`
- `Rossi: Export Workspace to Rodin ZIP`
- `Rossi: Build Checked Rodin ZIP`
- `Rossi: Validate Current File`
- `Rossi: Validate Workspace`
- `Rossi: Convert Current File to Unicode`
- `Rossi: Convert Current File to ASCII`
- `Rossi: Animate with ProB`
- `Rossi: Model Check with ProB`
- `Rossi: Check Toolchain`

Rodin and conversion commands shell out to the configured `rossi.tool.path`. ProB commands are forwarded to the language server and use the `rossi.prob.*` settings.

## Contributing

Contributions are welcome! See the [main repository](https://github.com/eventb-rossi/rossi) for development guidelines.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](../../LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Resources

- [Event-B Language](https://wiki.event-b.org/index.php/Event-B_Language)
- [Rodin Platform](https://www.event-b.org/)
- [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
