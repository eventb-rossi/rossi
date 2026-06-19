# Event-B Emacs Installation Guide

This guide provides detailed installation instructions for setting up Event-B language support in Emacs.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Installation Methods](#installation-methods)
  - [Method 1: Using use-package (Recommended)](#method-1-using-use-package-recommended)
  - [Method 2: Manual Installation](#method-2-manual-installation)
  - [Method 3: Using Straight.el](#method-3-using-straightel)
- [Configuration](#configuration)
- [Verification](#verification)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

### Required

1. **Emacs 26.1 or later**
   ```bash
   emacs --version
   ```
   If you need to install or upgrade Emacs:
   - Ubuntu/Debian: `sudo apt install emacs`
   - Arch Linux: `sudo pacman -S emacs`
   - macOS: `brew install emacs` or `brew install --cask emacs`
   - Or download from: https://www.gnu.org/software/emacs/

2. **Rust toolchain** (to build the language server)
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source $HOME/.cargo/env
   ```

3. **Rossi Language Server**
   ```bash
   # Clone the repository
   git clone https://github.com/eventb-rossi/rossi
   cd rossi

   # Build and install the language server
   cargo install --path crates/eventb-lsp

   # Verify installation
   eventb-language-server --version
   ```

   The server will be installed to `~/.cargo/bin/eventb-language-server`.

### Recommended

1. **lsp-mode** - LSP client for Emacs
   ```elisp
   ;; Will be installed via package manager in the next steps
   ```

2. **company** - Completion framework
   ```elisp
   ;; Will be installed via package manager in the next steps
   ```

3. **yasnippet** - Template expansion for the bundled Event-B snippets
   ```elisp
   ;; Will be installed via package manager in the next steps
   ```

---

## Installation Methods

### Method 1: Using use-package (Recommended)

This is the recommended approach for most users.

#### Step 1: Ensure Package Archives are Configured

Add to your Emacs configuration (`~/.emacs.d/init.el` or `~/.config/emacs/init.el`):

```elisp
;; Enable package management
(require 'package)

;; Add MELPA repository (for lsp-mode and company)
(add-to-list 'package-archives '("melpa" . "https://melpa.org/packages/") t)

;; Initialize package system
(package-initialize)

;; Ensure use-package is installed
(unless (package-installed-p 'use-package)
  (package-refresh-contents)
  (package-install 'use-package))

(require 'use-package)
(setq use-package-always-ensure t)  ; Automatically install packages
```

#### Step 2: Install Dependencies

Add to your configuration:

```elisp
;; LSP Mode
(use-package lsp-mode
  :ensure t
  :commands (lsp lsp-deferred)
  :init
  (setq lsp-keymap-prefix "C-c l")
  :config
  (lsp-enable-which-key-integration t))

;; LSP UI (optional but recommended)
(use-package lsp-ui
  :ensure t
  :hook (lsp-mode . lsp-ui-mode)
  :config
  (setq lsp-ui-doc-enable t)
  (setq lsp-ui-doc-position 'bottom))

;; Company completion
(use-package company
  :ensure t
  :hook (prog-mode . company-mode)
  :config
  (setq company-minimum-prefix-length 1)
  (setq company-idle-delay 0.1))

;; Snippets (template expansion)
(use-package yasnippet
  :ensure t
  :config
  (add-to-list 'yas-snippet-dirs "/path/to/rossi/editors/emacs/snippets")
  (yas-reload-all))
```

#### Step 3: Install Event-B Mode

Add to your configuration (replace `/path/to/rossi` with the actual path):

```elisp
;; Event-B Mode
(use-package eventb-mode
  :load-path "/path/to/rossi/editors/emacs"
  :mode "\\.eventb\\'"
  :hook ((eventb-mode . lsp-deferred)
         (eventb-mode . yas-minor-mode))
  :config
  ;; Configure Event-B settings
  (setq lsp-rossi-format-use-unicode t)
  (setq lsp-rossi-format-indentation "    ")

  ;; Backslash-leader Unicode input (\to, \and, \forall, …) is auto-enabled
  ;; by eventb-mode; set this to nil to opt out.
  (setq eventb-enable-input-method t)

  ;; Enable format on save (optional)
  (add-hook 'eventb-mode-hook
            (lambda ()
              (add-hook 'before-save-hook #'lsp-format-buffer nil t))))
```

#### Step 4: Reload Configuration

Restart Emacs or evaluate the configuration:
```
M-x eval-buffer
```

Then install packages:
```
M-x package-install-selected-packages
```

---

### Method 2: Manual Installation

If you prefer not to use `use-package` or want more control:

#### Step 1: Install Dependencies

```
M-x package-refresh-contents
M-x package-install RET lsp-mode
M-x package-install RET lsp-ui
M-x package-install RET company
M-x package-install RET yasnippet
```

#### Step 2: Copy Event-B Mode Files

```bash
cd rossi/editors/emacs

# Copy to Emacs load path (create directory if it doesn't exist)
mkdir -p ~/.emacs.d/lisp
cp eventb-mode.el eventb-input.el ~/.emacs.d/lisp/
```

The `snippets/` directory can stay in place; you add its path to
`yas-snippet-dirs` in the next step.

#### Step 3: Configure Emacs

Add to `~/.emacs.d/init.el`:

```elisp
;; Add custom lisp directory to load path
(add-to-list 'load-path "~/.emacs.d/lisp")

;; Load and configure lsp-mode
(require 'lsp-mode)
(setq lsp-keymap-prefix "C-c l")
(add-hook 'lsp-mode-hook #'lsp-enable-which-key-integration)

;; Load lsp-ui (optional)
(require 'lsp-ui)
(add-hook 'lsp-mode-hook #'lsp-ui-mode)

;; Load company
(require 'company)
(add-hook 'prog-mode-hook #'company-mode)

;; Load yasnippet and register the Event-B snippets
(require 'yasnippet)
(add-to-list 'yas-snippet-dirs "/path/to/rossi/editors/emacs/snippets")
(yas-reload-all)

;; Load Event-B mode (it loads eventb-input and eventb-commands on demand)
(require 'eventb-mode)

;; Enable LSP and snippets for Event-B files
;; (the backslash-leader input method is auto-enabled by eventb-mode)
(add-hook 'eventb-mode-hook #'lsp-deferred)
(add-hook 'eventb-mode-hook #'yas-minor-mode)

;; Configure Event-B settings
(setq lsp-rossi-format-use-unicode t)
(setq lsp-rossi-format-indentation "    ")
(setq eventb-enable-input-method t)   ; set to nil to opt out of \-leader input
```

#### Step 4: Restart Emacs

```bash
emacs
```

---

### Method 3: Using Straight.el

If you use `straight.el` as your package manager:

```elisp
;; Install dependencies
(straight-use-package 'lsp-mode)
(straight-use-package 'lsp-ui)
(straight-use-package 'company)
(straight-use-package 'yasnippet)

;; Register the Event-B snippets
(add-to-list 'yas-snippet-dirs "/path/to/rossi/editors/emacs/snippets")
(yas-reload-all)

;; Install Event-B mode from local path
(use-package eventb-mode
  :straight (eventb-mode :type built-in
                         :local-repo "/path/to/rossi/editors/emacs")
  :mode "\\.eventb\\'"
  :hook ((eventb-mode . lsp-deferred)
         (eventb-mode . yas-minor-mode))
  :config
  ;; \-leader Unicode input is auto-enabled by eventb-mode
  (setq lsp-rossi-format-use-unicode t))
```

---

## Configuration

### Basic Configuration

Minimal configuration for Event-B support:

```elisp
;; Load Event-B mode
(require 'eventb-mode)
(add-hook 'eventb-mode-hook #'lsp-deferred)
```

### Recommended Configuration

Enhanced configuration with keybindings and settings:

```elisp
;; LSP Mode configuration
(use-package lsp-mode
  :ensure t
  :init
  (setq lsp-keymap-prefix "C-c l")
  :config
  (lsp-enable-which-key-integration t)
  (setq lsp-headerline-breadcrumb-enable t)
  (setq lsp-idle-delay 0.5)
  ;; Pinned Event-B defaults
  (setq lsp-semantic-tokens-enable t)   ; Server-driven semantic highlighting
  (setq lsp-extend-selection t))        ; Smart expand/shrink selection

;; LSP UI configuration
(use-package lsp-ui
  :ensure t
  :hook (lsp-mode . lsp-ui-mode)
  :config
  (setq lsp-ui-doc-enable t)
  (setq lsp-ui-doc-position 'bottom)
  (setq lsp-ui-doc-show-with-cursor t)
  (setq lsp-ui-sideline-enable t)
  (setq lsp-ui-sideline-show-diagnostics t)
  (setq lsp-ui-sideline-show-code-actions t))

;; Company configuration
(use-package company
  :ensure t
  :hook (prog-mode . company-mode)
  :config
  (setq company-minimum-prefix-length 1)
  (setq company-idle-delay 0.1)
  (setq company-selection-wrap-around t))

;; Snippets configuration
(use-package yasnippet
  :ensure t
  :config
  (add-to-list 'yas-snippet-dirs "/path/to/rossi/editors/emacs/snippets")
  (yas-reload-all))

;; Event-B Mode configuration
(use-package eventb-mode
  :load-path "/path/to/rossi/editors/emacs"
  :mode "\\.eventb\\'"
  :hook ((eventb-mode . lsp-deferred)
         (eventb-mode . yas-minor-mode))
  :bind (:map eventb-mode-map
         ;; Formatting
         ("C-c C-f"   . lsp-format-buffer)
         ;; Code actions
         ("C-c C-a"   . lsp-execute-code-action)
         ;; Rename
         ("C-c r"     . lsp-rename)
         ;; Documentation
         ("C-c h"     . lsp-describe-thing-at-point)
         ;; Notation conversion
         ("C-c C-u"   . eventb-convert-to-unicode)
         ("C-c C-d"   . eventb-convert-to-ascii)
         ;; Checker
         ("C-c C-v"   . eventb-validate))
  :config
  ;; Event-B specific settings
  (setq lsp-rossi-format-use-unicode t)
  (setq lsp-rossi-format-indentation "    ")
  (setq lsp-rossi-format-max-line-length 100)
  (setq lsp-rossi-diagnostics-enabled t)
  (setq lsp-rossi-diagnostics-debounce-ms 500)
  (setq lsp-rossi-completion-enabled t)

  ;; Backslash-leader Unicode input (\to, \and, \forall, …) is auto-enabled
  ;; by eventb-mode; set this to nil to opt out.
  (setq eventb-enable-input-method t)

  ;; Enable format on save
  (add-hook 'eventb-mode-hook
            (lambda ()
              (add-hook 'before-save-hook #'lsp-format-buffer nil t))))
```

### Custom Server Path

If `eventb-language-server` is not in your PATH:

```elisp
(setq eventb-language-server-command "~/.cargo/bin/eventb-language-server")

;; Or with debug logging:
(setq eventb-language-server-command
      '("sh" "-c" "RUST_LOG=debug exec ~/.cargo/bin/eventb-language-server"))
```

### Additional Keybindings

Global LSP keybindings:

```elisp
(define-key lsp-mode-map (kbd "C-c l") lsp-command-map)

;; Additional keybindings
(with-eval-after-load 'lsp-mode
  (define-key lsp-mode-map (kbd "M-.") 'lsp-find-definition)
  (define-key lsp-mode-map (kbd "M-?") 'lsp-find-references)
  (define-key lsp-mode-map (kbd "C-c l r r") 'lsp-rename)
  (define-key lsp-mode-map (kbd "C-c l a a") 'lsp-execute-code-action)
  (define-key lsp-mode-map (kbd "C-c l f f") 'lsp-format-buffer))
```

---

## Verification

### Check LSP Status

```
M-x lsp-describe-session
```

Should show:
```
Workspace: /path/to/your/project
Server: eventb-ls (server-id: eventb-ls)
Status: Running
```

### View LSP Logs

If something isn't working:

```
M-x lsp-workspace-show-log
```

Or enable detailed logging:

```elisp
(setq lsp-log-io t)
```

Then check the `*lsp-log*` buffer.

---

## Troubleshooting

### Server Not Found

**Problem**: `M-x lsp-describe-session` shows no server or "Failed to start server"

**Solution**:
```bash
# Check if server is in PATH
which eventb-language-server

# If not found, add to PATH or specify full path
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

Or in Emacs:
```elisp
(setq eventb-language-server-command "~/.cargo/bin/eventb-language-server")
```

### Server Crashes on Startup

**Problem**: LSP starts but immediately crashes

**Solution**:
```bash
# Test the server manually
eventb-language-server

# Check Rust installation
cargo --version

# Rebuild the server
cd rossi
cargo clean
cargo install --path crates/eventb-lsp --force
```

### No Syntax Highlighting

**Problem**: Event-B file has no syntax highlighting

**Solution**:
```
;; Check current major mode
M-x describe-mode

;; Should show "Event-B mode"
;; If not, manually enable it:
M-x eventb-mode

;; Check if eventb-mode is loaded:
M-x locate-library RET eventb-mode
```

### No Completions

**Problem**: Pressing `M-TAB` doesn't trigger completions

**Solution**:
1. Ensure company-mode is active:
   ```
   M-x company-mode
   ```

2. Check if LSP client is connected:
   ```
   M-x lsp-describe-session
   ```

3. Verify company backend includes LSP:
   ```elisp
   M-: (bound-and-true-p company-backends)
   ```
   Should include `company-capf`.

4. Try manual completion:
   ```
   M-x company-complete
   ```

### Snippets Not Expanding

**Problem**: Typing `mch` then `TAB` does not expand a template

**Solution**:
1. Ensure yasnippet is active in the buffer:
   ```
   M-x yas-minor-mode
   ```

2. Verify the snippet directory is registered and loaded:
   ```elisp
   M-: yas-snippet-dirs        ; should include .../editors/emacs/snippets
   M-x yas-reload-all
   ```

3. List the snippets available in the buffer:
   ```
   M-x yas-describe-tables
   ```

### Input Method Not Inserting Unicode

**Problem**: Typing `\to` does not produce →

**Solution**:
1. Make sure the package is loaded and the method is active:
   ```
   M-x eventb-activate-input-method
   ```
   The mode line should show the `EvB` indicator.

2. Confirm the method is registered:
   ```
   M-x list-input-methods
   ```
   Look for `eventb`. If missing, load it:
   ```elisp
   (require 'eventb-input)
   ```

3. Remember it is **leader-only**: `=>` and the other eager combos are not bound
   in Emacs by design. Use the backslash spelling (`\implies`) or
   `M-x eventb-convert-to-unicode` to convert the whole buffer.

### Unicode Characters Not Displaying

**Problem**: Operators show as □ or � or boxes

**Solution**:
1. Ensure UTF-8 encoding:
   ```elisp
   (prefer-coding-system 'utf-8)
   (set-default-coding-systems 'utf-8)
   (set-terminal-coding-system 'utf-8)
   (set-keyboard-coding-system 'utf-8)
   ```

2. Install a font with Unicode support:
   - JetBrains Mono
   - Fira Code
   - Cascadia Code
   - DejaVu Sans Mono

3. Configure Emacs to use the font:
   ```elisp
   (set-frame-font "JetBrains Mono-12" nil t)
   (add-to-list 'default-frame-alist '(font . "JetBrains Mono-12"))
   ```

### Permission Denied

**Problem**: Cannot execute `eventb-language-server`

**Solution**:
```bash
# Make the binary executable
chmod +x ~/.cargo/bin/eventb-language-server

# Verify
ls -l ~/.cargo/bin/eventb-language-server
```

### LSP Mode Not Found

**Problem**: `(require 'lsp-mode)` fails with "Cannot open load file"

**Solution**:
1. Install lsp-mode:
   ```
   M-x package-refresh-contents
   M-x package-install RET lsp-mode
   ```

2. Verify installation:
   ```
   M-x locate-library RET lsp-mode
   ```

3. Check package archives are configured:
   ```elisp
   (add-to-list 'package-archives '("melpa" . "https://melpa.org/packages/") t)
   (package-initialize)
   ```

### Event-B Mode Not Loading

**Problem**: `(require 'eventb-mode)` fails

**Solution**:
1. Verify the file is in your load path:
   ```
   M-x locate-library RET eventb-mode
   ```

2. Check load path includes the directory:
   ```elisp
   M-: load-path
   ```

3. Add to load path if missing:
   ```elisp
   (add-to-list 'load-path "/path/to/rossi/editors/emacs")
   ```

4. Verify the file exists:
   ```bash
   ls -l /path/to/rossi/editors/emacs/eventb-mode.el
   ```

### Performance Issues

**Problem**: Emacs is slow with LSP enabled

**Solution**:
1. Reduce LSP UI features:
   ```elisp
   (setq lsp-ui-doc-enable nil)           ; Disable hover documentation
   (setq lsp-ui-sideline-enable nil)      ; Disable sideline info
   (setq lsp-enable-symbol-highlighting nil) ; Disable symbol highlighting
   ```

2. Increase idle delay:
   ```elisp
   (setq lsp-idle-delay 0.5)              ; Wait 0.5s before updating
   (setq company-idle-delay 0.2)          ; Wait 0.2s before showing completions
   ```

3. Disable some features:
   ```elisp
   (setq lsp-enable-file-watchers nil)    ; Disable file watching
   (setq lsp-enable-folding nil)          ; Disable folding
   ```

---

## Next Steps

After successful installation:

1. **Customize**: Adjust keybindings and settings to your preference

## Resources

- **Main Repository**: https://github.com/eventb-rossi/rossi
- **Emacs LSP Documentation**: https://emacs-lsp.github.io/lsp-mode/
- **lsp-mode Wiki**: https://emacs-lsp.github.io/lsp-mode/page/installation/
- **Company Mode**: https://company-mode.github.io/

## Getting Help

- **Issues**: https://github.com/eventb-rossi/rossi/issues
- **Discussions**: https://github.com/eventb-rossi/rossi/discussions
- **Emacs Help**: `M-x describe-mode`, `M-x lsp-describe-session`
