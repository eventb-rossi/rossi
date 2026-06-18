//! Editor syntax-highlighting grammars, generated from the canonical token
//! tables so they can never drift from the parser.
//!
//! The single sources of truth are [`rossi::keywords::KEYWORDS`],
//! [`rossi::operators::OPERATOR_SPELLINGS`], and [`rossi::builtins::BUILTIN_WORDS`],
//! which are themselves kept in sync with `crates/rossi/src/grammar.pest` and
//! `docs/EVENTB_LANGUAGE_REFERENCE.md` by unit tests in those modules. Here we
//! fold them into one format-neutral [`Model`] of token *groups*, then render it
//! into each editor's native grammar (TextMate, Sublime, Vim, Emacs, Zed).
//!
//! ## Why a highlighter can be generated but a parser cannot
//!
//! TextMate / Sublime / Vim-syntax / Emacs-font-lock are all *lexical* (regex)
//! highlighters: every distinction they draw is token data, which is exactly
//! what the tables hold. The tree-sitter consumers (Zed, nvim-treesitter,
//! Helix) need a tree-sitter grammar, but only a *lexical* one: the [`zed`]
//! emitter generates the token rules (the regex alternations that recognise
//! each coloured class) into the standalone grammar's `grammar.js` and the
//! node→capture lines into its `highlights.scm` (both as marked regions inside
//! hand-maintained scaffolding) — so the table-derived part stays generated and
//! the (small) parser scaffold does not pretend to parse Event-B.
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
//! - **Per-group case sensitivity, mirroring `grammar.pest`.** Only the
//!   structural keywords are case-insensitive tokens (`^"context"` …) and so
//!   match case-insensitively. Every other word — the literal atoms
//!   (`NAT`/`INT`, `TRUE`/`true`/`bool`/…), the `UNION`/`INTER` quantifier
//!   words, the mathematical operator words (`dom`, `ran`, `mod`, `or`, …),
//!   the builtins (`card`, `finite`, …) and `POW`/`POW1` — is an exact-case
//!   token, so `DOM`, `Card`, `pow`, `nat`, `Union` are ordinary identifiers
//!   ([`rossi::builtins::RESERVED_OPERATOR_WORDS`] / `RESERVED_ATOM_WORDS` are
//!   exact-case, Rodin parity). Those groups match exactly, in the canonical
//!   spelling from the tables, with both cases of the boolean atoms emitted
//!   (`TRUE` the value, `true` the predicate literal).
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
pub mod zed;

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
    /// `Word` groups only: whether `grammar.pest` lexes these spellings
    /// case-insensitively (`^"…"` rules). Exact-case groups carry the
    /// canonical spelling (`POW`, `dom`) and must not fold — `DOM`, `Card`,
    /// `pow` are ordinary identifiers. Always `false` for `Symbol` groups.
    pub case_insensitive: bool,
    /// Members, already deduplicated and ordered (sorted for `Word`,
    /// longest-first for `Symbol`).
    pub members: Vec<String>,
}

impl TokenGroup {
    /// Build this group's match regex for an Oniguruma engine (TextMate, Sublime).
    /// Word groups get word boundaries (case-insensitive only when the grammar's
    /// tokens are); symbol groups are a bare longest-first alternation.
    pub fn regex_oniguruma(&self) -> String {
        let alts = self
            .members
            .iter()
            .map(|m| escape_oniguruma(m))
            .collect::<Vec<_>>()
            .join("|");
        match (self.kind, self.case_insensitive) {
            (MatchKind::Word, true) => format!("(?i)\\b({alts})\\b"),
            (MatchKind::Word, false) => format!("\\b({alts})\\b"),
            (MatchKind::Symbol, _) => format!("({alts})"),
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

/// The boolean atoms, every canonical spelling in its own kernel-language
/// case: the values `TRUE`/`FALSE` and the type `BOOL` (uppercase), the
/// predicate literals `true`/`false` and the `bool(P)` conversion (lowercase).
/// They read as constants. `BUILTIN_WORDS` over-folds them to lowercase, so the
/// generator sources them here instead; the `boolean_words_are_reserved_tokens`
/// test pins each one to the parser's exact-case keyword vocabulary.
const BOOLEAN_WORDS: &[&str] = &["TRUE", "FALSE", "BOOL", "true", "false", "bool"];

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
        // The only case-insensitive words in the grammar (`^"context"` …).
        for kw in KEYWORDS {
            let bucket = match kw.group {
                KeywordGroup::Status | KeywordGroup::Inline => &mut keyword_other,
                _ => &mut keyword_control,
            };
            for spelling in kw.spellings {
                bucket.push(spelling.to_lowercase());
            }
        }

        // The boolean atoms, both cases (`TRUE` value vs `true` predicate
        // literal): exact-case constants the lowercase-only `BUILTIN_WORDS`
        // cannot supply.
        for word in BOOLEAN_WORDS {
            constant_words.push((*word).to_string());
        }

        // Operators: atoms (∅, ℕ, NAT …) read as constants; every other
        // spelling is split per-spelling so e.g. Or contributes `∨` (symbol)
        // and `or` (word). All are exact-case tokens kept in their canonical
        // spelling (`NAT`, `UNION`, `dom`, `POW`) — Rodin reserves exact
        // spellings, so `nat`, `Union`, `DOM`, `pow` are ordinary identifiers.
        for op in OPERATOR_SPELLINGS {
            let is_atom = op.category == OperatorCategory::ExpressionAtom;
            let (words, symbols) = if is_atom {
                (&mut constant_words, &mut constant_symbols)
            } else {
                (&mut operator_words, &mut operator_symbols)
            };
            push_spelling(words, symbols, op.unicode);
            push_spelling(words, symbols, op.ascii);
        }

        // Built-ins: skip words already covered as operators (dom, ran, POW …),
        // number-set atoms (nat, int …) or booleans — matched case-insensitively
        // so a lowercase vocabulary entry resolves to its uppercase/exact
        // canonical spelling. The rest are support functions/predicates (card,
        // finite, partition …), exact-case like their grammar tokens.
        for word in BUILTIN_WORDS {
            let covered = operator_words
                .iter()
                .chain(&constant_words)
                .any(|o| o.eq_ignore_ascii_case(word));
            if !covered {
                builtins.push((*word).to_string());
            }
        }

        let groups = vec![
            word_group(Scope::KeywordControl, keyword_control, true),
            word_group(Scope::KeywordOther, keyword_other, true),
            symbol_group(Scope::ConstantLanguage, constant_symbols),
            word_group(Scope::ConstantLanguage, constant_words, false),
            word_group(Scope::SupportFunction, builtins, false),
            symbol_group(Scope::KeywordOperator, operator_symbols),
            word_group(Scope::KeywordOperator, operator_words, false),
        ];

        Model { groups }
    }
}

/// Route a spelling to its word or symbol bucket, keeping its canonical
/// spelling — the operator words are exact-case (`POW` must not become `pow`),
/// and the structural keywords (the only case-insensitive words) are folded by
/// their own loop, not here.
fn push_spelling(words: &mut Vec<String>, symbols: &mut Vec<String>, spelling: &str) {
    if is_word(spelling) {
        words.push(spelling.to_string());
    } else {
        symbols.push(spelling.to_string());
    }
}

/// Sorted, deduplicated word group (order is cosmetic; sorted for determinism).
fn word_group(scope: Scope, mut members: Vec<String>, case_insensitive: bool) -> TokenGroup {
    members.sort();
    members.dedup();
    TokenGroup {
        scope,
        kind: MatchKind::Word,
        case_insensitive,
        members,
    }
}

/// Order two spellings longest-first, ties broken lexically — so an engine that
/// tries alternatives left-to-right (Oniguruma/Vim `|`, or a JS `(?:…)` regex)
/// matches the longest token first (`<=>` before `<`, `events` before `event`).
/// The lexical tie-break keeps the result stable and byte-reproducible.
pub(super) fn longest_first(a: &str, b: &str) -> std::cmp::Ordering {
    b.len().cmp(&a.len()).then_with(|| a.cmp(b))
}

/// Deduplicated symbol group, ordered longest-first so an ordered-alternation
/// engine matches the longest token (`<=>` before `<`).
fn symbol_group(scope: Scope, mut members: Vec<String>) -> TokenGroup {
    members.sort();
    members.dedup();
    members.sort_by(|a, b| longest_first(a, b));
    TokenGroup {
        scope,
        kind: MatchKind::Symbol,
        case_insensitive: false,
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
    /// The standalone tree-sitter grammar (published as
    /// `eventb-rossi/tree-sitter-eventb`, vendored here as a submodule). The token
    /// rules live in a generated region; the surrounding scaffold — and any future
    /// hand-written structural rules — are hand-maintained (so this is a region
    /// target). The Zed extension and nvim-treesitter/Helix all consume this repo.
    pub const TS_GRAMMAR: &str = "editors/tree-sitter-eventb/grammar.js";
    /// The standalone grammar's highlight queries: node→capture mapping. The
    /// generated token captures live in a region so hand-written captures for
    /// future structural nodes can sit outside it (region target). The one
    /// hand-editable highlights file — Zed's bundled copy derives from it via
    /// [`COPIES`].
    pub const TS_HIGHLIGHTS: &str = "editors/tree-sitter-eventb/queries/highlights.scm";
    /// Canonical token classification exported as data: `{ node_name: [spellings] }`,
    /// generated whole-file and byte-checked here, then consumed by the standalone
    /// repo's behavioral test so it can verify the *built* parser still classifies
    /// every canonical spelling correctly — a contract that survives the grammar
    /// being hand-extended (the test asserts behavior, not source text).
    pub const TS_TOKENS: &str = "editors/tree-sitter-eventb/test/tokens.json";
    /// Zed's bundled copy of the highlight queries (Zed loads queries from the
    /// extension's `languages/` dir, not the grammar repo). A [`COPIES`] entry.
    pub const ZED_HIGHLIGHTS: &str = "editors/zed/languages/eventb/highlights.scm";
    /// Examples directory in the standalone grammar repo. Every file in it is a
    /// [`COPIES`] destination, so orphans are pruned like any fully-generated dir.
    pub const TS_EXAMPLES_DIR: &str = "editors/tree-sitter-eventb/examples";

    // Snippet libraries.
    pub const SNIPPETS_VSCODE: &str = "editors/vscode/snippets/eventb.json";
    pub const NVIM_SNIPPETS_PACKAGE: &str = "editors/neovim/snippets/package.json";
    pub const NVIM_SNIPPETS_JSON: &str = "editors/neovim/snippets/eventb.json";
    /// Zed's copy of the VS Code snippet JSON (same format); the filename is the
    /// lowercased Zed language name (`Event-B` → `event-b.json`). A [`COPIES`] entry.
    pub const ZED_SNIPPETS: &str = "editors/zed/snippets/event-b.json";

    /// Verbatim copies, `(source, destination)`: the destination file carries the
    /// source's content byte-for-byte. A generated source is copied from its
    /// freshly rendered content (never from disk, so a copy cannot lag its source
    /// within one run); a non-generated source is read from disk. Why each exists:
    /// - Zed loads queries and snippets only from the extension's own
    ///   directories, never from the grammar repo, so it bundles copies of
    ///   [`TS_HIGHLIGHTS`] and [`SNIPPETS_VSCODE`].
    /// - The standalone grammar repo ships example models (also the prepared
    ///   Linguist samples) copied from `crates/rossi/examples`.
    pub const COPIES: &[(&str, &str)] = &[
        (SNIPPETS_VSCODE, ZED_SNIPPETS),
        (TS_HIGHLIGHTS, ZED_HIGHLIGHTS),
        (
            "crates/rossi/examples/bank_account_machine.eventb",
            "editors/tree-sitter-eventb/examples/bank_account_machine.eventb",
        ),
        (
            "crates/rossi/examples/counter.eventb",
            "editors/tree-sitter-eventb/examples/counter.eventb",
        ),
        (
            "crates/rossi/examples/simple_machine.eventb",
            "editors/tree-sitter-eventb/examples/simple_machine.eventb",
        ),
        (
            "crates/rossi/examples/traffic_light_machine.eventb",
            "editors/tree-sitter-eventb/examples/traffic_light_machine.eventb",
        ),
    ];
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
        // Exact-case groups keep the canonical spelling from the tables.
        assert!(all.contains(&"POW".to_string()));
        assert!(!all.contains(&"pow".to_string()));
        assert!(all.contains(&"card".to_string()));
        assert!(all.contains(&"partition".to_string()));
    }

    #[test]
    fn word_case_policy_mirrors_the_grammar() {
        // The only case-insensitive tokens in grammar.pest (`^"…"`) are the
        // structural keywords. Everything else — literal atoms, the boolean
        // values/literals, UNION/INTER, the math operator words, builtins,
        // POW/POW1 — is exact-case.
        let model = Model::build();
        let find = |member: &str| {
            model
                .groups
                .iter()
                .find(|g| g.members.iter().any(|m| m == member))
                .unwrap_or_else(|| panic!("{member:?} not classified"))
        };
        for ci in ["context", "skip"] {
            assert!(find(ci).case_insensitive, "{ci:?} must match (?i)");
        }
        for exact in [
            "true", "TRUE", "BOOL", "NAT1", "UNION", "INTER", "dom", "ran", "mod", "or", "POW",
            "POW1", "card", "finite",
        ] {
            assert!(
                !find(exact).case_insensitive,
                "{exact:?} must match exact-case (Rodin reserves exact spellings; \
                 `nat`, `Union`, `DOM`, `Card`, `pow` are ordinary identifiers)"
            );
        }
        // The (?i) regex carries the flag; the exact regex must not.
        assert!(find("dom").regex_oniguruma().starts_with("\\b("));
        assert!(find("context").regex_oniguruma().starts_with("(?i)"));
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
        // Both cases: the values/type `TRUE`/`FALSE`/`BOOL` and the predicate
        // literals / conversion `true`/`false`/`bool`.
        for b in ["TRUE", "FALSE", "BOOL", "true", "false", "bool"] {
            assert!(consts.contains(&b.to_string()), "{b} should be a constant");
            assert!(!funcs.contains(&b.to_string()));
        }
        assert!(funcs.contains(&"card".to_string()));
        assert!(funcs.contains(&"finite".to_string()));
        // The constant words are exact-case — the grammar split `TRUE` (the
        // value) from `true` (the predicate literal), so neither folds.
        let const_words = model
            .groups
            .iter()
            .find(|g| g.scope == Scope::ConstantLanguage && g.kind == MatchKind::Word)
            .expect("constant word group");
        assert!(!const_words.case_insensitive);
    }

    /// SSOT guard: every boolean spelling the generator emits is one of the
    /// parser's exact-case keyword tokens, so [`BOOLEAN_WORDS`] cannot drift
    /// from the grammar's reserved boolean vocabulary (`builtins::is_reserved_name`).
    #[test]
    fn boolean_words_are_reserved_tokens() {
        for w in BOOLEAN_WORDS {
            assert!(
                rossi::builtins::is_reserved_name(w),
                "BOOLEAN_WORDS entry {w:?} is not a reserved keyword token"
            );
        }
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
