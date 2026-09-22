# Changelog

## Unreleased

### Security

- The settings naming an executable — `rossi.tool.path`,
  `rossi.languageServer.path`, `rossi.rodin.path` and `rossi.animate.path` — are
  now user/machine settings, and `rossi.rodin.workspace` is machine-overridable.
  A workspace's own `.vscode/settings.json` could previously supply them, so
  opening a cloned repository was enough to have the extension launch a program
  of that repository's choosing — `rossi.languageServer.path` before any user
  action. Move them to your user `settings.json` if you had been setting them
  per project.
- The extension now declares limited support for untrusted workspaces, so it
  stays usable in Restricted Mode; the five settings above are ignored there in
  favour of the built-in defaults.

### Fixed

- The auto-downloaded toolchain is now kept in lock-step with the extension: the
  extension downloads only the binaries matching its own version (no silent
  fall-back to a different release), and removes previously downloaded versions
  from global storage on update instead of leaving them behind. (#146)

### Changed

- Open and broken proof obligations underline only the element's `@label`
  instead of the whole element, so a machine with many open invariants is no
  longer wavy end to end.

### Added

- A `rossi.validate.runtime` setting (off by default) that passes `--runtime`
  to `rossi validate`, on save and from the Validate commands, so the EB1xx
  runtime-translation suitability checks show up in Problems.
- Proof status is drawn in the gutter the way the Dafny and Lean extensions
  draw verification status: a rail beside every event and clause, one green
  check on a block whose obligations are all closed, and per-label icons with
  a gray (open) or amber (broken) rail otherwise. The rail is lighter than the
  icon beside it and each theme has its own tones, since a gutter icon cannot
  take a theme colour. `rossi.proofObligations.gutter` switches back to plain
  per-line icons or turns the gutter off.
- A `rossi.proofObligations.diagnostics` setting choosing what an open or
  broken obligation's diagnostic underlines: the element's `@label` (the
  default), the whole element, or nothing.
- A `source.fixAll.rossi` code action that rewrites every operator spelling to
  the `rossi.format.useUnicode` convention and changes nothing else, so
  `"[eventb]": { "editor.codeActionsOnSave": { "source.fixAll.rossi": "explicit" } }`
  keeps a project's operators in line on save without reformatting.
- A `rossi.format.enforceUnicode` setting (off by default) that flags ASCII
  operator spellings outside comments and labels with an advisory diagnostic
  and a quick fix, so a Unicode-only convention is visible before save.
- Sensible per-language editor defaults for `.eventb` files: semantic
  highlighting is enabled out of the box, and the ambiguous / non-basic-ASCII
  Unicode warnings are silenced so Event-B's math operators (`∀ ∃ ⇒ ∈ ↦ ℕ`) no
  longer trigger spurious warnings. Scoped to Event-B only — other languages are
  unaffected, and no theme colors are overridden.

## 0.1.0

Initial release.

### Added

- Event-B language support for `.eventb` files: syntax highlighting, code
  snippets, and a language configuration (brackets, comments, auto-closing).
- Language server features powered by the Rossi toolchain: real-time
  diagnostics, document symbols/outline, formatting (Unicode or ASCII
  operators), completion, and hover.
- ASCII-to-Unicode symbol input as you type (e.g. `=>` → `⇒`, `\and` → `∧`).
- Rodin integration commands: import a Rodin project, export the current file or
  workspace to a Rodin ZIP, build a checked Rodin ZIP, and open in Rodin.
- Validation commands and keybindings, Unicode/ASCII conversion commands, and a
  `Check Toolchain` command.
- Automatic toolchain download: when `eventb-language-server` and `rossi` are not
  found on `PATH` or configured paths, the extension fetches and verifies the
  matching prebuilt binaries from the project's GitHub release.
- A four-step "Get Started with Event-B" walkthrough.
