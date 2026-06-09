//! The standalone tree-sitter grammar (`editors/tree-sitter-eventb`, published as
//! `eventb-rossi/tree-sitter-eventb`) and its highlight queries. This grammar is
//! consumed by the Zed extension, nvim-treesitter, Helix and friends; the captures
//! use the standard ecosystem names (`@keyword`, `@operator`, …).
//!
//! tree-sitter consumers need a *lexical* grammar to start: a parser that
//! recognises each coloured token class as its own node. We generate exactly that
//! — the token rules — into `grammar.js`'s marked region, and the token→capture
//! lines into `highlights.scm`'s marked region (so hand-written captures for future
//! structural nodes can live outside it). The grammar's surrounding scaffold
//! (`source_file`, `identifier`, `number`, `string`, `comment`, `label`,
//! punctuation, `extras`, `word`) — and any later hand-written structural rules —
//! are hand-maintained, since they are structure rather than token data.
//!
//! We also emit a token *manifest* ([`tokens_manifest`]): the canonical
//! classification as plain JSON the standalone repo's behavioral test reads to
//! check the built parser still tokenizes every canonical spelling correctly —
//! a contract that holds even after the grammar is hand-extended.
//!
//! ## Why one node per (class, kind), and no `prec`
//!
//! tree-sitter's lexer breaks ties **by precedence first, then by length**. So
//! `token(prec(1, …))` on a keyword would make `mod` win over the longer
//! `model` (stealing its prefix) and `/` win over the `//` comment. We therefore
//! emit *no* precedence and let plain longest-match do the work: `model` (the
//! identifier) is longer than `mod`, and `//…` (the comment) is longer than `/`.
//!
//! For the one case longest-match cannot settle — an exact-length tie like
//! `context` (keyword) vs `context` (identifier) — the grammar declares
//! `word: $ => $.identifier`, enabling tree-sitter's keyword extraction, which
//! resolves a whole-word match to the keyword. Keyword extraction only applies
//! to *pure word* tokens, so each class is split into a `*_word` node (a
//! case-insensitive regex, extractable) and a `*_sym` node (exact string
//! literals that never collide with identifiers). Within one word regex JS
//! alternation is leftmost — not longest — so the spellings are sorted
//! longest-first (`events` before `event`). Symbol literals need no ordering
//! (the lexer's longest-match picks `<=>` over `<`) and no escaping.

use super::{Markers, MatchKind, Model, Scope, TokenGroup};

/// The generated region inside the otherwise hand-maintained `grammar.js`.
pub const MARKERS: Markers = Markers {
    begin: "// >>> rossi gen-grammars (generated, do not edit)",
    end: "// <<< rossi gen-grammars",
};

/// The generated region inside the standalone grammar's `queries/highlights.scm`
/// (the one hand-editable highlights file; Zed's bundled copy is written verbatim
/// from it). The token→capture lines are generated; hand-written captures for
/// future structural nodes live outside the region, so highlighting can be
/// hand-extended without breaking the byte check.
pub const MARKERS_SCM: Markers = Markers {
    begin: "; >>> rossi gen-grammars (generated, do not edit)",
    end: "; <<< rossi gen-grammars",
};

/// The tree-sitter node (rule) name a coloured class is emitted as, split by
/// match kind so word nodes stay pure (keyword-extractable) and symbol nodes
/// stay free of identifier collisions. The hand-maintained `_token` rule in
/// `grammar.js` lists exactly these names, and [`render_highlights_region`]
/// captures them — so a new [`Scope`] variant breaks this `match` until it is
/// handled.
pub fn node_name(scope: Scope, kind: MatchKind) -> &'static str {
    match (scope, kind) {
        (Scope::KeywordControl, _) => "keyword",
        (Scope::KeywordOther, _) => "status_keyword",
        (Scope::SupportFunction, _) => "builtin",
        (Scope::ConstantLanguage, MatchKind::Word) => "constant_word",
        (Scope::ConstantLanguage, MatchKind::Symbol) => "constant_sym",
        (Scope::KeywordOperator, MatchKind::Word) => "operator_word",
        (Scope::KeywordOperator, MatchKind::Symbol) => "operator_sym",
    }
}

/// The tree-sitter highlight capture a class maps to in `highlights.scm` — the
/// standard ecosystem capture names (nvim-treesitter/Helix conventions, which
/// Zed also resolves to theme styles). The generated grammar splits each class
/// into a `*_word` and/or `*_sym` node (see [`node_name`]); both map to this one
/// capture. Kept beside `node_name` as a renderer-local mapping (not a method on
/// the shared `Scope`), since both are tree-sitter-only — matching how
/// `vim_group` lives in `vim.rs`.
fn capture_name(scope: Scope) -> &'static str {
    match scope {
        Scope::KeywordControl | Scope::KeywordOther => "keyword",
        Scope::ConstantLanguage => "constant.builtin",
        Scope::SupportFunction => "function.builtin",
        Scope::KeywordOperator => "operator",
    }
}

/// Render the generated token-rule region of `grammar.js` (between the markers).
/// One rule per non-empty model group, in model order. Ends with the closing
/// marker's indentation, which the splice drops from the region itself.
pub fn render_grammar_region(model: &Model) -> String {
    let mut out = String::new();
    for group in &model.groups {
        if group.members.is_empty() {
            continue;
        }
        let name = node_name(group.scope, group.kind);
        out.push_str(&format!("    {name}: $ => {},\n", token_expr(group)));
    }
    out.push_str("    ");
    out
}

/// Render the generated region of `highlights.scm` (between [`MARKERS_SCM`]):
/// one capture per generated node (locked to [`capture_name`]) plus the fixed
/// structural captures. Spliced into the standalone grammar's
/// `queries/highlights.scm`; Zed's bundled copy is then written verbatim from the
/// spliced result. Hand-written captures for future structural nodes live outside
/// the region and are preserved by the splice.
pub fn render_highlights_region(model: &Model) -> String {
    let mut out = String::new();
    // One capture per non-empty group; `node_name` gives each a distinct node
    // (so does `render_grammar_region`, which relies on the same uniqueness).
    for group in &model.groups {
        if group.members.is_empty() {
            continue;
        }
        let name = node_name(group.scope, group.kind);
        out.push_str(&format!("({}) @{}\n", name, capture_name(group.scope)));
    }
    out.push_str(
        "\n(comment) @comment\n\
         (string) @string\n\
         (number) @number\n\
         (label) @label\n\
         (identifier) @variable\n\n\
         [\"(\" \")\" \"[\" \"]\" \"{\" \"}\"] @punctuation.bracket\n\
         \",\" @punctuation.delimiter\n",
    );
    out
}

/// Render the canonical token manifest (`paths::TS_TOKENS`): a JSON object
/// `{ node_name: [spellings…] }` over every non-empty model group. Generated and
/// byte-checked here, then read by the standalone repo's behavioral test, which
/// parses each spelling with the built grammar and asserts it tokenizes to the
/// matching node — so "the grammar's core matches gen-grammars" stays verifiable
/// even after the grammar is hand-extended (the test asserts behavior, not text).
///
/// Keys are emitted in sorted order (a `BTreeMap`, so the ordering cannot be
/// flipped by a dependency enabling serde_json's `preserve_order` feature) and
/// each value keeps the group's own order (sorted words / longest-first symbols),
/// so the output is deterministic and byte-reproducible.
pub fn tokens_manifest(model: &Model) -> String {
    let mut map = std::collections::BTreeMap::new();
    for group in &model.groups {
        if group.members.is_empty() {
            continue;
        }
        map.insert(node_name(group.scope, group.kind), &group.members);
    }
    let mut out = serde_json::to_string_pretty(&map).expect("serialize token manifest");
    out.push('\n');
    out
}

/// The tree-sitter token expression for one group: a case-insensitive,
/// longest-first regex for a word group, or a `choice` of exact string literals
/// for a symbol group. Both are wrapped in `token(…)` so the node is one leaf.
fn token_expr(group: &TokenGroup) -> String {
    match group.kind {
        MatchKind::Word => {
            // JS alternation is leftmost-not-longest, so `events` must precede
            // `event` (see `super::longest_first`).
            let mut words: Vec<&str> = group.members.iter().map(String::as_str).collect();
            words.sort_by(|a, b| super::longest_first(a, b));
            // Escape regex metacharacters before splicing into the `/(?:…)/i`
            // literal. The metacharacter set is identical for Oniguruma and JS
            // RegExp, so we reuse `escape_oniguruma`. Every word member is
            // alphanumeric today (so this is a no-op), but it keeps the word path
            // as safe as the symbol path, which escapes via `js_string`.
            let alts: Vec<String> = words.iter().map(|w| super::escape_oniguruma(w)).collect();
            format!("token(/(?:{})/i)", alts.join("|"))
        }
        MatchKind::Symbol => {
            let lits: Vec<String> = group.members.iter().map(|s| js_string(s)).collect();
            if lits.len() == 1 {
                format!("token({})", lits[0])
            } else {
                format!("token(choice({}))", lits.join(", "))
            }
        }
    }
}

/// A JavaScript double-quoted string literal (with surrounding quotes), escaping
/// the backslash and double-quote a JS string must escape. Operator spellings
/// like `\/`, `/\` and `\` carry backslashes; private-use glyphs pass through raw.
fn js_string(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every generated node name must be referenced by the hand-maintained
    /// `_token` choice in `grammar.js`; otherwise that class would tokenize but
    /// never reach the tree (silent missing highlight). This is the one coupling
    /// between the generated region and the hand-written scaffold, so it is
    /// guarded explicitly.
    fn grammar_js() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../editors/tree-sitter-eventb/grammar.js");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    #[test]
    fn generated_nodes_are_listed_in_token_choice() {
        let model = Model::build();
        let grammar = grammar_js();
        for group in &model.groups {
            if group.members.is_empty() {
                continue;
            }
            let name = node_name(group.scope, group.kind);
            assert!(
                grammar.contains(&format!("$.{name},")),
                "grammar.js `_token` is missing `$.{name}` (generated node has no place in the tree)"
            );
        }
    }

    #[test]
    fn highlights_capture_every_node_and_the_structural_tokens() {
        let model = Model::build();
        let scm = render_highlights_region(&model);
        for group in &model.groups {
            if group.members.is_empty() {
                continue;
            }
            let name = node_name(group.scope, group.kind);
            let capture = capture_name(group.scope);
            assert!(
                scm.contains(&format!("({name}) @{capture}\n")),
                "highlights.scm is missing `({name}) @{capture}`"
            );
        }
        for fixed in [
            "(comment) @comment",
            "(string) @string",
            "(number) @number",
            "(label) @label",
            "(identifier) @variable",
            "@punctuation.bracket",
        ] {
            assert!(scm.contains(fixed), "highlights.scm is missing `{fixed}`");
        }
    }

    #[test]
    fn tokens_manifest_lists_every_node_and_all_members() {
        let model = Model::build();
        let json: serde_json::Value =
            serde_json::from_str(&tokens_manifest(&model)).expect("manifest is valid JSON");
        let obj = json.as_object().expect("manifest is a JSON object");
        for group in &model.groups {
            if group.members.is_empty() {
                continue;
            }
            let name = node_name(group.scope, group.kind);
            let arr = obj
                .get(name)
                .unwrap_or_else(|| panic!("manifest missing node `{name}`"))
                .as_array()
                .unwrap_or_else(|| panic!("manifest `{name}` is not an array"));
            let listed: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
            for m in &group.members {
                assert!(
                    listed.contains(&m.as_str()),
                    "manifest `{name}` is missing spelling `{m}`"
                );
            }
        }
        // Spot-check the contract the behavioral test relies on.
        assert!(
            obj["keyword"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "context")
        );
        assert!(
            obj["builtin"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "card")
        );
        assert!(
            obj["operator_sym"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "∈")
        );
    }

    /// Extract the `|`-separated alternatives from a `token(/(?:…)/i)` rule line.
    fn word_alternatives(region: &str, rule: &str) -> Vec<String> {
        let line = region
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{rule}:")))
            .unwrap_or_else(|| panic!("missing rule {rule}"));
        let body = line
            .split_once("(?:")
            .and_then(|(_, rest)| rest.split_once(")/i)"))
            .map(|(body, _)| body)
            .unwrap_or_else(|| panic!("rule {rule} is not a `token(/(?:…)/i)` regex: {line}"));
        body.split('|').map(str::to_string).collect()
    }

    #[test]
    fn word_rules_are_longest_first() {
        let model = Model::build();
        let region = render_grammar_region(&model);
        // JS alternation is leftmost-not-longest: within one word regex, if `a`
        // is a prefix of a longer `b`, then `b` must come first or `a` would
        // shadow it. Assert the invariant for every generated word group rather
        // than for specific spellings (which come and go from the tables).
        let mut pairs_checked = 0;
        for group in &model.groups {
            if !matches!(group.kind, MatchKind::Word) || group.members.is_empty() {
                continue;
            }
            let rule = node_name(group.scope, group.kind);
            let alts = word_alternatives(&region, rule);
            for (i, a) in alts.iter().enumerate() {
                for (j, b) in alts.iter().enumerate() {
                    if i != j && b.len() > a.len() && b.starts_with(a.as_str()) {
                        pairs_checked += 1;
                        assert!(
                            j < i,
                            "in `{rule}`, longer `{b}` must precede its prefix `{a}`: {alts:?}"
                        );
                    }
                }
            }
        }
        // events/event, nat/nat1, pow/pow1 all exist, so the loop must have
        // exercised the ordering — guard against the check silently going dark.
        assert!(
            pairs_checked > 0,
            "no prefix pair found to exercise ordering"
        );
    }

    #[test]
    fn symbol_rules_are_string_literals() {
        let model = Model::build();
        let region = render_grammar_region(&model);
        let op_sym = region
            .lines()
            .find(|l| l.trim_start().starts_with("operator_sym:"))
            .expect("operator_sym rule");
        // No regex — exact string literals the lexer's longest-match orders.
        assert!(op_sym.contains("token(choice("));
        assert!(!op_sym.contains("/(?:"));
        assert!(op_sym.contains("\"∈\""));
        assert!(op_sym.contains("\"<=>\""));
    }

    #[test]
    fn js_string_escapes_backslashes() {
        assert_eq!(js_string("\\/"), "\"\\\\/\""); // set union ASCII `\/`
        assert_eq!(js_string("∈"), "\"∈\"");
    }
}
