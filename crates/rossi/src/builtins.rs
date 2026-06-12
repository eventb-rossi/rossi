//! Built-in Event-B identifiers (reserved words of the mathematical language).
//!
//! This is the single source of truth for the built-in type/constant, function,
//! and predicate identifiers that may not be used as user identifiers. The word
//! list is taken from *The Event-B Mathematical Language* (`docs/kernel_lang.pdf`,
//! §2.2, p.4) and the alphabetic spellings of operator words.
//!
//! The non-ASCII symbol atoms (`ℕ ℕ1 ℙ ℙ1 ℤ` …) are operator spellings handled by
//! [`crate::operators`]; only identifier-shaped words live here.
//!
//! [`crate::ast::BuiltinFunction`] and [`crate::ast::BuiltinPredicate`] remain the
//! sources used during parsing; the `builtins_cover_parsed_vocabulary` test keeps
//! this list from drifting away from them and from the operator words.

/// Reserved built-in identifiers, lowercase. Membership is case-insensitive.
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
/// `succ` parse as bare atoms; `TRUE`/`FALSE`/`BOOL`/`bool` lex as keyword
/// tokens there) but can never *name* a user identifier. Together with
/// [`RESERVED_OPERATOR_WORDS`] this forms the full §2.2 list.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BuiltinFunction, BuiltinPredicate};

    #[test]
    fn is_builtin_is_case_insensitive() {
        assert!(is_builtin("card"));
        assert!(is_builtin("CARD"));
        assert!(is_builtin("Partition"));
        assert!(!is_builtin("not_a_builtin"));
    }

    /// Every `BuiltinFunction` variant, with `true` for the closed
    /// (paren-mandating) forms and `false` for the generic atoms. The match
    /// in [`is_closed_builtin`] is exhaustive, so adding a variant without
    /// classifying it here is a compile error — the forcing function that
    /// keeps the reserved lists from silently lagging the parser vocabulary.
    const ALL_BUILTIN_FUNCTIONS: &[BuiltinFunction] = &[
        BuiltinFunction::Card,
        BuiltinFunction::Min,
        BuiltinFunction::Max,
        BuiltinFunction::Id,
        BuiltinFunction::Prj1,
        BuiltinFunction::Prj2,
        BuiltinFunction::Union,
        BuiltinFunction::Inter,
    ];

    const ALL_BUILTIN_PREDICATES: &[BuiltinPredicate] =
        &[BuiltinPredicate::Finite, BuiltinPredicate::Partition];

    fn is_closed_builtin(f: BuiltinFunction) -> bool {
        match f {
            BuiltinFunction::Card
            | BuiltinFunction::Min
            | BuiltinFunction::Max
            | BuiltinFunction::Union
            | BuiltinFunction::Inter => true,
            BuiltinFunction::Id | BuiltinFunction::Prj1 | BuiltinFunction::Prj2 => false,
        }
    }

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
        // Every *closed* builtin the parser resolves (applied form only) must
        // be a reserved operator word, so adding a builtin can't reintroduce
        // the issue-#30 inconsistency (applied form resolves, bare form
        // silently parses as an identifier); the generic atoms must stay
        // legal bare yet un-namable. Classification comes from the
        // exhaustive `is_closed_builtin`.
        for &f in ALL_BUILTIN_FUNCTIONS {
            assert_eq!(
                is_reserved_operator_word(f.name()),
                is_closed_builtin(f),
                "BuiltinFunction {f:?} misclassified in RESERVED_OPERATOR_WORDS"
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
}
