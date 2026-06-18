//! Built-in Event-B identifiers (reserved words of the mathematical language).
//!
//! This is the single source of truth for the identifier-shaped vocabulary of
//! the mathematical language: the kernel_lang §2.2 reserved words
//! ([`RESERVED_OPERATOR_WORDS`] / [`RESERVED_ATOM_WORDS`], exact-case, used by
//! the parser to reject them as user identifiers) and the case-folded
//! [`BUILTIN_WORDS`] vocabulary consumed by the editor-grammar generator.
//! [`is_reserved_name`] composes the per-word case rules into the blocklist
//! tools use when *introducing* a name (rename).
//!
//! The non-ASCII symbol atoms (`ℕ ℕ1 ℙ ℙ1 ℤ` …) are operator spellings handled by
//! [`crate::operators`]; only identifier-shaped words live here.
//!
//! [`crate::ast::BuiltinFunction`] and [`crate::ast::BuiltinPredicate`] remain the
//! sources used during parsing; the `builtins_cover_parsed_vocabulary` test keeps
//! this list from drifting away from them and from the operator words.

/// Built-in identifier-shaped vocabulary, lowercase; membership is
/// case-insensitive. This is the *vocabulary* list (editor-grammar generator,
/// coarse membership tests) — it deliberately folds case and so over-matches
/// the grammar's tokens. For "can this string name a user identifier?" use
/// [`is_reserved_name`], which applies each word's own case rule.
pub const BUILTIN_WORDS: &[&str] = &[
    // Built-in types and boolean constants (kernel_lang §2.2)
    "bool",
    "true",
    "false",
    "nat",
    "nat1",
    "int",
    "pow",
    "pow1",
    // Built-in functions (kernel_lang §2.2)
    "card",
    "min",
    "max",
    "id",
    "prj1",
    "prj2",
    "pred",
    "succ",
    // Built-in predicates
    "finite",
    "partition",
    // Operator words (alphabetic operator spellings)
    "dom",
    "ran",
    "mod",
    "not",
    "or",
    "oftype",
    "union",
    "inter",
];

/// Whether `word` is a built-in Event-B identifier (case-insensitive).
pub fn is_builtin(word: &str) -> bool {
    BUILTIN_WORDS.iter().any(|w| w.eq_ignore_ascii_case(word))
}

/// Reserved operator words of the kernel_lang §2.2 list (exact case): the
/// closed-operator words, only meaningful applied to a parenthesized
/// argument (`card(S)`, `dom(r)`, …) or as the infix `mod`. Illegal both as
/// declared names and as plain identifiers inside formulas.
///
/// Reservation is exact-case, matching Rodin's lexer (`isValidIdentifierName`
/// rejects exactly these spellings; `Dom`, `CARD`, `Union` are ordinary
/// identifiers there). The rossi-only ASCII operator spellings (`or`, `not`,
/// `circ`, `oftype`, `POW`, `NAT`, …) are deliberately absent: Rodin is
/// Unicode-only and accepts them as identifiers.
pub const RESERVED_OPERATOR_WORDS: &[&str] = &[
    "card",
    "dom",
    "finite",
    "inter",
    "max",
    "min",
    "mod",
    "partition",
    "ran",
    "union",
];

/// The remaining kernel_lang §2.2 reserved words (exact case): generic atoms
/// and literals that are legal *in formulas* (`id`, `prj1`, `prj2`, `pred`,
/// `succ` parse as bare atoms — the [`crate::ast::AtomicBuiltinKind`] relational
/// atoms; `TRUE`/`FALSE`/`BOOL`/`bool` lex as keyword tokens there) but can
/// never *name* a user identifier. Together with [`RESERVED_OPERATOR_WORDS`]
/// this forms the full §2.2 list.
pub const RESERVED_ATOM_WORDS: &[&str] = &[
    "BOOL", "FALSE", "TRUE", "bool", "id", "pred", "prj1", "prj2", "succ",
];

/// Whether `word` is in the full kernel_lang §2.2 reserved list (exact case).
/// Checked wherever a user identifier is being *named*: declarations,
/// assignment targets, predicate-application heads, recovery, XML import.
pub fn is_reserved_word(word: &str) -> bool {
    is_reserved_operator_word(word) || RESERVED_ATOM_WORDS.contains(&word)
}

/// Whether `word` may not appear as a plain (unapplied) identifier inside a
/// formula (exact-case).
pub fn is_reserved_operator_word(word: &str) -> bool {
    RESERVED_OPERATOR_WORDS.contains(&word)
}

/// Grammar keyword-token spellings (the `kw_*` atoms and quantified-set
/// operators in `grammar.pest`), each lexing in exactly its kernel-language
/// case — uppercase number/set atoms and boolean values/type, lowercase
/// predicate literals and the `bool` conversion. A name spelled like any of
/// these can never be a user identifier; other-case spellings (`Nat`, `pow`,
/// `Bool`) are ordinary identifiers. `TRUE`/`FALSE`/`BOOL`/`bool` also live in
/// [`RESERVED_ATOM_WORDS`] and `union`/`inter` in [`RESERVED_OPERATOR_WORDS`];
/// they are listed here too so the keyword-token case rule sits in one place.
/// Structural keywords are covered separately by [`crate::keywords::is_keyword`].
const KEYWORD_TOKEN_WORDS: &[&str] = &[
    "NAT", "NAT1", "INT", "UNION", "INTER", "BOOL", "TRUE", "FALSE", "bool", "true", "false",
];

/// ASCII operator spellings that are tokens only in rossi's *textual syntax*
/// (its documented ASCII extension); the official language is Unicode-only
/// (`∨ ¬ ∘ ⦂ ℙ`), so Rodin accepts these words as ordinary identifiers and
/// rossi's parser does too. Bare uses even round-trip — but in applied or
/// operator position the spelling lexes as the operator and the formula
/// *silently changes meaning*: a user function `POW` applied as `POW(S)`
/// parses as the powerset `ℙ(S)`, `not(x) = 1` as `¬(x = 1)`. Exact-case,
/// like the tokens: `OR`, `Circ`, `pow` are unaffected identifiers.
const ASCII_OPERATOR_WORDS: &[&str] = &["circ", "not", "oftype", "or", "POW", "POW1"];

/// Whether `word` cannot (or cannot safely) *name* a user identifier in
/// rossi's textual syntax — the blocklist for tools that introduce names,
/// e.g. rename. Every word is matched exact-case, matching its grammar token:
/// - kernel_lang §2.2 reserved words ([`is_reserved_word`]; `Dom`, `Card` stay
///   usable, matching the parser);
/// - grammar keyword tokens (`KEYWORD_TOKEN_WORDS`; `Nat`, `pow` stay usable);
/// - rossi's ASCII operator spellings (`ASCII_OPERATOR_WORDS`).
///
/// Callers should also reject structural keywords via
/// [`crate::keywords::is_keyword`] (case-insensitive, like their tokens).
pub fn is_reserved_name(word: &str) -> bool {
    is_reserved_word(word)
        || KEYWORD_TOKEN_WORDS.contains(&word)
        || ASCII_OPERATOR_WORDS.contains(&word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AtomicBuiltinKind, BuiltinFunction, BuiltinPredicate};

    #[test]
    fn is_builtin_is_case_insensitive() {
        assert!(is_builtin("card"));
        assert!(is_builtin("CARD"));
        assert!(is_builtin("Partition"));
        assert!(!is_builtin("not_a_builtin"));
    }

    /// Every `BuiltinFunction` variant. These are all *closed* (paren-mandating)
    /// forms — listing one here pins it against the reserved-operator-word list
    /// so the facade can't silently lag the parser vocabulary. The generic
    /// relational atoms live in [`AtomicBuiltinKind`] / [`ALL_ATOMIC_BUILTINS`].
    const ALL_BUILTIN_FUNCTIONS: &[BuiltinFunction] = &[
        BuiltinFunction::Card,
        BuiltinFunction::Min,
        BuiltinFunction::Max,
        BuiltinFunction::Union,
        BuiltinFunction::Inter,
    ];

    const ALL_BUILTIN_PREDICATES: &[BuiltinPredicate] =
        &[BuiltinPredicate::Finite, BuiltinPredicate::Partition];

    #[test]
    fn builtins_cover_parsed_vocabulary() {
        // Every name the parser recognizes for a built-in must be reserved, so the
        // facade can never drift from what the grammar actually accepts.
        for f in ALL_BUILTIN_FUNCTIONS {
            assert!(
                is_builtin(f.name()),
                "BuiltinFunction {:?} missing from BUILTIN_WORDS",
                f
            );
        }
        for p in ALL_BUILTIN_PREDICATES {
            assert!(
                is_builtin(p.name()),
                "BuiltinPredicate {:?} missing from BUILTIN_WORDS",
                p
            );
        }
        // Alphabetic operator words that double as reserved identifiers.
        for w in ["not", "or", "dom", "ran", "mod", "oftype"] {
            assert!(
                is_builtin(w),
                "operator word {w:?} missing from BUILTIN_WORDS"
            );
        }
        // The spec's `pred`/`succ`, previously missing from rename's blocklist.
        assert!(is_builtin("pred"));
        assert!(is_builtin("succ"));
    }

    #[test]
    fn reserved_words_are_exact_case() {
        assert!(is_reserved_word("dom"));
        assert!(is_reserved_word("TRUE"));
        assert!(is_reserved_word("pred"));
        // Rodin reserves exact spellings only.
        assert!(!is_reserved_word("Dom"));
        assert!(!is_reserved_word("DOM"));
        assert!(!is_reserved_word("true"));
        // rossi ASCII extensions are not Rodin-reserved.
        assert!(!is_reserved_word("or"));
        assert!(!is_reserved_word("POW"));

        assert!(is_reserved_operator_word("dom"));
        assert!(is_reserved_operator_word("mod"));
        // Generic atoms are legal bare expressions.
        assert!(!is_reserved_operator_word("id"));
        assert!(!is_reserved_operator_word("pred"));
        assert!(!is_reserved_operator_word("succ"));
    }

    #[test]
    fn reserved_name_follows_each_words_token_case_rule() {
        // §2.2 reserved words: exact-case only.
        assert!(is_reserved_name("dom"));
        assert!(is_reserved_name("card"));
        assert!(is_reserved_name("pred"));
        assert!(!is_reserved_name("Dom"));
        assert!(!is_reserved_name("CARD"));
        // Grammar keyword tokens: exact-case only. Uppercase number/set atoms
        // and boolean values/type; lowercase predicate literals and `bool`.
        for w in [
            "NAT", "NAT1", "INT", "UNION", "INTER", "TRUE", "FALSE", "BOOL", "true", "false",
            "bool",
        ] {
            assert!(is_reserved_name(w), "{w:?} lexes as a token");
        }
        // Other-case spellings are ordinary identifiers now.
        for w in ["Nat", "nat", "Int", "Union", "True", "Bool", "FALSe"] {
            assert!(!is_reserved_name(w), "{w:?} must be a usable identifier");
        }
        // ASCII operator spellings: exact-case only.
        assert!(is_reserved_name("or"));
        assert!(is_reserved_name("circ"));
        assert!(is_reserved_name("POW"));
        assert!(!is_reserved_name("OR"));
        assert!(!is_reserved_name("Circ"));
        assert!(!is_reserved_name("pow"));
        // Ordinary identifiers pass.
        assert!(!is_reserved_name("count"));
        assert!(!is_reserved_name("domain"));
    }

    #[test]
    fn reserved_name_covers_the_grammars_word_vocabulary() {
        // Every word the (case-folded) vocabulary list knows is blocked in at
        // least its canonical spelling — is_reserved_name must never be more
        // permissive than the grammar's own tokens.
        for w in BUILTIN_WORDS {
            assert!(
                is_reserved_name(w) || is_reserved_name(&w.to_uppercase()),
                "vocabulary word {w:?} unblocked in every spelling"
            );
        }
    }

    #[test]
    fn reserved_sets_are_consistent() {
        // The two exact-case lists are disjoint halves of §2.2, and
        // everything reserved is covered by the (case-insensitive) rename
        // blocklist.
        for w in RESERVED_OPERATOR_WORDS {
            assert!(
                !RESERVED_ATOM_WORDS.contains(w),
                "{w:?} must live in exactly one reserved list"
            );
        }
        for w in RESERVED_OPERATOR_WORDS.iter().chain(RESERVED_ATOM_WORDS) {
            assert!(
                is_builtin(w),
                "reserved word {w:?} missing from BUILTIN_WORDS"
            );
        }
    }

    #[test]
    fn reserved_operator_words_cover_closed_builtins() {
        // Every closed builtin (applied form only) must be a reserved operator
        // word, so adding one can't reintroduce the issue-#30 inconsistency
        // (applied form resolves, bare form silently parses as an identifier).
        for &f in ALL_BUILTIN_FUNCTIONS {
            assert!(
                is_reserved_operator_word(f.name()),
                "BuiltinFunction {f:?} missing from RESERVED_OPERATOR_WORDS"
            );
            assert!(
                is_reserved_word(f.name()),
                "BuiltinFunction {f:?} must be un-namable as a user identifier"
            );
        }
        for p in ALL_BUILTIN_PREDICATES {
            assert!(
                is_reserved_operator_word(p.name()),
                "BuiltinPredicate {p:?} missing from RESERVED_OPERATOR_WORDS"
            );
        }
    }

    #[test]
    fn atomic_builtins_are_reserved_atoms() {
        // `AtomicBuiltinKind` is the SSOT for the relational atoms. Pin every
        // variant against the reserved-word vocabulary so they can't drift:
        // each is a legal *bare* expression (NOT a reserved operator word) yet
        // can never *name* a user identifier (it is a reserved atom word).
        for a in AtomicBuiltinKind::ALL {
            assert_eq!(AtomicBuiltinKind::from_name(a.name()), Some(a));
            assert!(
                !is_reserved_operator_word(a.name()),
                "relational atom {a:?} must be legal bare"
            );
            assert!(
                RESERVED_ATOM_WORDS.contains(&a.name()),
                "relational atom {a:?} must be un-namable"
            );
            assert!(is_reserved_word(a.name()));
            assert!(is_builtin(a.name()));
        }
    }

    /// SSOT guard: every word treated as an ASCII operator spelling is the
    /// ASCII form of a real operator, so this lexing-collision blocklist cannot
    /// drift from the operator tables.
    #[test]
    fn ascii_operator_words_are_operator_spellings() {
        for word in ASCII_OPERATOR_WORDS {
            assert!(
                crate::operators::OPERATOR_SPELLINGS
                    .iter()
                    .any(|op| op.ascii == *word),
                "ASCII_OPERATOR_WORDS entry {word:?} is not an operator's ASCII spelling"
            );
        }
    }
}
