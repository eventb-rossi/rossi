# Event-B (Rossi) — Installation Guide

Quick guide to installing the Event-B (Rossi) extension for VS Code. For
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
offline machine, or a custom build — use a package manager (each installs both
`rossi` and `eventb-language-server`); the extension then picks them up from
`PATH` (or point the settings at them):

```bash
# Homebrew (macOS / Linux)
brew tap eventb-rossi/tap && brew install rossi

# APT (Ubuntu 26.04 "Resolute" or later)
curl -fsSL https://eventb-rossi.github.io/apt/KEY.gpg \
  | sudo gpg --dearmor -o /etc/apt/keyrings/eventb.gpg
echo "deb [signed-by=/etc/apt/keyrings/eventb.gpg] https://eventb-rossi.github.io/apt resolute main" \
  | sudo tee /etc/apt/sources.list.d/eventb.list
sudo apt update && sudo apt install rossi

# Scoop (Windows)
scoop bucket add eventb https://github.com/eventb-rossi/scoop-eventb
scoop install eventb/rossi

# Gentoo
eselect repository eventb-rossi && emaint sync -r eventb-rossi && emerge -av rossi

# Fedora (COPR)
sudo dnf copr enable @eventb-rossi/eventb-copr && sudo dnf install rossi

# cargo
cargo install rossi-cli eventb-lsp
```

Or build from source:

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

**Option A: From the Marketplace / Open VSX (recommended)**

- **VS Code** — open Extensions (`Ctrl+Shift+X`), search "Event-B", and click
  Install; install from the
  [Marketplace listing](https://marketplace.visualstudio.com/items?itemName=rossi.event-b);
  or run `code --install-extension rossi.event-b`.
- **VSCodium** — install from
  [Open VSX](https://open-vsx.org/extension/rossi/event-b) or run
  `codium --install-extension rossi.event-b`.

**Option B: From a VSIX file**

Build the VSIX from the repository, then install it. From the repository root:
```bash
cd editors/vscode
npm ci
npm run package
code --install-extension event-b-0.1.0.vsix
```

`npm run package` prints the name of the generated `.vsix`; adjust the install
command if the version differs.

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
The `Open in Rodin` code lens additionally works without configuration when Rodin is available at the platform default:

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

These are user/machine settings: a workspace's own `.vscode/settings.json`
cannot supply them, so cloning a repository can never redirect the binaries
the extension launches. To edit them by hand, open your **user**
`settings.json` (`Ctrl+Shift+P` → *Preferences: Open User Settings (JSON)*):
```json
{
  "rossi.languageServer.path": "/path/to/eventb-language-server",
  "rossi.tool.path": "/path/to/rossi"
}
```

Configure `rossi.rodin.path` — a user setting too — only if the `Open in Rodin` code lens cannot use the platform default (the lens builds into a persistent `.rossi/rodin` workspace next to your sources — relocatable via `rossi.rodin.workspace`, which a trusted workspace may also set — so proofs made in Rodin survive rebuilds; consider gitignoring `.rossi/`). Examples:

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

### The Rodin math font

Four Event-B operators have no standard Unicode code point, so Rodin encodes
them in the Unicode Private Use Area: `<<->` (U+E100), `<->>` (U+E101),
`<<->>` (U+E102) and `<+` (U+E103). Rossi writes them in ASCII, which renders
in any font, so **you do not need this font for normal use**. Install it only
if you turn on `rossi.format.privateUseGlyphs` to exchange files with a tool
that reads no other spelling, such as Rodin's own editors.

The font is Brave Sans Mono Roman, shipped with the extension at
`fonts/bravesansmono_roman.ttf` inside the installed extension directory:

- macOS: `~/.vscode/extensions/rossi.event-b-<version>/fonts/`
- Linux: `~/.vscode/extensions/rossi.event-b-<version>/fonts/`
- Windows: `%USERPROFILE%\.vscode\extensions\rossi.event-b-<version>\fonts\`

**VS Code cannot install or load a font for you** — `editor.fontFamily`
resolves only against fonts installed in the operating system — so install the
file yourself:

- macOS: double-click the `.ttf` and press *Install Font*.
- Windows: right-click the `.ttf` and choose *Install*.
- Linux: `mkdir -p ~/.local/share/fonts && cp bravesansmono_roman.ttf
  ~/.local/share/fonts/ && fc-cache -f`

Restart VS Code afterwards, then name the font as a fallback for Event-B files
in `settings.json`. The extension does not set your editor font: a
language-specific default contributed by an extension outranks your own
`editor.fontFamily`, so shipping one would replace the font of everyone who
never enables `rossi.format.privateUseGlyphs`.

```json
{
  "[eventb]": {
    "editor.fontFamily": "'Your Font', 'Brave Sans Mono', monospace"
  }
}
```

Order matters. A font stack resolves per glyph, first match wins, so listing
the math font after your own keeps your font everywhere it has a glyph and
falls back to Brave Sans Mono only for the four private-use code points it
does not.

The font is a Bitstream Vera Sans Mono derivative by ETH Zurich, redistributed
unmodified under the Bitstream Vera Fonts License; see
[LICENSE-BraveSansMono.txt](LICENSE-BraveSansMono.txt).

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
code --uninstall-extension rossi.event-b
```

Or via Extensions view: Right-click extension > Uninstall
