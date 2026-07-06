# Changelog

## Unreleased

### Fixed

- The auto-downloaded toolchain is now kept in lock-step with the extension: the
  extension downloads only the binaries matching its own version (no silent
  fall-back to a different release), and removes previously downloaded versions
  from global storage on update instead of leaving them behind. (#146)

### Added

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
