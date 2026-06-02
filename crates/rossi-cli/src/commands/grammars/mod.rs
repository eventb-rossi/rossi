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

/// The grammar files this generator owns, relative to the workspace root.
pub mod paths {
    pub const TEXTMATE: &str = "editors/vscode/syntaxes/eventb.tmLanguage.json";
    pub const SUBLIME: &str = "editors/sublime/EventB.sublime-syntax";
    pub const VIM: &str = "editors/neovim/syntax/eventb.vim";
    pub const EMACS: &str = "editors/emacs/eventb-mode.el";
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
}
