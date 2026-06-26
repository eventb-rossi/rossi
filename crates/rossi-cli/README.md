# rossi-cli

[![crates.io](https://img.shields.io/crates/v/rossi-cli?label=crates.io)](https://crates.io/crates/rossi-cli)

The `rossi` command-line tool for the Event-B formal modeling language —
part of the
[Rossi](https://github.com/eventb-rossi/rossi) toolchain. It wraps the
[`rossi`](https://crates.io/crates/rossi) parser, the
[`rossi-build`](https://crates.io/crates/rossi-build) static checker, and the
[`eventb-lsp`](https://crates.io/crates/eventb-lsp) language server behind a
single binary named `rossi`.

## Installation

From crates.io:

```bash
cargo install rossi-cli
```

This installs a `rossi` executable. Prebuilt packages are also available from
the major package managers (each also installs the `eventb-language-server`):

```bash
# Homebrew (macOS / Linux)
brew tap eventb-rossi/tap
brew install rossi

# APT (Ubuntu 26.04 "Resolute" or later)
curl -fsSL https://eventb-rossi.github.io/apt/KEY.gpg \
  | sudo gpg --dearmor -o /etc/apt/keyrings/eventb.gpg
echo "deb [signed-by=/etc/apt/keyrings/eventb.gpg] https://eventb-rossi.github.io/apt resolute main" \
  | sudo tee /etc/apt/sources.list.d/eventb.list
sudo apt update
sudo apt install rossi

# Scoop (Windows)
scoop bucket add eventb https://github.com/eventb-rossi/scoop-eventb
scoop install eventb/rossi

# Gentoo
eselect repository eventb-rossi
emaint sync -r eventb-rossi
emerge -av rossi

# Fedora (COPR)
sudo dnf copr enable @eventb-rossi/eventb-copr
sudo dnf install rossi
```

## Subcommands

| Subcommand | Purpose |
|------------|---------|
| `validate` | Validate `.eventb` files, Rodin `.zip` archives, or unzipped Rodin project directories. |
| `import`   | Import a Rodin `.zip` / `.buc` / `.bum` / directory into `.eventb` text. |
| `export`   | Export `.eventb` / `.txt` / directory into a Rodin `.zip` archive. |
| `fmt`      | Reformat Event-B in place (operator convention, indentation). |
| `build`    | Static-check a Rodin project and emit `.bcc` / `.bcm` checked XML. |
| `lsp`      | Run the Rossi language server over stdio (equivalent to the `eventb-language-server` binary). |
| `completions` | Print a shell completion script to stdout (run `rossi completions --help` for the supported shells). |

```bash
rossi validate model.eventb
rossi fmt --ascii model.eventb
rossi build project.zip
```

Run `rossi --help` (or `rossi <subcommand> --help`) for the full set of
options. See the [project README](https://github.com/eventb-rossi/rossi) for
the complete toolchain and editor integrations.

## Shell completions

`rossi completions <shell>` writes a completion script to stdout, generated
from the same command tree the CLI is parsed with — so it always matches the
installed version. Redirect it where your shell looks for completions:

```bash
# bash — system-wide
rossi completions bash | sudo tee /etc/bash_completion.d/rossi >/dev/null

# zsh — a directory on your $fpath
rossi completions zsh > ~/.zsh/completions/_rossi

# fish
rossi completions fish > ~/.config/fish/completions/rossi.fish
```

Or load it for the current session only with `eval "$(rossi completions bash)"`.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
