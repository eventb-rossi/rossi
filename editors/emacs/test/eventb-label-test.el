;;; eventb-label-test.el --- Camille label highlighting tests -*- lexical-binding: t; -*-

;; Run: emacs --batch -L editors/emacs -l editors/emacs/test/eventb-label-test.el
;;      -f ert-run-tests-batch-and-exit

(require 'ert)
(require 'eventb-mode)

(ert-deftest eventb-labels-preserve-punctuation-and-stop-at-whitespace ()
  (dolist (label '("a:" "a::" ":" "метка:" "😀:" "a@b" "SAF5\""
                  "inv//1" "inv/*1" "a\u0085"))
    (dolist (separator '(" " "\t" "\u001c" "\u00a0" "\u2007"))
      (with-temp-buffer
        (let ((eventb-enable-input-method nil))
          (eventb-mode))
        (insert "@" label separator "1 = 1\nend\n")
        (font-lock-ensure)
        (dotimes (offset (1+ (length label)))
          (should (eq (get-text-property (1+ offset) 'face)
                      'font-lock-preprocessor-face)))
        (goto-char (point-min))
        (search-forward "1 = 1")
        (should (eq (get-text-property (- (point) 5) 'face)
                    'font-lock-constant-face))
        (should-not (nth 8 (syntax-ppss (- (point) 5))))))))

(ert-deftest eventb-labels-do-not-override-comments-or-strings ()
  (dolist (source '("// @label:\n" "/* @label: */\n" "\"@label:\"\n"))
    (with-temp-buffer
      (let ((eventb-enable-input-method nil))
        (eventb-mode))
      (insert source)
      (font-lock-ensure)
      (goto-char (point-min))
      (search-forward "@")
      (should (memq (get-text-property (1- (point)) 'face)
                    '(font-lock-comment-face font-lock-string-face))))))

(ert-deftest eventb-label-syntax-updates-after-editing-the-sigil ()
  (with-temp-buffer
    (let ((eventb-enable-input-method nil))
      (eventb-mode))
    (insert "@inv//1 1 = 1\nend\n")
    (font-lock-ensure)
    (should-not (nth 4 (syntax-ppss 9)))
    (goto-char (point-min))
    (delete-char 1)
    (font-lock-ensure)
    (should (nth 4 (syntax-ppss 8)))
    (goto-char (point-min))
    (insert "@")
    (font-lock-ensure)
    (should-not (nth 4 (syntax-ppss 9)))
    (should (eq (get-text-property 5 'face) 'font-lock-preprocessor-face))))

;;; eventb-label-test.el ends here
