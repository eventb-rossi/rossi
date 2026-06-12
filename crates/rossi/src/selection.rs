//! Source-span hierarchy for smart selection (LSP `textDocument/selectionRange`).
//!
//! Smart "expand/shrink selection" needs, for a cursor offset, the stack of
//! progressively larger source ranges that enclose it
//! (token → subexpression → predicate → clause → component).
//!
//! The typed [`crate::ast`] only records spans on a few nodes (components,
//! events, labeled predicates/actions) — `Expression`/`Predicate` carry none —
//! so it cannot expand *within* a formula. The Pest parse tree, by contrast,
//! attaches a span to every grammar rule, and the rule nesting already encodes
//! exactly the hierarchy we want. We walk that tree here and keep pest fully
//! encapsulated inside this crate, exposing only [`Span`] values.

use pest::Parser;

use crate::ast::Span;
use crate::nesting;
use crate::parser::{RossiParser, Rule, with_parser_stack};

/// Return the source spans enclosing `offset`, ordered outermost → innermost.
///
/// Each span is trimmed of surrounding whitespace (pest includes trailing
/// layout in a rule's span), and the runs of identical spans produced by the
/// single-child precedence wrappers in the expression grammar
/// (`expression → … → primary_expr → identifier`) are collapsed — so every entry
/// strictly contains the next and none grabs a trailing newline.
///
/// Returns an empty vector when `text` fails to parse or no rule encloses
/// `offset`. Containment is half-open (`start <= offset < end`): at a token
/// boundary the cursor binds to the token on its right, not the (whitespace-
/// padded) token on its left.
pub fn enclosing_spans(text: &str, offset: usize) -> Vec<Span> {
    // Both the pest parse and `collect_path` recurse on formula nesting;
    // refuse over-deep inputs (like the parser proper) and give the rest the
    // same stack headroom.
    let Ok(depth) = nesting::check_nesting(text) else {
        return Vec::new();
    };
    with_parser_stack(depth, || {
        // `components` accepts both single- and multi-component files; for a
        // single component its trimmed span coincides with the component's,
        // so `dedup` collapses the extra outermost entry.
        let Ok(pairs) = RossiParser::parse(Rule::components, text) else {
            return Vec::new();
        };

        let mut spans = Vec::new();
        if let Some(pair) = pairs.into_iter().find(|p| encloses(p.as_span(), offset)) {
            collect_path(pair, offset, text, &mut spans);
        }

        // Collapse the runs of identical spans left by passthrough wrappers
        // (and by whitespace trimming making neighbours coincide).
        spans.dedup();
        spans
    })
}

/// Half-open containment (`start <= offset < end`) that ignores zero-width spans
/// (e.g. `EOI`). Half-open avoids whitespace-padded sibling spans overlapping at
/// token boundaries.
fn encloses(span: pest::Span, offset: usize) -> bool {
    span.start() < span.end() && offset >= span.start() && offset < span.end()
}

/// Push `pair`'s (whitespace-trimmed) span, then descend into the unique child
/// enclosing `offset`. The tree walk uses the raw pest spans; only the emitted
/// spans are trimmed.
fn collect_path(pair: pest::iterators::Pair<Rule>, offset: usize, text: &str, out: &mut Vec<Span>) {
    let raw = pair.as_span();
    if let Some(span) = trim_span(text, raw.start(), raw.end()) {
        out.push(span);
    }

    for child in pair.into_inner() {
        if encloses(child.as_span(), offset) {
            collect_path(child, offset, text, out);
            break;
        }
    }
}

/// Shrink `[start, end)` past surrounding ASCII whitespace. Returns `None` if the
/// range is empty or all whitespace. ASCII whitespace bytes are single-byte and
/// never appear inside a multi-byte UTF-8 sequence, so byte stepping is safe.
fn trim_span(text: &str, mut start: usize, mut end: usize) -> Option<Span> {
    let bytes = text.as_bytes();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    (start < end).then_some(Span { start, end })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte offset of the first occurrence of `needle` in `text`.
    fn at(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present in text")
    }

    fn slice(text: &str, span: &Span) -> String {
        text[span.start..span.end].to_string()
    }

    #[test]
    fn nests_from_token_to_component() {
        let text = "MACHINE m\nINVARIANTS\n  @inv1 x + 1 > 0\nEND\n";
        let spans = enclosing_spans(text, at(text, "x + 1"));

        assert!(!spans.is_empty(), "expected a span stack");

        // Outermost covers the whole machine; innermost is the `x` token.
        assert!(slice(text, &spans[0]).starts_with("MACHINE"));
        assert_eq!(slice(text, spans.last().unwrap()), "x");

        // Strictly shrinking, outermost → innermost.
        for pair in spans.windows(2) {
            let outer = pair[0].end - pair[0].start;
            let inner = pair[1].end - pair[1].start;
            assert!(outer > inner, "spans must strictly shrink: {pair:?}");
        }

        // The subexpression and the full predicate are intermediate steps.
        let slices: Vec<String> = spans.iter().map(|s| slice(text, s)).collect();
        assert!(slices.iter().any(|s| s == "x + 1"), "got {slices:?}");
        assert!(slices.iter().any(|s| s == "x + 1 > 0"), "got {slices:?}");
    }

    #[test]
    fn handles_multibyte_unicode() {
        let text = "CONTEXT c\nAXIOMS\n  @axm1 a ∈ ℕ\nEND\n";

        // Cursor on the ASCII identifier.
        let on_a = enclosing_spans(text, at(text, "a ∈"));
        assert_eq!(slice(text, on_a.last().unwrap()), "a");

        // Cursor on the multi-byte ℕ symbol (3 UTF-8 bytes).
        let on_nat = enclosing_spans(text, at(text, "ℕ"));
        assert_eq!(slice(text, on_nat.last().unwrap()), "ℕ");
    }

    #[test]
    fn unparsable_input_yields_empty() {
        assert!(enclosing_spans("@@@ not an event-b component", 0).is_empty());
    }
}
