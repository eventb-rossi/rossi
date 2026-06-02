//! Editor syntax-highlighting grammars, generated from the canonical token
//! tables so they can never drift from the parser.
//!
//! The single sources of truth are [`rossi::keywords::KEYWORDS`],
//! [`rossi::operators::OPERATOR_SPELLINGS`], and [`rossi::builtins::BUILTIN_WORDS`],
//! which are themselves kept in sync with `crates/rossi/src/grammar.pest` and
//! `docs/EVENTB_LANGUAGE_REFERENCE.md` by unit tests in those modules. Here we
//! fold them into one format-neutral [`Model`] of token *groups*, then render it
//! into each editor's native regex grammar (TextMate, Sublime, Vim, Emacs).
//!
//! ## Why a highlighter can be generated but a parser cannot
//!
//! TextMate / Sublime / Vim-syntax / Emacs-font-lock are all *lexical* (regex)
//! highlighters: every distinction they draw is token data, which is exactly
//! what the tables hold. (Tree-sitter, by contrast, needs a full parser grammar
//! and is intentionally out of scope.)
//!
//! ## Correctness rules baked into the [`Model`]
//!
//! - **Symbolic vs word.** A spelling whose first character is an ASCII letter
//!   (`mod`, `dom`, `or`, `POW`, `NAT`, …) is matched as a whole *word* (with
//!   boundaries); everything else (`<=>`, `|->`, `:=`, `∈`, `ℙ`, …) is matched
//!   raw. This keeps `mod` from lighting up inside `model`.
//! - **Longest-first.** Symbol groups are sorted by descending byte length so an
//!   ordered-alternation engine (Oniguruma `|`, Vim `\|`) matches `<=>` before
//!   `<`, `|->` before `|`, `:=` before `:`.
//! - **Case-insensitive words.** Event-B reserves its words case-insensitively
//!   (`grammar.pest` uses `^"…"`, [`rossi::builtins::is_builtin`] folds case), so
//!   word groups are emitted lowercased and matched case-insensitively. This also
//!   folds the `TRUE`/`true`, `BOOL`/`bool`, `POW`/`pow` spelling pairs into one.
//! - **One operator colour.** The six semantic operator sub-scopes the old
//!   hand-written grammars used were cosmetic (themes rarely distinguish them)
//!   and a frequent source of cross-category shadowing bugs. We collapse every
//!   operator into a single `keyword.operator` class so a global longest-first
//!   ordering is provably correct.

pub mod emacs;
pub mod input_emacs;
pub mod operators_nvim;
pub mod snippets_emacs;
pub mod snippets_nvim;
pub mod snippets_vscode;
pub mod sublime;
pub mod textmate;
pub mod vim;

use rossi::builtins::BUILTIN_WORDS;
use rossi::keywords::{KEYWORDS, KeywordGroup};
use rossi::operators::{OPERATOR_SPELLINGS, OperatorCategory};

/// How a token group is matched in the generated regex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// Whole-word, case-insensitive, with word boundaries (`mod`, `CONTEXT`).
    Word,
    /// Raw symbols, ordered longest-first (`<=>`, `∈`, `:=`).
    Symbol,
}

/// The coloured token classes the generator emits. Each maps to one TextMate
/// scope; emitters translate it to their own highlight group. Adding a variant
/// makes every emitter's `match` fail to compile until it handles it, so the
/// producer and consumers can never drift via a mistyped scope string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    KeywordControl,
    KeywordOther,
    ConstantLanguage,
    SupportFunction,
    KeywordOperator,
}

impl Scope {
    /// The TextMate / Sublime scope name (a sub-scope of `source.eventb`).
    pub fn textmate(self) -> &'static str {
        match self {
            Scope::KeywordControl => "keyword.control.eventb",
            Scope::KeywordOther => "keyword.other.eventb",
            Scope::ConstantLanguage => "constant.language.eventb",
            Scope::SupportFunction => "support.function.eventb",
            Scope::KeywordOperator => "keyword.operator.eventb",
        }
    }
}

/// One coloured token class: a scope plus its members.
#[derive(Debug, Clone)]
pub struct TokenGroup {
    pub scope: Scope,
    pub kind: MatchKind,
    /// Members, already deduplicated and ordered (lowercased for `Word`,
    /// longest-first for `Symbol`).
    pub members: Vec<String>,
}

impl TokenGroup {
    /// Build this group's match regex for an Oniguruma engine (TextMate, Sublime).
    /// Word groups get case-insensitive word boundaries; symbol groups are a bare
    /// longest-first alternation.
    pub fn regex_oniguruma(&self) -> String {
        let alts = self
            .members
            .iter()
            .map(|m| escape_oniguruma(m))
            .collect::<Vec<_>>()
            .join("|");
        match self.kind {
            MatchKind::Word => format!("(?i)\\b({alts})\\b"),
            MatchKind::Symbol => format!("({alts})"),
        }
    }
}

/// First character is an ASCII letter — matched as a word, not a symbol.
fn is_word(spelling: &str) -> bool {
    spelling
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
}

/// Boolean atoms live in `builtins` (lowercase) but read as constants.
const BOOLEAN_WORDS: &[&str] = &["bool", "true", "false"];

/// The format-neutral token model, built once from the canonical tables.
pub struct Model {
    pub groups: Vec<TokenGroup>,
}

impl Model {
    /// Classify every canonical spelling into exactly one coloured group.
    pub fn build() -> Model {
        let mut keyword_control: Vec<String> = Vec::new();
        let mut keyword_other: Vec<String> = Vec::new();
        let mut operator_words: Vec<String> = Vec::new();
        let mut operator_symbols: Vec<String> = Vec::new();
        let mut constant_words: Vec<String> = Vec::new();
        let mut constant_symbols: Vec<String> = Vec::new();
        let mut builtins: Vec<String> = Vec::new();

        // Structural keywords: section/event headers vs status/inline modifiers.
        for kw in KEYWORDS {
            let bucket = match kw.group {
                KeywordGroup::Status | KeywordGroup::Inline => &mut keyword_other,
                _ => &mut keyword_control,
            };
            for spelling in kw.spellings {
                bucket.push(spelling.to_lowercase());
            }
        }

        // Operators: atoms (∅, ℕ, ℤ …) read as constants; every other spelling
        // is split per-spelling so e.g. Or contributes `∨` (symbol) and `or`
        // (word), PowerSet contributes `ℙ` (symbol) and `POW` (word).
        for op in OPERATOR_SPELLINGS {
            if op.category == OperatorCategory::ExpressionAtom {
                push_spelling(&mut constant_words, &mut constant_symbols, op.unicode);
                push_spelling(&mut constant_words, &mut constant_symbols, op.ascii);
            } else {
                push_spelling(&mut operator_words, &mut operator_symbols, op.unicode);
                push_spelling(&mut operator_words, &mut operator_symbols, op.ascii);
            }
        }

        // Built-ins: skip words already covered as operators (dom, ran, mod …) or
        // constants (nat, int …); booleans read as constants; the rest are
        // support functions/predicates (card, finite, partition …).
        for word in BUILTIN_WORDS {
            let w = word.to_lowercase();
            if operator_words.contains(&w) || constant_words.contains(&w) {
                continue;
            }
            if BOOLEAN_WORDS.contains(&w.as_str()) {
                constant_words.push(w);
            } else {
                builtins.push(w);
            }
        }

        let groups = vec![
            word_group(Scope::KeywordControl, keyword_control),
            word_group(Scope::KeywordOther, keyword_other),
            symbol_group(Scope::ConstantLanguage, constant_symbols),
            word_group(Scope::ConstantLanguage, constant_words),
            word_group(Scope::SupportFunction, builtins),
            symbol_group(Scope::KeywordOperator, operator_symbols),
            word_group(Scope::KeywordOperator, operator_words),
        ];

        Model { groups }
    }
}

/// Route a spelling to its word or symbol bucket.
fn push_spelling(words: &mut Vec<String>, symbols: &mut Vec<String>, spelling: &str) {
    if is_word(spelling) {
        words.push(spelling.to_lowercase());
    } else {
        symbols.push(spelling.to_string());
    }
}

/// Sorted, deduplicated word group (order is cosmetic; sorted for determinism).
fn word_group(scope: Scope, mut members: Vec<String>) -> TokenGroup {
    members.sort();
    members.dedup();
    TokenGroup {
        scope,
        kind: MatchKind::Word,
        members,
    }
}

/// Deduplicated symbol group, ordered longest-first so an ordered-alternation
/// engine matches the longest token (`<=>` before `<`). Ties broken lexically
/// for a stable, byte-reproducible result.
fn symbol_group(scope: Scope, mut members: Vec<String>) -> TokenGroup {
    members.sort();
    members.dedup();
    members.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    TokenGroup {
        scope,
        kind: MatchKind::Symbol,
        members,
    }
}

/// Escape a literal for an Oniguruma regex (TextMate and Sublime).
pub fn escape_oniguruma(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '.' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// An Elisp string literal (with surrounding quotes). Escapes backslash and
/// double quote — the two characters a double-quoted Elisp string must escape.
/// (`regexp-opt` handles any regex escaping for callers that build patterns.)
pub(super) fn elisp_string(s: &str) -> String {
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

/// The files this generator owns, relative to the workspace root.
pub mod paths {
    // Syntax-highlighting grammars.
    pub const TEXTMATE: &str = "editors/vscode/syntaxes/eventb.tmLanguage.json";
    pub const SUBLIME: &str = "editors/sublime/EventB.sublime-syntax";
    pub const VIM: &str = "editors/neovim/syntax/eventb.vim";
    pub const EMACS: &str = "editors/emacs/eventb-mode.el";

    // Snippet libraries.
    pub const SNIPPETS_VSCODE: &str = "editors/vscode/snippets/eventb.json";
    pub const NVIM_SNIPPETS_PACKAGE: &str = "editors/neovim/snippets/package.json";
    pub const NVIM_SNIPPETS_JSON: &str = "editors/neovim/snippets/eventb.json";
    /// Directory holding one yasnippet file per snippet (per the `eventb-mode`
    /// major mode); individual files are `<dir>/<prefix>`.
    pub const EMACS_SNIPPETS_DIR: &str = "editors/emacs/snippets/eventb-mode";

    // Operator/input-method tables shared with the LSP `rossi/operatorTable`.
    pub const NVIM_OPERATORS: &str = "editors/neovim/lua/eventb/operators.lua";
    pub const EMACS_INPUT: &str = "editors/emacs/eventb-input.el";
}

/// Markers delimiting the generated region inside an otherwise hand-maintained
/// file (Vim, Emacs). The line-comment leader differs per language.
pub struct Markers {
    pub begin: &'static str,
    pub end: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(model: &Model, scope: Scope, kind: MatchKind) -> Vec<String> {
        model
            .groups
            .iter()
            .filter(|g| g.scope == scope && g.kind == kind)
            .flat_map(|g| g.members.iter().cloned())
            .collect()
    }

    fn all_members(model: &Model) -> Vec<String> {
        model
            .groups
            .iter()
            .flat_map(|g| g.members.iter().cloned())
            .collect()
    }

    #[test]
    fn every_canonical_spelling_is_classified_once() {
        let model = Model::build();
        let all = all_members(&model);
        // Spot-check representatives of each class.
        assert!(all.contains(&"context".to_string()));
        assert!(all.contains(&"status".to_string()));
        assert!(all.contains(&"theorem".to_string()));
        assert!(all.contains(&"skip".to_string()));
        assert!(all.iter().any(|m| m == "<=>"));
        assert!(all.iter().any(|m| m == "|->"));
        assert!(all.iter().any(|m| m == "∈"));
        assert!(all.iter().any(|m| m == "ℙ"));
        assert!(all.contains(&"pow".to_string()));
        assert!(all.contains(&"card".to_string()));
        assert!(all.contains(&"partition".to_string()));
    }

    #[test]
    fn symbols_are_longest_first() {
        let model = Model::build();
        let ops = group(&model, Scope::KeywordOperator, MatchKind::Symbol);
        let pos = |needle: &str| ops.iter().position(|m| m == needle);
        // Prefix tokens must come after the longer tokens that contain them.
        assert!(pos("<=>") < pos("<="));
        assert!(pos("<=") < pos("<"));
        assert!(pos("|->") < pos("|"));
        assert!(pos(":=") < pos(":"));
    }

    #[test]
    fn no_stale_tokens_leak_in() {
        let model = Model::build();
        let all = all_members(&model);
        // The bogus tokens the hand-written grammars carried must be gone.
        for stale in ["extended", "℘", "⁻¹", "⊲", ">-<", ":<-"] {
            assert!(
                !all.iter().any(|m| m == stale),
                "stale token {stale:?} leaked into the model"
            );
        }
    }

    #[test]
    fn booleans_read_as_constants_not_builtins() {
        let model = Model::build();
        let consts = group(&model, Scope::ConstantLanguage, MatchKind::Word);
        let funcs = group(&model, Scope::SupportFunction, MatchKind::Word);
        for b in ["true", "false", "bool"] {
            assert!(consts.contains(&b.to_string()), "{b} should be a constant");
            assert!(!funcs.contains(&b.to_string()));
        }
        assert!(funcs.contains(&"card".to_string()));
        assert!(funcs.contains(&"finite".to_string()));
    }

    /// The generated Neovim `operators.lua` must expose exactly the rows the LSP
    /// serves over `rossi/operatorTable` — same `ascii != unicode` filter, same
    /// fields — so the editor input method and the language server can never
    /// disagree on the ASCII↔Unicode mapping. We assert by reconstructing the
    /// expected `{ ascii = …, unicode = …, … }` line for every
    /// [`operator_rows`] row and checking the rendered module contains it (in
    /// order), and that the row count matches.
    #[test]
    fn nvim_operators_match_lsp_rows() {
        use rossi_lsp::server::operator_rows;
        let rendered = operators_nvim::render();
        let rows = operator_rows();

        // One emitted row line per LSP row, no more, no fewer.
        let emitted = rendered.matches("{ ascii = ").count();
        assert_eq!(
            emitted,
            rows.len(),
            "operators.lua emitted {emitted} rows but the LSP serves {}",
            rows.len()
        );

        // Every LSP row appears verbatim, in declaration order. Reuse the
        // emitter's own `lua_string` escaping so the needle matches byte-for-byte
        // (operator glyphs include private-use codepoints `{:?}` would escape).
        let s = operators_nvim::lua_string;
        let mut cursor = 0usize;
        for row in &rows {
            let aliases = row
                .aliases
                .iter()
                .map(|a| s(a))
                .collect::<Vec<_>>()
                .join(", ");
            let aliases = if aliases.is_empty() {
                String::new()
            } else {
                format!(" {aliases} ")
            };
            let needle = format!(
                "{{ ascii = {}, unicode = {}, aliases = {{{}}}, symbolic = {}, eager = {} }}",
                s(&row.ascii),
                s(&row.unicode),
                aliases,
                row.symbolic,
                row.eager
            );
            let at = rendered[cursor..].find(&needle).unwrap_or_else(|| {
                panic!("operators.lua is missing row {needle} (or it is out of order)")
            });
            cursor += at + needle.len();
        }
    }

    /// The generated Emacs Quail method must define a `\<key>` rule for every
    /// alias the LSP serves (and a `\<ascii>` rule for every alphabetic row),
    /// each mapping to that row's Unicode glyph — derived from the same
    /// `operator_rows()` filter, so the input method can never drift from the
    /// canonical mapping. Leader-only by design: no bare (non-backslash) rules.
    #[test]
    fn emacs_quail_matches_lsp_rows() {
        use rossi_lsp::server::operator_rows;
        let rendered = input_emacs::render();
        let rows = operator_rows();

        // Reuse the emitter's own `elisp_string` escaping for the Unicode glyph
        // so needles match byte-for-byte regardless of how `{:?}` would render it.
        let es = super::elisp_string;
        let mut expected_rules = 0usize;
        for row in &rows {
            let unicode = es(&row.unicode);
            for alias in &row.aliases {
                let rule = format!("({} {})", es(&format!("\\{alias}")), unicode);
                assert!(
                    rendered.contains(&rule),
                    "eventb-input.el is missing alias rule {rule}"
                );
                expected_rules += 1;
            }
            if !row.symbolic && !row.aliases.contains(&row.ascii) {
                let rule = format!("({} {})", es(&format!("\\{}", row.ascii)), unicode);
                assert!(
                    rendered.contains(&rule),
                    "eventb-input.el is missing alphabetic rule {rule}"
                );
                expected_rules += 1;
            }
        }

        // Exactly the expected number of rules — no extras, in particular no
        // eager non-backslash rules (every rule line starts with `("\\`).
        let emitted = rendered.matches("(\"\\\\").count();
        assert_eq!(
            emitted, expected_rules,
            "eventb-input.el emitted {emitted} rules but {expected_rules} were expected"
        );
    }
}
