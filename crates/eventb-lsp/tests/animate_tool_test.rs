//! End-to-end tests of the animate pipeline against the real
//! `eventb-animate` binary: in-memory sources → closure → static check →
//! temp Rodin project → tool run → classified verdict + findings.
//!
//! `#[ignore]` by default: they need eventb-animate 7.x (and a JVM)
//! installed. Run locally:
//!
//!   EVENTB_ANIMATE=<path> cargo test -p eventb-lsp --test animate_tool_test -- --ignored --nocapture --test-threads=1
//!
//! The tool is located via the `EVENTB_ANIMATE` environment variable (the
//! test-only convention shared with rossi-build's animate tests), falling
//! back to `eventb-animate` on PATH. Each test skips silently when the tool
//! is not available. `--test-threads=1` matters: concurrent ProB startups
//! race and crash, while the lens flow itself is single-flight anyway.
//!
//! `check_finds_invariant_violation` doubles as the contract pin for the
//! report's `violatedInvariants` strings: it asserts that the printed
//! predicate maps back to the declaring `@label`, which is the one
//! empirical assumption in the diagnostics mapping.

use std::sync::Arc;

use eventb_lsp::animate::diagnostics::Anchor;
use eventb_lsp::animate::report::Verdict;
use eventb_lsp::animate::{AnimateMode, ExecuteInput, execute};
use eventb_lsp::config::AnimateConfig;
use eventb_lsp::cross_references::CrossReferenceManager;
use eventb_lsp::document::DocumentManager;
use eventb_lsp::lsp_types::Url;

const OK_CTX: &str = "CONTEXT ok_ctx\nCONSTANTS\n    bound\nAXIOMS\n    @axm1 bound = 3\nEND\n";

/// Finite (4 states), deadlock-free, invariant holds: an exhaustive OK.
const OK_MACHINE: &str = "MACHINE ok_m\nSEES\n    ok_ctx\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ 0‥bound\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\n\n    EVENT inc\n    WHERE\n        @grd1 x < bound\n    THEN\n        @act1 x := x + 1\n    END\n\n    EVENT reset\n    WHERE\n        @grd1 x = bound\n    THEN\n        @act1 x := 0\n    END\nEND\n";

/// `inc` is always enabled, so `@inv2 x < 3` breaks after three steps.
const VIOLATING_MACHINE: &str = "MACHINE viol_m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ≥ 0\n    @inv2 x < 3\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\n\n    EVENT inc\n    THEN\n        @act1 x := x + 1\n    END\nEND\n";

/// `step` guards on `x > 5` but `x` starts at 0: an immediate deadlock.
const DEADLOCK_MACHINE: &str = "MACHINE dl_m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\n\n    EVENT step\n    WHERE\n        @grd1 x > 5\n    THEN\n        @act1 x := x + 1\n    END\nEND\n";

/// The INITIALISATION invariant-preservation PO is plainly false
/// (`x := 0` vs `x < 0`): the disprover must find the counterexample.
const DISPROVABLE_MACHINE: &str = "MACHINE po_m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x < 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n";

/// The configured-or-PATH tool, verified runnable via `--version` (which
/// deliberately skips the ProB extraction). `None` skips the test.
fn available_tool() -> Option<String> {
    let configured =
        std::env::var("EVENTB_ANIMATE").unwrap_or_else(|_| "eventb-animate".to_string());
    match std::process::Command::new(&configured)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Some(configured),
        _ => {
            eprintln!("SKIP: eventb-animate is not runnable ({configured})");
            None
        }
    }
}

fn test_uri(filename: &str) -> Url {
    Url::parse(&format!("file:///animate-tool-test/{filename}")).unwrap()
}

/// An [`ExecuteInput`] over in-memory buffers, or `None` (skip) when the
/// tool is not installed.
fn input_with_tool(
    files: &[(&str, &str)],
    clicked: &str,
    machine: &str,
    mode: AnimateMode,
) -> Option<ExecuteInput> {
    let path = available_tool()?;
    let cross_references = Arc::new(CrossReferenceManager::new());
    let documents = Arc::new(DocumentManager::new());
    for (filename, text) in files {
        let uri = test_uri(filename);
        cross_references.update_component(uri.as_str().to_owned(), text);
        documents.open(uri, 1, text.to_string());
    }
    Some(ExecuteInput {
        mode,
        uri: test_uri(clicked),
        machine: machine.to_string(),
        documents,
        cross_references,
        config: AnimateConfig {
            path,
            time_limit_secs: 60,
            ..AnimateConfig::default()
        },
        rodin_project_dir: None,
    })
}

#[tokio::test]
#[ignore]
async fn check_reports_success() {
    let Some(input) = input_with_tool(
        &[("ok_ctx.eventb", OK_CTX), ("ok_m.eventb", OK_MACHINE)],
        "ok_m.eventb",
        "ok_m",
        AnimateMode::Check,
    ) else {
        return;
    };
    let outcome = execute(input).await.expect("the check runs");
    match &outcome.verdict {
        Verdict::CheckOk { reason, states } => {
            assert_eq!(
                reason, "exhaustive",
                "4 states must be exhaustable: {outcome:?}"
            );
            assert_eq!(*states, 4, "0‥3 is four states");
        }
        other => panic!("expected CheckOk, got {other:?}"),
    }
    assert!(
        outcome.findings.is_empty(),
        "a clean verdict must retract findings: {:?}",
        outcome.findings
    );
}

#[tokio::test]
#[ignore]
async fn check_finds_invariant_violation() {
    let Some(input) = input_with_tool(
        &[("viol_m.eventb", VIOLATING_MACHINE)],
        "viol_m.eventb",
        "viol_m",
        AnimateMode::Check,
    ) else {
        return;
    };
    let outcome = execute(input).await.expect("the check runs");
    match &outcome.verdict {
        Verdict::InvariantViolation { violated, .. } => {
            assert!(
                !violated.is_empty(),
                "the report lists the violated predicate"
            );
        }
        other => panic!("expected InvariantViolation, got {other:?}"),
    }
    // The contract pin: the tool's printed predicate must map back to the
    // declaring @label, not fall through to the section-level fallback.
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.anchor == Anchor::InvariantLabel("inv2".to_string())
                && f.component == "viol_m"),
        "expected an inv2-anchored finding, got {:?}",
        outcome.findings
    );
}

#[tokio::test]
#[ignore]
async fn check_finds_deadlock() {
    let Some(input) = input_with_tool(
        &[("dl_m.eventb", DEADLOCK_MACHINE)],
        "dl_m.eventb",
        "dl_m",
        AnimateMode::Check,
    ) else {
        return;
    };
    let outcome = execute(input).await.expect("the check runs");
    assert!(
        matches!(outcome.verdict, Verdict::Deadlock { .. }),
        "expected Deadlock, got {:?}",
        outcome.verdict
    );
    assert_eq!(outcome.findings.len(), 1);
    assert_eq!(outcome.findings[0].code, "animate-deadlock");
}

#[tokio::test]
#[ignore]
async fn po_disprove_finds_counterexample() {
    let Some(input) = input_with_tool(
        &[("po_m.eventb", DISPROVABLE_MACHINE)],
        "po_m.eventb",
        "po_m",
        AnimateMode::Po,
    ) else {
        return;
    };
    let outcome = execute(input).await.expect("the po gate runs");
    match &outcome.verdict {
        Verdict::PoDisproved { disproved, .. } => {
            assert!(
                disproved
                    .iter()
                    .any(|po| po.name.contains("INITIALISATION") && po.name.contains("inv1")),
                "expected the INITIALISATION inv1 PO, got {disproved:?}"
            );
        }
        other => panic!("expected PoDisproved, got {other:?}"),
    }
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.anchor == Anchor::InvariantLabel("inv1".to_string())),
        "the disproved PO must anchor on @inv1: {:?}",
        outcome.findings
    );
}

#[tokio::test]
#[ignore]
async fn po_disprove_reports_open_pos_without_failure() {
    let Some(input) = input_with_tool(
        &[("ok_ctx.eventb", OK_CTX), ("ok_m.eventb", OK_MACHINE)],
        "ok_m.eventb",
        "ok_m",
        AnimateMode::Po,
    ) else {
        return;
    };
    let outcome = execute(input).await.expect("the po gate runs");
    // Every PO of the OK machine is true: the disprover finds no
    // counterexample. Whether the solver proves them all (PoOk) or some
    // stay open within the timeout (PoNoCounterexample) is solver-
    // dependent; both are non-failures and neither produces findings.
    assert!(
        matches!(
            outcome.verdict,
            Verdict::PoOk { .. } | Verdict::PoNoCounterexample { .. }
        ),
        "expected a non-failure po verdict, got {:?}",
        outcome.verdict
    );
    assert!(
        outcome.findings.is_empty(),
        "open-but-not-disproved POs must not become diagnostics: {:?}",
        outcome.findings
    );
}

/// Build `machine_text` with rossi-build and return its generated
/// `(bpo, bps)` contents. Same generator as the lens; the project name
/// deliberately differs ("fixture" vs the lens's "rossi_animate"), pinning
/// that the reconcile comparison is project-independent (`PoView` strips
/// the leading `/PROJECT/` handle segment).
fn generate_proof_files(machine_text: &str, component: &str) -> (String, String) {
    let components = rossi::parse_components(machine_text).unwrap();
    let xml = rossi::to_xml(&components[0]);
    let project_component =
        rossi_build::ProjectComponent::from_xml(format!("{component}.bum"), &xml).unwrap();
    let project = rossi_build::Project::new("fixture", vec![project_component]);
    let result = rossi_build::build(&project);
    let file = |name: &str| result.file(name).expect(name).contents.clone();
    (
        file(&format!("{component}.bpo")),
        file(&format!("{component}.bps")),
    )
}

/// A recorded proof state claiming every obligation of `machine_text` is
/// discharged, written into a temp directory posing as the shared Rodin
/// workspace project.
fn discharged_fixture(machine_text: &str, component: &str) -> tempfile::TempDir {
    let fixture = tempfile::Builder::new()
        .prefix("animate-recorded-proofs-")
        .tempdir()
        .unwrap();
    let (bpo, bps) = generate_proof_files(machine_text, component);
    std::fs::write(fixture.path().join(format!("{component}.bpo")), bpo).unwrap();
    std::fs::write(
        fixture.path().join(format!("{component}.bps")),
        bps.replace("confidence=\"-99\"", "confidence=\"1000\""),
    )
    .unwrap();
    fixture
}

#[tokio::test]
#[ignore]
async fn po_disprove_trusts_recorded_discharges() {
    let Some(mut input) = input_with_tool(
        &[("po_m.eventb", DISPROVABLE_MACHINE)],
        "po_m.eventb",
        "po_m",
        AnimateMode::Po,
    ) else {
        return;
    };
    // The recorded state claims the (actually false) INITIALISATION PO is
    // discharged, and the buffer matches the recorded model — so the gate
    // must trust it and never run the disprover, which would disprove it.
    let fixture = discharged_fixture(DISPROVABLE_MACHINE, "po_m");
    input.rodin_project_dir = Some(fixture.path().to_path_buf());

    let outcome = execute(input).await.expect("the po gate runs");
    assert!(
        matches!(outcome.verdict, Verdict::PoOk { .. }),
        "a recorded discharge must skip the disprover, got {:?}",
        outcome.verdict
    );
    assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);
}

#[tokio::test]
#[ignore]
async fn po_disprove_reattempts_stale_discharges() {
    // The recorded state was made for `x := 0`, but the buffer now says
    // `x := 1`: the INV sequent changed, so the stamp guard resets the
    // recorded discharge and the disprover finds the counterexample —
    // a stale proof can never mask a disproof.
    let edited = DISPROVABLE_MACHINE.replace("x := 0", "x := 1");
    let Some(mut input) = input_with_tool(
        &[("po_m.eventb", edited.as_str())],
        "po_m.eventb",
        "po_m",
        AnimateMode::Po,
    ) else {
        return;
    };
    let fixture = discharged_fixture(DISPROVABLE_MACHINE, "po_m");
    input.rodin_project_dir = Some(fixture.path().to_path_buf());

    let outcome = execute(input).await.expect("the po gate runs");
    match &outcome.verdict {
        Verdict::PoDisproved { disproved, .. } => {
            assert!(
                disproved
                    .iter()
                    .any(|po| po.name.contains("INITIALISATION") && po.name.contains("inv1")),
                "expected the INITIALISATION inv1 PO, got {disproved:?}"
            );
        }
        other => panic!("expected PoDisproved, got {other:?}"),
    }
}
