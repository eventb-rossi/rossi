//! Shared helpers for locating identifiers in source text by cursor position.
//!
//! Several LSP providers (definition, hover, references) need "what identifier
//! is under the cursor?". Keep that logic in one place so the three variants
//! can't drift apart.

use crate::lsp_types::{Location, Position, Range, Url};
use crate::text_utils;
use rossi::ast::Span;

/// Return the identifier that contains `position`, together with its range.
///
/// An identifier is a maximal run of `text_utils::is_identifier_char` characters
/// (alphanumeric plus `_`). Returns `None` when the line or character position is
/// out of bounds, or when `position` is not on an identifier character.
pub fn identifier_at_position(text: &str, position: Position) -> Option<(String, Range)> {
    let line = text.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let char_pos = position.character as usize;

    if char_pos >= chars.len() {
        return None;
    }

    let mut start = char_pos;
    while start > 0 && text_utils::is_identifier_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = char_pos;
    while end < chars.len() && text_utils::is_identifier_char(chars[end]) {
        end += 1;
    }

    if start < end {
        let identifier: String = chars[start..end].iter().collect();
        let range = Range::new(
            Position::new(position.line, start as u32),
            Position::new(position.line, end as u32),
        );
        Some((identifier, range))
    } else {
        None
    }
}

/// Return the token that contains `position`, together with its range.
///
/// A cursor on an identifier character belongs to that identifier. Any other
/// cursor targets the character under it: first as an operator from the
/// canonical `rossi::operators` table (maximal munch, so multi-character
/// operators like `:=` come back whole no matter which of their characters
/// the cursor sits on), then as the trailing edge of the identifier ending
/// just before the cursor, and finally as the bare character.
///
/// Returns `None` when the line or character position is out of bounds.
pub fn word_at_position(text: &str, position: Position) -> Option<(String, Range)> {
    let line = text.lines().nth(position.line as usize)?;
    let char_pos = position.character as usize;

    // The operator must be tried before `identifier_at_position`'s
    // trailing-edge rule, or an operator glued to an identifier would lose
    // its first character to the word on its left (`count:=1`, cursor on `:`).
    let on_identifier = line
        .chars()
        .nth(char_pos)
        .is_some_and(text_utils::is_identifier_char);
    if on_identifier {
        return identifier_at_position(text, position);
    }

    let char_range = |start: usize, end: usize| {
        Range::new(
            Position::new(position.line, start as u32),
            Position::new(position.line, end as u32),
        )
    };

    if let Some((operator, range)) = rossi::operators::operator_at(line, char_pos) {
        return Some((operator.to_string(), char_range(range.start, range.end)));
    }

    // Cursor just past a word (trailing edge) keeps resolving to that word.
    if let Some(hit) = identifier_at_position(text, position) {
        return Some(hit);
    }

    let ch = line.chars().nth(char_pos)?;
    Some((ch.to_string(), char_range(char_pos, char_pos + 1)))
}

/// Convert an LSP [`Position`] to a byte offset into `text`.
///
/// Columns are counted in characters (Unicode scalar values), matching
/// [`Span::to_line_col`] and the rest of this crate. Returns `None` when the
/// position is out of bounds (a position at end-of-file is accepted).
pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let mut line = 0;
    let mut col = 0;
    let mut offset = 0;

    for ch in text.chars() {
        if line == position.line as usize && col == position.character as usize {
            return Some(offset);
        }

        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }

        offset += ch.len_utf8();
    }

    // Position at end of file.
    (line == position.line as usize && col == position.character as usize).then_some(offset)
}

/// Convert a source [`Span`] (byte offsets) to an LSP [`Range`].
///
/// Columns are counted in characters, consistent with [`position_to_offset`].
/// Both endpoints are located in a single walk over the prefix `[0, span.end)`.
pub fn span_to_range(span: &Span, source: &str) -> Range {
    let mut line = 0u32;
    let mut col = 0u32;
    let mut start = None;

    for (i, c) in source.char_indices() {
        if start.is_none() && i >= span.start {
            start = Some(Position::new(line, col));
        }
        if i >= span.end {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }

    // Positions at or past end-of-source fall through to the final cursor.
    let end = Position::new(line, col);
    Range {
        start: start.unwrap_or(end),
        end,
    }
}

/// Find every whole-word occurrence of `identifier` in `text`, skipping comments,
/// and return them as LSP `Location`s in `uri`.
///
/// Matching is comment-aware (via `text_utils::CommentTracker`) and respects
/// identifier word boundaries. When `line_range` is `Some((start, end))`, only
/// lines in that inclusive range contribute matches (the tracker still advances
/// over skipped lines so block-comment state stays correct).
pub fn find_whole_word_locations(
    text: &str,
    identifier: &str,
    uri: &Url,
    line_range: Option<(usize, usize)>,
) -> Vec<Location> {
    let mut locations = Vec::new();
    let id_chars: Vec<char> = identifier.chars().collect();
    let mut tracker = text_utils::CommentTracker::new();

    for (line_idx, line) in text.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let code_spans = tracker.code_spans(&chars);

        if let Some((start_line, end_line)) = line_range
            && (line_idx < start_line || line_idx > end_line)
        {
            continue;
        }

        for span in &code_spans {
            let mut col = span.start;
            while col + id_chars.len() <= span.end {
                let matches = chars[col..col + id_chars.len()] == id_chars;
                if matches {
                    let before_ok = col == 0 || !text_utils::is_identifier_char(chars[col - 1]);
                    let after_ok = col + id_chars.len() >= chars.len()
                        || !text_utils::is_identifier_char(chars[col + id_chars.len()]);

                    if before_ok && after_ok {
                        locations.push(Location::new(
                            uri.clone(),
                            Range::new(
                                Position::new(line_idx as u32, col as u32),
                                Position::new(line_idx as u32, (col + id_chars.len()) as u32),
                            ),
                        ));
                    }
                }
                col += 1;
            }
        }
    }

    locations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn word_at(text: &str, position: Position) -> Option<String> {
        word_at_position(text, position).map(|(word, _)| word)
    }

    #[test]
    fn word_in_middle_of_identifier() {
        assert_eq!(word_at("count := 0", pos(0, 2)).as_deref(), Some("count"));
    }

    #[test]
    fn word_at_start_of_identifier() {
        assert_eq!(word_at("count := 0", pos(0, 0)).as_deref(), Some("count"));
    }

    #[test]
    fn multichar_operator_at_position() {
        // cursor on `:` or `=` of `:=` — the whole operator comes back
        assert_eq!(word_at("count := 0", pos(0, 6)).as_deref(), Some(":="));
        assert_eq!(word_at("count := 0", pos(0, 7)).as_deref(), Some(":="));
    }

    #[test]
    fn unspaced_operator_beats_trailing_identifier() {
        // `:=` glued to `count` — the cursor on `:` targets the operator, not
        // the trailing edge of the identifier (issue #34 for unspaced sources).
        assert_eq!(word_at("count:=1", pos(0, 5)).as_deref(), Some(":="));
        assert_eq!(word_at("count:=1", pos(0, 6)).as_deref(), Some(":="));
    }

    #[test]
    fn lone_colon_is_an_operator_token() {
        // ASCII set membership; pins the single-char operator path.
        assert_eq!(word_at("x : S", pos(0, 2)).as_deref(), Some(":"));
    }

    #[test]
    fn trailing_edge_still_resolves_identifier() {
        // Cursor on the space right after `count` — no operator there, so
        // the trailing-edge rule keeps the identifier.
        assert_eq!(word_at("count := 0", pos(0, 5)).as_deref(), Some("count"));
    }

    #[test]
    fn operator_range_at_position() {
        let (word, range) = word_at_position("count := 0", pos(0, 7)).unwrap();
        assert_eq!(word, ":=");
        assert_eq!(range, Range::new(pos(0, 6), pos(0, 8)));
    }

    #[test]
    fn single_char_fallback_on_non_operator_punctuation() {
        let (word, range) = word_at_position("f (x)", pos(0, 2)).unwrap();
        assert_eq!(word, "(");
        assert_eq!(range, Range::new(pos(0, 2), pos(0, 3)));
    }

    #[test]
    fn underscored_identifier() {
        assert_eq!(word_at("my_var := 0", pos(0, 3)).as_deref(), Some("my_var"));
    }

    #[test]
    fn out_of_bounds_line() {
        assert!(word_at("x", pos(5, 0)).is_none());
    }

    #[test]
    fn out_of_bounds_char() {
        assert!(word_at("x", pos(0, 99)).is_none());
    }

    #[test]
    fn multibyte_identifier() {
        // chars (not bytes) are counted
        assert_eq!(word_at("α_name := 0", pos(0, 3)).as_deref(), Some("α_name"));
    }

    #[test]
    fn position_to_offset_basic() {
        let text = "line1\nline2\nline3";
        assert_eq!(position_to_offset(text, pos(0, 0)), Some(0));
        assert_eq!(position_to_offset(text, pos(0, 5)), Some(5));
        assert_eq!(position_to_offset(text, pos(1, 0)), Some(6));
        assert_eq!(position_to_offset(text, pos(1, 3)), Some(9));
        assert_eq!(position_to_offset(text, pos(2, 5)), Some(17));
    }

    #[test]
    fn position_to_offset_multibyte() {
        // `∈` is 3 UTF-8 bytes but one column; the byte offset reflects that.
        let text = "a ∈ S";
        assert_eq!(position_to_offset(text, pos(0, 0)), Some(0)); // 'a'
        assert_eq!(position_to_offset(text, pos(0, 2)), Some(2)); // '∈'
        assert_eq!(position_to_offset(text, pos(0, 4)), Some(6)); // 'S' (2 + 3 + 1)
    }

    #[test]
    fn position_to_offset_out_of_bounds() {
        assert_eq!(position_to_offset("x", pos(5, 0)), None);
    }

    #[test]
    fn span_to_range_multibyte() {
        // Range over `S`, which sits after the 3-byte `∈`.
        let text = "a ∈ S";
        let s_byte = text.find('S').unwrap();
        let range = span_to_range(
            &Span {
                start: s_byte,
                end: s_byte + 1,
            },
            text,
        );
        assert_eq!(range.start, Position::new(0, 4));
        assert_eq!(range.end, Position::new(0, 5));
    }
}
