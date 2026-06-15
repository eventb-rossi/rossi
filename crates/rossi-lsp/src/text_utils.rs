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
