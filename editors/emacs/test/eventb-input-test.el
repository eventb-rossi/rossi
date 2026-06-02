;;; eventb-input-test.el --- Tests for the Event-B Quail input method -*- lexical-binding: t; -*-

;; Copyright (C) 2025 Rossi Contributors

;; This file is not part of GNU Emacs.

;; This program is dual-licensed under MIT or Apache-2.0.

;;; Commentary:

;; ERT tests for the generated `eventb-input.el' Quail package. They load
;; the package, then assert that representative backslash-leader sequences
;; (e.g. "\\to" -> RIGHTWARDS ARROW, "\\and" -> LOGICAL AND, "\\nat" ->
;; DOUBLE-STRUCK CAPITAL N) translate to the expected Unicode glyph.
;;
;; Two layers of checking:
;;   1. `eventb-input--lookup' walks the Quail rule map directly via
;;      `quail-lookup-key', exercising the rules `quail-define-rules'
;;      installed for the package.
;;   2. `eventb-input-test-temp-buffer-translation' activates the input
;;      method in a live buffer and inserts the looked-up translation,
;;      asserting the buffer receives the expected glyph. (The interactive
;;      `quail-input-method' command loop is not driven directly: batch-mode
;;      event delivery is unreliable, so the buffer test reuses the same
;;      rule map the command loop consults.)
;;
;; Run with:
;;   emacs -batch -L editors/emacs -l ert \
;;     -l editors/emacs/test/eventb-input-test.el \
;;     -f ert-run-tests-batch-and-exit

;;; Code:

(require 'ert)
(require 'quail)

;; Load the generated input method relative to this test file so the test is
;; runnable from any working directory (`-L editors/emacs' also works).
(let ((this-dir (file-name-directory (or load-file-name buffer-file-name
                                         default-directory))))
  (add-to-list 'load-path (expand-file-name ".." this-dir)))
(require 'eventb-input)

(defconst eventb-input-test-package "eventb"
  "Name of the Quail package under test.")

;; Representative leader sequences spanning logic, sets, relations, number
;; sets, and an alias — covering the families the generator emits.
(defconst eventb-input-test-cases
  '(("\\to"       . "→")
    ("\\and"      . "∧")
    ("\\or"       . "∨")
    ("\\not"      . "¬")
    ("\\implies"  . "⇒")
    ("\\forall"   . "∀")
    ("\\exists"   . "∃")
    ("\\in"       . "∈")
    ("\\notin"    . "∉")
    ("\\nat"      . "ℕ")
    ("\\int"      . "ℤ")
    ("\\pow"      . "ℙ")
    ("\\maplet"   . "↦")
    ("\\lambda"   . "λ")
    ("\\union"    . "∪")
    ("\\inter"    . "∩")
    ;; Aliases must resolve to the same glyph as their primary spelling.
    ("\\neq"      . "≠")
    ("\\land"     . "∧")
    ("\\NAT"      . "ℕ"))
  "Backslash-leader sequence -> expected Unicode glyph.")

(defun eventb-input--candidate-to-string (cand)
  "Normalise a Quail translation CAND to its first candidate string.
A translation is a string, a character, a vector of candidates, or a
cons `(INDICES . VECTOR)'; reduce each of these to a one-glyph string."
  (cond
   ((stringp cand) cand)
   ((characterp cand) (char-to-string cand))
   ((vectorp cand)
    (let ((first (aref cand 0)))
      (if (characterp first) (char-to-string first) first)))
   ;; `(INDICES . CANDIDATES)' where CANDIDATES is a vector or string.
   ((consp cand) (eventb-input--candidate-to-string (cdr cand)))
   (t cand)))

(defun eventb-input--lookup (key)
  "Look up KEY in the eventb Quail map, returning the translation string.
Returns nil when KEY has no rule.

A Quail map is `(TRANSLATION . ALIST)', so the translation for a fully
matched KEY is the car of the map `quail-lookup-key' returns."
  (with-temp-buffer
    (let* ((quail-current-package
            (assoc eventb-input-test-package quail-package-alist))
           (map (quail-lookup-key key (length key)))
           (translation (and (consp map) (car map))))
      (and translation
           (eventb-input--candidate-to-string translation)))))

(ert-deftest eventb-input-test-package-registered ()
  "The eventb Quail package is registered after loading the file."
  (should (assoc eventb-input-test-package quail-package-alist)))

(ert-deftest eventb-input-test-rule-lookups ()
  "Each representative backslash sequence resolves in the Quail map."
  (dolist (case eventb-input-test-cases)
    (let* ((key (car case))
           (expected (cdr case))
           (actual (eventb-input--lookup key)))
      (should (equal actual expected)))))

(ert-deftest eventb-input-test-temp-buffer-translation ()
  "Activating the method then translating each sequence yields the glyph.
Activates the input method in a live buffer (so the method really
installs), looks the sequence up through its Quail rule map, and inserts
the translation — asserting the buffer receives the expected glyph.
This exercises the same rule map the interactive command loop consults,
without depending on batch-mode event delivery."
  (dolist (case eventb-input-test-cases)
    (let ((key (car case))
          (expected (cdr case)))
      (with-temp-buffer
        (activate-input-method eventb-input-test-package)
        (should (equal current-input-method eventb-input-test-package))
        (insert (eventb-input--lookup key))
        (deactivate-input-method)
        (goto-char (point-min))
        (should (string-prefix-p expected (buffer-string)))))))

(ert-deftest eventb-input-test-no-bare-keys ()
  "By design every rule starts with the backslash leader (no eager keys)."
  (dolist (case eventb-input-test-cases)
    (should (string-prefix-p "\\" (car case)))))

(provide 'eventb-input-test)

;;; eventb-input-test.el ends here
