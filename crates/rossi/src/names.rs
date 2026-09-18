//! Lexical name classes — the single source of truth for what counts as a
//! valid name, shared by the text grammar (kept in sync by a parity test),
//! the XML importer, and the LSP.
//!
//! Event-B distinguishes two kinds of names (issue #28):
//!
//! - **Mathematical identifiers** — carrier sets, constants, variables, event
//!   parameters, and every name inside a formula. kernel_lang §2.2 defines
//!   these per the Unicode identifier rules (no hyphens); Rodin enforces the
//!   same via `Character.isJavaIdentifierStart/Part`, excluding only `λ` and
//!   `$`. We accept exactly those classes (the Unicode general categories, via
//!   pest's tables, so the grammar and this module share one source), with an
//!   optional single trailing `'` for a primed after-state variable
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

use pest::unicode::{
    CONNECTOR_PUNCTUATION, CURRENCY_SYMBOL, DECIMAL_NUMBER, LETTER, LETTER_NUMBER, MATH_SYMBOL,
    NONSPACING_MARK, SPACING_MARK,
};

/// First character of a mathematical identifier (and of a component name):
/// Java's `isJavaIdentifierStart` minus `λ` and `$`, as in Rodin's lexer.
/// Mirrors the grammar's `ident_start` (parity-tested).
#[inline]
pub fn is_math_identifier_start(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphabetic() || c == '_';
    }
    c != 'λ'
        && !MATH_SYMBOL(c)
        && (LETTER(c) || LETTER_NUMBER(c) || CURRENCY_SYMBOL(c) || CONNECTOR_PUNCTUATION(c))
}

/// Non-first character of a mathematical identifier's core: Java's
/// `isJavaIdentifierPart` minus `λ` and `$`. The prime suffix is positional
/// (see [`check_math_identifier`]), so `'` is not a part char. Mirrors the
/// grammar's `word_char` (parity-tested).
#[inline]
pub fn is_math_identifier_part(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    c != 'λ'
        && !MATH_SYMBOL(c)
        && (LETTER(c)
            || LETTER_NUMBER(c)
            || CURRENCY_SYMBOL(c)
            || CONNECTOR_PUNCTUATION(c)
            || DECIMAL_NUMBER(c)
            || NONSPACING_MARK(c)
            || SPACING_MARK(c))
}

/// Regex character-class *body* (the part between `[` and `]`) matching
/// [`is_math_identifier_start`] — the first char of any identifier or
/// component name. The canonical regex spelling of the name charset for
/// editor-highlighter generators (consumed today by the Emacs generator),
/// pinned by a parity test. POSIX classes are Unicode-aware in the editors
/// that consume them and agree with the predicate on ASCII; the exact
/// Unicode category set (and the `λ`/`$` exclusion) is a highlighter
/// approximation. A generator wraps these classes in its own regex flavor.
pub const IDENT_START_CLASS: &str = "[:alpha:]_";

/// Regex character-class body matching [`is_math_identifier_part`] — the
/// non-first chars of an identifier core and the chars of a component name's
/// `-`-joined segments. The math-identifier prime suffix is positional and not
/// part of this class. See [`IDENT_START_CLASS`].
pub const IDENT_PART_CLASS: &str = "[:alnum:]_";

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
                write!(f, "must start with a letter or '_', got {c:?}")
            }
            NameError::BadChar(c) => write!(f, "contains unsupported character {c:?}"),
            NameError::EmptyHyphenSegment => write!(
                f,
                "'-' must be followed by a letter, digit or '_' (no trailing or doubled '-')"
            ),
        }
    }
}

/// Check a mathematical identifier: `ident_start word_char* "'"?`.
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

/// `true` iff `s` carries the after-state prime, mirroring Rodin's
/// `FreeIdentifier.isPrimed()` (which is literally a suffix test). Which
/// positions may carry one is the rule's business, not the predicate's —
/// see `rossi_build::identifiers`.
pub fn is_primed_identifier(s: &str) -> bool {
    s.ends_with('\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_identifiers() {
        // `x'` carries a lone trailing prime (after-state variable).
        // Rodin's identifier classes: any letter starts one (`ä`, `α`, `ℝ`),
        // and a built-in glyph followed by identifier characters is one
        // identifier (`ℤx`); reservation of the bare glyph is positional.
        for ok in [
            "x",
            "_x",
            "x'",
            "events_of_partition",
            "A1",
            "machine",
            "ä",
            "α",
            "ℝ",
            "ℤx",
            "x٣",
            "€",
        ] {
            assert!(is_valid_math_identifier(ok), "{ok:?} should be valid");
        }
        for (bad, err) in [
            ("", NameError::Empty),
            ("1a", NameError::BadStart('1')),
            ("'a", NameError::BadStart('\'')),
            ("-a", NameError::BadStart('-')),
            ("a-b", NameError::BadChar('-')),
            ("a b", NameError::BadChar(' ')),
            // Rodin excludes the lambda letter and the meta-variable sigil.
            ("λ", NameError::BadStart('λ')),
            ("xλ", NameError::BadChar('λ')),
            ("$x", NameError::BadStart('$')),
            // The binder dot is punctuation, never an identifier character;
            // a subscript digit is a number but not a decimal digit.
            ("x·y", NameError::BadChar('·')),
            ("x₁", NameError::BadChar('₁')),
            // The prime is a single suffix: no doubled or interior prime.
            ("x''", NameError::BadChar('\'')),
            ("x'y", NameError::BadChar('\'')),
        ] {
            assert_eq!(check_math_identifier(bad), Err(err), "{bad:?}");
        }
    }

    #[test]
    fn primed_identifiers() {
        for primed in ["x'", "_'", "A1'"] {
            assert!(is_primed_identifier(primed), "{primed:?} is primed");
        }
        for plain in ["x", "_x", "A1", ""] {
            assert!(!is_primed_identifier(plain), "{plain:?} is not primed");
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
    /// predicates are checked to agree with it on ASCII, where the POSIX
    /// classes are exact. Keeps the generated grammars in step with
    /// [`is_math_identifier_start`] / [`is_math_identifier_part`].
    #[test]
    fn ident_class_consts_match_predicates() {
        assert_eq!(IDENT_START_CLASS, "[:alpha:]_");
        assert_eq!(IDENT_PART_CLASS, "[:alnum:]_");

        let start = |c: char| c.is_ascii_alphabetic() || c == '_';
        let part = |c: char| start(c) || c.is_ascii_digit();
        for c in '\0'..='\x7f' {
            assert_eq!(is_math_identifier_start(c), start(c), "start {c:?}");
            assert_eq!(is_math_identifier_part(c), part(c), "part {c:?}");
        }
    }

    /// The predicates are Rodin's classes: Java's `isJavaIdentifierStart` /
    /// `isJavaIdentifierPart` (letters, letter numbers, currency symbols and
    /// connector punctuation start; digits and combining marks continue),
    /// minus `λ` and `$`, which Rodin's lexer excludes explicitly.
    #[test]
    fn predicates_follow_rodins_identifier_classes() {
        for c in ['a', '_', 'ä', 'α', 'ℝ', 'ℤ', 'ℕ', 'ℙ', 'Ⅳ', '€', '‿'] {
            assert!(is_math_identifier_start(c), "start {c:?}");
            assert!(is_math_identifier_part(c), "part {c:?}");
        }
        for c in ['1', '٣', '\u{301}', '\u{93e}'] {
            assert!(!is_math_identifier_start(c), "start {c:?}");
            assert!(is_math_identifier_part(c), "part {c:?}");
        }
        for c in ['λ', '$', '·', '\'', '-', ' ', '∈', '∅', '⊤', '·', '′', '×'] {
            assert!(!is_math_identifier_start(c), "start {c:?}");
            assert!(!is_math_identifier_part(c), "part {c:?}");
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
            "α",
            "ℝ",
            "ℤ",
            "ℕ1",
            "ℤx",
            "x₁",
            "λ",
            "xλ",
            "$x",
            "x·y",
            "€",
        ];
        for s in samples {
            assert_eq!(
                full_match(Rule::component_name, s),
                is_valid_component_name(s),
                "component_name grammar/predicate disagree on {s:?}"
            );
            // The grammar's `identifier` also declines the bare built-in
            // glyphs, which are tokens (`reserved_glyph`); this module stays
            // purely lexical and leaves that to `builtins`.
            assert_eq!(
                full_match(Rule::identifier, s),
                is_valid_math_identifier(s) && !crate::builtins::RESERVED_GLYPH_WORDS.contains(&s),
                "identifier grammar/predicate disagree on {s:?}"
            );
        }
        // Every reserved glyph is an identifier to this module and a token
        // to the grammar: the two lists cover the same five spellings.
        for glyph in crate::builtins::RESERVED_GLYPH_WORDS {
            assert!(is_valid_math_identifier(glyph), "{glyph:?}");
            assert!(
                !full_match(Rule::identifier, glyph),
                "{glyph:?} must stay a token"
            );
        }
    }
}
