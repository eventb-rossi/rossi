//! Emacs font-lock (`editors/emacs/eventb-mode.el`).
//!
//! Region-only: this generator owns the token `defconst`s and the
//! `eventb-font-lock-keywords` rules between the markers; the mode definition,
//! syntax table, indentation and LSP wiring stay hand-maintained.
//!
//! `regexp-opt` does the regex escaping and longest-match construction, so the
//! emitter only writes Elisp string lists. Matching is case-insensitive via
//! `font-lock-keywords-case-fold-search` (set in the mode body), which lets the
//! lowercased word lists match the uppercase spellings models actually use.

use super::{Markers, MatchKind, Model, Scope};

pub const MARKERS: Markers = Markers {
    begin: ";; >>> rossi gen-grammars (generated, do not edit)",
    end: ";; <<< rossi gen-grammars",
};

/// Render the generated region body (no markers, ends with a newline).
pub fn render(model: &Model) -> String {
    let mut out = String::new();

    let control = members_for(model, Scope::KeywordControl, MatchKind::Word);
    let status = members_for(model, Scope::KeywordOther, MatchKind::Word);
    let constants = members_for(model, Scope::ConstantLanguage, MatchKind::Word);
    let constant_symbols = members_for(model, Scope::ConstantLanguage, MatchKind::Symbol);
    let builtins = members_for(model, Scope::SupportFunction, MatchKind::Word);
    let operator_words = members_for(model, Scope::KeywordOperator, MatchKind::Word);
    let operator_symbols = members_for(model, Scope::KeywordOperator, MatchKind::Symbol);

    defconst(
        &mut out,
        "eventb-keywords",
        &control,
        "Event-B section and event keywords.",
    );
    defconst(
        &mut out,
        "eventb-status-keywords",
        &status,
        "Event-B status and inline modifiers.",
    );
    defconst(
        &mut out,
        "eventb-constants",
        &constants,
        "Event-B literal constants and number sets.",
    );
    defconst(
        &mut out,
        "eventb-builtins",
        &builtins,
        "Event-B built-in functions and predicates.",
    );
    defconst(
        &mut out,
        "eventb-operator-words",
        &operator_words,
        "Event-B alphabetic operators.",
    );
    defconst(
        &mut out,
        "eventb-constant-symbols",
        &constant_symbols,
        "Event-B symbolic constants.",
    );
    defconst(
        &mut out,
        "eventb-operator-symbols",
        &operator_symbols,
        "Event-B symbolic operators.",
    );

    out.push_str(
        r#"(defvar eventb-font-lock-keywords
  `((,(regexp-opt eventb-keywords 'words) . font-lock-keyword-face)
    (,(regexp-opt eventb-status-keywords 'words) . font-lock-keyword-face)
    ("\\<EVENT\\s-+\\([a-zA-Z_][a-zA-Z0-9_]*\\)" 1 font-lock-function-name-face)
    ("\\<\\(CONTEXT\\|MACHINE\\)\\s-+\\([a-zA-Z_][a-zA-Z0-9_]*\\)" 2 font-lock-type-face)
    (,(regexp-opt eventb-constants 'words) . font-lock-constant-face)
    (,(regexp-opt eventb-constant-symbols) . font-lock-constant-face)
    (,(regexp-opt eventb-builtins 'words) . font-lock-function-name-face)
    (,(regexp-opt eventb-operator-words 'words) . font-lock-builtin-face)
    (,(regexp-opt eventb-operator-symbols) . font-lock-builtin-face)
    ("\\<[0-9]+\\>" . font-lock-constant-face)
    ("@[A-Za-z0-9_]+" . font-lock-preprocessor-face))
  "Font lock keywords for Event-B mode (comments and strings come from the syntax table).")
"#,
    );

    out
}

fn members_for(model: &Model, scope: Scope, kind: MatchKind) -> Vec<String> {
    model
        .groups
        .iter()
        .filter(|g| g.scope == scope && g.kind == kind)
        .flat_map(|g| g.members.iter().cloned())
        .collect()
}

/// `(defconst NAME '("a" "b" …) "DOC.")`.
fn defconst(out: &mut String, name: &str, members: &[String], doc: &str) {
    let items = members
        .iter()
        .map(|m| elisp_string(m))
        .collect::<Vec<_>>()
        .join(" ");
    out.push_str(&format!("(defconst {name}\n  '({items})\n  \"{doc}\")\n\n"));
}

/// An Elisp string literal. `regexp-opt` handles regex escaping later, so we only
/// escape what a string literal needs: backslash and double quote.
fn elisp_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}
