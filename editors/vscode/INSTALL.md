# Rossi Event-B - Installation Guide

Quick guide to installing the Rossi Event-B extension for VS Code. For
features, settings, and usage, see the [README](README.md).

## Quick Install

### Step 1: Get the LSP Server and CLI

The extension needs the Rossi language server for editor features and the
`rossi` CLI for the Rodin import/export/build/validation commands.

**The extension installs them for you.** On first activation, if neither binary
is found through the `rossi.languageServer.path` / `rossi.tool.path` settings or
on your `PATH`, the extension downloads the prebuilt binaries for your platform
from [GitHub Releases](https://github.com/eventb-rossi/rossi/releases), verifies
them against the release `SHA256SUMS`, and caches them. No manual step is needed
on Linux, macOS, or Windows (x86_64 or ARM64).

To install the binaries yourself instead — for an unsupported platform, an
offline machine, or a custom build — build them from source; the extension then
picks them up from `PATH` (or point the settings at them):

```bash
git clone https://github.com/eventb-rossi/rossi.git
cd rossi
cargo build --release --bin eventb-language-server --bin rossi
```

Then copy the binaries to a directory in your PATH:
```bash
# Linux/macOS
sudo cp target/release/eventb-language-server /usr/local/bin/
sudo cp target/release/rossi /usr/local/bin/

# Or add to PATH in your shell config (~/.bashrc, ~/.zshrc):
export PATH="$PATH:/path/to/rossi/target/release"
```

### Step 2: Install the VS Code Extension

**Option A: From VSIX File**

Build the VSIX from the repository, then install it. From the repository root:
```bash
cd editors/vscode
npm ci
npm run package
code --install-extension rossi-eventb-0.1.0.vsix
```

`npm run package` prints the name of the generated `.vsix`; adjust the install
command if the version differs.

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

No configuration needed if `eventb-language-server` and `rossi` are in your PATH.
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
   /path/to/eventb-language-server
   /path/to/rossi
   ```

Or edit `settings.json` directly:
```json
{
  "rossi.languageServer.path": "/path/to/eventb-language-server",
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

For the full list of settings (formatting, diagnostics, completion, symbol
input) and a complete example configuration, see the
[README](README.md#extension-settings).

## Troubleshooting

### Extension Not Working

**Check Output Panel:**
1. View > Output
2. Select "Rossi Language Server" from dropdown
3. Look for errors

**Common Issues:**

**LSP server not found**
```
Error: spawn eventb-language-server ENOENT
```
Solution: The extension downloads the server automatically, so this usually
means the download was skipped or failed (no network, or an unsupported
platform). Check the Rossi output channel, then install the server manually
(Step 1) or set `rossi.languageServer.path`.

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

## Uninstalling

```bash
code --uninstall-extension rossi.rossi-eventb
```

Or via Extensions view: Right-click extension > Uninstall
