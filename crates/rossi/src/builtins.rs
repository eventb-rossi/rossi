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

    #[test]
    fn builtins_cover_parsed_vocabulary() {
        // Every name the parser recognizes for a built-in must be reserved, so the
        // facade can never drift from what the grammar actually accepts.
        let functions = [
            BuiltinFunction::Card,
            BuiltinFunction::Min,
            BuiltinFunction::Max,
            BuiltinFunction::Id,
            BuiltinFunction::Prj1,
            BuiltinFunction::Prj2,
        ];
        for f in functions {
            assert!(
                is_builtin(f.name()),
                "BuiltinFunction {:?} missing from BUILTIN_WORDS",
                f
            );
        }
        for p in [BuiltinPredicate::Finite, BuiltinPredicate::Partition] {
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
}
