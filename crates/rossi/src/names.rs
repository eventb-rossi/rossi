//! Lexical name classes — the single source of truth for what counts as a
//! valid name, shared by the text grammar (kept in sync by a parity test),
//! the XML importer, and the LSP.
//!
//! Event-B distinguishes two kinds of names (issue #28):
//!
//! - **Mathematical identifiers** — carrier sets, constants, variables, event
//!   parameters, and every name inside a formula. kernel_lang §2.2 defines
//!   these per the Unicode identifier rules (no hyphens); Rodin enforces the
//!   same via `Character.isJavaIdentifierStart/Part`. We restrict to ASCII,
//!   with an optional single trailing `'` for a primed after-state variable
//!   (`x'`) — Rodin's lexer attaches one prime to a plain identifier, so the
//!   prime is a suffix, never interior or repeated.
//!
//! - **Component names** — machine/context names, REFINES/SEES/EXTENDS
//!   targets, and event names. In Rodin these are file names and labels:
//!   bare strings never parsed as formulas, so hyphens are common in real
//!   models (`A-C0`, `CTX-1`) but primes never appear. Our textual format
//!   must lex them after `MACHINE`/`EVENT`/…, so we accept the prime-less math
//!   charset extended with interior `-` separators (the `component_name`
//!   grammar rule).
//!
//! Reserved-word checks (`dom`, `card`, …) are positional and live in
//! [`crate::builtins`]; this module is purely lexical.

/// First character of a mathematical identifier (and of a component name).
pub fn is_math_identifier_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Non-first character of a mathematical identifier's core. The prime suffix is
/// positional (see [`check_math_identifier`]), so `'` is not a part char.
pub fn is_math_identifier_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Regex character-class *body* (the part between `[` and `]`) matching
/// [`is_math_identifier_start`] — the first char of any identifier or
/// component name. The canonical regex spelling of the name charset for
/// editor-highlighter generators (consumed today by the Emacs generator),
/// kept in step with the predicate by a parity test. A generator wraps these
/// classes in its own regex flavor.
pub const IDENT_START_CLASS: &str = "A-Za-z_";

/// Regex character-class body matching [`is_math_identifier_part`] — the
/// non-first chars of an identifier core and the chars of a component name's
/// `-`-joined segments. The math-identifier prime suffix is positional and not
/// part of this class. See [`IDENT_START_CLASS`].
pub const IDENT_PART_CLASS: &str = "A-Za-z0-9_";

/// Why a name failed [`check_math_identifier`] / [`check_component_name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    BadStart(char),
    BadChar(char),
    /// A `-` not followed by a letter, digit or `_` — i.e. a trailing or
    /// doubled hyphen. Such a name could never be re-lexed by the text
    /// grammar's `component_name` rule.
    EmptyHyphenSegment,
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::Empty => write!(f, "empty"),
            NameError::BadStart(c) => {
                write!(f, "must start with ASCII letter or '_', got {c:?}")
            }
            NameError::BadChar(c) => write!(f, "contains unsupported character {c:?}"),
            NameError::EmptyHyphenSegment => write!(
                f,
                "'-' must be followed by a letter, digit or '_' (no trailing or doubled '-')"
            ),
        }
    }
}

/// Check a mathematical identifier: `(ASCII_ALPHA | "_") word_char* "'"?`.
/// A single optional trailing prime denotes an Event-B after-state variable
/// (`x'`); it attaches only at the end, so `x''` and `x'y` are rejected — this
/// matches Rodin's lexer, where the prime follows a plain identifier. Mirrors
/// the grammar's `identifier` rule exactly (parity-tested).
pub fn check_math_identifier(s: &str) -> Result<(), NameError> {
    let core = s.strip_suffix('\'').unwrap_or(s);
    let mut chars = core.chars();
    let first = chars.next().ok_or(NameError::Empty)?;
    if !is_math_identifier_start(first) {
        return Err(NameError::BadStart(first));
    }
    for c in chars {
        if !is_math_identifier_part(c) {
            return Err(NameError::BadChar(c));
        }
    }
    Ok(())
}

/// Check a component name: the prime-less identifier core optionally extended
/// with `-`-joined segments (`ident_core ("-" word_char+)*`). Segments after a
/// `-` may start with a digit (`ENV_C-1`); file labels carry no prime. Mirrors
/// the grammar's `component_name` rule exactly (parity-tested).
pub fn check_component_name(s: &str) -> Result<(), NameError> {
    let mut chars = s.chars().peekable();
    let first = chars.next().ok_or(NameError::Empty)?;
    if !is_math_identifier_start(first) {
        return Err(NameError::BadStart(first));
    }
    while let Some(c) = chars.next() {
        if c == '-' {
            // Hyphens are separators: each must open a non-empty segment.
            match chars.peek() {
                Some(&next) if is_math_identifier_part(next) => {}
                _ => return Err(NameError::EmptyHyphenSegment),
            }
        } else if !is_math_identifier_part(c) {
            return Err(NameError::BadChar(c));
        }
    }
    Ok(())
}

/// `true` iff [`check_math_identifier`] accepts `s`.
pub fn is_valid_math_identifier(s: &str) -> bool {
    check_math_identifier(s).is_ok()
}

/// `true` iff [`check_component_name`] accepts `s`.
pub fn is_valid_component_name(s: &str) -> bool {
    check_component_name(s).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_identifiers() {
        // `x'` carries a lone trailing prime (after-state variable).
        for ok in ["x", "_x", "x'", "events_of_partition", "A1", "machine"] {
            assert!(is_valid_math_identifier(ok), "{ok:?} should be valid");
        }
        for (bad, err) in [
            ("", NameError::Empty),
            ("1a", NameError::BadStart('1')),
            ("'a", NameError::BadStart('\'')),
            ("-a", NameError::BadStart('-')),
            ("a-b", NameError::BadChar('-')),
            ("a b", NameError::BadChar(' ')),
            ("ä", NameError::BadStart('ä')),
            // The prime is a single suffix: no doubled or interior prime.
            ("x''", NameError::BadChar('\'')),
            ("x'y", NameError::BadChar('\'')),
        ] {
            assert_eq!(check_math_identifier(bad), Err(err), "{bad:?}");
        }
    }

    #[test]
    fn component_names() {
        for ok in [
            "M-ALPHA",
            "CTX-1",
            "do-step",
            "end-to-end",
            "a-1-2",
            "x",
            "_x",
        ] {
            assert!(is_valid_component_name(ok), "{ok:?} should be valid");
        }
        for (bad, err) in [
            ("", NameError::Empty),
            ("-a", NameError::BadStart('-')),
            ("a-", NameError::EmptyHyphenSegment),
            ("a--b", NameError::EmptyHyphenSegment),
            ("a-b-", NameError::EmptyHyphenSegment),
            ("1a", NameError::BadStart('1')),
            ("a b", NameError::BadChar(' ')),
            ("a.b", NameError::BadChar('.')),
            // File labels carry no prime — the after-state suffix is a
            // math-identifier feature only.
            ("x'", NameError::BadChar('\'')),
            ("a-b'", NameError::BadChar('\'')),
            ("a-'", NameError::EmptyHyphenSegment),
        ] {
            assert_eq!(check_component_name(bad), Err(err), "{bad:?}");
        }
    }

    /// Parity for the highlighter char-class consts: their literal spelling is
    /// pinned (so the generated charset can't be edited by accident), and the
    /// predicates are checked to accept exactly the chars that spelling
    /// describes (so a predicate change without a matching const change fails
    /// here). Keeps the generated grammars in step with
    /// [`is_math_identifier_start`] / [`is_math_identifier_part`].
    #[test]
    fn ident_class_consts_match_predicates() {
        assert_eq!(IDENT_START_CLASS, "A-Za-z_");
        assert_eq!(IDENT_PART_CLASS, "A-Za-z0-9_");

        let start = |c: char| c.is_ascii_alphabetic() || c == '_';
        let part = |c: char| start(c) || c.is_ascii_digit();
        // ASCII plus the Latin-1 supplement (covers non-ASCII letters like 'ä').
        for c in '\0'..='\u{2ff}' {
            assert_eq!(is_math_identifier_start(c), start(c), "start {c:?}");
            assert_eq!(is_math_identifier_part(c), part(c), "part {c:?}");
        }
    }

    /// Parity: the pest grammar rules must accept exactly what the Rust
    /// predicates accept — this module is the single source of truth, and
    /// the grammar is its mirror.
    #[test]
    fn grammar_parity() {
        use crate::parser::{RossiParser, Rule};
        use pest::Parser;

        // A grammar rule matches `s` fully iff parse succeeds AND consumes
        // all input (pest rules match prefixes otherwise).
        fn full_match(rule: Rule, s: &str) -> bool {
            RossiParser::parse(rule, s)
                .ok()
                .and_then(|mut pairs| pairs.next())
                .is_some_and(|p| p.as_str() == s)
        }

        let samples = [
            "x",
            "_x",
            "x'",
            "x''",
            "x'y",
            "A1",
            "events_of_partition",
            "machine",
            "M-ALPHA",
            "CTX-1",
            "do-step",
            "end-to-end",
            "events-x",
            "a-1-2",
            "a-b'",
            "a-'",
            "the-MACHINE-x",
            "",
            "1a",
            "'a",
            "-a",
            "a-",
            "a--b",
            "a-b-",
            "a b",
            "a.b",
            "ä",
        ];
        for s in samples {
            assert_eq!(
                full_match(Rule::component_name, s),
                is_valid_component_name(s),
                "component_name grammar/predicate disagree on {s:?}"
            );
            assert_eq!(
                full_match(Rule::identifier, s),
                is_valid_math_identifier(s),
                "identifier grammar/predicate disagree on {s:?}"
            );
        }
    }
}
