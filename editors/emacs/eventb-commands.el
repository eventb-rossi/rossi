;;; eventb-commands.el --- Rossi toolchain commands for Event-B -*- lexical-binding: t; -*-

;; Copyright (C) 2025 Rossi Contributors

;; Author: Rossi Contributors
;; URL: https://github.com/eventb-rossi/rossi
;; Version: 0.1.0
;; Package-Requires: ((emacs "26.1"))
;; Keywords: languages, event-b, formal-methods

;; This file is not part of GNU Emacs.

;; This program is dual-licensed under MIT or Apache-2.0.

;;; Commentary:

;; Interactive commands that drive the `rossi' command-line tool from
;; Emacs, mirroring the VS Code extension's command palette:
;;
;; - `eventb-convert-to-unicode' / `eventb-convert-to-ascii': reformat the
;;   current buffer in place, switching the operator convention via
;;   `rossi fmt -' (the buffer is piped through stdin so unsaved edits are
;;   converted without forcing a save).
;; - `eventb-validate' / `eventb-validate-workspace': run
;;   `rossi validate --format json --continue-on-error' and surface the
;;   diagnostics in a `compilation-mode' buffer.
;; - `eventb-import' / `eventb-export' / `eventb-build': prompt for paths and
;;   invoke the matching `rossi' subcommand.
;;
;; Load alongside `eventb-mode':
;;
;;   (require 'eventb-commands)
;;
;; and bind whichever commands you like, e.g.
;;
;;   (with-eval-after-load 'eventb-mode
;;     (define-key eventb-mode-map (kbd "C-c C-u") #'eventb-convert-to-unicode)
;;     (define-key eventb-mode-map (kbd "C-c C-d") #'eventb-convert-to-ascii)
;;     (define-key eventb-mode-map (kbd "C-c C-v") #'eventb-validate))

;;; Code:

(require 'compile)

;; `json-parse-buffer' is built in on Emacs 27+; fall back to `json-read'
;; (json.el) on Emacs 26. Both are loaded lazily inside `eventb--parse-json'.
(require 'json)

;;; Customization

(defgroup eventb-commands nil
  "Rossi toolchain commands for Event-B."
  :group 'eventb
  :prefix "eventb-")

(defcustom rossi-tool-path "rossi"
  "Path to the `rossi' command-line executable.
Either a bare command name resolved on `exec-path' or an absolute path."
  :type 'string
  :group 'eventb-commands)

;;; Helpers

(defun eventb--tool-path ()
  "Return the `rossi' executable, signalling a clear error when missing."
  (or (executable-find rossi-tool-path)
      (and (file-name-absolute-p rossi-tool-path)
           (file-executable-p rossi-tool-path)
           rossi-tool-path)
      (user-error "Cannot find the `rossi' tool (%s); set `rossi-tool-path'"
                  rossi-tool-path)))

(defun eventb--buffer-file ()
  "Return the file backing the current buffer, or signal an error."
  (or (buffer-file-name)
      (user-error "Buffer %s is not visiting a file" (buffer-name))))

(defun eventb--parse-json (string)
  "Parse JSON STRING into a list of alists, preferring native parsing.
Objects become alists and arrays become lists so the result is uniform
across `json-parse-string' and `json-read'."
  (if (fboundp 'json-parse-string)
      (json-parse-string string
                         :object-type 'alist
                         :array-type 'list
                         :null-object nil
                         :false-object nil)
    (let ((json-object-type 'alist)
          (json-array-type 'list)
          (json-key-type 'symbol)
          (json-false nil)
          (json-null nil))
      (json-read-from-string string))))

;;; Operator conversion

(defun eventb--convert (style)
  "Reformat the current buffer in place, applying STYLE (\"--unicode\" or \"--ascii\").
The whole buffer is piped through `rossi fmt -' and replaced with the
formatter's output, leaving point near where it was."
  (eventb--tool-path)
  (let* ((point (point))
         (errfile (make-temp-file "rossi-fmt-err"))
         (status (unwind-protect
                     (call-process-region (point-min) (point-max)
                                          rossi-tool-path
                                          t (list t errfile) nil
                                          "fmt" "-" style)
                   nil)))
    (unwind-protect
        (unless (eq status 0)
          (let ((stderr (with-temp-buffer
                          (ignore-errors (insert-file-contents errfile))
                          (string-trim (buffer-string)))))
            (user-error "rossi fmt %s failed: %s"
                        style
                        (if (string-empty-p stderr)
                            (format "exit %s" status)
                          stderr))))
      (ignore-errors (delete-file errfile)))
    (goto-char (min point (point-max)))))

;;;###autoload
(defun eventb-convert-to-unicode ()
  "Convert the current buffer's Event-B operators to Unicode in place."
  (interactive)
  (eventb--convert "--unicode")
  (message "Converted %s to Unicode" (buffer-name)))

;;;###autoload
(defun eventb-convert-to-ascii ()
  "Convert the current buffer's Event-B operators to ASCII in place."
  (interactive)
  (eventb--convert "--ascii")
  (message "Converted %s to ASCII" (buffer-name)))

;;; Validation

(defconst eventb--validate-buffer-name "*rossi-validate*"
  "Name of the buffer that shows `rossi validate' diagnostics.")

(defun eventb--severity-prefix (severity)
  "Map a validation SEVERITY string to a `compilation-mode' marker.
Mirrors the VS Code extension: anything other than warning/info/hint is
treated as an error."
  (pcase severity
    ("warning" "warning")
    ("info" "info")
    ("hint" "info")
    (_ "error")))

(defun eventb--validation-message (row)
  "Build a one-line diagnostic message for validation ROW.
Prefixes the rule id and, when present, the inner filename and origin,
matching the VS Code extension's `validationMessage'."
  (let ((parts '())
        (rule (alist-get 'rule_id row))
        (inner (alist-get 'inner_filename row))
        (origin (alist-get 'origin row))
        (error (alist-get 'error row))
        (severity (alist-get 'severity row)))
    (when rule
      (push (format "[%s]" rule) parts))
    (when inner
      (push (format "%s:" inner) parts))
    (when origin
      (push (format "%s:" origin) parts))
    (push (or error severity "Validation issue") parts)
    (mapconcat #'identity (nreverse parts) " ")))

(defun eventb--validation-path (row default-dir)
  "Resolve the on-disk path a validation ROW refers to, relative to DEFAULT-DIR.
Joins the inner filename for unzipped components, like the VS Code
extension's `validationDiagnosticPath'."
  (let* ((file (alist-get 'file row))
         (target (if (file-name-absolute-p file)
                     file
                   (expand-file-name file default-dir)))
         (inner (alist-get 'inner_filename row)))
    (if (and inner
             (not (string-equal (downcase (or (file-name-extension target t) ""))
                                ".zip")))
        (expand-file-name inner target)
      target)))

(defun eventb--run-validate (inputs default-dir)
  "Run `rossi validate' over INPUTS (a list of paths) from DEFAULT-DIR.
Renders each diagnostic into a `compilation-mode' buffer so `next-error'
jumps to the offending file."
  (eventb--tool-path)
  (let* ((default-directory (file-name-as-directory default-dir))
         (outfile (make-temp-file "rossi-validate-out"))
         (status (unwind-protect
                     (apply #'call-process rossi-tool-path nil
                            (list :file outfile) nil
                            "validate" "--format" "json"
                            "--continue-on-error" inputs)
                   nil))
         (stdout (with-temp-buffer
                   (ignore-errors (insert-file-contents outfile))
                   (buffer-string))))
    (ignore-errors (delete-file outfile))
    (let ((rows (condition-case err
                    (eventb--parse-json stdout)
                  (error
                   (user-error "Failed to parse rossi validation JSON: %s"
                               (error-message-string err))))))
      (eventb--render-validation rows default-dir)
      (if (eq status 0)
          (message "Rossi validation completed")
        (message "Rossi validation found issues; see %s"
                 eventb--validate-buffer-name)))))

(defun eventb--validation-line-col (row)
  "Return a (LINE . COLUMN) cons for ROW, both 1-indexed.
Uses ROW's `region' when present (already 1-indexed, the compilation-mode
convention); falls back to (1 . 1) for diagnostics with no source position
\(Rodin-XML-sourced or project-level)."
  (let ((region (alist-get 'region row)))
    (if region
        (cons (alist-get 'start_line region)
              (alist-get 'start_column region))
      (cons 1 1))))

(defun eventb--render-validation (rows default-dir)
  "Render validation ROWS into the diagnostics buffer, relative to DEFAULT-DIR."
  (let ((buffer (get-buffer-create eventb--validate-buffer-name))
        (count 0))
    (with-current-buffer buffer
      (let ((inhibit-read-only t))
        (erase-buffer)
        (insert "rossi validate\n\n")
        (dolist (row rows)
          (when (or (alist-get 'error row) (alist-get 'severity row))
            (setq count (1+ count))
            ;; "file:line:col: severity: message" is the canonical
            ;; compilation-mode shape, anchored on the diagnostic's region.
            (let ((pos (eventb--validation-line-col row)))
              (insert (format "%s:%d:%d: %s: %s\n"
                              (eventb--validation-path row default-dir)
                              (car pos)
                              (cdr pos)
                              (eventb--severity-prefix (alist-get 'severity row))
                              (eventb--validation-message row))))))
        (insert (format "\n%d diagnostic(s).\n" count))
        (compilation-mode)
        (goto-char (point-min))))
    (display-buffer buffer)))

;;;###autoload
(defun eventb-validate ()
  "Validate the current Event-B file with `rossi validate'."
  (interactive)
  (let ((file (eventb--buffer-file)))
    (when (and (buffer-modified-p) (y-or-n-p "Save buffer before validating? "))
      (save-buffer))
    (eventb--run-validate (list file) (file-name-directory file))))

;;;###autoload
(defun eventb-validate-workspace ()
  "Validate an entire directory tree of Event-B files with `rossi validate'."
  (interactive)
  (let ((dir (read-directory-name "Validate directory: "
                                  (or (when (buffer-file-name)
                                        (file-name-directory (buffer-file-name)))
                                      default-directory)
                                  nil t)))
    (eventb--run-validate (list (directory-file-name dir)) dir)))

;;; Import / export / build

(defun eventb--run-tool (subcommand input output progress)
  "Run `rossi SUBCOMMAND INPUT -o OUTPUT', reporting PROGRESS on success."
  (eventb--tool-path)
  (let* ((errfile (make-temp-file "rossi-tool-err"))
         (status (unwind-protect
                     (call-process rossi-tool-path nil (list nil errfile) nil
                                   subcommand input "-o" output)
                   nil))
         (stderr (with-temp-buffer
                   (ignore-errors (insert-file-contents errfile))
                   (string-trim (buffer-string)))))
    (ignore-errors (delete-file errfile))
    (if (eq status 0)
        (message "%s -> %s" progress output)
      (user-error "rossi %s failed: %s"
                  subcommand
                  (if (string-empty-p stderr) (format "exit %s" status) stderr)))))

;;;###autoload
(defun eventb-import ()
  "Import a Rodin project (.zip/.buc/.bum) into an Event-B directory."
  (interactive)
  (let ((input (read-file-name "Import Rodin project: " nil nil t))
        (output (read-directory-name "Import into directory: ")))
    (eventb--run-tool "import" input output "Imported Rodin project")))

;;;###autoload
(defun eventb-export ()
  "Export an Event-B file or directory to a Rodin ZIP."
  (interactive)
  (let* ((input (read-file-name "Export Event-B file/dir: " nil
                                (buffer-file-name) t))
         (output (read-file-name "Export to Rodin ZIP: " nil nil nil
                                 (concat (file-name-base
                                          (directory-file-name input))
                                         ".zip"))))
    (eventb--run-tool "export" input output "Exported Rodin ZIP")))

;;;###autoload
(defun eventb-build ()
  "Build a checked Rodin ZIP from an Event-B file or directory."
  (interactive)
  (let* ((input (read-file-name "Build Event-B file/dir: " nil
                                (buffer-file-name) t))
         (output (read-file-name "Build to checked Rodin ZIP: " nil nil nil
                                 (concat (file-name-base
                                          (directory-file-name input))
                                         ".checked.zip"))))
    (eventb--run-tool "build" input output "Built checked Rodin ZIP")))

(provide 'eventb-commands)

;;; eventb-commands.el ends here
