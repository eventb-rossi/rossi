;;; eventb-machine-commands-test.el --- Rossi LSP command tests -*- lexical-binding: t; -*-

;; Run: emacs --batch -L editors/emacs -l editors/emacs/test/eventb-machine-commands-test.el
;;      -f ert-run-tests-batch-and-exit

(require 'ert)
(require 'cl-lib)
(require 'eventb-mode)

(defvar lsp-mode)

(ert-deftest eventb-machine-commands-send-the-current-document-and-position ()
  (with-temp-buffer
    (let ((eventb-enable-input-method nil))
      (eventb-mode))
    (setq buffer-file-name "/tmp/M.eventb")
    (insert "MACHINE M\nEND\n")
    (goto-char (point-min))
    (let ((lsp-mode t)
          (calls nil))
      (cl-letf (((symbol-function 'lsp-workspaces) (lambda () '(rossi)))
                ((symbol-function 'lsp-text-document-identifier)
                 (lambda () '(:uri "file:///tmp/M.eventb")))
                ((symbol-function 'lsp-point-to-position)
                 (lambda (point) (should (= point (point)))
                   '(:line 0 :character 0)))
                ((symbol-function 'lsp-send-execute-command)
                 (lambda (command args) (push (list command args) calls))))
        (eventb-open-in-rodin)
        (eventb-model-check)
        (eventb-disprove-pos))
      (should (equal (nreverse calls)
                     '(("rossi.rodin.open" ["file:///tmp/M.eventb"])
                       ("rossi.animate.check" ["file:///tmp/M.eventb" (:line 0 :character 0)])
                       ("rossi.animate.po" ["file:///tmp/M.eventb" (:line 0 :character 0)])))))))

(ert-deftest eventb-machine-commands-require-an-active-server ()
  (with-temp-buffer
    (let ((eventb-enable-input-method nil))
      (eventb-mode))
    (setq buffer-file-name "/tmp/M.eventb")
    (let ((lsp-mode nil))
      (should-error (eventb-model-check) :type 'user-error))))

;;; eventb-machine-commands-test.el ends here
