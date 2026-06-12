//! Neovim / Vim syntax (`editors/neovim/syntax/eventb.vim`).
//!
//! Region-only: the file keeps its hand-maintained boilerplate (the
//! `b:current_syntax` guard) and this generator owns the block between the
//! markers — every `syn` item plus the `hi def link` wiring.
//!
//! Vim prefers the *longest* match among separate `syn` items, so cross-item
//! ordering is not load-bearing; but inside one `syn match` alternation Vim
//! takes the *first* matching branch, so symbol groups are emitted longest-first
//! (already guaranteed by the model). Word groups use `syn keyword`, which wins
//! over `syn match` and respects `syn case ignore` for case-insensitive matching.

use std::collections::HashSet;

use super::{Markers, MatchKind, Model, Scope, TokenGroup};

pub const MARKERS: Markers = Markers {
    begin: "\" >>> rossi gen-grammars (generated, do not edit)",
    end: "\" <<< rossi gen-grammars",
};

/// Vim syntax group + `hi def link` target for each model scope.
fn vim_group(scope: Scope) -> (&'static str, &'static str) {
    match scope {
        Scope::KeywordControl => ("eventbKeyword", "Keyword"),
        Scope::KeywordOther => ("eventbStatusKeyword", "Keyword"),
        Scope::ConstantLanguage => ("eventbConstant", "Constant"),
        Scope::SupportFunction => ("eventbBuiltin", "Function"),
        Scope::KeywordOperator => ("eventbOperator", "Operator"),
    }
}

/// Render the generated region body (no markers, ends with a newline).
pub fn render(model: &Model) -> String {
    let mut out = String::new();

    // Word groups: `syn keyword`, under the case mode of their grammar
    // tokens — `syn case ignore` for the `^"…"` tokens (structural keywords,
    // literals, UNION/INTER), `syn case match` for the exact-case math words
    // (`DOM`, `Card`, `pow` are ordinary identifiers and must not light up).
    out.push_str("syn case ignore\n");
    for group in &model.groups {
        if group.kind == MatchKind::Word && group.case_insensitive {
            let (name, _) = vim_group(group.scope);
            out.push_str(&format!("syn keyword {name} {}\n", group.members.join(" ")));
        }
    }
    out.push_str("syn case match\n");
    for group in &model.groups {
        if group.kind == MatchKind::Word && !group.case_insensitive {
            let (name, _) = vim_group(group.scope);
            out.push_str(&format!("syn keyword {name} {}\n", group.members.join(" ")));
        }
    }
    out.push('\n');

    // Symbol groups: one `syn match` per group, longest-first alternation.
    for group in &model.groups {
        if group.kind == MatchKind::Symbol {
            let (name, _) = vim_group(group.scope);
            out.push_str(&format!(
                "syn match {name} \"{}\"\n",
                vim_alternation(group)
            ));
        }
    }
    out.push('\n');

    // Fixed structural items.
    out.push_str("syn match eventbNumber \"\\<\\d\\+\\>\"\n");
    out.push_str("syn region eventbString start='\"' end='\"' contains=eventbEscape\n");
    out.push_str("syn match eventbEscape \"\\\\[nrt\\\\\\\"]\" contained\n");
    out.push_str("syn match eventbComment \"//.*$\"\n");
    out.push_str("syn region eventbComment start=\"/\\*\" end=\"\\*/\"\n");
    out.push_str("syn match eventbLabel \"@[A-Za-z0-9_]\\+\"\n");
    out.push_str("syn match eventbIdentifier \"\\<[a-zA-Z_][a-zA-Z0-9_']*\\>\"\n");
    out.push_str("syn match eventbDelimiter \"[(){}\\[\\]]\"\n");
    out.push('\n');

    // Highlight links (generator owns every group name it introduces). Several
    // scopes share a Vim group (operator symbols + words, constant symbols +
    // words), so dedup by name rather than relying on group order.
    let mut links: Vec<(&str, &str)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for group in &model.groups {
        let link = vim_group(group.scope);
        if seen.insert(link.0) {
            links.push(link);
        }
    }
    links.extend([
        ("eventbNumber", "Number"),
        ("eventbString", "String"),
        ("eventbEscape", "SpecialChar"),
        ("eventbComment", "Comment"),
        ("eventbLabel", "Label"),
        ("eventbIdentifier", "Identifier"),
        ("eventbDelimiter", "Delimiter"),
    ]);
    for (name, target) in links {
        out.push_str(&format!("hi def link {name} {target}\n"));
    }

    out
}

/// Join a symbol group into a Vim `\|` alternation (already longest-first).
fn vim_alternation(group: &TokenGroup) -> String {
    group
        .members
        .iter()
        .map(|m| vim_escape(m))
        .collect::<Vec<_>>()
        .join("\\|")
}

/// Escape a literal for a Vim "magic" regex. In magic mode `| ( ) { } < > + ? =`
/// are already literal; only these need a backslash.
fn vim_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '.' | '*' | '[' | ']' | '^' | '$' | '~' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
