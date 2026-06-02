# Rossi Event-B - Installation Guide

Quick guide to installing and using the Rossi Event-B extension for VS Code.

## Quick Install

### Step 1: Install the LSP Server and CLI

The extension requires the Rossi Language Server for editor features and the
`rossi` CLI for Rodin import/export/build/validation commands. Install them
using one of these methods:

**Option A: From Source (Recommended)**
```bash
git clone https://github.com/eventb-rossi/rossi.git
cd rossi
cargo build --release --bin rossi-language-server --bin rossi
```

Then copy the binaries to a directory in your PATH:
```bash
# Linux/macOS
sudo cp target/release/rossi-language-server /usr/local/bin/
sudo cp target/release/rossi /usr/local/bin/

# Or add to PATH in your shell config (~/.bashrc, ~/.zshrc):
export PATH="$PATH:/path/to/rossi/target/release"
```

**Option B: Download Pre-built Binary**
*(When available)*
Download from [GitHub Releases](https://github.com/eventb-rossi/rossi/releases)

### Step 2: Install the VS Code Extension

**Option A: From VSIX File**
```bash
code --install-extension rossi-eventb-0.1.0.vsix
```

**Option B: From VS Code Marketplace**
*(When published)*
1. Open Extensions (Ctrl+Shift+X)
2. Search for "Rossi Event-B"
3. Click Install

### Step 3: Verify Installation

1. Create a test file: `test.eventb`
2. Type the following:
   ```eventb
   CONTEXT test
   SETS
       VALUE
   END
   ```
3. You should see:
   - Syntax highlighting
   - No error underlines (diagnostics working)
   - Outline view populated (document symbols)

## Configuration

### Basic Setup

No configuration needed if `rossi-language-server` and `rossi` are in your PATH.
`Open in Rodin` additionally works without configuration when Rodin is available at the platform default:

- macOS: `/Applications/Rodin.app`
- Windows: `rodin.exe` in `PATH`
- Linux: `rodin` in `PATH`

### Custom Tool Paths

If either Rossi binary is not in `PATH`, configure it:

1. Open Settings (Ctrl+,)
2. Search for "rossi"
3. Set "Event-B: Language Server Path" and "Rossi: Tool Path" to the full paths:
   ```
   /path/to/rossi-language-server
   /path/to/rossi
   ```

Or edit `settings.json` directly:
```json
{
  "rossi.languageServer.path": "/path/to/rossi-language-server",
  "rossi.tool.path": "/path/to/rossi"
}
```

Configure `rossi.rodin.path` only if `Open in Rodin` cannot use the platform default. Examples:

```json
{
  "rossi.rodin.path": "/Applications/Rodin.app"
}
```

```json
{
  "rossi.rodin.path": "C:\\tools\\rodin\\rodin.exe"
}
```

```json
{
  "rossi.rodin.path": "/opt/rodin/rodin"
}
```

### Recommended Settings

```json
{
  "rossi.languageServer.path": "rossi-language-server",
  "rossi.tool.path": "rossi",
  "rossi.rodin.path": "",
  "rossi.format.useUnicode": true,
  "rossi.format.indentation": "    ",
  "rossi.format.maxLineLength": 100,
  "rossi.diagnostics.enabled": true,
  "rossi.diagnostics.debounceMs": 500,
  "rossi.completion.enabled": true,
  "rossi.prob.enabled": true,
  "rossi.prob.path": "",
  "rossi.prob.timeout": 10000,
  "rossi.prob.animateSteps": 5,
  "editor.formatOnSave": true,
  "editor.quickSuggestions": {
    "other": true,
    "comments": false,
    "strings": false
  }
}
```

## Features & Usage

### Syntax Highlighting

Automatic for `.eventb` files. Supports:
- Unicode operators: ∧, ∨, ⇒, ∈, ∀, ∃
- ASCII operators: /\, \/, =>, :, !, #
- Keywords: CONTEXT, MACHINE, EVENTS, etc.

### Code Snippets

Type a prefix and press Tab:

| Prefix | Description |
|--------|-------------|
| `ctx` | Context template |
| `mch` | Machine template |
| `evt` | Event with guards and actions |
| `init` | Initialisation event |
| `axm` | Labeled axiom |
| `inv` | Labeled invariant |
| `forall` | Universal quantification |
| `exists` | Existential quantification |

### Document Symbols (Outline)

- View > Open View > Outline
- Or press Ctrl+Shift+O to search symbols
- Shows hierarchical structure of your model

### Formatting

- Format Document: Shift+Alt+F
- Toggle Unicode/ASCII: Change `rossi.format.useUnicode` setting
- Convert current file: Command Palette > `Rossi: Convert Current File to Unicode` or `Rossi: Convert Current File to ASCII`

### Rodin Import / Export / Build

These commands require the `rossi` CLI. Configure `rossi.tool.path` if it is not in your PATH.

- Command Palette > `Rossi: Import Rodin Project`
- Command Palette > `Rossi: Export Current File to Rodin ZIP`
- Command Palette > `Rossi: Export Workspace to Rodin ZIP`
- Command Palette > `Rossi: Open in Rodin`
- Command Palette > `Rossi: Build Checked Rodin ZIP`
- Command Palette > `Rossi: Validate Current File`
- Command Palette > `Rossi: Validate Workspace`
- Command Palette > `Rossi: Check Toolchain`

`Open in Rodin` requires the Rodin IDE. With `rossi.rodin.path` left empty, the extension uses `/Applications/Rodin.app` on macOS, `rodin.exe` on Windows, and `rodin` on Linux. The command exports a temporary one-way Rodin project, registers it in a temporary Rodin workspace, and launches Rodin. Edits made in Rodin are not synced back to `.eventb` files. Validation results are shown in the Problems panel and detailed command output is written to the `Rossi` output channel.

### ProB

If `probcli` is installed, the language server provides ProB code lenses. You can also run:

- Command Palette > `Rossi: Animate with ProB`
- Command Palette > `Rossi: Model Check with ProB`

### Code Completion

Start typing anywhere - suggestions appear automatically:
- Keywords (CONTEXT, MACHINE, etc.)
- Operators (Unicode and ASCII)
- Declared identifiers (sets, constants, variables)

### Hover Documentation

Hover over identifiers to see:
- Type information
- Where defined
- Declaration context

### Go to Definition

- Ctrl+Click on identifier
- Or F12
- Or Right-click > Go to Definition

### Find All References

- Right-click > Find All References
- Or Shift+F12
- Shows all uses of an identifier

### Rename Symbol

- Right-click > Rename Symbol
- Or F2
- Renames consistently throughout the file

### Workspace Symbols

- Press Ctrl+T
- Search for symbols across all `.eventb` files
- Quick navigation in large projects

### Diagnostics (Error Checking)

Real-time syntax error detection:
- Red underlines show errors
- Hover for error message
- Check Problems panel (Ctrl+Shift+M)

## Keyboard Shortcuts

| Action | Windows/Linux | macOS |
|--------|---------------|-------|
| Format Document | Shift+Alt+F | Shift+Option+F |
| Go to Definition | F12 | F12 |
| Find References | Shift+F12 | Shift+F12 |
| Rename Symbol | F2 | F2 |
| Symbol Search | Ctrl+Shift+O | Cmd+Shift+O |
| Workspace Symbols | Ctrl+T | Cmd+T |
| Code Completion | Ctrl+Space | Ctrl+Space |
| Quick Fix | Ctrl+. | Cmd+. |

## Troubleshooting

### Extension Not Working

**Check Output Panel:**
1. View > Output
2. Select "Rossi Language Server" from dropdown
3. Look for errors

**Common Issues:**

**LSP server not found**
```
Error: spawn rossi-language-server ENOENT
```
Solution: Install the server or configure the path in settings.

**LSP server crashes**
```
The Rossi Language Server crashed 5 times...
```
Solution: Check server is built correctly, try rebuilding.

**No syntax highlighting**
- Check file extension is `.eventb`
- Check language mode (bottom-right corner) is "Event-B"
- Restart VS Code

**Snippets not appearing**
- Press Ctrl+Space to trigger manually
- Enable in Settings > Editor > Suggest > Snippets

**Formatting not working**
- Check for syntax errors (formatting requires valid syntax)
- Check Problems panel (Ctrl+Shift+M)

### Getting Help

1. Check the [README](README.md)
2. Run `Rossi: Check Toolchain` from the Command Palette
3. [GitHub Issues](https://github.com/eventb-rossi/rossi/issues)
4. [GitHub Discussions](https://github.com/eventb-rossi/rossi/discussions)

## Examples

### Example 1: Simple Counter Context

```eventb
CONTEXT counter_ctx
CONSTANTS
    MAX_VALUE
AXIOMS
    axm1: MAX_VALUE = 100
    axm2: MAX_VALUE > 0
END
```

### Example 2: Counter Machine

```eventb
MACHINE counter
SEES counter_ctx
VARIABLES
    count
INVARIANTS
    inv1: count ∈ ℕ
    inv2: count ≤ MAX_VALUE
EVENTS
    INITIALISATION
    BEGIN
        count := 0
    END

    EVENT increment
    WHERE
        grd1: count < MAX_VALUE
    THEN
        act1: count := count + 1
    END

    EVENT decrement
    WHERE
        grd1: count > 0
    THEN
        act1: count := count - 1
    END
END
```

### Example 3: Using Snippets

1. Type `mch` and press Tab
2. Fill in the placeholders
3. Press Tab to move between fields
4. Customize as needed

## Uninstalling

```bash
code --uninstall-extension rossi.rossi-eventb
```

Or via Extensions view: Right-click extension > Uninstall

## What's Next?

- Explore all code snippets (type prefix + Tab)
- Try workspace-wide symbol search (Ctrl+T)
- Enable format on save for consistent style
- Check out LSP features like hover and go-to-definition

## Feedback

Found a bug or have a feature request?
- [Report an Issue](https://github.com/eventb-rossi/rossi/issues/new)
- [Start a Discussion](https://github.com/eventb-rossi/rossi/discussions)

Enjoy using Rossi Event-B! 🎉
