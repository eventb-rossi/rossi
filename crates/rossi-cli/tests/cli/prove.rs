//! `rossi prove` — replay-mode coverage: re-running reasoners on
//! their recorded inputs, on top of the dependency-based verdicts.

use crate::helpers::{rossi_command, tempdir_unique};

/// One obligation `evt/inv1/INV` with goal `x=1` and hypothesis `x=1`.
const BPO: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.poFile org.eventb.core.poStamp="0">
<org.eventb.core.poPredicateSet name="ALLHYP" org.eventb.core.poStamp="0">
<org.eventb.core.poIdentifier name="x" org.eventb.core.type="ℤ"/>
<org.eventb.core.poPredicate name="PRD0" org.eventb.core.predicate="x=1"/>
</org.eventb.core.poPredicateSet>
<org.eventb.core.poSequent name="evt/inv1/INV" org.eventb.core.poStamp="0">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x=1"/>
</org.eventb.core.poSequent>
</org.eventb.core.poFile>
"#;

/// A proof closing the obligation with `hyp`: fully replayable.
const BPR_HYP: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.prFile version="1">
<org.eventb.core.prProof name="evt/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.prFresh="" org.eventb.core.prGoal="p0" org.eventb.core.prHyps="p0" org.eventb.core.psManual="false">
<org.eventb.core.prRule name="r0" org.eventb.core.confidence="1000" org.eventb.core.prDisplay="hyp" org.eventb.core.prGoal="p0" org.eventb.core.prHyps="p0"/>
<org.eventb.core.prIdent name="x" org.eventb.core.type="ℤ"/>
<org.eventb.core.prPred name="p0" org.eventb.core.predicate="x=1"/>
<org.eventb.core.prReas name="r0" org.eventb.core.prRID="org.eventb.core.seqprover.hyp"/>
</org.eventb.core.prProof>
</org.eventb.core.prFile>
"#;

/// The same dependencies, but the recorded rule claims `trueGoal` on a
/// goal that is not `⊤`. The dependency check cannot see that (goal
/// and hypotheses still match), so only replay catches it.
const BPR_BOGUS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.prFile version="1">
<org.eventb.core.prProof name="evt/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.prFresh="" org.eventb.core.prGoal="p0" org.eventb.core.prHyps="" org.eventb.core.psManual="false">
<org.eventb.core.prRule name="r0" org.eventb.core.confidence="1000" org.eventb.core.prDisplay="⊤ goal" org.eventb.core.prGoal="p0" org.eventb.core.prHyps=""/>
<org.eventb.core.prIdent name="x" org.eventb.core.type="ℤ"/>
<org.eventb.core.prPred name="p0" org.eventb.core.predicate="x=1"/>
<org.eventb.core.prReas name="r0" org.eventb.core.prRID="org.eventb.core.seqprover.trueGoal"/>
</org.eventb.core.prProof>
</org.eventb.core.prFile>
"#;

fn project_dir(name: &str, bpr: &str) -> std::path::PathBuf {
    let dir = tempdir_unique(name);
    std::fs::write(dir.join("M0.bpo"), BPO).unwrap();
    std::fs::write(dir.join("M0.bpr"), bpr).unwrap();
    dir
}

#[test]
fn prove_replay_rederives_an_implemented_proof() {
    let dir = project_dir("rossi-cli-prove-replay", BPR_HYP);
    let output = rossi_command()
        .args(["prove", "--replay", dir.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("1 discharged"), "{stdout}");
    assert!(
        stdout.contains("Replay: 1 replayed, 0 skipped (unimplemented reasoners), 0 failed"),
        "{stdout}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn prove_replay_fails_a_rule_its_reasoner_rejects() {
    // Without --replay the dependency verdict alone accepts the proof.
    let dir = project_dir("rossi-cli-prove-replay-bogus", BPR_BOGUS);
    let output = rossi_command()
        .args(["prove", dir.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(output.status.success());

    // With --replay the trueGoal reasoner re-runs and rejects the rule.
    let output = rossi_command()
        .args(["prove", "--replay", dir.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("evt/inv1/INV: discharged (replay FAILED)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Replay: 0 replayed, 0 skipped (unimplemented reasoners), 1 failed"),
        "{stdout}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn prove_replay_summarises_an_example_archive() {
    // The bundled archives' proofs come from an automatic prover;
    // replay must never fail on them (proofs whose reasoners are not
    // implemented yet are skipped, and the archive's recorded broken
    // rows drive the exit code, not replay).
    let output = rossi_command()
        .args(["prove", "--replay", "../rossi/examples/binary-search.zip"])
        .output()
        .expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Replay: "), "{stdout}");
    assert!(stdout.contains(", 0 failed"), "{stdout}");
}

#[test]
fn prove_json_lists_every_obligation_with_its_verdict() {
    let dir = project_dir("rossi-cli-prove-json", BPR_HYP);
    let output = rossi_command()
        .args([
            "prove",
            "--replay",
            "--format",
            "json",
            dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("a JSON report");
    assert_eq!(report["summary"]["discharged"], 1, "{report}");
    assert_eq!(report["summary"]["replayed"], 1, "{report}");
    assert_eq!(
        report["obligations"],
        serde_json::json!([{
            "component": "M0",
            "name": "evt/inv1/INV",
            "status": "discharged",
            "replay": {"outcome": "replayed"},
        }]),
        "{report}"
    );
    assert_eq!(report["messages"], serde_json::json!([]), "{report}");
}
