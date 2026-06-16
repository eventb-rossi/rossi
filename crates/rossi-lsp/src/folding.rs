//! Folding Range provider for Event-B.
//!
//! Folds are derived from the parsed AST — the single source of truth — so the
//! block structure never has to be re-derived by line scanning. For each
//! component the provider emits:
//! - the component block (`CONTEXT`/`MACHINE` … `END`), from its span;
//! - each clause section (`SETS`, `VARIABLES`, `INVARIANTS`, `EVENTS`, …), from
//!   its recorded [`rossi::ast::ClauseRegion`];
//! - each event and the `INITIALISATION`, from their spans.
//!
//! The server feeds the document's shared, recovery-tolerant parse, so folds
//! survive a local syntax error instead of collapsing with it.

use crate::lsp_types::{FoldingRange, FoldingRangeKind};
use crate::position::span_to_range;
use rossi::ast::{Component, Machine, Span};

/// Provides folding ranges for Event-B documents.
pub struct FoldingRangeProvider;

impl FoldingRangeProvider {
    pub fn new() -> Self {
        Self
    }

    /// Folding ranges for already-parsed components.
    ///
    /// The production entry point: the server passes the document's shared parse
    /// (and the exact text it was produced from), so folding never re-parses.
    /// `source` must be the text those components' spans index into.
    pub fn folding_ranges_from_components(
        &self,
        components: &[Component],
        source: &str,
    ) -> Option<Vec<FoldingRange>> {
        let mut ranges = Vec::new();
        for component in components {
            collect_component_folds(component, source, &mut ranges);
        }
        (!ranges.is_empty()).then_some(ranges)
    }

    /// Convenience: parse `text` with recovery, then fold it.
    ///
    /// Used by tests and any caller without a shared parse at hand; the server
    /// uses [`Self::folding_ranges_from_components`] with the cached parse.
    pub fn folding_ranges(&self, text: &str) -> Option<Vec<FoldingRange>> {
        let parse = rossi::parse_components_with_recovery(text);
        let components = parse.component.as_deref().unwrap_or_default();
        self.folding_ranges_from_components(components, text)
    }
}

impl Default for FoldingRangeProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Append a `Region` fold for `span`, but only when it covers more than one line
/// (a single-line span has nothing to collapse).
///
/// Component, clause, and event spans are recorded line-tight by the parser (it
/// drops trailing whitespace), so the span maps straight to a fold range.
fn push_span(span: Span, source: &str, ranges: &mut Vec<FoldingRange>) {
    let range = span_to_range(&span, source);
    if range.end.line > range.start.line {
        ranges.push(FoldingRange {
            start_line: range.start.line,
            start_character: None,
            end_line: range.end.line,
            end_character: None,
            kind: Some(FoldingRangeKind::Region),
            collapsed_text: None,
        });
    }
}

/// Emit the block, clause, and event folds for one component.
fn collect_component_folds(component: &Component, source: &str, ranges: &mut Vec<FoldingRange>) {
    if let Some(span) = component.span() {
        push_span(span, source, ranges);
    }

    for clause in component.clauses() {
        push_span(clause.span, source, ranges);
    }

    if let Component::Machine(machine) = component {
        collect_event_folds(machine, source, ranges);
    }
}

/// Emit a fold for the `INITIALISATION` and for each event.
fn collect_event_folds(machine: &Machine, source: &str, ranges: &mut Vec<FoldingRange>) {
    if let Some(span) = machine.initialisation.as_ref().and_then(|init| init.span) {
        push_span(span, source, ranges);
    }
    for event in &machine.events {
        if let Some(span) = event.span {
            push_span(span, source, ranges);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folds(text: &str) -> Vec<FoldingRange> {
        FoldingRangeProvider::new()
            .folding_ranges(text)
            .unwrap_or_default()
    }

    fn has(ranges: &[FoldingRange], start: u32, end: u32) -> bool {
        ranges
            .iter()
            .any(|r| r.start_line == start && r.end_line == end)
    }

    #[test]
    fn folds_context_block_and_clauses() {
        // 0 CONTEXT c | 1 SETS | 2 S | 3 T | 4 CONSTANTS k | 5 END
        let ranges = folds("CONTEXT c\nSETS\n    S\n    T\nCONSTANTS k\nEND");
        assert!(has(&ranges, 0, 5), "context block; got {ranges:?}");
        assert!(has(&ranges, 1, 3), "sets clause; got {ranges:?}");
        // A single-line clause has nothing to collapse.
        assert!(
            !ranges.iter().any(|r| r.start_line == 4),
            "single-line CONSTANTS must not fold; got {ranges:?}"
        );
    }

    #[test]
    fn machine_block_reaches_its_own_end() {
        // Regression: the machine block must reach the machine END, not stop at
        // the first nested event END (the historic line-scanning bug).
        let text = "machine m\n\
            variables\n    x\n\
            invariants\n    @i x > 0\n\
            events\n\
            \x20   event INITIALISATION\n    then\n        @a x := 0\n    end\n\
            \x20   event step\n    then\n        @b x := 0\n    end\n\
            end";
        let ranges = folds(text);
        let last = text.lines().count() as u32 - 1;
        assert!(
            has(&ranges, 0, last),
            "machine block must span to its own END (line {last}); got {ranges:?}"
        );
    }

    #[test]
    fn folds_events_clause_and_each_event() {
        // 0 machine | 1 events | 2 event evt | 3 then | 4 @a x := 1 | 5 end | 6 end
        let text = "machine m\nevents\n    event evt\n    then\n        @a x := 1\n    end\nend";
        let ranges = folds(text);
        assert!(has(&ranges, 0, 6), "machine block; got {ranges:?}");
        // EVENTS clause ends at the last event's END (5), not the machine END (6).
        assert!(has(&ranges, 1, 5), "events clause; got {ranges:?}");
        assert!(has(&ranges, 2, 5), "event block; got {ranges:?}");
    }

    #[test]
    fn folds_initialisation() {
        // 0 machine | 1 events | 2 event INITIALISATION | 3 then | 4 @a x := 0 | 5 end | 6 end
        let text = "machine m\nevents\n    event INITIALISATION\n    then\n        @a x := 0\n    end\nend";
        let ranges = folds(text);
        assert!(has(&ranges, 2, 5), "initialisation block; got {ranges:?}");
    }

    #[test]
    fn folds_survive_a_syntax_error() {
        // The broken invariant forces recovery; folds for the block, the clauses,
        // and the event must still appear (the point of folding from the
        // recovery-tolerant parse).
        let text = "machine m\n\
            variables\n    x\n\
            invariants\n    @i broken @#$ syntax\n\
            events\n\
            \x20   event step\n    then\n        @a x := 0\n    end\n\
            end";
        let ranges = folds(text);
        let last = text.lines().count() as u32 - 1;
        assert!(
            has(&ranges, 0, last),
            "machine block must fold despite the error; got {ranges:?}"
        );
        // 6 event step | 7 then | 8 @a x := 0 | 9 end
        assert!(
            has(&ranges, 6, 9),
            "the recoverable event must still fold; got {ranges:?}"
        );
    }

    #[test]
    fn all_ranges_are_region_kind() {
        let text = "machine m\nvariables\n    x\n    y\ninvariants\n    @i x > 0\nend";
        for range in folds(text) {
            assert_eq!(range.kind, Some(FoldingRangeKind::Region));
        }
    }

    #[test]
    fn empty_document_has_no_folds() {
        assert!(FoldingRangeProvider::new().folding_ranges("").is_none());
    }
}
