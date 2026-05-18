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
- **ProB Integration**: Run ProB animator and model checker

## Quick Start

### 1. Install the Language Server

```bash
# Clone the repository (if you haven't already)
git clone https://github.com/eventb-rossi/rossi
cd rossi

# Build and install the language server
cargo install --path crates/rossi-lsp

# Verify installation
rossi-language-server --version
```

The server will be installed to `~/.cargo/bin/rossi-language-server`.

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
       axm1: x = 42
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
(setq lsp-rossi-format-max-line-length 100)  ; Maximum line length

;; Diagnostics options
(setq lsp-rossi-diagnostics-enabled t)       ; Enable/disable diagnostics
(setq lsp-rossi-diagnostics-debounce-ms 500) ; Debounce delay in milliseconds

;; Completion options
(setq lsp-rossi-completion-enabled t)        ; Enable/disable completion
```

### Custom Server Path

If the server is not in your PATH:

```elisp
(setq rossi-language-server-command "/path/to/rossi-language-server")

;; Or with arguments:
(setq rossi-language-server-command '("/path/to/rossi-language-server" "--log-level" "debug"))
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

### ProB Integration

Run ProB directly from Emacs:
- Code lens appears on MACHINE/CONTEXT declarations
- Use `M-x lsp-avy-lens` to execute code lens actions
- Animate or model check your specifications
- Counterexamples shown as diagnostics

## Recommended Packages

### Essential
- [lsp-mode](https://github.com/emacs-lsp/lsp-mode) - LSP client for Emacs
- [company](https://github.com/company-mode/company-mode) - Completion framework

### Highly Recommended
- [lsp-ui](https://github.com/emacs-lsp/lsp-ui) - Enhanced LSP UI (sideline, documentation)
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
  (setq lsp-headerline-breadcrumb-enable t))

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

;; Event-B Mode
(use-package eventb-mode
  :load-path "/path/to/rossi/editors/emacs"
  :mode "\\.eventb\\'"
  :hook (eventb-mode . lsp-deferred)
  :bind (:map eventb-mode-map
         ("C-c C-f" . lsp-format-buffer)
         ("C-c C-a" . lsp-execute-code-action)
         ("C-c r"   . lsp-rename))
  :config
  ;; Event-B specific settings
  (setq lsp-rossi-format-use-unicode t)
  (setq lsp-rossi-format-indentation "    ")

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
which rossi-language-server
```

If not found, specify the full path:
```elisp
(setq rossi-language-server-command "~/.cargo/bin/rossi-language-server")
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
