//! The verdicts of an eventb-animate run, and the mapping from the tool's
//! printed violated invariants back to the declarations of the closure.
//!
//! Reading the report itself lives in the shared driver crate; this module
//! re-exports its verdict types for the lens flows and keeps the one step
//! that needs the editor's closure.

pub use eventb_animate_driver::report::{PoResult, Report, StateBinding, Verdict};
pub(crate) use eventb_animate_driver::report::{classify_check, classify_po, parse};

use super::closure::InvariantInfo;

/// Map the tool's printed violated-invariant strings back to the
/// closure's declarations, and collect the strings nothing matched so a
/// violation is never silently dropped.
///
/// The matching rule itself is the driver's
/// ([`eventb_animate_driver::report::match_violated`]); this keeps the
/// editor's shape, where every hit becomes one diagnostic and the
/// leftovers become a section-level one.
pub(crate) fn match_violated<'a>(
    violated: &[String],
    invariants: &'a [InvariantInfo],
    machine: &str,
) -> (Vec<&'a InvariantInfo>, Vec<String>) {
    let mut matched: Vec<&InvariantInfo> = Vec::new();
    let mut unmatched = Vec::new();
    let hits = eventb_animate_driver::report::match_violated(violated, invariants, machine);
    for (printed, hits) in violated.iter().zip(hits) {
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
