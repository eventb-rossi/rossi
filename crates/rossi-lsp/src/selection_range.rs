//! Selection Range provider for Event-B (`textDocument/selectionRange`).
//!
//! Powers the editor's smart "Expand / Shrink Selection" command: for each cursor
//! it returns the innermost enclosing source range, linked outward through
//! `parent` to progressively larger ranges
//! (token → subexpression → predicate → clause → component).
//!
//! The hierarchy comes from [`rossi::enclosing_spans`], which derives it from the
//! Pest parse tree — no separate grammar, no tree-sitter.

use crate::identifier_utils::{identifier_at_position, position_to_offset, span_to_range};
use crate::lsp_types::{Position, Range, SelectionRange};

/// Provides selection ranges for Event-B documents.
pub struct SelectionRangeProvider;

impl SelectionRangeProvider {
    pub fn new() -> Self {
        Self
    }

    /// One [`SelectionRange`] per requested position, in the same order.
    pub fn selection_ranges(&self, text: &str, positions: &[Position]) -> Vec<SelectionRange> {
        positions
            .iter()
            .map(|&pos| selection_range_at(text, pos))
            .collect()
    }
}

impl Default for SelectionRangeProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the nested selection range for a single position.
fn selection_range_at(text: &str, position: Position) -> SelectionRange {
    let spans = position_to_offset(text, position)
        .map(|offset| rossi::enclosing_spans(text, offset))
        .unwrap_or_default();

    // Spans are outermost → innermost; fold so each node's `parent` points one
    // level out and the returned node is the innermost range.
    let node = spans.iter().fold(None, |parent, span| {
        Some(SelectionRange {
            range: span_to_range(span, text),
            parent: parent.map(Box::new),
        })
    });

    node.unwrap_or_else(|| fallback_range(text, position))
}

/// When the document doesn't parse (common mid-edit) or the cursor sits outside
/// any rule, fall back to the identifier under the cursor — or a zero-width range
/// at the cursor. Every position must yield exactly one [`SelectionRange`].
fn fallback_range(text: &str, position: Position) -> SelectionRange {
    let range = identifier_at_position(text, position)
        .map(|(_, range)| range)
        .unwrap_or_else(|| Range::new(position, position));
    SelectionRange {
        range,
        parent: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    /// Collect a selection range's ranges from innermost to outermost.
    fn chain(mut sr: &SelectionRange) -> Vec<Range> {
        let mut ranges = vec![sr.range];
        while let Some(parent) = &sr.parent {
            ranges.push(parent.range);
            sr = parent;
        }
        ranges
    }

    fn contains(outer: Range, inner: Range) -> bool {
        outer.start <= inner.start && inner.end <= outer.end
    }

    #[test]
    fn expands_from_token_outward() {
        let text = "MACHINE m\nINVARIANTS\n  @inv1 x + 1 > 0\nEND\n";
        let provider = SelectionRangeProvider::new();

        // Cursor on the `x` token (line 2, col 8).
        let result = provider.selection_ranges(text, &[pos(2, 8)]);
        assert_eq!(result.len(), 1);

        let ranges = chain(&result[0]);
        // Innermost is the `x` token.
        assert_eq!(ranges[0], Range::new(pos(2, 8), pos(2, 9)));
        // Several strictly-nesting levels, each parent containing its child.
        assert!(
            ranges.len() >= 3,
            "expected a multi-level chain: {ranges:?}"
        );
        for pair in ranges.windows(2) {
            assert!(contains(pair[1], pair[0]), "parent must contain child");
            assert_ne!(pair[1], pair[0], "levels must differ");
        }
        // Outermost reaches the component header on line 0.
        assert_eq!(ranges.last().unwrap().start, pos(0, 0));
    }

    #[test]
    fn one_result_per_position() {
        let text = "MACHINE m\nINVARIANTS\n  @inv1 x + 1 > 0\nEND\n";
        let provider = SelectionRangeProvider::new();
        let result = provider.selection_ranges(text, &[pos(0, 0), pos(2, 8), pos(2, 12)]);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn unicode_operator_token() {
        // `∈` is one column wide; the innermost range should cover exactly it.
        let text = "CONTEXT c\nAXIOMS\n  @axm1 a ∈ ℕ\nEND\n";
        let provider = SelectionRangeProvider::new();
        // "  @axm1 a ∈ ℕ" — '∈' is at column 10.
        let result = provider.selection_ranges(text, &[pos(2, 10)]);
        let inner = result[0].range;
        assert_eq!(inner, Range::new(pos(2, 10), pos(2, 11)));
    }

    #[test]
    fn falls_back_when_unparsable() {
        // Not a valid component: enclosing_spans is empty, so we fall back to the
        // identifier under the cursor instead of panicking or returning nothing.
        let text = "garbage";
        let provider = SelectionRangeProvider::new();
        let result = provider.selection_ranges(text, &[pos(0, 2)]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].range, Range::new(pos(0, 0), pos(0, 7)));
        assert!(result[0].parent.is_none());
    }
}
