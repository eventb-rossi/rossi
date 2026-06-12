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
