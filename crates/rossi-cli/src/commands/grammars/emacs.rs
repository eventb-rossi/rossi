//! Emacs font-lock (`editors/emacs/eventb-mode.el`).
//!
//! Region-only: this generator owns the token `defconst`s and the
//! `eventb-font-lock-keywords` rules between the markers; the mode definition,
//! syntax table, indentation and LSP wiring stay hand-maintained.
//!
//! ## Case handling
//!
//! Emacs regexes have no inline `(?i)`, and
//! `font-lock-keywords-case-fold-search` is a single buffer-wide flag — with it
//! set, the exact-case math words would fold too (`DOM`, `Card`, `pow` are
//! ordinary identifiers; see `rossi::builtins::RESERVED_OPERATOR_WORDS`). So the
//! mode body sets the flag to `nil` and *case-insensitive* word groups (the
//! grammar's `^"…"` tokens: structural keywords, literals, `UNION`/`INTER`)
//! get their folding baked into the pattern as character classes
//! (`[Cc][Oo][Nn]…`), prebuilt here in Rust. Exact-case word groups and the
//! symbol groups (where case cannot apply) use plain alternations; symbols
//! still go through `regexp-opt` for escaping.

use super::{Markers, MatchKind, Model, Scope, TokenGroup};

pub const MARKERS: Markers = Markers {
    begin: ";; >>> rossi gen-grammars (generated, do not edit)",
    end: ";; <<< rossi gen-grammars",
};

/// Render the generated region body (no markers, ends with a newline).
pub fn render(model: &Model) -> String {
    let mut out = String::new();

    // Word groups: prebuilt regex strings (see the module doc for the case
    // story). One defconst per group, in model order.
    for group in word_groups(model) {
        let (name, doc) = word_const(group);
        defconst_regexp(&mut out, name, &word_regex(group), doc);
    }

    // Symbol groups: string lists, combined with `regexp-opt` at load time
    // (it does the escaping; case cannot apply to symbols).
    let constant_symbols = members_for(model, Scope::ConstantLanguage, MatchKind::Symbol);
    let operator_symbols = members_for(model, Scope::KeywordOperator, MatchKind::Symbol);
    defconst_list(
        &mut out,
        "eventb-constant-symbols",
        &constant_symbols,
        "Event-B symbolic constants.",
    );
    defconst_list(
        &mut out,
        "eventb-operator-symbols",
        &operator_symbols,
        "Event-B symbolic operators.",
    );

    // The two name-capturing rules reference case-insensitive keywords, so
    // they carry the same baked-in folding.
    let event_kw = fold_ascii_case("event");
    let context_kw = fold_ascii_case("context");
    let machine_kw = fold_ascii_case("machine");
    let name = component_name_regex();
    out.push_str(&format!(
        r#"(defvar eventb-font-lock-keywords
  `((,eventb-keywords-regexp . font-lock-keyword-face)
    (,eventb-status-keywords-regexp . font-lock-keyword-face)
    ("\\<{event_kw}\\s-+{name}" 1 font-lock-function-name-face)
    ("\\<\\(?:{context_kw}\\|{machine_kw}\\)\\s-+{name}" 1 font-lock-type-face)
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
"#,
    ));

    out
}

/// The Emacs regex capturing a `component_name` as group 1, mirroring the
/// grammar rule (`identifier ("-" part+)*`) so hyphenated names like
/// `end-update` highlight whole. The charset is the single source of truth in
/// `rossi::names`; only this Emacs regex flavor lives here. Double backslashes:
/// the pattern is emitted inside an Elisp string literal.
fn component_name_regex() -> String {
    format!(
        r"\\([{s}][{p}]*\\(?:-[{p}]+\\)*\\)",
        s = rossi::names::IDENT_START_CLASS,
        p = rossi::names::IDENT_PART_CLASS,
    )
}

/// The non-empty word groups, in model order.
fn word_groups(model: &Model) -> impl Iterator<Item = &TokenGroup> {
    model
        .groups
        .iter()
        .filter(|g| g.kind == MatchKind::Word && !g.members.is_empty())
}

/// The defconst name and docstring for one word group.
fn word_const(group: &TokenGroup) -> (&'static str, &'static str) {
    match (group.scope, group.case_insensitive) {
        (Scope::KeywordControl, _) => (
            "eventb-keywords-regexp",
            "Event-B section and event keywords (any case).",
        ),
        (Scope::KeywordOther, _) => (
            "eventb-status-keywords-regexp",
            "Event-B status and inline modifiers (any case).",
        ),
        (Scope::ConstantLanguage, _) => (
            "eventb-constants-regexp",
            "Event-B literal constants and number sets (any case).",
        ),
        (Scope::SupportFunction, _) => (
            "eventb-builtins-regexp",
            "Event-B built-in functions and predicates (exact case).",
        ),
        (Scope::KeywordOperator, true) => (
            "eventb-quantifier-words-regexp",
            "Event-B quantifier words UNION/INTER (any case).",
        ),
        (Scope::KeywordOperator, false) => (
            "eventb-operator-words-regexp",
            "Event-B alphabetic operators (exact case).",
        ),
    }
}

/// The Emacs regex for one word group: `\<\(?:ALT\|…\)\>`, longest-first, with
/// ASCII letters folded into character classes for case-insensitive groups.
fn word_regex(group: &TokenGroup) -> String {
    let mut words: Vec<&str> = group.members.iter().map(String::as_str).collect();
    words.sort_by(|a, b| super::longest_first(a, b));
    let alts: Vec<String> = words
        .iter()
        .map(|w| {
            if group.case_insensitive {
                fold_ascii_case(w)
            } else {
                (*w).to_string()
            }
        })
        .collect();
    format!("\\<\\(?:{}\\)\\>", alts.join("\\|"))
}

/// `context` → `[Cc][Oo][Nn][Tt][Ee][Xx][Tt]`. Digits and `_` pass through.
/// All word members are ASCII alphanumerics (the `is_word` routing guarantees
/// the first character is; the tables contain no exotic tails).
fn fold_ascii_case(word: &str) -> String {
    word.chars()
        .map(|c| {
            if c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_uppercase(), c.to_ascii_lowercase())
            } else {
                c.to_string()
            }
        })
        .collect()
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
fn defconst_list(out: &mut String, name: &str, members: &[String], doc: &str) {
    let items = members
        .iter()
        .map(|m| super::elisp_string(m))
        .collect::<Vec<_>>()
        .join(" ");
    out.push_str(&format!("(defconst {name}\n  '({items})\n  \"{doc}\")\n\n"));
}

/// `(defconst NAME "REGEX" "DOC.")`.
fn defconst_regexp(out: &mut String, name: &str, regex: &str, doc: &str) {
    out.push_str(&format!(
        "(defconst {name}\n  {}\n  \"{doc}\")\n\n",
        super::elisp_string(regex)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_groups_fold_exact_groups_do_not() {
        let model = Model::build();
        let region = render(&model);
        // Structural keywords fold into character classes…
        assert!(region.contains("[Cc][Oo][Nn][Tt][Ee][Xx][Tt]"));
        // …the exact-case math words appear verbatim, unfolded.
        assert!(region.contains("dom"));
        assert!(!region.contains("[Dd][Oo][Mm]"));
        assert!(region.contains("POW1"));
        assert!(!region.contains("[Pp][Oo][Ww]"));
        // UNION/INTER are the case-insensitive operator words.
        assert!(region.contains("eventb-quantifier-words-regexp"));
        assert!(region.contains("[Uu][Nn][Ii][Oo][Nn]"));
    }

    #[test]
    fn fold_ascii_case_passes_digits() {
        assert_eq!(fold_ascii_case("nat1"), "[Nn][Aa][Tt]1");
    }

    #[test]
    fn name_capture_spans_hyphen_segments() {
        // The EVENT/CONTEXT/MACHINE name capture mirrors `component_name`, so it
        // continues past `-` (issue #36) rather than stopping at the first
        // segment, and the built regex actually lands in the rendered region.
        let region = render(&Model::build());
        let name = component_name_regex();
        assert!(
            name.contains(r"\\(?:-[A-Za-z0-9_']+\\)*"),
            "regex must include a hyphen-segment group, got {name}"
        );
        assert!(
            region.contains(&name),
            "name capture must appear in the rendered region:\n{region}"
        );
    }
}
