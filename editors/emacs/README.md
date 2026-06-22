# Event-B Language Support for Emacs

This directory contains Emacs configuration for Event-B formal modeling, providing comprehensive language support through the Rossi Language Server.

## Features

### 🎨 Syntax Highlighting
- Full syntax highlighting for Event-B constructs
- Support for both Unicode (∧, ∨, ⇒, ∈) and ASCII operators (/\, \/, =>, :)
- Highlighting for:
  - Keywords (CONTEXT, MACHINE, EVENTS, etc.)
  - Logical, set, relation, and arithmetic operators
  - Labels (axioms, invariants, guards, actions)
  - Comments and numbers
  - Event and component names

### 🔍 LSP Features (via Language Server)
- **Real-time Diagnostics**: Instant feedback on syntax errors
- **Document Symbols**: Hierarchical outline and quick navigation
- **Code Formatting**: Auto-format with Unicode or ASCII operators
- **Code Completion**: Context-aware suggestions
- **Hover Documentation**: Operator and symbol documentation
- **Go-to-Definition**: Jump to symbol definitions (across files!)
- **Find References**: Find all symbol usages
- **Rename Symbol**: Rename symbols across your workspace
- **Workspace Symbols**: Search for symbols across files
- **Document Links**: Click SEES/REFINES/EXTENDS to navigate
- **Signature Help**: Parameter hints for quantifiers and lambda
- **Code Actions**: Quick fixes and refactorings
- **Folding Ranges**: Collapse/expand code sections
- **Selection Range**: Smart expand/shrink of the active region

### ✏️ Snippets (yasnippet)
- Ready-made templates for the common Event-B constructs
- Expand by typing a key and pressing `TAB`: `mch`, `ctx`, `evt`, `inv`, `grd`, `act`, and 9 more
- Tab through the mirrored fields; bodies use Unicode operators by default

### ⌨️ Unicode Input Method (Quail)
- Type Unicode operators with a backslash leader: `\to` → →, `\and` → ∧, `\nat` → ℕ, `\forall` → ∀
- Generated from the same canonical operator table as the language server, so the spellings never drift
- Leader-only by design — `=>`, `<=>`, and the other eager symbolic combos are a Neovim/VS Code convenience and are intentionally not bound here

### 🛠️ Commands
- Convert the current buffer between Unicode and ASCII notation
- Validate the current file against the Rossi checker
- Import, export, and build Rodin projects

## Quick Start

### 1. Install the Language Server

Install `eventb-language-server` via your package manager (Homebrew, Scoop,
Gentoo, or Fedora COPR — each installs it alongside the `rossi` CLI) or with
`cargo install eventb-lsp`. See the
[main Installation guide](../../README.md#installation) for the full matrix.

To build from a clone instead:

```bash
# Clone the repository (if you haven't already)
git clone https://github.com/eventb-rossi/rossi
cd rossi

# Build and install the language server
cargo install --path crates/eventb-lsp

# Verify installation
eventb-language-server --version
```

The server will be installed to `~/.cargo/bin/eventb-language-server`.

### 2. Install Emacs Configuration

**Option A: Using use-package (Recommended)**

Add to your Emacs configuration (`~/.emacs.d/init.el` or `~/.config/emacs/init.el`):

```elisp
(use-package eventb-mode
  :load-path "/path/to/rossi/editors/emacs"
  :mode "\\.eventb\\'"
  :hook (eventb-mode . lsp-deferred)
  :config
  ;; Configure Event-B settings
  (setq lsp-rossi-format-use-unicode t)
  (setq lsp-rossi-format-indentation "    "))
```

**Option B: Manual Configuration**

Add to your Emacs configuration:

```elisp
;; Add to load path
(add-to-list 'load-path "/path/to/rossi/editors/emacs")

;; Load Event-B mode
(require 'eventb-mode)

;; Enable LSP automatically
(add-hook 'eventb-mode-hook #'lsp-deferred)

;; Configure settings (optional)
(setq lsp-rossi-format-use-unicode t)
(setq lsp-rossi-format-indentation "    ")
```

### 3. Install Required Emacs Packages

Ensure you have `lsp-mode` installed. Using `use-package`:

```elisp
(use-package lsp-mode
  :ensure t
  :commands (lsp lsp-deferred)
  :init
  (setq lsp-keymap-prefix "C-c l")
  :config
  (lsp-enable-which-key-integration t))

;; Optional: Enhanced UI for LSP
(use-package lsp-ui
  :ensure t
  :hook (lsp-mode . lsp-ui-mode)
  :config
  (setq lsp-ui-doc-enable t)
  (setq lsp-ui-doc-position 'bottom)
  (setq lsp-ui-sideline-enable t))

;; Optional: Completion framework
(use-package company
  :ensure t
  :hook (prog-mode . company-mode)
  :config
  (setq company-minimum-prefix-length 1)
  (setq company-idle-delay 0.0))
```

### 4. Verify Installation

1. Open an Event-B file: `M-x find-file RET test.eventb RET`
2. Type some Event-B code:
   ```eventb
   CONTEXT test
   CONSTANTS
       x
   AXIOMS
       @axm1 x = 42
   END
   ```
3. Check LSP status: `M-x lsp-describe-session`

## Configuration Options

### Language Server Settings

All settings can be customized via Emacs customization interface (`M-x customize-group RET eventb RET`) or directly in your configuration:

```elisp
;; Formatting options
(setq lsp-rossi-format-use-unicode t)        ; Use Unicode (∧, ∨, ⇒) or ASCII (/\, \/, =>)
(setq lsp-rossi-format-indentation "    ")   ; Indentation string (spaces or tabs)
(setq lsp-rossi-format-max-line-length 100)  ; Parsed for future wrapping; not applied yet

;; Diagnostics options
(setq lsp-rossi-diagnostics-enabled t)       ; Enable/disable diagnostics
(setq lsp-rossi-diagnostics-debounce-ms 500) ; Parsed for future debouncing; diagnostics are immediate

;; Completion options
(setq lsp-rossi-completion-enabled t)        ; Enable/disable completion
```

### Pinned LSP Client Defaults

For the best Event-B experience, enable these `lsp-mode` features. They light up
capabilities the server already provides:

```elisp
(setq lsp-semantic-tokens-enable t)  ; Server-driven semantic highlighting
(setq lsp-extend-selection t)        ; Enable smart expand/shrink selection
```

With `lsp-extend-selection`, `M-x lsp-extend-selection` grows the active region
to the next syntactic scope (and `lsp-shrink-selection` reverses it).

### Custom Server Path

If the server is not in your PATH:

```elisp
(setq eventb-language-server-command "/path/to/eventb-language-server")

;; Or with debug logging:
(setq eventb-language-server-command
      '("sh" "-c" "RUST_LOG=debug exec /path/to/eventb-language-server"))
```

### Keybindings

Add custom keybindings for Event-B mode:

```elisp
(use-package eventb-mode
  :load-path "/path/to/rossi/editors/emacs"
  :mode "\\.eventb\\'"
  :bind (:map eventb-mode-map
         ("C-c C-f" . lsp-format-buffer)
         ("C-c C-a" . lsp-execute-code-action)
         ("C-c r"   . lsp-rename))
  :hook (eventb-mode . lsp-deferred))
```

## Features Overview

### Code Completion

Type to trigger completion:
- Keywords: `CONTEXT`, `MACHINE`, `EVENTS`, etc.
- Operators: Type `:` to get `∈`, type `/\` to get `∧`
- Symbols: Variables, constants, parameters from context

Completion is triggered automatically or manually with `M-x company-complete` (or your completion framework's command).

### Hover Documentation

Hover over any operator or symbol to see:
- Operator documentation with examples
- Symbol types and definitions
- Cross-references

Use `C-c l h h` or `lsp-ui-doc-glance` to show documentation.

### Go-to-Definition

Press `M-.` (or `C-c l g g`) on:
- Variables → Jump to VARIABLES clause
- Constants → Jump to CONSTANTS or axiom definition
- Event names → Jump to EVENT declaration
- SEES references → Open the context file!
- REFINES references → Open the abstract machine!

### Find References

Press `M-?` (or `C-c l g r`) to find all usages of:
- Variables across guards, actions, invariants
- Constants across axioms, guards, actions
- Events across refinement chains

### Rename Symbol

Press `C-c l r r` (or `M-x lsp-rename`) to rename:
- Variables, constants, parameters
- Updates all references across all files in workspace
- Safe refactoring with validation

### Code Actions

Press `C-c l a a` (or `M-x lsp-execute-code-action`) for:
- **Convert operators**: ASCII ↔ Unicode
- **Add missing END**: Quick fix for parse errors
- **Add missing clauses**: INVARIANTS, AXIOMS, etc.
- **Sort clauses**: Alphabetically sort VARIABLES, CONSTANTS

### Document Symbols

Press `C-c l g a` (or `M-x lsp-ui-imenu`) to see:
- Hierarchical outline of your Event-B file
- Quick navigation to any section

### Formatting

Press `C-c C-f` (or `M-x lsp-format-buffer`) to:
- Apply consistent indentation
- Normalize operators (Unicode or ASCII)
- Order clauses properly

Enable format-on-save:

```elisp
(add-hook 'eventb-mode-hook
          (lambda ()
            (add-hook 'before-save-hook #'lsp-format-buffer nil t)))
```

### Snippets

This directory ships [yasnippet](https://github.com/joaotavora/yasnippet)
templates under `snippets/eventb-mode/`. To enable them, add the directory to
`yas-snippet-dirs` and turn on `yas-minor-mode` in `eventb-mode`:

```elisp
(use-package yasnippet
  :ensure t
  :config
  (add-to-list 'yas-snippet-dirs "/path/to/rossi/editors/emacs/snippets")
  (yas-reload-all)
  (add-hook 'eventb-mode-hook #'yas-minor-mode))
```

Type a key and press `TAB` to expand it, then `TAB` again to move between fields:

| Key | Expands to |
|-----|------------|
| `mch` | A MACHINE skeleton (VARIABLES/INVARIANTS/EVENTS) |
| `ctx` | A CONTEXT skeleton (SETS/CONSTANTS/AXIOMS) |
| `evt` | An EVENT with WHERE/THEN |
| `inv` | An invariant label and predicate |
| `grd` | A guard label and predicate |
| `act` | An action label and assignment |

These are six of the 15 bundled keys; the rest mirror the VS Code and Neovim
snippet set (`axm`, `init`, `actnd`, `forall`, `lambda`, `refines`, …). Run
`M-x yas-describe-tables` in an Event-B buffer to list them all.

### Unicode Input Method

`eventb-input.el` defines a [Quail](https://www.gnu.org/software/emacs/manual/html_node/emacs/Input-Methods.html)
input method named `eventb` for typing Event-B Unicode operators. `eventb-mode`
activates it automatically because the `eventb-enable-input-method` defcustom
defaults to `t`; you can also toggle it per buffer or activate it by hand:

```elisp
;; Auto-enable in every Event-B buffer (default)
(setq eventb-enable-input-method t)
```

```
;; Toggle on/off in the current buffer
C-c C-i                          ; eventb-toggle-input-method

;; Or activate it explicitly
M-x eventb-activate-input-method
```

When the method is active the mode line shows the `EvB` indicator. It is a
**leader-only** method: prefix the spelling with a backslash and the symbol is
inserted immediately.

| Type | Inserts |
|------|---------|
| `\to` | → |
| `\and` | ∧ |
| `\or` | ∨ |
| `\not` | ¬ |
| `\nat` | ℕ |
| `\int` | ℤ |
| `\forall` | ∀ |
| `\exists` | ∃ |
| `\in` | ∈ |
| `\maplet` | ↦ |
| `\lambda` | λ |

Most operators accept several aliases (`\implies` and `\imp` both give ⇒). The
rule set is generated from the same canonical operator table as the language
server, so editor input and `rossi/operatorTable` can never disagree.

> Note: the eager symbolic combos that convert as you type — `=>` → ⇒, `<=>` → ⇔,
> `|->` → ↦, `:=` → ≔ — are a Neovim/VS Code-only convenience by design. In Emacs
> use the backslash leader, or the `Convert to Unicode` command below to convert a
> whole buffer at once.

### Commands

Beyond the LSP code actions, `eventb-mode` provides commands that drive the
Rossi CLI:

| Command | Action |
|---------|--------|
| `M-x eventb-convert-to-unicode` | Rewrite the current buffer with Unicode operators |
| `M-x eventb-convert-to-ascii` | Rewrite the current buffer with ASCII operators |
| `M-x eventb-validate` | Validate the current file against the Rossi checker |
| `M-x eventb-validate-workspace` | Validate every `.eventb` file in the workspace |
| `M-x eventb-import` | Import a Rodin project into `.eventb` files |
| `M-x eventb-export` | Export the current file to a Rodin ZIP |
| `M-x eventb-build` | Build a checked Rodin ZIP |
| `M-x eventb-toggle-input-method` (`C-c C-i`) | Toggle the backslash-leader Unicode input |

The conversion, validation, import, export, and build commands shell out to the
`rossi` CLI; ensure it is on your `PATH` or set `rossi-tool-path` to its
location.

```elisp
(setq rossi-tool-path "~/.cargo/bin/rossi")  ; defaults to "rossi" on exec-path
```

Suggested keybindings:

```elisp
(use-package eventb-mode
  :load-path "/path/to/rossi/editors/emacs"
  :mode "\\.eventb\\'"
  :bind (:map eventb-mode-map
         ("C-c C-u" . eventb-convert-to-unicode)
         ("C-c C-d" . eventb-convert-to-ascii)
         ("C-c C-v" . eventb-validate))
  :hook (eventb-mode . lsp-deferred))
```

## Recommended Packages

### Essential
- [lsp-mode](https://github.com/emacs-lsp/lsp-mode) - LSP client for Emacs
- [company](https://github.com/company-mode/company-mode) - Completion framework

### Highly Recommended
- [lsp-ui](https://github.com/emacs-lsp/lsp-ui) - Enhanced LSP UI (sideline, documentation)
- [yasnippet](https://github.com/joaotavora/yasnippet) - Template expansion for the bundled snippets
- [flycheck](https://github.com/flycheck/flycheck) - On-the-fly syntax checking
- [which-key](https://github.com/justbur/emacs-which-key) - Display available keybindings

### Optional but Useful
- [consult-lsp](https://github.com/gagbo/consult-lsp) - LSP integration with consult
- [helm-lsp](https://github.com/emacs-lsp/helm-lsp) - Helm integration for LSP
- [lsp-treemacs](https://github.com/emacs-lsp/lsp-treemacs) - Tree view for symbols
- [dap-mode](https://github.com/emacs-lsp/dap-mode) - Debugger integration (future)

## Example: Complete Configuration

Here's a complete example configuration using `use-package`:

```elisp
;; LSP Mode
(use-package lsp-mode
  :ensure t
  :commands (lsp lsp-deferred)
  :init
  (setq lsp-keymap-prefix "C-c l")
  :config
  (lsp-enable-which-key-integration t)
  (setq lsp-headerline-breadcrumb-enable t)
  ;; Pinned Event-B defaults
  (setq lsp-semantic-tokens-enable t)   ; Semantic highlighting
  (setq lsp-extend-selection t))        ; Smart expand/shrink selection

;; LSP UI
(use-package lsp-ui
  :ensure t
  :hook (lsp-mode . lsp-ui-mode)
  :config
  (setq lsp-ui-doc-enable t)
  (setq lsp-ui-doc-position 'bottom)
  (setq lsp-ui-doc-show-with-cursor t)
  (setq lsp-ui-sideline-enable t)
  (setq lsp-ui-sideline-show-diagnostics t))

;; Company (completion)
(use-package company
  :ensure t
  :hook (prog-mode . company-mode)
  :config
  (setq company-minimum-prefix-length 1)
  (setq company-idle-delay 0.1))

;; Snippets
(use-package yasnippet
  :ensure t
  :config
  (add-to-list 'yas-snippet-dirs "/path/to/rossi/editors/emacs/snippets")
  (yas-reload-all))

;; Event-B Mode
(use-package eventb-mode
  :load-path "/path/to/rossi/editors/emacs"
  :mode "\\.eventb\\'"
  :hook ((eventb-mode . lsp-deferred)
         (eventb-mode . yas-minor-mode))
  :bind (:map eventb-mode-map
         ("C-c C-f"   . lsp-format-buffer)
         ("C-c C-a"   . lsp-execute-code-action)
         ("C-c r"     . lsp-rename)
         ("C-c C-u"   . eventb-convert-to-unicode)
         ("C-c C-d"   . eventb-convert-to-ascii)
         ("C-c C-v"   . eventb-validate))
  :config
  ;; Event-B specific settings
  (setq lsp-rossi-format-use-unicode t)
  (setq lsp-rossi-format-indentation "    ")

  ;; Backslash-leader Unicode input is auto-enabled by eventb-mode;
  ;; set this to nil to opt out.
  (setq eventb-enable-input-method t)

  ;; Enable format on save
  (add-hook 'eventb-mode-hook
            (lambda ()
              (add-hook 'before-save-hook #'lsp-format-buffer nil t))))
```

## Keyboard Shortcuts Reference

### Navigation
| Key | Action |
|-----|--------|
| `M-.` | Go to definition (`lsp-find-definition`) |
| `M-,` | Return to previous location (`xref-pop-marker-stack`) |
| `M-?` | Find references (`lsp-find-references`) |
| `C-c l g g` | Go to definition |
| `C-c l g r` | Find references |
| `C-c l g i` | Go to implementation |
| `C-c l g a` | Show document symbols (imenu) |

### Documentation
| Key | Action |
|-----|--------|
| `C-c l h h` | Show hover documentation |
| `K` (in lsp-ui-doc) | Toggle documentation |

### Code Actions
| Key | Action |
|-----|--------|
| `C-c l a a` | Execute code action |
| `C-c l r r` | Rename symbol |
| `C-c C-f` | Format buffer |

### Event-B Commands (suggested binds)
| Key | Action |
|-----|--------|
| `TAB` (after a key) | Expand yasnippet (`mch`, `evt`, `inv`, …) |
| `\to` `\and` `\forall` … | Insert Unicode via the input method |
| `C-c C-u` | `eventb-convert-to-unicode` |
| `C-c C-d` | `eventb-convert-to-ascii` |
| `C-c C-v` | `eventb-validate` |

### Diagnostics
| Key | Action |
|-----|--------|
| `M-g n` | Next diagnostic (`flycheck-next-error`) |
| `M-g p` | Previous diagnostic (`flycheck-previous-error`) |
| `C-c l g e` | Show diagnostics list |

### Workspace
| Key | Action |
|-----|--------|
| `C-c l w a` | Add workspace folder |
| `C-c l w r` | Remove workspace folder |
| `C-c l g w` | Workspace symbols |

### LSP Session
| Command | Action |
|---------|--------|
| `M-x lsp-describe-session` | Show LSP session info |
| `M-x lsp-workspace-restart` | Restart language server |
| `M-x lsp` | Manually start LSP |

## Troubleshooting

### Server not starting

Check if the server is in your PATH:
```bash
which eventb-language-server
```

If not found, specify the full path:
```elisp
(setq eventb-language-server-command "~/.cargo/bin/eventb-language-server")
```

### No syntax highlighting

Ensure the file is in Event-B mode:
```
M-x describe-mode
```

Should show "Event-B mode". If not, manually enable it:
```
M-x eventb-mode
```

### No completions

Check if company-mode is active:
```
M-x describe-mode
```

Should list `Company` in minor modes. Enable it:
```
M-x company-mode
```

Ensure LSP client is connected:
```
M-x lsp-describe-session
```

### Unicode characters not displaying

Ensure your Emacs supports UTF-8:
```elisp
(prefer-coding-system 'utf-8)
(set-default-coding-systems 'utf-8)
(set-terminal-coding-system 'utf-8)
(set-keyboard-coding-system 'utf-8)
```

Install a font that supports Unicode symbols:
- JetBrains Mono
- Fira Code
- Cascadia Code
- DejaVu Sans Mono

Configure Emacs to use the font:
```elisp
(set-frame-font "JetBrains Mono-12" nil t)
```

### LSP server logs

Enable LSP logging:
```elisp
(setq lsp-log-io t)
```

View server logs:
```
M-x lsp-workspace-show-log
```

### Performance issues

Reduce LSP UI features:
```elisp
(setq lsp-ui-doc-enable nil)           ; Disable hover documentation popup
(setq lsp-ui-sideline-enable nil)      ; Disable sideline info
(setq lsp-enable-symbol-highlighting nil) ; Disable symbol highlighting
```

Increase debounce delay:
```elisp
(setq lsp-idle-delay 0.5)              ; Delay before updating (seconds)
```

## Contributing

Contributions are welcome! Please see the main repository for contribution guidelines:
https://github.com/eventb-rossi/rossi

## License

Dual licensed under MIT or Apache-2.0, matching the main Rossi project.

## Resources

- **Main Repository**: https://github.com/eventb-rossi/rossi
- **Event-B Resources**:
  - [Event-B.org](https://www.event-b.org/)
  - [Event-B Wiki](https://wiki.event-b.org/)
  - [Rodin Platform](https://www.event-b.org/platform.html)
  - [ProB Model Checker](https://prob.hhu.de/)
- **Emacs LSP Resources**:
  - [lsp-mode](https://emacs-lsp.github.io/lsp-mode/)
  - [lsp-ui](https://emacs-lsp.github.io/lsp-ui/)

## Support

- **Issues**: https://github.com/eventb-rossi/rossi/issues
- **Discussions**: https://github.com/eventb-rossi/rossi/discussions
