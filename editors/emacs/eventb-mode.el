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

;;; Syntax highlighting

(defconst eventb-keywords
  '("CONTEXT" "MACHINE" "END"
    "EXTENDS" "SEES" "REFINES"
    "SETS" "CONSTANTS" "AXIOMS" "THEOREMS"
    "VARIABLES" "INVARIANTS" "VARIANT"
    "EVENTS" "INITIALISATION" "EVENT"
    "REFINES" "ANY" "WHERE" "WHEN" "WITH" "WITNESS" "THEN" "BEGIN"
    "ordinary" "convergent" "anticipated")
  "Event-B keywords.")

(defconst eventb-logical-operators-unicode
  '("¬" "∧" "∨" "⇒" "⇔" "∀" "∃")
  "Event-B logical operators (Unicode).")

(defconst eventb-logical-operators-ascii
  '("not" "/\\\\" "\\\\/" "=>" "<=>" "!" "#")
  "Event-B logical operators (ASCII).")

(defconst eventb-set-operators-unicode
  '("∈" "∉" "⊂" "⊆" "⊄" "⊈" "∩" "∪" "∖" "×" "℘" "ℙ" "ℤ" "ℕ" "ℕ1" "∅")
  "Event-B set operators (Unicode).")

(defconst eventb-set-operators-ascii
  '(":" "/:" "<:" "<<:" "/<:" "/<<:" "/\\\\" "\\\\/" "\\\\" "**" "POW" "INT" "NAT" "NAT1" "{}")
  "Event-B set operators (ASCII).")

(defconst eventb-arithmetic-operators
  '("+" "-" "*" "/" "^" "mod" ".." "min" "max" "card")
  "Event-B arithmetic operators.")

(defconst eventb-relation-operators-unicode
  '("↔" "→" "⇸" "↠" "⤔" "↣" "↪" "⤀" "⊗" "∥" "◁" "⩤" "▷" "⩥" "⊕" "∘" "∼" "⊤" "⊥")
  "Event-B relation operators (Unicode).")

(defconst eventb-relation-operators-ascii
  '("<->" "->" "+->" "->>" "+->>" ">+>" ">->" ">>->" "><" "||" "<|" "<<|" "|>" "|>>" "<+" ";" "~" "prj1" "prj2" "id")
  "Event-B relation operators (ASCII).")

(defconst eventb-action-operators
  '(":=" ":|" ":∈" ":<-" ":(" ":)")
  "Event-B action operators.")

(defconst eventb-constants
  '("TRUE" "FALSE" "BOOL")
  "Event-B built-in constants.")

(defvar eventb-font-lock-keywords
  `(
    ;; Keywords
    (,(regexp-opt eventb-keywords 'words) . font-lock-keyword-face)

    ;; Built-in constants
    (,(regexp-opt eventb-constants 'words) . font-lock-constant-face)

    ;; Labels (e.g., "axm1:", "inv2:", "grd3:", "act1:")
    ("\\<\\([a-zA-Z_][a-zA-Z0-9_]*\\):" 1 font-lock-variable-name-face)

    ;; Event names (after EVENT keyword)
    ("\\<EVENT\\s-+\\([a-zA-Z_][a-zA-Z0-9_]*\\)" 1 font-lock-function-name-face)

    ;; Context and Machine names
    ("\\<\\(CONTEXT\\|MACHINE\\)\\s-+\\([a-zA-Z_][a-zA-Z0-9_]*\\)" 2 font-lock-type-face)

    ;; Logical operators (Unicode)
    (,(regexp-opt eventb-logical-operators-unicode) . font-lock-builtin-face)

    ;; Logical operators (ASCII)
    (,(regexp-opt eventb-logical-operators-ascii t) . font-lock-builtin-face)

    ;; Set operators (Unicode)
    (,(regexp-opt eventb-set-operators-unicode) . font-lock-builtin-face)

    ;; Set operators (ASCII)
    (,(regexp-opt eventb-set-operators-ascii t) . font-lock-builtin-face)

    ;; Arithmetic operators
    (,(regexp-opt eventb-arithmetic-operators t) . font-lock-builtin-face)

    ;; Relation operators (Unicode)
    (,(regexp-opt eventb-relation-operators-unicode) . font-lock-builtin-face)

    ;; Relation operators (ASCII)
    (,(regexp-opt eventb-relation-operators-ascii t) . font-lock-builtin-face)

    ;; Action operators
    (,(regexp-opt eventb-action-operators) . font-lock-builtin-face)

    ;; Numbers
    ("\\<[0-9]+\\>" . font-lock-constant-face)

    ;; Comments (C-style and line comments)
    ("//.*$" . font-lock-comment-face)
    ("/\\*.*?\\*/" . font-lock-comment-face)
    )
  "Font lock keywords for Event-B mode.")

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
  "Maximum line length for Event-B formatting."
  :type 'integer
  :group 'eventb)

(defcustom lsp-rossi-diagnostics-enabled t
  "Enable or disable Event-B diagnostics."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-diagnostics-debounce-ms 500
  "Debounce delay in milliseconds for Event-B diagnostics."
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

  ;; Comments
  (setq-local comment-start "// ")
  (setq-local comment-end "")
  (setq-local comment-start-skip "//+\\s-*")

  ;; Indentation
  (setq-local indent-line-function 'eventb-indent-line)
  (setq-local tab-width 4)
  (setq-local indent-tabs-mode nil)

  )

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.eventb\\'" . eventb-mode))

(provide 'eventb-mode)

;;; eventb-mode.el ends here
