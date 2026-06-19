# Event-B Language Support for Sublime Text, bat, and delta

## Intro

`EventB.sublime-syntax` is a Sublime Text syntax definition. It is read by the
[`syntect`](https://github.com/trishume/syntect) library, so the same file gives
Event-B highlighting in **Sublime Text**, **[bat](https://github.com/sharkdp/bat)**
(`cat` with wings), and **[delta](https://github.com/dandavison/delta)** (the git
pager). The last two do not support the Rossi language server, only syntax highlighting.

> **Generated file — do not edit by hand.** It is produced from the canonical
> token tables by `cargo run -p rossi-cli -- gen-grammars` and checked in CI.
> Change the tables in `crates/rossi/src/{operators,keywords,builtins}.rs` and
> regenerate.

## Sublime Text

Copy `EventB.sublime-syntax` into your `Packages/User/` directory
(`Preferences → Browse Packages…`). Files ending in `.eventb` highlight
automatically.

To enable the Rossi language server, you need to install [Package Control](https://docs.sublimetext.io/guide/package-control/usage.html) and the [LSP package](https://packages.sublimetext.io/packages/LSP).

Open the Sublime Text Command Palette:

```
Windows/Linux: Ctrl + Shift + P
macOS: Cmd + Shift + P
```

Type and select Preferences: `LSP Server Configurations`.This will open a split window layout. The left side displays the default settings, while the right side displays your user customization file (LanguageServers.sublime-settings). Add the following snippet:

```
{
	"eventb-language-server": {
      "enabled": true,
      "command": ["eventb-language-server"],
      "selector": "source.eventb"
    }
}
```

This assumes that you already installed LSP and have `eventb-language-server` in your `PATH`.

## bat and delta

```sh
mkdir -p "$(bat --config-dir)/syntaxes"
cp EventB.sublime-syntax "$(bat --config-dir)/syntaxes/"
bat cache --build
bat --list-languages | grep -i event-b   # confirm it registered
```

`delta` reuses bat's syntax set, so once bat knows Event-B, `git diff` of an
`.eventb` file through delta is highlighted too. Verify with:

```sh
bat sample.eventb
```
