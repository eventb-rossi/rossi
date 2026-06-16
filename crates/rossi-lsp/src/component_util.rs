//! Shared helpers for multi-component documents.
//!
//! A `.eventb` file may contain several `CONTEXT`/`MACHINE` blocks (the
//! output of `rossi import --merge`). Providers that used to parse a document
//! into a single [`Component`] use these helpers instead: parse every
//! component and pick the one under the cursor for position-based features.
//! Cross-file lookups by name go through
//! [`ComponentLoader`](crate::component_loader::ComponentLoader), which parses
//! each file at most once per request.

use rossi::Component;

/// Parse every component in `text`, recovering a partial AST from local syntax
/// errors. A single broken predicate no longer blanks out the whole document:
/// the component it sits in is recovered (sans the broken clause) and its
/// siblings parse normally. Returns an empty vector only when nothing at all
/// could be recovered — there is no `CONTEXT`/`MACHINE` header to anchor on.
pub fn parse_all(text: &str) -> Vec<Component> {
    rossi::parse_components_with_recovery(text)
        .component
        .unwrap_or_default()
}

/// Inclusive line window of a component within `text`, for bounding line-based
/// text searches to that component in a multi-component document. The whole
/// file when the span is missing (XML import, error recovery) — the
/// single-component behavior.
pub fn component_line_window(component: &Component, text: &str) -> (usize, usize) {
    match component.span() {
        Some(span) => (
            text[..span.start.min(text.len())].matches('\n').count(),
            text[..span.end.min(text.len())].matches('\n').count(),
        ),
        None => (0, usize::MAX),
    }
}

/// Iterate `(line_number, line)` pairs of `text` restricted to an inclusive
/// line window (as produced by [`component_line_window`]).
pub fn lines_in_window(text: &str, window: (usize, usize)) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .take(window.1.saturating_add(1))
        .skip(window.0)
}

/// The component containing byte `offset`.
///
/// Falls back gracefully when no span contains the offset (the cursor sits in
/// inter-component whitespace, or a recovered component carries no spans):
/// the last component that starts at or before the offset, else the first
/// component. Returns `None` only for an empty slice.
pub fn component_at_offset(components: &[Component], offset: usize) -> Option<&Component> {
    components
        .iter()
        .find(|c| c.span().is_some_and(|s| s.contains(offset)))
        .or_else(|| {
            components
                .iter()
                .rev()
                .find(|c| c.span().is_some_and(|s| s.start <= offset))
        })
        .or_else(|| components.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_COMPONENTS: &str = "CONTEXT C0\nEND\n\nMACHINE M0\nVARIABLES\n    x\nEND\n";

    #[test]
    fn parse_all_returns_every_component() {
        let components = parse_all(TWO_COMPONENTS);
        let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["C0", "M0"]);
    }

    #[test]
    fn parse_all_returns_empty_on_error() {
        // No CONTEXT/MACHINE header: recovery has nothing to anchor on, so the
        // result is still empty.
        assert!(parse_all("not event-b").is_empty());
    }

    #[test]
    fn parse_all_recovers_partial_components_on_local_error() {
        // A broken predicate (`@a k ∈` with no right-hand side) used to fail
        // the whole strict parse and blank the document. Recovery keeps both
        // components: the broken one (sans the bad axiom) and its sibling.
        let text = "CONTEXT C0\nCONSTANTS\n    k\nAXIOMS\n    @a k ∈\nEND\n\nMACHINE M0\nVARIABLES\n    x\nEND\n";
        let components = parse_all(text);
        let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["C0", "M0"]);
    }

    #[test]
    fn component_at_offset_dispatches_by_position() {
        let components = parse_all(TWO_COMPONENTS);
        let in_c0 = TWO_COMPONENTS.find("C0").unwrap();
        let in_m0 = TWO_COMPONENTS.find("x").unwrap();

        assert_eq!(
            component_at_offset(&components, in_c0).unwrap().name(),
            "C0"
        );
        assert_eq!(
            component_at_offset(&components, in_m0).unwrap().name(),
            "M0"
        );
    }

    #[test]
    fn component_at_offset_gap_binds_to_preceding_component() {
        let components = parse_all(TWO_COMPONENTS);
        // The blank line between the two components.
        let gap = TWO_COMPONENTS.find("\n\n").unwrap() + 1;
        assert_eq!(component_at_offset(&components, gap).unwrap().name(), "C0");
    }

    #[test]
    fn component_at_offset_without_spans_falls_back_to_first() {
        let components = vec![Component::Context(rossi::Context::new("c".into()))];
        assert_eq!(component_at_offset(&components, 42).unwrap().name(), "c");
        assert!(component_at_offset(&[], 0).is_none());
    }
}
