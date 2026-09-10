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
;;
;; Installation:
;;
;; 1. Install the Rossi Language Server:
;;    cargo install --path crates/eventb-lsp
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
;;   (setq lsp-rossi-format-style "camille")
;;   (setq lsp-rossi-format-use-unicode t)
;;   (setq lsp-rossi-format-indentation "  ")
;;   (setq lsp-rossi-diagnostics-enabled t)

;;; Code:

;; lsp-mode integration is loaded when lsp-mode is available

;;; Customization

(defgroup eventb nil
  "Support for Event-B formal modeling."
  :group 'languages
  :prefix "eventb-")

(defcustom eventb-language-server-command "eventb-language-server"
  "Command to start the Event-B language server.
Can be a string (command name) or a list (command with arguments)."
  :type '(choice (string :tag "Command name")
                 (repeat :tag "Command with arguments" string))
  :group 'eventb)

(defcustom eventb-math-font "Brave Sans Mono"
  "Font family used for the Event-B private-use operator glyphs.
Rodin encodes four operators in the Unicode Private Use Area
\(U+E100..U+E103), so only a font carrying those glyphs can display
them.  `eventb-apply-math-font' maps that range --- and nothing else ---
to this family, leaving the rest of the buffer in your own font.

Set to nil to leave the fontset alone."
  :type '(choice (string :tag "Font family")
                 (const :tag "Do not remap the glyph range" nil))
  :group 'eventb)

(defcustom eventb-enable-input-method t
  "When non-nil, activate the Event-B Unicode input method in new buffers.
The input method is the Quail package named \"eventb\" (see
`eventb-input.el'); it lets you type Event-B operators with a backslash
leader, e.g. \\\\to inserts a RIGHTWARDS ARROW.  Toggle it at any time
with `eventb-toggle-input-method'."
  :type 'boolean
  :group 'eventb)

;;; Private-use operator glyphs

(defconst eventb-private-use-glyph-range '(?\uE100 . ?\uE103)
  "Characters Rodin encodes in the Unicode Private Use Area.
U+E100 TREL \\=`<<->', U+E101 SREL \\=`<->>', U+E102 STREL \\=`<<->>' and
U+E103 OVR \\=`<+' --- the complete set, matching rossi's own operator
table.  Only a font carrying these glyphs can display them.")

(defvar eventb--math-font-resolved nil
  "Family `eventb-apply-math-font' last resolved against a real display.
Opening one Event-B buffer after another must not repeat a font lookup
whose answer cannot have changed --- and the miss is the common case, for
everyone who never installs the font.  Recorded only once there *is* a
display, so an Emacs daemon started without a frame still resolves the
font when it gets one.")

;;;###autoload
(defun eventb-apply-math-font (&optional force)
  "Display the Event-B private-use operator glyphs in `eventb-math-font'.

Maps `eventb-private-use-glyph-range' --- and only that range --- onto the
font, so the rest of the buffer keeps whatever family you have chosen.
This is finer-grained than editors that can only replace the whole editor
font.

Does nothing when `eventb-math-font' is nil, when the font is not
installed, or on a terminal, where the terminal emulator picks the font
rather than Emacs.  The answer is remembered per family, so calling this
from a mode hook costs one lookup rather than one per buffer; FORCE
(always, when called interactively) resolves the font again, which is
what to call after installing it.

Returns non-nil when it applied the mapping."
  (interactive (list t))
  (when (and eventb-math-font
             (display-multi-font-p)
             (or force (not (equal eventb--math-font-resolved eventb-math-font))))
    (setq eventb--math-font-resolved eventb-math-font)
    (let ((spec (font-spec :family eventb-math-font)))
      (when (find-font spec)
        (set-fontset-font t eventb-private-use-glyph-range spec)
        t))))

;;; Syntax highlighting

;; >>> cargo xtask gen-grammars (generated, do not edit)
(defconst eventb-keywords-regexp
  "\\<\\(?:[Ii][Nn][Ii][Tt][Ii][Aa][Ll][Ii][Ss][Aa][Tt][Ii][Oo][Nn]\\|[Ii][Nn][Vv][Aa][Rr][Ii][Aa][Nn][Tt][Ss]\\|[Cc][Oo][Nn][Ss][Tt][Aa][Nn][Tt][Ss]\\|[Vv][Aa][Rr][Ii][Aa][Bb][Ll][Ee][Ss]\\|[Tt][Hh][Ee][Oo][Rr][Ee][Mm][Ss]\\|[Cc][Oo][Nn][Tt][Ee][Xx][Tt]\\|[Ee][Xx][Tt][Ee][Nn][Dd][Ss]\\|[Mm][Aa][Cc][Hh][Ii][Nn][Ee]\\|[Rr][Ee][Ff][Ii][Nn][Ee][Ss]\\|[Vv][Aa][Rr][Ii][Aa][Nn][Tt]\\|[Ww][Ii][Tt][Nn][Ee][Ss][Ss]\\|[Aa][Xx][Ii][Oo][Mm][Ss]\\|[Ee][Vv][Ee][Nn][Tt][Ss]\\|[Ss][Tt][Aa][Tt][Uu][Ss]\\|[Bb][Ee][Gg][Ii][Nn]\\|[Ee][Vv][Ee][Nn][Tt]\\|[Ww][Hh][Ee][Rr][Ee]\\|[Ss][Ee][Ee][Ss]\\|[Ss][Ee][Tt][Ss]\\|[Tt][Hh][Ee][Nn]\\|[Ww][Hh][Ee][Nn]\\|[Ww][Ii][Tt][Hh]\\|[Aa][Nn][Yy]\\|[Ee][Nn][Dd]\\)\\>"
  "Event-B section and event keywords (any case).")

(defconst eventb-status-keywords-regexp
  "\\<\\(?:[Aa][Nn][Tt][Ii][Cc][Ii][Pp][Aa][Tt][Ee][Dd]\\|[Cc][Oo][Nn][Vv][Ee][Rr][Gg][Ee][Nn][Tt]\\|[Oo][Rr][Dd][Ii][Nn][Aa][Rr][Yy]\\|[Tt][Hh][Ee][Oo][Rr][Ee][Mm]\\|[Ss][Kk][Ii][Pp]\\)\\>"
  "Event-B status and inline modifiers (any case).")

(defconst eventb-constants-regexp
  "\\<\\(?:FALSE\\|false\\|BOOL\\|NAT1\\|TRUE\\|bool\\|true\\|INT\\|NAT\\)\\>"
  "Event-B literal constants and number sets (exact case).")

(defconst eventb-builtins-regexp
  "\\<\\(?:partition\\|finite\\|card\\|pred\\|prj1\\|prj2\\|succ\\|max\\|min\\|id\\)\\>"
  "Event-B built-in functions and predicates (exact case).")

(defconst eventb-operator-words-regexp
  "\\<\\(?:oftype\\|INTER\\|UNION\\|POW1\\|circ\\|POW\\|dom\\|mod\\|not\\|ran\\|or\\)\\>"
  "Event-B alphabetic operators (exact case).")

(defconst eventb-constant-symbols
  '("ℕ1" "ℕ" "ℤ" "∅" "⊤" "⊥" "{}")
  "Event-B symbolic constants.")

(defconst eventb-operator-symbols
  '("<<->>" "/<<:" ":∈" ":∣" "<->>" "<<->" ">->>" "ℙ1" "+->" "+>>" "-->" "->>" "/<:" "<->" "<<:" "<<|" "<=>" ">+>" ">->" "|->" "|>>" "‥" "ℙ" "→" "↔" "↠" "↣" "↦" "⇒" "⇔" "⇸" "∀" "∃" "∈" "∉" "−" "∖" "∗" "∘" "∣" "∥" "∧" "∨" "∩" "∪" "∼" "≔" "≠" "≤" "≥" "⊂" "⊄" "⊆" "⊈" "⊗" "⋂" "⋃" "▷" "◁" "⤀" "⤔" "⤖" "⦂" "⩤" "⩥" "" "" "" "" "**" ".." "/:" "/=" "/\\" "::" ":=" ":|" "<+" "<:" "<=" "<|" "=>" "><" ">=" "\\/" "|>" "||" "¬" "·" "×" "÷" "λ" "!" "#" "%" "&" "*" "+" "-" "." "/" ":" ";" "<" "=" ">" "\\" "^" "|" "~")
  "Event-B symbolic operators.")

(defconst eventb-label-regexp
  "@[^\u0009\u000A\u000B\u000C\u000D\u001C\u001D\u001E\u001F\u0020\u00A0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200A\u2028\u2029\u202F\u205F\u3000]+"
  "Camille label, ending at grammar whitespace.")

(defvar eventb-font-lock-keywords
  `((,eventb-label-regexp . font-lock-preprocessor-face)
    (,eventb-keywords-regexp . font-lock-keyword-face)
    (,eventb-status-keywords-regexp . font-lock-keyword-face)
    ("\\<[Ee][Vv][Ee][Nn][Tt]\\s-+\\([A-Za-z_][A-Za-z0-9_]*\\(?:-[A-Za-z0-9_]+\\)*\\)" 1 font-lock-function-name-face)
    ("\\<\\(?:[Cc][Oo][Nn][Tt][Ee][Xx][Tt]\\|[Mm][Aa][Cc][Hh][Ii][Nn][Ee]\\)\\s-+\\([A-Za-z_][A-Za-z0-9_]*\\(?:-[A-Za-z0-9_]+\\)*\\)" 1 font-lock-type-face)
    (,eventb-constants-regexp . font-lock-constant-face)
    (,(regexp-opt eventb-constant-symbols) . font-lock-constant-face)
    (,eventb-builtins-regexp . font-lock-function-name-face)
    (,eventb-operator-words-regexp . font-lock-builtin-face)
    (,(regexp-opt eventb-operator-symbols) . font-lock-builtin-face)
    ("\\<[0-9]+\\>" . font-lock-constant-face))
  "Font lock keywords for Event-B mode (comments and strings come from the syntax table).
Word patterns carry their own case folding; `font-lock-keywords-case-fold-search'
must stay nil so the exact-case math words (dom, card, POW, …) do not fold.")
;; <<< cargo xtask gen-grammars

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

;; Quotes and comment markers within a label are ordinary label characters.
;; Extend incremental rescans to whole lines so editing the sigil also clears
;; the old label's syntax properties.
(defun eventb--syntax-propertize-labels (start end)
  "Mark label characters between START and END as punctuation syntax."
  (goto-char start)
  (while (re-search-forward eventb-label-regexp end t)
    (let ((from (match-beginning 0))
          (to (match-end 0)))
      (unless (nth 8 (save-excursion (syntax-ppss from)))
        (put-text-property from to 'syntax-table (string-to-syntax "."))))))

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

(defcustom lsp-rossi-format-style ""
  "Formatting style preset: \"camille\" or \"rossi\".
Empty follows the language server's default preset."
  :type 'string
  :group 'eventb)

(defcustom lsp-rossi-format-use-unicode t
  "Use Unicode operators (∧, ∨, ⇒, ∈) instead of ASCII (/\\, \\/, =>, :)."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-format-indentation ""
  "Indentation string (spaces or tabs) for Event-B formatting.
Empty follows the style preset (2 spaces camille, 4 spaces rossi)."
  :type 'string
  :group 'eventb)

(defcustom lsp-rossi-format-private-use-glyphs nil
  "Spell four operators with Rodin's private-use glyphs, not ASCII.
`<<->' (U+E100), `<->>' (U+E101), `<<->>' (U+E102) and `<+' (U+E103)
have no standard Unicode code point.  Rossi writes them in ASCII, which
renders in any font; enable this to exchange files with a tool that reads
only Rodin's spelling.  Has no effect with `lsp-rossi-format-use-unicode'
off.

The glyphs need a font that has them --- see `eventb-math-font'."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-format-max-line-width 120
  "Maximum line width when formatting Event-B text.
Long formulas wrap onto operator-leading continuation lines; 0
disables wrapping."
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

(defcustom lsp-rossi-inlay-hints-enabled t
  "Show inferred declaration types as inlay hints.
Machine variables, event parameters, and context constants get their
inferred type rendered after the name.  Rendering also requires
`lsp-inlay-hint-enable', which `eventb-mode' pins on."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-inlay-hints-well-definedness t
  "Mark formulas carrying a non-trivial well-definedness condition.
Such formulas (function application, division, card, min/max, ...) get
a \"WD\" inlay hint whose tooltip shows the condition."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-inlay-hints-max-length 32
  "Maximum rendered length of a type inlay hint in characters.
Longer types are truncated with an ellipsis and shown in full in the
hint tooltip; 0 disables truncation."
  :type 'integer
  :group 'eventb)

(defcustom lsp-rossi-rodin-path ""
  "Rodin IDE executable, macOS .app bundle, or app name for Open in Rodin.
Empty selects the platform default (/Applications/Rodin.app, rodin.exe,
or rodin)."
  :type 'string
  :group 'eventb)

(defcustom lsp-rossi-rodin-workspace ""
  "Directory used as the shared Rodin workspace by Open in Rodin.
Proofs made in Rodin persist there and survive rebuilds.  Empty selects
.rossi/rodin inside the workspace root."
  :type 'string
  :group 'eventb)

(defcustom lsp-rossi-rodin-sync t
  "Mutual live synchronization with a running Rodin.
While Rodin is open on the shared workspace, saving a buffer rebuilds
its project (Rodin picks the edit up within a few seconds), and edits
saved in Rodin flow back into the .eventb sources automatically.
Turning it off also stops the workspace watcher."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-rodin-bridge t
  "Use the Rodin bridge plug-in when a running Rodin publishes one.
It ships with the eventb-rossi Rodin bundle and lets Open in Rodin
register a project in the live instance -- which otherwise needs
File > Import while Rodin holds the workspace -- and bring it forward.
With no plug-in installed this changes nothing."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-rodin-live-sync nil
  "Merge Rodin's unsaved model edits into the buffer as they are typed.
Needs the bridge plug-in, which pushes the in-memory component;
the save-driven merge runs either way.  Only a clean merge is
applied: where both sides changed the same lines the buffer is
left alone until they settle."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-rodin-mirror-proofs t
  "Bridge proof files between the checkout and the Rodin workspace.
At Open in Rodin session boundaries: proof files (.bpr/.bps/.bpo) next
to the .eventb sources are copied into the Rodin project when the lens
runs (the checkout wins), and the project's proof files are copied back
next to the sources when Rodin exits (the workspace wins; a proof
deleted in Rodin is deleted next to the sources too)."
  :type 'boolean
  :group 'eventb)

(defcustom lsp-rossi-rodin-proving-perspective nil
  "Open Rodin in its Proving perspective, not the Event-B modelling one.
On a Rodin workspace that has been opened before this needs the bridge
plug-in, which switches the perspective inside the running instance.
Without it only a workspace Rodin has never opened is affected, since
Rodin otherwise restores the perspective that was last active there."
  :type 'boolean
  :group 'eventb)

(with-eval-after-load 'lsp-mode
  (lsp-register-client
   (make-lsp-client
    :new-connection (lsp-stdio-connection
                     (lambda ()
                       (if (listp eventb-language-server-command)
                           eventb-language-server-command
                         (list eventb-language-server-command))))
    :major-modes '(eventb-mode)
    :server-id 'eventb-ls
    :priority 0
    :initialization-options
    (lambda ()
      `(:rossi (:format (:style ,lsp-rossi-format-style
                          :useUnicode ,lsp-rossi-format-use-unicode
                          :privateUseGlyphs ,(if lsp-rossi-format-private-use-glyphs
                                                 t :json-false)
                          :indentation ,lsp-rossi-format-indentation
                          :maxLineWidth ,lsp-rossi-format-max-line-width)
                 :diagnostics (:enabled ,lsp-rossi-diagnostics-enabled
                               :debounceMs ,lsp-rossi-diagnostics-debounce-ms)
                 :completion (:enabled ,lsp-rossi-completion-enabled)
                 :inlayHints (:enabled ,(if lsp-rossi-inlay-hints-enabled t :json-false)
                              :wellDefinedness ,(if lsp-rossi-inlay-hints-well-definedness t :json-false)
                              :maxLength ,lsp-rossi-inlay-hints-max-length)
                 :rodin (:path ,lsp-rossi-rodin-path
                         :workspace ,lsp-rossi-rodin-workspace
                         :sync ,(if lsp-rossi-rodin-sync t :json-false)
                         :bridge ,(if lsp-rossi-rodin-bridge t :json-false)
                         :liveSync ,(if lsp-rossi-rodin-live-sync t :json-false)
                         :mirrorProofs ,(if lsp-rossi-rodin-mirror-proofs t :json-false)
                         :provingPerspective ,(if lsp-rossi-rodin-proving-perspective
                                                  t :json-false))))))))

;;; Unicode input method

;; The Quail input method itself is generated by `cargo xtask gen-grammars' into
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

;;; Editor commands (Rodin/validation helpers)

;; Interactive commands live in eventb-commands.el.  Require it softly so
;; this file still byte-compiles and loads when that companion is absent.
(require 'eventb-commands nil t)

;; Silence the byte-compiler for commands provided by eventb-commands.el.
(declare-function eventb-convert-to-unicode "eventb-commands")
(declare-function eventb-convert-to-ascii "eventb-commands")
(declare-function eventb-validate "eventb-commands")
(declare-function eventb-validate-workspace "eventb-commands")
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
  (setq-local syntax-propertize-function #'eventb--syntax-propertize-labels)
  (add-hook 'syntax-propertize-extend-region-functions
            #'syntax-propertize-wholelines nil t)

  ;; Comments
  (setq-local comment-start "// ")
  (setq-local comment-end "")
  (setq-local comment-start-skip "//+\\s-*")

  ;; Indentation
  (setq-local indent-line-function 'eventb-indent-line)
  (setq-local tab-width 4)
  (setq-local indent-tabs-mode nil)

  ;; Pin opinionated LSP defaults for Event-B buffers.  Semantic tokens give
  ;; richer, server-driven highlighting, code lenses surface the
  ;; selection-range / refinement affordances, and inlay hints show the
  ;; server's inferred declaration types; all are off by default in
  ;; lsp-mode, so enable them buffer-locally here.
  (setq-local lsp-semantic-tokens-enable t)
  (setq-local lsp-lens-enable t)
  (setq-local lsp-inlay-hint-enable t)

  ;; Show the private-use operator glyphs in a font that has them, when one
  ;; is installed.  A no-op otherwise, including on a terminal.
  (eventb-apply-math-font)

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
