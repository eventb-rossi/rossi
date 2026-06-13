;;; eventb-mode.el --- Major mode for Event-B formal modeling -*- lexical-binding: t; -*-

;; Copyright (C) 2025 Rossi Contributors

;; Author: Rossi Contributors
;; URL: https://github.com/eventb-rossi/rossi
;; Version: 0.1.0
;; Package-Requires: ((emacs "26.1"))
;; Keywords: languages, event-b, formal-methods

;; This file is not part of GNU Emacs.

;; This program is dual-licensed under MIT or Apache-2.0.

;;; Commentary:

;; Event-B major mode provides comprehensive language support for Event-B
;; formal modeling through the Rossi Language Server.
;;
;; Features:
;; - Syntax highlighting for Event-B constructs
;; - LSP integration via lsp-mode
;; - Real-time diagnostics
;; - Code completion
;; - Go-to-definition and find-references
;; - Rename symbol across workspace
;; - Hover documentation
;; - Code formatting (Unicode/ASCII operators)
;; - Document symbols and navigation
;; - Workspace symbols search
;; - Document links (SEES, REFINES, EXTENDS)
;; - Signature help for quantifiers and lambda
;; - Code actions (quick fixes and refactorings)
;; - Folding ranges
;; - ProB integration
;;
;; Installation:
;;
;; 1. Install the Rossi Language Server:
;;    cargo install --path crates/rossi-lsp
;;
;; 2. Add to your Emacs configuration:
;;    (add-to-list 'load-path "/path/to/rossi/editors/emacs")
;;    (require 'eventb-mode)
;;
;; 3. Or use use-package:
;;    (use-package eventb-mode
;;      :load-path "/path/to/rossi/editors/emacs"
;;      :mode "\\.eventb\\'"
;;      :hook (eventb-mode . lsp-deferred))
;;
;; Configuration:
;;
;; Customize the language server settings via lsp-mode:
;;   (setq lsp-rossi-format-use-unicode t)
;;   (setq lsp-rossi-format-indentation "    ")
;;   (setq lsp-rossi-diagnostics-enabled t)

;;; Code:

;; lsp-mode integration is loaded when lsp-mode is available

;;; Customization

(defgroup eventb nil
  "Support for Event-B formal modeling."
  :group 'languages
  :prefix "eventb-")

(defcustom rossi-language-server-command "rossi-language-server"
  "Command to start the Event-B language server.
Can be a string (command name) or a list (command with arguments)."
  :type '(choice (string :tag "Command name")
                 (repeat :tag "Command with arguments" string))
  :group 'eventb)

(defcustom eventb-enable-input-method t
  "When non-nil, activate the Event-B Unicode input method in new buffers.
The input method is the Quail package named \"eventb\" (see
`eventb-input.el'); it lets you type Event-B operators with a backslash
leader, e.g. \\\\to inserts a RIGHTWARDS ARROW.  Toggle it at any time
with `eventb-toggle-input-method'."
  :type 'boolean
  :group 'eventb)

;;; Syntax highlighting

;; >>> rossi gen-grammars (generated, do not edit)
(defconst eventb-keywords-regexp
  "\\<\\(?:[Ii][Nn][Ii][Tt][Ii][Aa][Ll][Ii][Ss][Aa][Tt][Ii][Oo][Nn]\\|[Ii][Nn][Vv][Aa][Rr][Ii][Aa][Nn][Tt][Ss]\\|[Cc][Oo][Nn][Ss][Tt][Aa][Nn][Tt][Ss]\\|[Vv][Aa][Rr][Ii][Aa][Bb][Ll][Ee][Ss]\\|[Tt][Hh][Ee][Oo][Rr][Ee][Mm][Ss]\\|[Cc][Oo][Nn][Tt][Ee][Xx][Tt]\\|[Ee][Xx][Tt][Ee][Nn][Dd][Ss]\\|[Mm][Aa][Cc][Hh][Ii][Nn][Ee]\\|[Rr][Ee][Ff][Ii][Nn][Ee][Ss]\\|[Vv][Aa][Rr][Ii][Aa][Nn][Tt]\\|[Ww][Ii][Tt][Nn][Ee][Ss][Ss]\\|[Aa][Xx][Ii][Oo][Mm][Ss]\\|[Ee][Vv][Ee][Nn][Tt][Ss]\\|[Ss][Tt][Aa][Tt][Uu][Ss]\\|[Bb][Ee][Gg][Ii][Nn]\\|[Ee][Vv][Ee][Nn][Tt]\\|[Ww][Hh][Ee][Rr][Ee]\\|[Ss][Ee][Ee][Ss]\\|[Ss][Ee][Tt][Ss]\\|[Tt][Hh][Ee][Nn]\\|[Ww][Hh][Ee][Nn]\\|[Ww][Ii][Tt][Hh]\\|[Aa][Nn][Yy]\\|[Ee][Nn][Dd]\\)\\>"
  "Event-B section and event keywords (any case).")

(defconst eventb-status-keywords-regexp
  "\\<\\(?:[Aa][Nn][Tt][Ii][Cc][Ii][Pp][Aa][Tt][Ee][Dd]\\|[Cc][Oo][Nn][Vv][Ee][Rr][Gg][Ee][Nn][Tt]\\|[Oo][Rr][Dd][Ii][Nn][Aa][Rr][Yy]\\|[Tt][Hh][Ee][Oo][Rr][Ee][Mm]\\|[Ss][Kk][Ii][Pp]\\)\\>"
  "Event-B status and inline modifiers (any case).")

(defconst eventb-constants-regexp
  "\\<\\(?:[Ff][Aa][Ll][Ss][Ee]\\|[Bb][Oo][Oo][Ll]\\|[Nn][Aa][Tt]1\\|[Tt][Rr][Uu][Ee]\\|[Ii][Nn][Tt]\\|[Nn][Aa][Tt]\\)\\>"
  "Event-B literal constants and number sets (any case).")

(defconst eventb-builtins-regexp
  "\\<\\(?:partition\\|finite\\|card\\|pred\\|prj1\\|prj2\\|succ\\|max\\|min\\|id\\)\\>"
  "Event-B built-in functions and predicates (exact case).")

(defconst eventb-quantifier-words-regexp
  "\\<\\(?:[Ii][Nn][Tt][Ee][Rr]\\|[Uu][Nn][Ii][Oo][Nn]\\)\\>"
  "Event-B quantifier words UNION/INTER (any case).")

(defconst eventb-operator-words-regexp
  "\\<\\(?:oftype\\|POW1\\|circ\\|POW\\|dom\\|mod\\|not\\|ran\\|or\\)\\>"
  "Event-B alphabetic operators (exact case).")

(defconst eventb-constant-symbols
  '("ℕ1" "ℕ" "ℤ" "∅" "{}")
  "Event-B symbolic constants.")

(defconst eventb-operator-symbols
  '("<<->>" "/<<:" ":∈" ":∣" "<->>" "<<->" ">->>" "ℙ1" "+->" "+>>" "-->" "->>" "/<:" "<->" "<<:" "<<|" "<=>" ">+>" ">->" "|->" "|>>" "‥" "ℙ" "→" "↔" "↠" "↣" "↦" "⇒" "⇔" "⇸" "∀" "∃" "∈" "∉" "−" "∖" "∗" "∘" "∣" "∥" "∧" "∨" "∩" "∪" "∼" "≔" "≠" "≤" "≥" "⊂" "⊄" "⊆" "⊈" "⊗" "⋂" "⋃" "▷" "◁" "⤀" "⤔" "⤖" "⦂" "⩤" "⩥" "" "" "" "" "**" ".." "/:" "/=" "/\\" "::" ":=" ":|" "<+" "<:" "<=" "<|" "=>" "><" ">=" "\\/" "|>" "||" "¬" "·" "×" "÷" "λ" "!" "#" "%" "&" "*" "+" "-" "." "/" ":" ";" "<" "=" ">" "\\" "^" "|" "~")
  "Event-B symbolic operators.")

(defvar eventb-font-lock-keywords
  `((,eventb-keywords-regexp . font-lock-keyword-face)
    (,eventb-status-keywords-regexp . font-lock-keyword-face)
    ("\\<[Ee][Vv][Ee][Nn][Tt]\\s-+\\([A-Za-z_][A-Za-z0-9_']*\\(?:-[A-Za-z0-9_']+\\)*\\)" 1 font-lock-function-name-face)
    ("\\<\\(?:[Cc][Oo][Nn][Tt][Ee][Xx][Tt]\\|[Mm][Aa][Cc][Hh][Ii][Nn][Ee]\\)\\s-+\\([A-Za-z_][A-Za-z0-9_']*\\(?:-[A-Za-z0-9_']+\\)*\\)" 1 font-lock-type-face)
    (,eventb-constants-regexp . font-lock-constant-face)
    (,(regexp-opt eventb-constant-symbols) . font-lock-constant-face)
    (,eventb-builtins-regexp . font-lock-function-name-face)
    (,eventb-quantifier-words-regexp . font-lock-builtin-face)
    (,eventb-operator-words-regexp . font-lock-builtin-face)
    (,(regexp-opt eventb-operator-symbols) . font-lock-builtin-face)
    ("\\<[0-9]+\\>" . font-lock-constant-face)
    ("@[A-Za-z0-9_]+" . font-lock-preprocessor-face))
  "Font lock keywords for Event-B mode (comments and strings come from the syntax table).
Word patterns carry their own case folding; `font-lock-keywords-case-fold-search'
must stay nil so the exact-case math words (dom, card, POW, …) do not fold.")
;; <<< rossi gen-grammars

;;; Syntax table

(defvar eventb-mode-syntax-table
  (let ((table (make-syntax-table)))
    ;; C-style comments
    (modify-syntax-entry ?/ ". 124b" table)
    (modify-syntax-entry ?* ". 23" table)
    (modify-syntax-entry ?\n "> b" table)

    ;; Parentheses and brackets
    (modify-syntax-entry ?\( "()" table)
    (modify-syntax-entry ?\) ")(" table)
    (modify-syntax-entry ?\[ "(]" table)
    (modify-syntax-entry ?\] ")[" table)
    (modify-syntax-entry ?\{ "(}" table)
    (modify-syntax-entry ?\} "){" table)

    ;; Operators
    (modify-syntax-entry ?: "." table)
    (modify-syntax-entry ?= "." table)
    (modify-syntax-entry ?< "." table)
    (modify-syntax-entry ?> "." table)
    (modify-syntax-entry ?+ "." table)
    (modify-syntax-entry ?- "." table)
    (modify-syntax-entry ?| "." table)
    (modify-syntax-entry ?& "." table)
    (modify-syntax-entry ?! "." table)
    (modify-syntax-entry ?? "." table)
    (modify-syntax-entry ?~ "." table)

    ;; Strings (not typically used in Event-B, but for completeness)
    (modify-syntax-entry ?\" "\"" table)
    (modify-syntax-entry ?\' "\"" table)

    table)
  "Syntax table for Event-B mode.")

;;; Indentation

(defun eventb-indent-line ()
  "Indent current line as Event-B code."
  (interactive)
  (let ((indent-level 0)
        (current-indent (current-indentation)))
    (save-excursion
      (beginning-of-line)
      (cond
       ;; Top-level keywords (no indentation)
       ((looking-at "^\\s-*\\(CONTEXT\\|MACHINE\\|END\\)\\>")
        (setq indent-level 0))

       ;; Main clauses (one level)
       ((looking-at "^\\s-*\\(EXTENDS\\|SEES\\|REFINES\\|SETS\\|CONSTANTS\\|AXIOMS\\|THEOREMS\\|VARIABLES\\|INVARIANTS\\|VARIANT\\|EVENTS\\|INITIALISATION\\)\\>")
        (setq indent-level 1))

       ;; EVENT keyword (one level)
       ((looking-at "^\\s-*EVENT\\>")
        (setq indent-level 1))

       ;; Event subclauses (two levels)
       ((looking-at "^\\s-*\\(ANY\\|WHERE\\|WHEN\\|WITH\\|WITNESS\\|THEN\\|BEGIN\\|ordinary\\|convergent\\|anticipated\\)\\>")
        (setq indent-level 2))

       ;; Default: maintain previous indentation or indent one level
       (t
        (save-excursion
          (if (bobp)
              (setq indent-level 0)
            (forward-line -1)
            (setq indent-level (/ (current-indentation) tab-width)))))))

    ;; Apply indentation
    (indent-line-to (* indent-level tab-width))))

;;; LSP integration

(defcustom lsp-rossi-format-use-unicode t
  "Use Unicode operators (∧, ∨, ⇒, ∈) instead of ASCII (/\\, \\/, =>, :)."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-format-indentation "    "
  "Indentation string (spaces or tabs) for Event-B formatting."
  :type 'string
  :group 'eventb)

(defcustom lsp-rossi-format-max-line-length 100
  "Reserved for future formatter wrapping; currently parsed but not applied."
  :type 'integer
  :group 'eventb)

(defcustom lsp-rossi-diagnostics-enabled t
  "Enable or disable Event-B diagnostics."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-diagnostics-debounce-ms 500
  "Reserved for future diagnostic debouncing; diagnostics currently run immediately."
  :type 'integer
  :group 'eventb)

(defcustom lsp-rossi-completion-enabled t
  "Enable or disable Event-B code completion."
  :type 'boolean
  :group 'eventb)

(with-eval-after-load 'lsp-mode
  (lsp-register-client
   (make-lsp-client
    :new-connection (lsp-stdio-connection
                     (lambda ()
                       (if (listp rossi-language-server-command)
                           rossi-language-server-command
                         (list rossi-language-server-command))))
    :major-modes '(eventb-mode)
    :server-id 'eventb-ls
    :priority 0
    :initialization-options
    (lambda ()
      `(:rossi (:format (:useUnicode ,lsp-rossi-format-use-unicode
                          :indentation ,lsp-rossi-format-indentation
                          :maxLineLength ,lsp-rossi-format-max-line-length)
                 :diagnostics (:enabled ,lsp-rossi-diagnostics-enabled
                               :debounceMs ,lsp-rossi-diagnostics-debounce-ms)
                 :completion (:enabled ,lsp-rossi-completion-enabled
                              :triggerCharacters [":" "." "(" "{"])
                 :trace (:server "off")))))))

;;; Unicode input method

;; The Quail input method itself is generated by `rossi gen-grammars' into
;; eventb-input.el; loading it defines the "eventb" input method.  It is
;; required lazily (in `eventb-activate-input-method') rather than at
;; top-level so that this file byte-compiles and loads even when the
;; generated package is not yet on `load-path'.

;;;###autoload
(defun eventb-activate-input-method ()
  "Activate the Event-B Unicode input method in the current buffer.
See `eventb-input.el' for the backslash-leader spellings."
  (interactive)
  (require 'eventb-input)
  (set-input-method "eventb"))

;;;###autoload
(defun eventb-toggle-input-method ()
  "Toggle the Event-B Unicode input method in the current buffer."
  (interactive)
  (if (equal current-input-method "eventb")
      (deactivate-input-method)
    (eventb-activate-input-method)))

;;; Editor commands (Rodin/ProB/validation helpers)

;; Interactive commands live in eventb-commands.el.  Require it softly so
;; this file still byte-compiles and loads when that companion is absent.
(require 'eventb-commands nil t)

;; Silence the byte-compiler for commands provided by eventb-commands.el.
(declare-function eventb-convert-to-unicode "eventb-commands")
(declare-function eventb-convert-to-ascii "eventb-commands")
(declare-function eventb-validate "eventb-commands")
(declare-function eventb-validate-workspace "eventb-commands")
(declare-function eventb-animate-prob "eventb-commands")
(declare-function eventb-model-check-prob "eventb-commands")
(declare-function eventb-import "eventb-commands")
(declare-function eventb-export "eventb-commands")
(declare-function eventb-build "eventb-commands")

;; lsp-mode entry points used by the keymap below.
(declare-function lsp-extend-selection "lsp-mode")

;;; Keymap

(defvar eventb-mode-map
  (let ((map (make-sparse-keymap)))
    ;; Selection range (smart expand/shrink).
    (define-key map (kbd "C-c C-SPC") #'lsp-extend-selection)
    ;; Unicode input method.
    (define-key map (kbd "C-c C-i") #'eventb-toggle-input-method)
    ;; Editor commands (eventb-commands.el).  Bound by symbol so they
    ;; resolve at runtime even if that companion library is missing.
    (define-key map (kbd "C-c C-v") #'eventb-validate)
    (define-key map (kbd "C-c C-S-v") #'eventb-validate-workspace)
    (define-key map (kbd "C-c C-u") #'eventb-convert-to-unicode)
    (define-key map (kbd "C-c C-a") #'eventb-convert-to-ascii)
    (define-key map (kbd "C-c C-p a") #'eventb-animate-prob)
    (define-key map (kbd "C-c C-p m") #'eventb-model-check-prob)
    (define-key map (kbd "C-c C-r i") #'eventb-import)
    (define-key map (kbd "C-c C-r e") #'eventb-export)
    (define-key map (kbd "C-c C-r b") #'eventb-build)
    map)
  "Keymap for `eventb-mode'.")

;;; Mode definition

;;;###autoload
(define-derived-mode eventb-mode prog-mode "Event-B"
  "Major mode for editing Event-B formal specifications.

Event-B is a formal method for system-level modeling and analysis,
used in safety-critical systems and formal verification.

\\{eventb-mode-map}"
  :syntax-table eventb-mode-syntax-table

  ;; Font lock
  (setq font-lock-defaults '(eventb-font-lock-keywords))
  ;; The generated word patterns carry their own case folding (structural
  ;; keywords fold; the math words dom/card/POW/… are exact-case tokens, and
  ;; DOM/Card/pow are ordinary identifiers), so matching must not fold here.
  (setq-local font-lock-keywords-case-fold-search nil)

  ;; Comments
  (setq-local comment-start "// ")
  (setq-local comment-end "")
  (setq-local comment-start-skip "//+\\s-*")

  ;; Indentation
  (setq-local indent-line-function 'eventb-indent-line)
  (setq-local tab-width 4)
  (setq-local indent-tabs-mode nil)

  ;; Pin opinionated LSP defaults for Event-B buffers.  Semantic tokens give
  ;; richer, server-driven highlighting and code lenses surface the
  ;; selection-range / refinement affordances; both are off by default in
  ;; lsp-mode, so enable them buffer-locally here.
  (setq-local lsp-semantic-tokens-enable t)
  (setq-local lsp-lens-enable t)

  ;; Activate the Unicode input method by default (respecting the toggle).
  ;; Downgrade any failure (e.g. a not-yet-loadable input package) to a
  ;; warning so that buffer setup, font-lock and LSP still come up.
  (when eventb-enable-input-method
    (condition-case err
        (eventb-activate-input-method)
      (error
       (display-warning
        'eventb
        (format "Could not activate the Event-B input method: %s"
                (error-message-string err))
        :warning)))))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.eventb\\'" . eventb-mode))

(provide 'eventb-mode)

;;; eventb-mode.el ends here
