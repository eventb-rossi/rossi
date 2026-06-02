# Event-B syntax for Sublime Text, bat, and delta

`EventB.sublime-syntax` is a Sublime Text syntax definition. It is read by the
[`syntect`](https://github.com/trishume/syntect) library, so the same file gives
Event-B highlighting in **Sublime Text**, **[bat](https://github.com/sharkdp/bat)**
(`cat` with wings), and **[delta](https://github.com/dandavison/delta)** (the git
pager) — none of which run the Rossi language server.

> **Generated file — do not edit by hand.** It is produced from the canonical
> token tables by `cargo run -p rossi-cli -- gen-grammars` and checked in CI.
> Change the tables in `crates/rossi/src/{operators,keywords,builtins}.rs` and
> regenerate.

## Sublime Text

Copy `EventB.sublime-syntax` into your `Packages/User/` directory
(`Preferences → Browse Packages…`). Files ending in `.eventb` highlight
automatically.

## bat (and delta)

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
