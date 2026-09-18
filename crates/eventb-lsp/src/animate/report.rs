//! Deserialization and classification of eventb-animate's JSON report v4.
//!
//! Every run uses `--json -`: stdout carries exactly one JSON document and
//! all human output goes to stderr. The report's `status`/`finding` carry the
//! verdict; since 7.0 the exit code names the *failure* kind on top of that —
//! 66 for an input the tool could not use, 70 for its own failure — so the two
//! are read together. A usage error writes no report at all and is reported
//! with the stderr tail.

use serde::Deserialize;

use super::AnimateError;
use super::closure::{InvariantInfo, normalize_predicate};

/// The subset of the format-4 report the lens flows consume. Tolerant by
/// construction: unknown fields are ignored and missing ones default, so
/// point releases of the tool cannot break classification.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Report {
    pub format_version: u32,
    pub tool: String,
    pub status: String,
    pub message: Option<String>,
    pub completion: Option<Completion>,
    pub search_statistics: Option<SearchStatistics>,
    pub checks: Vec<Check>,
    pub finding: Option<ReportFinding>,
    pub counterexample: Option<Counterexample>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Completion {
    pub phase: String,
    pub reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SearchStatistics {
    pub states_discovered: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Check {
    pub name: String,
    pub outcome: String,
    pub message: Option<String>,
    pub bindings: Vec<StateBinding>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ReportFinding {
    pub category: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Counterexample {
    pub transitions: Vec<String>,
    pub violating_state: String,
    pub violated_invariants: Vec<String>,
    pub bindings: Vec<StateBinding>,
}

/// One identifier of the violating state with the tool's rendering of its
/// value — the structured form of `violatingState`, absent from reports of
/// older tool versions.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct StateBinding {
    pub name: String,
    pub value: String,
}

/// One classified run, whichever mode produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Check: no violation found. `reason == "exhaustive"` distinguishes a
    /// full state space from a `--states`/`--time-limit` bounded pass.
    CheckOk { reason: String, states: u64 },
    /// Check: the search ended without a verdict.
    CheckIncomplete { reason: String },
    /// Check: an invariant broke; `violated` are the tool's printed
    /// predicate strings, `bindings` the structured state (empty from
    /// older tools).
    InvariantViolation {
        violated: Vec<String>,
        state: String,
        bindings: Vec<StateBinding>,
        steps: usize,
    },
    /// Check: a reachable state enables no event.
    Deadlock {
        state: String,
        bindings: Vec<StateBinding>,
        steps: usize,
    },
    /// Check: any other finding category (unreachable with the lens's flag
    /// set, but the report may still say so).
    OtherFinding { category: String, message: String },
    /// The model never loaded (`status == "error"`, `phase == "load"`).
    LoadError { message: String },
    /// Any other tool-side failure.
    EngineError { message: String },
    /// po: at least one obligation was definitely disproved.
    PoDisproved {
        disproved: Vec<PoResult>,
        total: usize,
    },
    /// po: obligations remain open but none could be disproved.
    PoNoCounterexample {
        open: usize,
        total: usize,
        /// Counterexamples under the selected hypotheses only (may be
        /// spurious) — reported in the verdict message, never as errors.
        spurious: usize,
    },
    /// po: every obligation passed the gate.
    PoOk { message: String },
    /// po: the gate itself failed (missing proof files, solver breakdown).
    PoError { message: String },
}

/// One disproved obligation: its qualified name
/// (`M1/INITIALISATION/inv4/INV`), the tool's counterexample message, and
/// the structured counterexample valuation (empty from older tools).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoResult {
    pub name: String,
    pub message: String,
    pub bindings: Vec<StateBinding>,
}

/// Parse stdout as a format-4 report, reporting the stderr tail when there
/// is none (usage errors and crashes write nothing to stdout).
pub(crate) fn parse(stdout: &str, stderr: &str) -> Result<Report, AnimateError> {
    let report: Report = serde_json::from_str(stdout).map_err(|_| {
        AnimateError::ToolFailed(format!(
            "no JSON report on stdout ({})",
            excerpt(if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            })
        ))
    })?;
    if report.format_version != 4 || report.tool != "eventb-animate" {
        return Err(AnimateError::ToolFailed(format!(
            "unexpected report shape (formatVersion {}, tool '{}')",
            report.format_version, report.tool
        )));
    }
    Ok(report)
}

/// The last part of a failed run's output, for the error message. The
/// excerpt rides the server log (`window/logMessage`), not a toast, so it
/// can afford enough of a Java stack trace to diagnose from.
fn excerpt(text: &str) -> String {
    if text.trim().is_empty() {
        return "no output".to_string();
    }
    crate::text_utils::output_excerpt(text, 2000)
}

fn report_message(report: &Report) -> String {
    report.message.clone().unwrap_or_default()
}

/// Classify a check-mode report. The vocabulary (`status`,
/// `finding.category`, `completion.phase == "load"`) is the same one the
/// corpus classifier in `crates/rossi-build/tests/animate_corpus.rs` reads;
/// the two decoders are separate code, so a vocabulary change must be
/// applied to both. `code` is the process exit code, which since 7.0 tells
/// an unusable input (66) from a tool failure (70).
pub(crate) fn classify_check(report: &Report, code: Option<i32>) -> Verdict {
    let reason = report
        .completion
        .as_ref()
        .map(|c| c.reason.clone())
        .unwrap_or_default();
    match report.status.as_str() {
        "ok" => Verdict::CheckOk {
            states: report
                .search_statistics
                .as_ref()
                .map_or(0, |s| s.states_discovered),
            reason,
        },
        "incomplete" => Verdict::CheckIncomplete { reason },
        "violation" => {
            let category = report
                .finding
                .as_ref()
                .map(|f| f.category.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let (violated, state, bindings, steps) = report
                .counterexample
                .as_ref()
                .map(|cx| {
                    (
                        cx.violated_invariants.clone(),
                        cx.violating_state.clone(),
                        cx.bindings.clone(),
                        cx.transitions.len(),
                    )
                })
                .unwrap_or_default();
            match category.as_str() {
                "invariant_violation" => Verdict::InvariantViolation {
                    violated,
                    state,
                    bindings,
                    steps,
                },
                "deadlock" => Verdict::Deadlock {
                    state,
                    bindings,
                    steps,
                },
                _ => Verdict::OtherFinding {
                    category,
                    message: report_message(report),
                },
            }
        }
        "error" => {
            let message = report_message(report);
            match code {
                Some(66) => Verdict::LoadError { message },
                Some(70) => Verdict::EngineError { message },
                // No code means a signal killed the tool; fall back to the
                // phase, which carries the same split inside the report.
                _ => {
                    if report
                        .completion
                        .as_ref()
                        .is_some_and(|c| c.phase == "load")
                    {
                        Verdict::LoadError { message }
                    } else {
                        Verdict::EngineError { message }
                    }
                }
            }
        }
        other => Verdict::EngineError {
            message: format!("unexpected report status '{other}'"),
        },
    }
}

/// Classify a po-mode report. A *disproof* is a `failed` check whose message
/// starts with `disproved` — the only definite negative the disprover emits;
/// `no counterexample found …` and `counterexample under the selected
/// hypotheses only …` checks stay open, which on an all-unattempted temp
/// build is the expected steady state, not a failure.
pub(crate) fn classify_po(report: &Report) -> Verdict {
    let total = report.checks.len();
    let disproved: Vec<PoResult> = report
        .checks
        .iter()
        .filter(|c| c.outcome == "failed" && check_message(c).starts_with("disproved"))
        .map(|c| PoResult {
            name: c.name.clone(),
            message: check_message(c).to_string(),
            bindings: c.bindings.clone(),
        })
        .collect();
    let spurious = report
        .checks
        .iter()
        .filter(|c| {
            c.outcome == "failed"
                && check_message(c).starts_with("counterexample under the selected hypotheses")
        })
        .count();
    match report.status.as_str() {
        "violation" if !disproved.is_empty() => Verdict::PoDisproved { disproved, total },
        "ok" => Verdict::PoOk {
            message: report_message(report),
        },
        // A "violation" whose failed checks carry no definite disproof (only
        // spurious/open ones, or a reworded disproof message) degrades to the
        // conservative no-counterexample verdict instead of the catch-all
        // error arm below.
        "violation" | "incomplete" => Verdict::PoNoCounterexample {
            open: report
                .checks
                .iter()
                .filter(|c| c.outcome != "passed")
                .count(),
            total,
            spurious,
        },
        "error" => Verdict::PoError {
            message: report_message(report),
        },
        other => Verdict::PoError {
            message: format!("unexpected report status '{other}'"),
        },
    }
}

fn check_message(check: &Check) -> &str {
    check.message.as_deref().unwrap_or_default()
}

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
    use crate::lsp_types::Uri;

    // Trimmed from docs/examples/json-report-v4-exhaustive.json in the
    // eventb-animate repository.
    const EXHAUSTIVE: &str = r#"{
        "formatVersion": 4, "tool": "eventb-animate", "toolVersion": "7.0",
        "command": "check", "machine": "M2", "status": "ok",
        "completion": {"classification": "complete", "phase": "search", "reason": "exhaustive"},
        "searchStatistics": {"statesDiscovered": 15, "statesProcessed": 15, "transitions": 34},
        "exitCode": 0,
        "message": "No invariant violation or deadlock found (full state space explored).",
        "checks": [
            {"name": "invariant", "outcome": "passed"},
            {"name": "deadlock", "outcome": "passed"}
        ]
    }"#;

    // Trimmed from docs/examples/json-report-v4-counterexample.json.
    const COUNTEREXAMPLE: &str = r#"{
        "formatVersion": 4, "tool": "eventb-animate", "toolVersion": "7.0",
        "command": "check", "machine": "M1", "status": "violation",
        "completion": {"classification": "counterexample", "phase": "search", "reason": "property_violation"},
        "searchStatistics": {"statesDiscovered": 4, "statesProcessed": 3, "transitions": 5},
        "exitCode": 1, "message": "Invariant violation found.",
        "checks": [
            {"name": "invariant", "outcome": "failed", "message": "Invariant violation found."},
            {"name": "deadlock", "outcome": "skipped", "message": "search stopped at the first violation"}
        ],
        "finding": {"category": "invariant_violation", "check": "invariant"},
        "counterexample": {
            "transitions": ["INITIALISATION()", "event()"],
            "violatingState": "(x = 1)",
            "violatedInvariants": ["inv1"],
            "bindings": [{"name": "x", "value": "1"}]
        }
    }"#;

    #[test]
    fn classifies_exhaustive_ok() {
        let report = parse(EXHAUSTIVE, "").unwrap();
        assert_eq!(
            classify_check(&report, None),
            Verdict::CheckOk {
                reason: "exhaustive".into(),
                states: 15
            }
        );
    }

    #[test]
    fn classifies_state_limited_ok_as_bounded() {
        let bounded =
            EXHAUSTIVE.replace("\"reason\": \"exhaustive\"", "\"reason\": \"state_limit\"");
        let report = parse(&bounded, "").unwrap();
        match classify_check(&report, None) {
            Verdict::CheckOk { reason, .. } => assert_eq!(reason, "state_limit"),
            other => panic!("expected bounded ok, got {other:?}"),
        }
    }

    #[test]
    fn classifies_counterexample() {
        let report = parse(COUNTEREXAMPLE, "").unwrap();
        assert_eq!(
            classify_check(&report, None),
            Verdict::InvariantViolation {
                violated: vec!["inv1".into()],
                state: "(x = 1)".into(),
                bindings: vec![StateBinding {
                    name: "x".into(),
                    value: "1".into()
                }],
                steps: 2
            }
        );
    }

    #[test]
    fn a_report_without_bindings_classifies_with_none() {
        // Older tool versions do not emit `bindings`; the unknown spelling
        // also pins the deserializer's ignore-unknown-fields tolerance.
        let legacy = COUNTEREXAMPLE.replace("\"bindings\"", "\"legacyBindings\"");
        let report = parse(&legacy, "").unwrap();
        match classify_check(&report, None) {
            Verdict::InvariantViolation { bindings, .. } => assert!(bindings.is_empty()),
            other => panic!("expected InvariantViolation, got {other:?}"),
        }
    }

    #[test]
    fn classifies_load_failure_and_engine_errors() {
        let load = r#"{
            "formatVersion": 4, "tool": "eventb-animate", "command": "check",
            "status": "error", "message": "Error loading model",
            "completion": {"classification": "none", "phase": "load", "reason": "load_error"}
        }"#;
        let report = parse(load, "").unwrap();
        // 66 (EX_NOINPUT) and 70 (EX_SOFTWARE) name the failure kind directly.
        assert_eq!(
            classify_check(&report, Some(66)),
            Verdict::LoadError {
                message: "Error loading model".into()
            }
        );
        assert!(matches!(
            classify_check(&report, Some(70)),
            Verdict::EngineError { .. }
        ));

        // Killed by a signal: the completion phase still carries the split.
        assert!(matches!(
            classify_check(&report, None),
            Verdict::LoadError { .. }
        ));
        let engine = load.replace("\"phase\": \"load\"", "\"phase\": \"search\"");
        assert!(matches!(
            classify_check(&parse(&engine, "").unwrap(), None),
            Verdict::EngineError { .. }
        ));
    }

    #[test]
    fn missing_report_carries_the_stderr_tail() {
        let error = parse("", "Error: Unmatched argument at index 1: 'bogus'").unwrap_err();
        match error {
            AnimateError::ToolFailed(message) => {
                assert!(message.contains("Unmatched argument"), "{message}");
            }
            other => panic!("expected ToolFailed, got {other:?}"),
        }
    }

    #[test]
    fn foreign_reports_are_rejected() {
        assert!(parse(r#"{"formatVersion": 3, "tool": "eventb-animate"}"#, "").is_err());
        assert!(parse(r#"{"formatVersion": 4, "tool": "other"}"#, "").is_err());
    }

    fn po_report(status: &str, checks: &str) -> Report {
        parse(
            &format!(
                r#"{{"formatVersion": 4, "tool": "eventb-animate", "command": "po",
                     "status": "{status}", "message": "gate message", "checks": [{checks}]}}"#
            ),
            "",
        )
        .unwrap()
    }

    #[test]
    fn po_disproved_only_counts_definite_disproofs() {
        // The four disprover outcomes the tool emits for open obligations
        // (PoCommand.classify), plus a discharged one.
        let checks = r#"
            {"name": "M/INITIALISATION/inv1/INV", "outcome": "failed",
             "message": "disproved (counterexample: x = 0)",
             "bindings": [{"name": "x", "value": "0"}]},
            {"name": "M/evt/inv1/INV", "outcome": "failed",
             "message": "no counterexample found (solver timeout after 1000 ms)"},
            {"name": "M/evt/grd1/GRD", "outcome": "failed",
             "message": "counterexample under the selected hypotheses only (may be spurious): y = 1"},
            {"name": "M/evt/act1/SIM", "outcome": "error",
             "message": "solver error: unsupported formula"},
            {"name": "M/old/INV", "outcome": "passed", "message": "discharged"}"#;
        match classify_po(&po_report("violation", checks)) {
            Verdict::PoDisproved { disproved, total } => {
                assert_eq!(disproved.len(), 1);
                assert_eq!(disproved[0].name, "M/INITIALISATION/inv1/INV");
                assert_eq!(
                    disproved[0].bindings,
                    vec![StateBinding {
                        name: "x".into(),
                        value: "0".into()
                    }]
                );
                assert_eq!(total, 5);
            }
            other => panic!("expected PoDisproved, got {other:?}"),
        }

        // Without a disproof, incomplete counts open and spurious checks.
        let open_only = r#"
            {"name": "M/evt/inv1/INV", "outcome": "failed",
             "message": "no counterexample found (solver timeout after 1000 ms)"},
            {"name": "M/evt/grd1/GRD", "outcome": "failed",
             "message": "counterexample under the selected hypotheses only (may be spurious)"},
            {"name": "M/old/INV", "outcome": "passed", "message": "discharged"}"#;
        assert_eq!(
            classify_po(&po_report("incomplete", open_only)),
            Verdict::PoNoCounterexample {
                open: 2,
                total: 3,
                spurious: 1
            }
        );

        assert_eq!(
            classify_po(&po_report(
                "ok",
                r#"{"name": "M/x/INV", "outcome": "passed"}"#
            )),
            Verdict::PoOk {
                message: "gate message".into()
            }
        );
        assert!(matches!(
            classify_po(&po_report("error", "")),
            Verdict::PoError { .. }
        ));
    }

    fn info(label: &str, component: &str, renderings: &[&str]) -> InvariantInfo {
        InvariantInfo {
            label: label.to_string(),
            component: component.to_string(),
            uri: ("file:///m.eventb").parse::<Uri>().unwrap(),
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

    #[test]
    fn po_violation_without_definite_disproof_stays_conservative() {
        // status "violation" but no `disproved …` check: the guarded arm is
        // skipped and the verdict must degrade to no-counterexample, not to
        // an "unexpected report status" error.
        let spurious_only = r#"
            {"name": "M/evt/grd1/GRD", "outcome": "failed",
             "message": "counterexample under the selected hypotheses only (may be spurious)"},
            {"name": "M/old/INV", "outcome": "passed", "message": "discharged"}"#;
        assert_eq!(
            classify_po(&po_report("violation", spurious_only)),
            Verdict::PoNoCounterexample {
                open: 1,
                total: 2,
                spurious: 1
            }
        );
    }
}
