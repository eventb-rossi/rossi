//! Shared line-level text helpers for the LSP providers.
//!
//! None of these helpers know about comments. Callers that scan document
//! structure (keywords, identifiers, clause boundaries) must pass lines of
//! text masked through [`rossi::comments::mask_comments_chars`] — the single
//! comment lexer — so that an `EVENT` or `END` spelled inside a `//` or
//! `/* */` comment is never mistaken for code. The char-preserving mask
//! keeps every line's char columns identical to the original, so positions
//! computed on masked lines are valid in the real document.

pub fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn identifier_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in line.chars() {
        if is_identifier_char(ch) {
            current.push(ch);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

pub fn first_identifier_word(line: &str) -> Option<String> {
    identifier_words(line).into_iter().next()
}

pub fn event_name_from_line(line: &str) -> Option<String> {
    // Whitespace-delimited so a hyphenated event name (`EVENT do-step`,
    // issue #28) comes back whole — `identifier_words` would split it at the
    // hyphen and return only `do`, so `event_line_range`'s match against the
    // AST event name would fail for every hyphenated-named event.
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        if token.eq_ignore_ascii_case("EVENT") {
            return tokens.next().map(str::to_string);
        }
    }
    None
}

pub fn is_clause_boundary_keyword(word: &str) -> bool {
    rossi::keywords::is_clause_boundary(word)
}

/// Whether `line`'s first whitespace-delimited token is the keyword `id`,
/// resolved case-insensitively through the canonical keyword table. A header
/// line like `EVENT do-step` or `sees C1` matches on its leading keyword
/// regardless of casing or what follows.
///
/// Uses the whole first token (not [`first_identifier_word`], which would strip
/// a leading `@`): a labelled action such as `@end y := 0` must NOT be read as
/// the `END` keyword.
pub fn line_keyword_is(line: &str, id: rossi::keywords::KeywordId) -> bool {
    line.split_whitespace()
        .next()
        .and_then(rossi::keywords::lookup)
        .map(|keyword| keyword.id)
        == Some(id)
}

/// Whether `line` begins a new structural region (clause, component, or event)
/// and so bounds a scan that walks a clause/section body — its first token is a
/// clause-boundary keyword, EXCEPT `STATUS`.
///
/// `STATUS` is a contextual keyword that only acts as one inside an EVENT and is
/// commonly used as a set/constant name, so a line that is just `STATUS` in a
/// declaration clause is a declaration to be found, not a boundary to stop at.
/// Matching the first token (not the whole line) keeps a header carrying inline
/// content — e.g. `EVENT incr` — recognised as a boundary.
pub fn is_declaration_scan_boundary(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    is_clause_boundary_keyword(first) && !first.eq_ignore_ascii_case("STATUS")
}

/// UTF-16 column (the LSP convention; see [`crate::position`]) of the first
/// whole-word occurrence of `word` in `line`, or `None` when it does not occur.
///
/// A match is a *whole word* when neither flanking character is an identifier
/// character ([`is_identifier_char`]) — so `count` does not match inside
/// `counter`, and a `-` flanks as a boundary. The scan continues past a rejected
/// substring hit, so a later whole-word match is still found (e.g. `x` in
/// `xs x`). The single source of truth for the LSP providers' word→column scan.
pub fn whole_word_utf16_col(line: &str, word: &str) -> Option<u32> {
    // Byte length of the word's first character. Every match begins with it, so
    // stepping past a rejected hit by this much lands on a char boundary (the
    // following `find` cannot slice mid-character). `?` also rejects an empty
    // word, which has no first character and so no whole-word match.
    let first_char_len = word.chars().next()?.len_utf8();
    let mut idx = 0;
    while let Some(rel) = line[idx..].find(word) {
        let abs = idx + rel;
        let before_ok = !line[..abs]
            .chars()
            .next_back()
            .is_some_and(is_identifier_char);
        let after = abs + word.len();
        let after_ok = !line[after..].chars().next().is_some_and(is_identifier_char);
        if before_ok && after_ok {
            return Some(crate::position::utf16_len(&line[..abs]));
        }
        idx = abs + first_char_len;
    }
    None
}

/// Whether `line` enters the clause named `clause`: its first
/// whitespace-delimited token equals `clause`, case-insensitively.
///
/// Uses the whole first token — like [`line_keyword_is`] and unlike
/// [`first_identifier_word`], which strips a leading `@` — so a labelled action
/// `@any x := 0` is not read as entering the `ANY` clause. Matching the first
/// token rather than the whole line keeps a header that carries inline
/// declarations (`SETS S1 S2`) recognised as entering the clause, and is
/// consistent with [`is_declaration_scan_boundary`], the matching exit check.
pub fn line_enters_clause(line: &str, clause: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case(clause))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_skips_substring_hits() {
        // First `count` sits inside `counter`; the scan must keep going and
        // return the standalone `count` rather than giving up (the bug the old
        // definition.rs copy carried).
        assert_eq!(whole_word_utf16_col("counter count", "count"), Some(8));
        // Single char after a longer word: `x` in `xs x` is at column 3.
        assert_eq!(whole_word_utf16_col("xs x", "x"), Some(3));
        // No standalone occurrence at all.
        assert_eq!(whole_word_utf16_col("counter", "count"), None);
    }

    #[test]
    fn whole_word_respects_identifier_boundaries() {
        let line = "my_var := my_var + my_variable";
        assert_eq!(whole_word_utf16_col(line, "my_var"), Some(0));
        assert_eq!(whole_word_utf16_col(line, "my_variable"), Some(19));
    }

    #[test]
    fn whole_word_column_is_utf16() {
        // BMP: `∈` is one UTF-16 unit, so `S` is at column 4.
        assert_eq!(whole_word_utf16_col("x ∈ S", "S"), Some(4));
        // Astral: `𝔹` is two UTF-16 units, so `count` after `𝔹x ` is at column
        // 4 (its char index would be 3).
        assert_eq!(whole_word_utf16_col("𝔹x count", "count"), Some(4));
    }

    #[test]
    fn whole_word_empty_and_missing() {
        assert_eq!(whole_word_utf16_col("count", ""), None);
        assert_eq!(whole_word_utf16_col("count", "total"), None);
    }

    #[test]
    fn line_enters_clause_matches_first_token() {
        assert!(line_enters_clause("SETS", "SETS"));
        assert!(line_enters_clause("  sets", "SETS")); // leading ws + casing
        assert!(line_enters_clause("SETS S1 S2", "SETS")); // inline declarations
        assert!(!line_enters_clause("@any x := 0", "ANY")); // labelled action
        assert!(!line_enters_clause("setsX", "SETS")); // not a separate token
        assert!(!line_enters_clause("", "SETS"));
    }
}
