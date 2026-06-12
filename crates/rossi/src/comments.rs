//! Lexical comment scanning shared by the recovery parser and the LSP.
//!
//! The strict pest grammar consumes comments silently, but every consumer
//! that scans raw source text (error recovery, semantic token placement)
//! must not mistake comment content for code — e.g. the colon in
//! `@axm1 1 = 1 // note: positive` must never act as a label separator
//! (issue #24). Camille and CamilleX avoid this class of bug by lexing
//! comments before any structural decision; this module is the equivalent
//! tokenization step for rossi's string-scanning paths.

use crate::ast::Span;

/// Comment and label byte spans of `source`, from a single lexical scan.
///
/// Each vector is sorted and disjoint by construction, and labels never
/// overlap comments. This struct is the one encoding of "which bytes are
/// opaque tokens": keyword and identifier scans consult it instead of
/// re-deriving label or comment extents themselves.
pub struct LexicalSpans {
    /// Every `//...` and `/*...*/` comment. A line comment ends before the
    /// terminating newline (unlike the grammar's `COMMENT` rule, which may
    /// consume it) so that masking preserves line structure. An unterminated
    /// block comment extends to the end of input.
    pub comments: Vec<Span>,
    /// Every `@`-label, `@` included. The grammar's compound-atomic `label`
    /// rule makes everything from `@` to the next whitespace opaque label
    /// text, so a keyword spelled inside a label (`@safety-END`) is not
    /// structural.
    pub labels: Vec<Span>,
}

impl LexicalSpans {
    /// Position-preserving copy of `source` with every comment byte replaced
    /// by a space.
    ///
    /// Newlines (and carriage returns) inside comments are kept, so the
    /// result has the same byte length and the same line layout as the
    /// input: any byte offset or (line, column) position computed on the
    /// masked text is valid in the original.
    pub fn mask_comments(&self, source: &str) -> String {
        let mut bytes = source.as_bytes().to_vec();
        for span in &self.comments {
            for b in &mut bytes[span.start..span.end] {
                if *b != b'\n' && *b != b'\r' {
                    *b = b' ';
                }
            }
        }
        // Comment delimiters are ASCII, so spans cover whole UTF-8 sequences
        // and blanking them cannot split a multi-byte character.
        String::from_utf8(bytes).expect("masking comments preserves UTF-8 validity")
    }
}

/// Lexically scan `source` for comments and labels.
///
/// Comment markers inside string literals and labels are ignored, matching
/// the grammar: `string_inner` spans lines and honors the `\"`/`\\` escapes,
/// and the compound-atomic `label` rule consumes any non-whitespace after
/// `@` (Rodin-imported labels like `@SAF5"` or `@inv//1` are plain label
/// text). A quote with no closing quote anywhere after it is treated as a
/// stray character, not a string start, so a half-typed string in a broken
/// document (the recovery parser's main diet) cannot hide later comments.
pub fn lexical_spans(source: &str) -> LexicalSpans {
    let bytes = source.as_bytes();
    let mut comments = Vec::new();
    let mut labels = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'@' => {
                // Label: everything up to the next whitespace is label text
                // (the grammar's `label_text`), never a comment or string.
                let start = i;
                i += 1;
                while i < bytes.len() && !matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                labels.push(Span { start, end: i });
            }
            b'"' => {
                // String literal: consume up to the matching closing quote.
                // Only `\"` and `\\` are escapes (grammar's `string_inner`).
                // If no closing quote exists, the quote is a stray character
                // and scanning resumes right after it.
                let mut j = i + 1;
                let close = loop {
                    match bytes.get(j) {
                        None => break None,
                        Some(b'"') => break Some(j),
                        Some(b'\\') if matches!(bytes.get(j + 1), Some(b'"') | Some(b'\\')) => {
                            j += 2;
                        }
                        Some(_) => j += 1,
                    }
                };
                i = close.map_or(i + 1, |close| close + 1);
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                let start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                comments.push(Span { start, end: i });
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                let start = i;
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                comments.push(Span { start, end: i });
            }
            _ => i += 1,
        }
    }

    LexicalSpans { comments, labels }
}

/// Byte spans of every `//...` and `/*...*/` comment in `source`.
/// See [`lexical_spans`] for the scanning rules.
pub fn comment_spans(source: &str) -> Vec<Span> {
    lexical_spans(source).comments
}

/// Position-preserving copy of `source` with every comment byte replaced
/// by a space. See [`LexicalSpans::mask_comments`].
pub fn mask_comments(source: &str) -> String {
    lexical_spans(source).mask_comments(source)
}

/// The span in `spans` (sorted and disjoint) containing byte `offset`,
/// if any. Binary search.
pub fn span_containing(spans: &[Span], offset: usize) -> Option<Span> {
    let i = spans.partition_point(|s| s.end <= offset);
    spans.get(i).copied().filter(|s| s.contains(offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_line_comment_preserving_layout() {
        let src = "@axm1 1 = 1 // note: positive\n@axm2 2 = 2\n";
        let masked = mask_comments(src);
        assert_eq!(masked.len(), src.len());
        let lines: Vec<&str> = masked.lines().collect();
        assert_eq!(lines[0].trim_end(), "@axm1 1 = 1");
        assert_eq!(lines[0].len(), "@axm1 1 = 1 // note: positive".len());
        assert_eq!(lines[1], "@axm2 2 = 2");
    }

    #[test]
    fn line_comment_span_excludes_newline() {
        let spans = comment_spans("x // c\ny");
        assert_eq!(spans.len(), 1);
        assert_eq!(&"x // c\ny"[spans[0].start..spans[0].end], "// c");
    }

    #[test]
    fn masks_block_comment_preserving_newlines() {
        let src = "a /* note:\n   spans lines */ b\n";
        let masked = mask_comments(src);
        assert_eq!(masked.len(), src.len());
        assert_eq!(masked.lines().count(), src.lines().count());
        let lines: Vec<&str> = masked.lines().collect();
        assert_eq!(lines[0].trim_end(), "a");
        assert_eq!(lines[1].trim_start(), "b");
        assert!(!masked.contains("note") && !masked.contains("*/"));
    }

    #[test]
    fn comment_markers_inside_string_are_not_comments() {
        let src = "s = \"http://example.org\" // real: comment\n";
        let spans = comment_spans(src);
        assert_eq!(spans.len(), 1);
        assert_eq!(&src[spans[0].start..spans[0].end], "// real: comment");
    }

    #[test]
    fn escaped_quote_keeps_string_mode() {
        let src = "s = \"a\\\" // not a comment\" + t\n";
        assert!(comment_spans(src).is_empty());
        assert_eq!(mask_comments(src), src);
    }

    #[test]
    fn backslash_before_newline_does_not_extend_string() {
        // `\` followed by a newline is not an escape; with no closing quote
        // anywhere the quote is a stray, so the comment is still recognized.
        let src = "s = \"oops\\\n// note: colon\n";
        let spans = comment_spans(src);
        assert_eq!(spans.len(), 1);
        assert_eq!(&src[spans[0].start..spans[0].end], "// note: colon");
    }

    #[test]
    fn unterminated_string_is_a_stray_quote() {
        // A half-typed string must not hide a comment on a later line.
        let src = "s = \"oops\n// next: line\n";
        let spans = comment_spans(src);
        assert_eq!(spans.len(), 1);
        assert_eq!(&src[spans[0].start..spans[0].end], "// next: line");
    }

    #[test]
    fn multiline_string_hides_comment_markers() {
        // The grammar's `string_inner` spans lines; `//` and `/*` on a
        // continuation line are string content, not comments (a phantom
        // `/*` span here used to swallow everything to end of input).
        let src = "s = \"line1\n// not: a comment\n/* neither */\" // real: one\n";
        let spans = comment_spans(src);
        assert_eq!(spans.len(), 1);
        assert_eq!(&src[spans[0].start..spans[0].end], "// real: one");
    }

    #[test]
    fn label_spans_cover_at_tokens() {
        let src = "@inv//1 x > 0 // note: real\n";
        let spans = lexical_spans(src);
        let labels: Vec<&str> = spans.labels.iter().map(|s| &src[s.start..s.end]).collect();
        assert_eq!(labels, ["@inv//1"]);
        let comments: Vec<&str> = spans
            .comments
            .iter()
            .map(|s| &src[s.start..s.end])
            .collect();
        assert_eq!(comments, ["// note: real"]);
        assert_eq!(span_containing(&spans.labels, 4).map(|s| s.start), Some(0));
        assert_eq!(span_containing(&spans.labels, 8), None);
    }

    #[test]
    fn comment_markers_inside_label_are_label_text() {
        // The compound-atomic `label` rule consumes any non-whitespace, so
        // `@inv//1` is a complete label and `@SAF5"` (a Rodin-imported label)
        // does not open a string that would hide the real comment.
        let src = "@inv//1 x > 0\n@SAF5\" y > 0 // note: real\n";
        let spans = comment_spans(src);
        assert_eq!(spans.len(), 1);
        assert_eq!(&src[spans[0].start..spans[0].end], "// note: real");
    }

    #[test]
    fn unterminated_block_comment_masked_to_eof() {
        let src = "a /* never: closed\nmore";
        let spans = comment_spans(src);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].end, src.len());
        assert_eq!(mask_comments(src), "a                 \n    ");
    }

    #[test]
    fn unicode_inside_comment_masked_without_panic() {
        // Each multi-byte char inside the comment becomes one space per byte;
        // byte length and line layout are preserved, code is untouched.
        let src = "x ∈ S // тип: ℕ\n";
        let masked = mask_comments(src);
        assert_eq!(masked.len(), src.len());
        assert!(masked.starts_with("x ∈ S "));
        assert_eq!(masked.trim_end(), "x ∈ S");
        assert!(masked.ends_with('\n'));
    }
}
