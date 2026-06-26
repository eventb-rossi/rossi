//! Conversion of rossi's internal findings into LSP [`Diagnostic`]s.
//!
//! This is the single place that turns a parse error (and, in future, other
//! rossi findings) into the `lsp_types::Diagnostic` the editor renders. All
//! byte-span → UTF-16 range mapping goes through [`crate::position`], so the
//! column convention can't drift from the rest of the server.

use crate::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// End byte offset of the token at byte offset `start`, for sizing a diagnostic
/// range when pest reports only a point: the end of the contiguous non-whitespace
/// run starting at `start`, bounded by the line. Zero-width at EOL/EOF, one char
/// when `start` lands on whitespace.
fn token_end_byte(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    match rest.chars().next() {
        None | Some('\n') => start, // EOF / EOL: zero-width
        Some(first) if first.is_whitespace() => start + first.len_utf8(), // 1-char span
        // The leading non-whitespace run ends at the first whitespace (or EOL).
        _ => start + rest.find(char::is_whitespace).unwrap_or(rest.len()),
    }
}

/// Collapse pest's multi-line rendering (a location header, the source line, a
/// caret, then an `= expected …` line) to a single line: the editor already
/// shows the location via the diagnostic range, so only the `expected …`
/// content carries information.
fn concise_pest_message(message: &str) -> String {
    message
        .lines()
        .map(str::trim_start)
        .find_map(|l| l.strip_prefix("= "))
        .map(|expected| format!("Syntax error: {expected}"))
        .unwrap_or_else(|| message.trim().to_string()) // fallback: never drop info
}

/// Convert a parse error to an LSP diagnostic
pub(crate) fn parse_error_to_diagnostic(error: &rossi::ParseError, text: &str) -> Diagnostic {
    use rossi::ParseError;

    // pest's multi-line dump is collapsed to a single line; located variants
    // keep their own message; everything else uses the Display rendering.
    let message = match error {
        ParseError::PestError { message, .. } => concise_pest_message(message),
        ParseError::RecoverableError { message, .. } | ParseError::ClauseError { message, .. } => {
            message.clone()
        }
        _ => error.to_string(),
    };

    Diagnostic {
        range: parse_error_range(error, text),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        source: Some("rossi".to_string()),
        message,
        related_information: None,
        tags: None,
        code_description: None,
        data: None,
    }
}

/// LSP range for a parse-error diagnostic, rendered through the single UTF-16
/// converter (issue #48).
///
/// Everything resolves to a byte `[start, end)`: a non-empty span (issue #42)
/// underlines the offending token directly; a zero-width span (pest reports a
/// single point) or a span-less variant gives only a start, so the token is
/// sized in bytes from there. A span-less start comes from the 1-indexed
/// (line, column) — those variants (nesting, clause-order, recovery) point at
/// ASCII keywords/clause content, where char and UTF-16 columns coincide.
fn parse_error_range(error: &rossi::ParseError, text: &str) -> Range {
    let span = error.span();
    let start = match span {
        Some(s) => s.start,
        None => {
            let (line, column) = error.position().unwrap_or((1, 1));
            let pos = Position::new(
                line.saturating_sub(1) as u32,
                column.saturating_sub(1) as u32,
            );
            crate::position::position_to_offset(text, pos).unwrap_or(text.len())
        }
    };
    let end = match span {
        Some(s) if s.start < s.end => s.end,
        _ => token_end_byte(text, start),
    };
    crate::position::span_to_range(&rossi::ast::Span { start, end }, text)
}

#[cfg(test)]
mod tests {
    use super::parse_error_to_diagnostic;
    use crate::lsp_types::Position;

    #[test]
    fn duplicate_clause_diagnostic_stays_on_one_line() {
        // A duplicate SETS clause yields a span-less ClauseError; the diagnostic
        // must be a single-line, token-sized range at the offending keyword, never
        // the whole multi-line clause.
        let text = "CONTEXT test\nSETS\n    S\nSETS\n    T\nEND\n";
        let error = rossi::parse(text).expect_err("duplicate SETS must fail");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(
            diagnostic.range.start.line, diagnostic.range.end.line,
            "clause diagnostic must stay on one line, got {:?}",
            diagnostic.range
        );
        // Sized to the duplicate `SETS` keyword on line 4 (0-indexed 3), not the body.
        assert_eq!(diagnostic.range.start, Position::new(3, 0));
        assert_eq!(diagnostic.range.end, Position::new(3, 4));
    }

    #[test]
    fn reserved_word_diagnostic_spans_the_word_issue_42() {
        // The reserved word `dom` used as a constant name carries a byte span
        // (issue #42); the diagnostic range comes from that span and covers the
        // whole 3-char word, not the old byte-length special case.
        let text = "CONTEXT c0\nCONSTANTS\n    dom\nEND\n";
        let error = rossi::parse(text).expect_err("`dom` is a reserved word");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(diagnostic.range.start, Position::new(2, 4));
        assert_eq!(diagnostic.range.end, Position::new(2, 7));
    }

    #[test]
    fn pest_diagnostic_uses_real_position() {
        // End-to-end through the real parser: the strict-parse error must
        // carry pest's structured position, not 0:0, and the range must be
        // sized to the offending token (the stray `+`), not a fixed width.
        let text = "CONTEXT c\nCONSTANTS\n    c1\n    +\nEND\n";
        let error = rossi::parse(text).expect_err("the stray `+` must fail strict parsing");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(diagnostic.range.start, Position::new(3, 4));
        // Token span: just the single-character `+`, not start + 10.
        assert_eq!(diagnostic.range.end, Position::new(3, 5));
        // Message is collapsed to a single line (issue #32): no pest caret art.
        assert!(diagnostic.message.starts_with("Syntax error:"));
        assert!(!diagnostic.message.contains("-->"));
        assert!(!diagnostic.message.contains('\n'));
    }

    #[test]
    fn pest_diagnostic_lists_symbols_not_rule_names() {
        // The expected-token list is rendered with the Event-B symbols a user
        // types, not pest's internal rule names (`op_in, op_notin, …` used to
        // leak straight into the diagnostic).
        let text = "CONTEXT c\nAXIOMS\n    @a S sdfsdf T\nEND\n";
        let error = rossi::parse(text).expect_err("`sdfsdf` where an operator is expected fails");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert!(
            diagnostic.message.contains('∈'),
            "expected-list should use symbols, got: {}",
            diagnostic.message
        );
        assert!(
            !diagnostic.message.contains("op_in"),
            "internal rule names must not leak, got: {}",
            diagnostic.message
        );
    }

    #[test]
    fn pest_diagnostic_sized_to_token_issue_32() {
        // Issue #32, example 1: a forgotten `@` on `axm2`. Through the real
        // LSP recovery path, the diagnostic must land on the offending line
        // (line 10) and underline just the token pest stopped at (`>`), rather
        // than a fixed 10-character block running past the end of the line.
        let text = concat!(
            "CONTEXT library_ctx\n",
            "EXTENDS\n",
            "    base_ctx\n",
            "SETS\n",
            "    BOOK READER\n",
            "CONSTANTS\n",
            "    max_loans\n",
            "AXIOMS\n",
            "    @axm1: max_loans = 5\n",
            "    axm2: max_loans > 0\n",
            "END\n",
        );
        let result = rossi::parse_components_with_recovery(text);
        let error = result
            .errors
            .first()
            .expect("recovery must report the error");
        let diagnostic = parse_error_to_diagnostic(error, text);
        // Line 10 (0-indexed 9), the `>` at column 21 (0-indexed 20).
        assert_eq!(diagnostic.range.start, Position::new(9, 20));
        assert_eq!(diagnostic.range.end, Position::new(9, 21));
        assert!(!diagnostic.message.contains("-->"));
    }

    #[test]
    fn trailing_operator_flags_only_the_broken_predicate() {
        // The reported edit: a `… ∈` invariant left dangling. The strict parser
        // runs past it into the next label, but only the broken predicate may be
        // flagged — the following @RolesPartition must stay clean.
        let text = concat!(
            "MACHINE m\n",
            "VARIABLES\n",
            "    Roles\n",
            "    AdmRoles\n",
            "INVARIANTS\n",
            "    @EntitiesPartition Roles ∈\n",
            "    @RolesPartition Roles ⊆ AdmRoles\n",
            "END\n",
        );
        let result = rossi::parse_components_with_recovery(text);
        let diagnostics: Vec<_> = result
            .errors
            .iter()
            .map(|e| parse_error_to_diagnostic(e, text))
            .collect();

        assert_eq!(
            diagnostics.len(),
            1,
            "only the broken predicate is flagged, got {diagnostics:?}"
        );
        // The diagnostic stays on the @EntitiesPartition line (0-indexed 5),
        // never reaching @RolesPartition on line 6.
        assert!(diagnostics[0].range.end.line < 6);
    }
}
