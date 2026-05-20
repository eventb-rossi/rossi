//! Shared helpers for locating identifiers in source text by cursor position.
//!
//! Several LSP providers (definition, hover, references) need "what identifier
//! is under the cursor?". Keep that logic in one place so the three variants
//! can't drift apart.

use crate::lsp_types::Position;

/// Return the word (identifier) that contains `position`.
///
/// A word is a maximal run of `is_alphanumeric()` characters plus `_`.
/// If `position` is not inside a word, the single character at `position` is
/// returned instead — callers doing identifier lookup will simply get no hit,
/// but providers that dispatch on punctuation (e.g. hover on operators) can
/// still use it.
///
/// Returns `None` when the line or character position is out of bounds.
pub fn get_word_at_position(text: &str, position: Position) -> Option<String> {
    let line = text.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let char_pos = position.character as usize;

    if char_pos >= chars.len() {
        return None;
    }

    let is_word = |c: char| c.is_alphanumeric() || c == '_';

    let mut start = char_pos;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }

    let mut end = char_pos;
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }

    if start < end {
        Some(chars[start..end].iter().collect())
    } else {
        Some(chars[char_pos].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn word_in_middle_of_identifier() {
        assert_eq!(
            get_word_at_position("count := 0", pos(0, 2)).as_deref(),
            Some("count")
        );
    }

    #[test]
    fn word_at_start_of_identifier() {
        assert_eq!(
            get_word_at_position("count := 0", pos(0, 0)).as_deref(),
            Some("count")
        );
    }

    #[test]
    fn single_char_fallback_on_operator() {
        // cursor on `:` — not a word char
        assert_eq!(
            get_word_at_position("count := 0", pos(0, 6)).as_deref(),
            Some(":"),
        );
    }

    #[test]
    fn underscored_identifier() {
        assert_eq!(
            get_word_at_position("my_var := 0", pos(0, 3)).as_deref(),
            Some("my_var")
        );
    }

    #[test]
    fn out_of_bounds_line() {
        assert!(get_word_at_position("x", pos(5, 0)).is_none());
    }

    #[test]
    fn out_of_bounds_char() {
        assert!(get_word_at_position("x", pos(0, 99)).is_none());
    }

    #[test]
    fn multibyte_identifier() {
        // chars (not bytes) are counted
        assert_eq!(
            get_word_at_position("α_name := 0", pos(0, 3)).as_deref(),
            Some("α_name")
        );
    }
}
