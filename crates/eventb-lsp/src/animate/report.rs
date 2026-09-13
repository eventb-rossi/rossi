//! The verdicts of an eventb-animate run, and the mapping from the tool's
//! printed violated invariants back to the declarations of the closure.
//!
//! Reading the report itself lives in the shared driver crate; this module
//! re-exports its verdict types for the lens flows and keeps the one step
//! that needs the editor's closure.

pub use eventb_animate_driver::report::{PoResult, Report, StateBinding, Verdict};
pub(crate) use eventb_animate_driver::report::{classify_check, classify_po, parse};

use super::closure::{InvariantInfo, normalize_predicate};

/// Map the tool's printed violated-invariant strings back to declarations:
/// whitespace-stripped comparison against the closure's renderings, with a
/// bare-label fallback. Unmatched strings are returned for the section-level
/// fallback diagnostic — a violation is never silently dropped.
///
/// Labels are only unique per machine, so when the bare-label fallback hits
/// several machines of the closure, the clicked `machine` wins — flagging an
/// unrelated same-labeled invariant in an ancestor would point the user at a
/// predicate the counterexample never violated. Identical *renderings* keep
/// all hits: byte-equal predicates really are all violated by the same state.
pub(crate) fn match_violated<'a>(
    violated: &[String],
    invariants: &'a [InvariantInfo],
    machine: &str,
) -> (Vec<&'a InvariantInfo>, Vec<String>) {
    let mut matched: Vec<&InvariantInfo> = Vec::new();
    let mut unmatched = Vec::new();
    for printed in violated {
        let normalized = normalize_predicate(printed);
        let hits: Vec<&InvariantInfo> = invariants
            .iter()
            .filter(|info| info.renderings.contains(&normalized))
            .collect();
        let hits = if hits.is_empty() {
            // The docs' fixtures (and possibly future tool versions) report
            // labels instead of predicate code.
            let label_hits: Vec<&InvariantInfo> = invariants
                .iter()
                .filter(|info| info.label == printed.trim())
                .collect();
            match label_hits.iter().find(|info| info.component == machine) {
                Some(own) if label_hits.len() > 1 => vec![*own],
                _ => label_hits,
            }
        } else {
            hits
        };
        if hits.is_empty() {
            unmatched.push(printed.clone());
            continue;
        }
        for hit in hits {
            if !matched
                .iter()
                .any(|m| m.label == hit.label && m.component == hit.component)
            {
                matched.push(hit);
            }
        }
    }
    (matched, unmatched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::Url;

    fn info(label: &str, component: &str, renderings: &[&str]) -> InvariantInfo {
        InvariantInfo {
            label: label.to_string(),
            component: component.to_string(),
            uri: Url::parse("file:///m.eventb").unwrap(),
            renderings: renderings.iter().map(|r| r.to_string()).collect(),
        }
    }

    #[test]
    fn invariant_matching_is_whitespace_and_rendering_insensitive() {
        let invariants = vec![
            info("inv1", "m0", &["x∈ℕ", "x:NAT"]),
            info("inv2", "m1", &["x<3"]),
        ];
        let (matched, unmatched) = match_violated(
            &[
                "x < 3".to_string(),
                "x : NAT".to_string(),
                "y=0".to_string(),
            ],
            &invariants,
            "m1",
        );
        assert_eq!(
            matched.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
            ["inv2", "inv1"]
        );
        assert_eq!(unmatched, ["y=0"]);
    }

    #[test]
    fn bare_labels_match_as_a_fallback() {
        let invariants = vec![info("inv1", "m", &["x∈ℕ"])];
        let (matched, unmatched) = match_violated(&["inv1".to_string()], &invariants, "m");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].component, "m");
        assert!(unmatched.is_empty());
    }

    #[test]
    fn ambiguous_bare_labels_prefer_the_clicked_machine() {
        // `inv1` is a distinct predicate in each machine of the chain —
        // labels are only unique per machine. Only the clicked machine's
        // declaration may be flagged.
        let invariants = vec![info("inv1", "m0", &["x∈ℕ"]), info("inv1", "m1", &["x<10"])];
        let (matched, unmatched) = match_violated(&["inv1".to_string()], &invariants, "m1");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].component, "m1");
        assert!(unmatched.is_empty());
        // When the clicked machine has no such label, all hits are kept —
        // dropping the finding entirely would hide a real violation.
        let (matched, _) = match_violated(&["inv1".to_string()], &invariants, "m2");
        assert_eq!(matched.len(), 2);
    }
}
