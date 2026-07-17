//! The single source of truth for translating between source byte offsets /
//! [`Span`]s and LSP [`Position`]/[`Range`] values.
//!
//! LSP addresses text in **UTF-16 code units** (the protocol's default
//! `positionEncoding`) — not bytes, and not Unicode scalar values. Every column
//! produced or consumed here is therefore a UTF-16 code-unit count. Throughout
//! the Basic Multilingual Plane — which covers every Event-B operator, keyword,
//! and identifier — one scalar value is one UTF-16 unit, so this matches the
//! historical char-count behaviour; it diverges only for astral characters
//! (e.g. `𝔹`, an emoji in a comment) that encode as a surrogate pair (two
//! UTF-16 units). Counting characters there would push every following column
//! one unit to the left of where the editor places it.
//!
//! Keep all `&str`-based conversion in this module so the column convention
//! cannot drift between providers. The rope-based edit path in
//! [`crate::document`] is the one unavoidable exception (it counts UTF-16 over a
//! `ropey::Rope` instead of a `&str`); it documents the same convention there.

use crate::lsp_types::{Position, Range};
use rossi::ast::Span;

/// UTF-16 code-unit length of `s` — the LSP "length" of a token.
pub fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

// --- Per-line converters -------------------------------------------------
//
// These map between the three single-line coordinate systems the providers
// use — char index (the unit of `Vec<char>` scanners), UTF-16 column (the LSP
// unit), and byte offset (for slicing) — for a `line` that contains no newline.
// They live here so the `ch.len_utf16()` accumulation exists in exactly one
// place. A UTF-16 column that lands inside a surrogate pair clamps forward to
// the next char boundary.

/// UTF-16 column of char index `char_col` on `line`: the UTF-16 length of its
/// first `char_col` characters. `char_col` may equal the line's char count.
pub fn char_col_to_utf16(line: &str, char_col: usize) -> u32 {
    line.chars()
        .take(char_col)
        .map(|c| c.len_utf16() as u32)
        .sum()
}

/// Char index on `line` for an incoming UTF-16 column; clamps to the char count
/// when the column is past the end.
pub fn utf16_to_char_col(line: &str, utf16_col: usize) -> usize {
    utf16_to_char_col_checked(line, utf16_col).unwrap_or_else(|| line.chars().count())
}

/// Char index on `line` for an incoming UTF-16 column; `None` when the column
/// is past the end. A column inside a surrogate pair clamps forward.
pub(crate) fn utf16_to_char_col_checked(line: &str, utf16_col: usize) -> Option<usize> {
    let mut units = 0usize;
    let mut char_idx = 0usize;
    for ch in line.chars() {
        if units >= utf16_col {
            return Some(char_idx);
        }
        units += ch.len_utf16();
        char_idx += 1;
    }
    (units >= utf16_col).then_some(char_idx)
}

/// Byte offset on `line` for an incoming UTF-16 column; `None` when the column
/// is past the end of the line.
pub fn utf16_to_byte(line: &str, utf16_col: usize) -> Option<usize> {
    let mut units = 0usize;
    for (byte_idx, ch) in line.char_indices() {
        if units >= utf16_col {
            return Some(byte_idx);
        }
        units += ch.len_utf16();
    }
    (units >= utf16_col).then_some(line.len())
}

/// LSP range for the `[start_col, end_col)` char-column run on the line at
/// 0-indexed `line_idx`, with the columns converted to UTF-16. Line-local: no
/// scan of the surrounding document.
pub fn line_run_to_range(line: &str, line_idx: u32, start_col: usize, end_col: usize) -> Range {
    Range::new(
        Position::new(line_idx, char_col_to_utf16(line, start_col)),
        Position::new(line_idx, char_col_to_utf16(line, end_col)),
    )
}

/// Convert a byte offset into `text` to an LSP [`Position`] (line, UTF-16 column).
///
/// An offset past the end of `text` clamps to the final position; an offset that
/// lands inside a multi-byte character rounds down to that character's start.
pub fn offset_to_position(text: &str, byte_offset: usize) -> Position {
    let offset = byte_offset.min(text.len());
    let mut line = 0u32;
    let mut col = 0u32; // UTF-16 code units on the current line
    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.len_utf16() as u32;
        }
    }
    Position::new(line, col)
}

/// Reusable byte-offset lookup for providers that emit many positions from
/// one source document.
pub(crate) struct PositionIndex<'a> {
    text: &'a str,
    line_offsets: Vec<usize>,
}

impl<'a> PositionIndex<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let mut line_offsets = vec![0];
        line_offsets.extend(
            text.char_indices()
                .filter_map(|(offset, ch)| (ch == '\n').then_some(offset + 1)),
        );
        Self { text, line_offsets }
    }

    pub(crate) fn position(&self, byte_offset: usize) -> Position {
        let mut offset = byte_offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset += 1;
        }
        let line = self.line_offsets.partition_point(|&start| start <= offset) - 1;
        let column = utf16_len(&self.text[self.line_offsets[line]..offset]);
        Position::new(line as u32, column)
    }
}

/// Convert an LSP [`Position`] (line, UTF-16 column) to a byte offset into `text`.
///
/// Returns `None` when the position is out of bounds; a position at end-of-file
/// is accepted. A column that lands inside a surrogate pair clamps forward to the
/// next character boundary.
pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let mut line = 0usize;
    let mut col = 0u32; // UTF-16 code units on the current line
    let mut offset = 0usize;

    for ch in text.chars() {
        if line == position.line as usize && col >= position.character {
            return Some(offset);
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf16() as u32;
        }
        offset += ch.len_utf8();
    }

    // Position at end of file (or end of the final line).
    (line == position.line as usize && col >= position.character).then_some(offset)
}

/// Convert a source [`Span`] (byte offsets) to an LSP [`Range`].
///
/// Both endpoints are located in a single walk over the prefix `[0, span.end)`.
/// Columns are UTF-16 code units, consistent with [`offset_to_position`].
pub fn span_to_range(span: &Span, source: &str) -> Range {
    let mut line = 0u32;
    let mut col = 0u32; // UTF-16 code units on the current line
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
            col += c.len_utf16() as u32;
        }
    }

    // Positions at or past end-of-source fall through to the final cursor.
    let end = Position::new(line, col);
    Range {
        start: start.unwrap_or(end),
        end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    #[test]
    fn per_line_converters_round_trip() {
        // ASCII: char/UTF-16/byte all coincide.
        assert_eq!(char_col_to_utf16("hello", 3), 3);
        assert_eq!(utf16_to_char_col("hello", 3), 3);
        assert_eq!(utf16_to_char_col_checked("hello", 5), Some(5));
        assert_eq!(utf16_to_char_col_checked("hello", 6), None);
        assert_eq!(utf16_to_byte("hello", 3), Some(3));
        assert_eq!(utf16_to_byte("hello", 5), Some(5));
        assert_eq!(utf16_to_byte("hello", 6), None); // past end

        // BMP multibyte (`∈` = 3 bytes, 1 char, 1 UTF-16 unit).
        assert_eq!(char_col_to_utf16("x ∈ y", 3), 3); // 'x',' ','∈' -> 3 units
        assert_eq!(utf16_to_char_col("x ∈ y", 3), 3); // back to the space after '∈'
        assert_eq!(utf16_to_byte("x ∈ y", 3), Some(5)); // byte after the 3-byte '∈'

        // Astral (`𝔹` = 2 UTF-16 units, 1 char, 4 bytes).
        assert_eq!(char_col_to_utf16("𝔹x", 1), 2); // one char past 𝔹 = 2 units
        assert_eq!(utf16_to_char_col("𝔹x", 2), 1); // column 2 is 'x' (char 1)
        assert_eq!(utf16_to_char_col("𝔹x", 1), 1); // mid-surrogate clamps forward
        assert_eq!(utf16_to_char_col_checked("𝔹x", 1), Some(1));
        assert_eq!(utf16_to_byte("𝔹x", 2), Some(4)); // byte of 'x'
    }

    #[test]
    fn line_run_to_range_is_utf16() {
        // `x` after the astral `𝔹` (cols 0..2): char run [1,2) -> UTF-16 [2,3).
        let r = line_run_to_range("𝔹x", 7, 1, 2);
        assert_eq!(r.start, pos(7, 2));
        assert_eq!(r.end, pos(7, 3));
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
    fn position_to_offset_multibyte_bmp() {
        // `∈` is 3 UTF-8 bytes but one UTF-16 unit (BMP); the byte offset reflects that.
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
    fn span_to_range_multibyte_bmp() {
        // Range over `S`, which sits after the 3-byte (1 UTF-16 unit) `∈`.
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

    // --- Astral-plane (surrogate-pair) characters: where UTF-16 != char count ---

    #[test]
    fn astral_char_counts_as_two_utf16_units() {
        // `𝔹` (U+1D539) is 4 UTF-8 bytes and *two* UTF-16 code units.
        assert_eq!(utf16_len("𝔹"), 2);
        let text = "𝔹x"; // 𝔹 occupies columns 0..2, x is at column 2
        // offset_to_position: byte offset of `x` (4) is UTF-16 column 2, not 1.
        assert_eq!(offset_to_position(text, 4), pos(0, 2));
        // Round-trip: column 2 maps back to the byte offset of `x`.
        assert_eq!(position_to_offset(text, pos(0, 2)), Some(4));
    }

    #[test]
    fn position_inside_surrogate_pair_clamps_forward() {
        // Column 1 lands in the middle of `𝔹`'s surrogate pair; clamp forward to
        // the next character boundary (the byte offset of `x`).
        let text = "𝔹x";
        assert_eq!(position_to_offset(text, pos(0, 1)), Some(4));
    }

    #[test]
    fn span_to_range_after_astral_char() {
        // A span over `x`, which sits after the two-UTF-16-unit `𝔹`.
        let text = "𝔹x";
        let x_byte = text.find('x').unwrap();
        let range = span_to_range(
            &Span {
                start: x_byte,
                end: x_byte + 1,
            },
            text,
        );
        assert_eq!(range.start, pos(0, 2));
        assert_eq!(range.end, pos(0, 3));
    }

    #[test]
    fn offset_to_position_multiline() {
        let text = "ab\ncd\nef";
        assert_eq!(offset_to_position(text, 0), pos(0, 0));
        assert_eq!(offset_to_position(text, 3), pos(1, 0));
        assert_eq!(offset_to_position(text, 4), pos(1, 1));
        assert_eq!(offset_to_position(text, 7), pos(2, 1));
        // Past end clamps to the final position.
        assert_eq!(offset_to_position(text, 999), pos(2, 2));
    }

    #[test]
    fn position_index_matches_single_offset_conversion() {
        let text = "a𝔹\n∈z";
        let index = PositionIndex::new(text);
        for offset in 0..=text.len() + 2 {
            assert_eq!(index.position(offset), offset_to_position(text, offset));
        }
    }

    #[test]
    fn recovery_span_round_trips_to_utf16_range() {
        // A byte span produced by the parser's error recovery must map through
        // `span_to_range` to the predicate's true UTF-16 location, even when the
        // predicate sits behind a multibyte line and contains an astral char.
        // This is the byte→UTF-16 leg of the same span the parser pins by bytes.
        let source = "CONTEXT c\nAXIOMS\n    @axm1 ∀x·x∈ℕ $$$\n    @axm2 ∀y·y∈ℕ ### 𝔹\nEND\n";

        let span = rossi::parse_with_recovery(source)
            .errors
            .iter()
            .find_map(|e| match e {
                rossi::ParseError::RecoverableError {
                    message,
                    span: Some(span),
                    ..
                } if message.contains("@axm2") => Some(*span),
                _ => None,
            })
            .expect("recovery reports the second broken axiom with a byte span");

        let range = span_to_range(&span, source);
        // Start: the `@` of `@axm2`, four spaces into line 3 (0-indexed).
        assert_eq!(range.start, pos(3, 4));
        // Width equals the UTF-16 length of the sliced predicate — counting the
        // astral `𝔹` as two units, so the end column is one past a naive char
        // count. `utf16_len` is computed independently of the range walk.
        assert_eq!(range.end.line, 3);
        assert_eq!(
            range.end.character - range.start.character,
            utf16_len(&source[span.start..span.end])
        );
    }
}
