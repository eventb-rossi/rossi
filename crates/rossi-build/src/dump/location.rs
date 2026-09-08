//! Source positions in the dump document.
//!
//! A span in the model is a pair of byte offsets. Consumers want lines and
//! columns as well, so every emitted span carries both.

use rossi::ast::Span;

/// A source position as the dump reports it.
///
/// `start` and `end` are UTF-8 byte offsets with `end` exclusive, exactly the
/// [`Span`] the model carries. `line` and `col` are 1-based and count
/// characters, not bytes, matching the region convention `rossi validate`
/// already emits. `file` names the source only where a document element
/// begins; the nodes below it all belong to the same file and leave it out.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SpanDump {
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub file: Option<String>,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// The line starts of one source, so a byte offset becomes a line and column
/// in logarithmic time.
///
/// [`Span::to_line_col`] rescans from byte zero on every call. That is fine
/// for a handful of diagnostics and quadratic for a dump, which positions
/// every node of every formula. Building the table once per component turns
/// the whole document's position mapping into a binary search plus one walk
/// of the containing line.
pub struct LineIndex<'a> {
    text: &'a str,
    /// Byte offset of the first character of each line. Always starts at 0,
    /// so it is never empty and the search below cannot underflow.
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        LineIndex { text, starts }
    }

    /// The 1-based line and character column of a byte offset.
    ///
    /// An offset past the end of the text clamps to the end rather than
    /// panicking: a span is only as trustworthy as the source it was taken
    /// against, and a position is not worth aborting a document over.
    #[must_use]
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = self.floor_char_boundary(offset.min(self.text.len()));
        // `starts[0]` is 0 and `offset` is non-negative, so at least one
        // element satisfies the predicate and the subtraction is safe.
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        let col = self.text[self.starts[line]..offset].chars().count() + 1;
        (line + 1, col)
    }

    /// The dump form of a span in this source.
    #[must_use]
    pub fn span(&self, span: Span, file: Option<String>) -> SpanDump {
        let (line, col) = self.line_col(span.start);
        let (end_line, end_col) = self.line_col(span.end);
        SpanDump {
            file,
            start: span.start,
            end: span.end,
            line,
            col,
            end_line,
            end_col,
        }
    }

    /// The largest character boundary at or below `offset`.
    ///
    /// Slicing the text to compute a column would panic on an offset that
    /// splits a multi-byte character. Formula spans come from the lexer and
    /// land on boundaries, but a span is data and the mapping must be total.
    fn floor_char_boundary(&self, mut offset: usize) -> usize {
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index must agree with the model's own conversion, which is
    /// 0-based, for every offset in a source. That function is the shared
    /// oracle: as long as this holds, the dump's positions and the
    /// diagnostics' regions cannot drift apart.
    fn agrees_with_model(text: &str) {
        let index = LineIndex::new(text);
        for offset in 0..=text.len() {
            if !text.is_char_boundary(offset) {
                continue;
            }
            let (line, col) = Span {
                start: offset,
                end: offset,
            }
            .to_line_col(text);
            assert_eq!(
                index.line_col(offset),
                (line + 1, col + 1),
                "offset {offset} of {text:?}"
            );
        }
    }

    #[test]
    fn agrees_with_the_model_on_plain_text() {
        agrees_with_model("machine m\nvariables x\nend\n");
    }

    #[test]
    fn agrees_with_the_model_on_multibyte_text() {
        agrees_with_model("@inv1: x ∈ ℙ(ℤ)\n@inv2: y ⊆ S\n");
    }

    #[test]
    fn agrees_with_the_model_on_crlf_text() {
        // A carriage return is an ordinary character on its line, so the
        // column of the newline that follows it counts it.
        agrees_with_model("axioms\r\n  @axm1: ⊤\r\n");
    }

    #[test]
    fn agrees_with_the_model_without_a_trailing_newline() {
        agrees_with_model("end");
    }

    #[test]
    fn an_empty_source_is_one_line() {
        assert_eq!(LineIndex::new("").line_col(0), (1, 1));
    }

    #[test]
    fn an_offset_past_the_end_clamps() {
        let index = LineIndex::new("ab\ncd");
        assert_eq!(index.line_col(99), index.line_col(5));
    }

    #[test]
    fn an_offset_inside_a_character_clamps_to_its_start() {
        // `ℤ` is three bytes at offset 0; offsets 1 and 2 split it.
        let index = LineIndex::new("ℤ ∈ S");
        assert_eq!(index.line_col(1), (1, 1));
        assert_eq!(index.line_col(2), (1, 1));
        assert_eq!(index.line_col(3), (1, 2));
    }

    #[test]
    fn a_span_maps_both_ends() {
        let index = LineIndex::new("ab\ncdef\n");
        let span = index.span(Span { start: 4, end: 7 }, Some("m.eventb".to_string()));
        assert_eq!(
            span,
            SpanDump {
                file: Some("m.eventb".to_string()),
                start: 4,
                end: 7,
                line: 2,
                col: 2,
                end_line: 2,
                end_col: 5,
            }
        );
    }
}
