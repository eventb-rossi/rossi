//! Source positions in the dump document.
//!
//! A span in the model is a pair of byte offsets. Consumers want lines and
//! columns as well, so every emitted span carries both.

use rossi::ast::{LineIndex, Span};

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

impl SpanDump {
    /// The dump form of `span`, positioned through `index`.
    #[must_use]
    pub fn of(index: &LineIndex<'_>, span: Span, file: Option<String>) -> SpanDump {
        let (line, col) = index.line_col(span.start);
        let (end_line, end_col) = index.line_col(span.end);
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
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_maps_both_ends() {
        let index = LineIndex::new("ab\ncdef\n");
        let span = SpanDump::of(
            &index,
            Span { start: 4, end: 7 },
            Some("m.eventb".to_string()),
        );
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
