//! Shared helpers for multi-component documents.
//!
//! A `.eventb` file may contain several `CONTEXT`/`MACHINE` blocks (the
//! output of `rossi import --merge`). Providers that used to parse a document
//! into a single [`Component`] use these helpers instead: parse every
//! component, pick the one under the cursor for position-based features, or
//! pick one by name for cross-file lookups.

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

/// Parse `text` and return the component named `name`, if any. Recovers from
/// local errors, so the component is still found when it — or a sibling in the
/// same file — fails a strict parse. A component whose header is too broken to
/// read keeps its placeholder name and so is simply not matched.
pub fn parse_named(text: &str, name: &str) -> Option<Component> {
    rossi::parse_components_with_recovery(text)
        .component?
        .into_iter()
        .find(|c| c.name() == name)
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
    fn parse_named_finds_component_despite_local_error() {
        // `parse_named` resolves a component even when the file does not parse
        // strictly — here the named machine is healthy but its sibling context
        // has a broken axiom.
        let text = "CONTEXT C0\nAXIOMS\n    @a k ∈\nEND\n\nMACHINE M0\nVARIABLES\n    x\nEND\n";
        let component = parse_named(text, "M0").expect("M0 resolves despite C0's error");
        assert!(matches!(component, Component::Machine(_)));
    }

    #[test]
    fn parse_named_finds_later_component() {
        let component = parse_named(TWO_COMPONENTS, "M0").unwrap();
        assert!(matches!(component, Component::Machine(_)));
        assert!(parse_named(TWO_COMPONENTS, "missing").is_none());
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
