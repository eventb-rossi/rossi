;;; eventb-math-font-test.el --- Tests for the private-use glyph fontset -*- lexical-binding: t; -*-

;; Copyright (C) 2025 Rossi Contributors

;; This file is not part of GNU Emacs.

;; This program is dual-licensed under MIT or Apache-2.0.

;;; Commentary:

;; ERT tests for `eventb-apply-math-font', which maps Rodin's four
;; private-use operator code points onto a font that has them.
;;
;; The mapping itself cannot be asserted in batch mode: there is no display,
;; so no font is resolvable.  What is worth pinning is exactly that --- the
;; function has to be a silent no-op wherever it cannot help, because
;; `eventb-mode' calls it on every buffer.  A terminal Emacs, a machine
;; without the font, and a user who set `eventb-math-font' to nil must all
;; open an Event-B file without an error or a message.
;;
;; Run with:
;;   emacs -batch -L editors/emacs -l ert \
;;     -l editors/emacs/test/eventb-math-font-test.el \
;;     -f ert-run-tests-batch-and-exit

;;; Code:

(require 'ert)
(require 'eventb-mode)

(ert-deftest eventb-math-font-range-matches-rodin ()
  "The remapped range is exactly Rodin's four private-use operators."
  (should (= (car eventb-private-use-glyph-range) #xE100))
  (should (= (cdr eventb-private-use-glyph-range) #xE103))
  ;; TREL, SREL, STREL, OVR — nothing else lives in the range.
  (should (= (1+ (- (cdr eventb-private-use-glyph-range)
                    (car eventb-private-use-glyph-range)))
             4)))

(ert-deftest eventb-apply-math-font-is-a-no-op-without-a-display ()
  "Batch mode has no resolvable font, so the call does nothing quietly."
  (should-not (display-multi-font-p))
  (should-not (eventb-apply-math-font)))

(ert-deftest eventb-apply-math-font-honours-a-nil-family ()
  "Setting `eventb-math-font' to nil leaves the fontset alone."
  (let ((eventb-math-font nil))
    (should-not (eventb-apply-math-font))))

(ert-deftest eventb-math-font-defaults-are-portable ()
  "Out of the box rossi writes ASCII, so no font is required to read it."
  (should-not (default-value 'lsp-rossi-format-private-use-glyphs))
  (should (equal (default-value 'eventb-math-font) "Brave Sans Mono")))

(ert-deftest eventb-mode-opens-a-buffer-without-the-font ()
  "`eventb-mode' calls the mapping on every buffer; it must not signal."
  (with-temp-buffer
    (insert "CONTEXT c\nEND\n")
    (let ((eventb-enable-input-method nil))
      (eventb-mode))
    (should (eq major-mode 'eventb-mode))))

(provide 'eventb-math-font-test)

;;; eventb-math-font-test.el ends here
