//! Proof-status oracle diff — compares `rossi_build::proofs` against
//! eventb-checker's `--proofs` pass over a corpus of real Rodin archives.
//!
//! Unlike the type-inference catalog, this is a GATE: proof-file scraping is
//! deterministic (no inference involved), so any divergence in the summary
//! counts or in the EB015/EB016 finding multiset fails the test. EB017
//! (proof-file parse error) is compared by count only — the error detail
//! text comes from the XML library and differs between Java and quick-xml.
//!
//! Oracle artifact, fixed in eventb-checker 1.4: older oracles used the
//! JDK's DOM parser, which refuses XML nested deeper than
//! `jdk.xml.maxElementDepth` (default 100) — large Rodin proof trees
//! exceed that (e.g. aman_v7's `M10_GUI.bpr`). Such an oracle drops the
//! whole file via EB017 and misreports its obligations as unattempted;
//! rossi's streaming reader has no such limit. Components hit by the
//! artifact (EB017 message mentioning `JAXP`/`jdk.xml`) are excluded from
//! comparison so the gate still works with pre-1.4 oracles; with 1.4+
//! nothing is excluded and every model compares in full.
//!
//! Ignored by default (needs the `eventb-checker` CLI and a models corpus).
//! Run:
//!
//!   cargo test -p rossi-build --test proof_oracle_diff -- --ignored --nocapture
//!
//! The corpus directory is taken from `EVENTB_CORPUS_DIR` (the test skips
//! when unset). The oracle runs from `PATH`; set `EVENTB_CHECKER` to
//! override.

use std::path::Path;
use std::process::Command;

use rossi_build::proofs;
use rossi_build::rules::RuleId;

mod common;
use common::{collect_zips, eventb_checker_bin, locate_corpus, oracle_available};

#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    total: u64,
    discharged: u64,
    reviewed: u64,
    pending: u64,
    unattempted: u64,
    broken: u64,
}

#[test]
#[ignore = "needs the eventb-checker CLI and a models corpus; run with --ignored"]
fn proof_oracle_diff() {
    let oracle = eventb_checker_bin();
    if !oracle_available(&oracle) {
        eprintln!(
            "SKIP proof_oracle_diff: `{oracle}` not runnable. Install the eventb-checker CLI \
             or set EVENTB_CHECKER to its path."
        );
        return;
    }
    let Some(dir) = locate_corpus() else {
        eprintln!("SKIP proof_oracle_diff: EVENTB_CORPUS_DIR is not set");
        return;
    };
    let zips = collect_zips(&dir);
    if zips.is_empty() {
        eprintln!(
            "SKIP proof_oracle_diff: no .zip models in {}",
            dir.display()
        );
        return;
    }
    eprintln!("oracle: {oracle}");
    eprintln!("corpus: {} zip(s) in {}", zips.len(), dir.display());

    let mut failures = Vec::new();
    for zip in &zips {
        let name = zip.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        match diff_one(&oracle, zip) {
            Ok(()) => eprintln!("  OK   {name}"),
            Err(e) => {
                eprintln!("  FAIL {name}: {e}");
                failures.push(format!("{name}: {e}"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} model(s) diverged from the oracle:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn diff_one(oracle: &str, zip: &Path) -> Result<(), String> {
    let report = proofs::check_zip_file(zip).map_err(|e| format!("rossi: {e}"))?;
    let rodin = rodin_proofs(oracle, zip)?;

    // Components whose proof files only the JDK failed to read: comparing
    // them would gate rossi on a Java parser limit, not on semantics.
    let excluded = &rodin.artifact_components;
    if !excluded.is_empty() {
        eprintln!("       note: oracle hit jdk.xml limits; excluding component(s) {excluded:?}");
    }

    // The oracle's summary counts the artifact components as unattempted,
    // so the summaries are only comparable when no component is excluded.
    if excluded.is_empty() {
        let rossi_summary = report.summary.map_or_else(Counts::default, |s| Counts {
            total: s.total as u64,
            discharged: s.discharged as u64,
            reviewed: s.reviewed as u64,
            pending: s.pending as u64,
            unattempted: s.unattempted as u64,
            broken: s.broken as u64,
        });
        if rossi_summary != rodin.summary {
            return Err(format!(
                "summary mismatch: rossi {rossi_summary:?} vs rodin {:?}",
                rodin.summary
            ));
        }
    }

    let mut rossi_findings: Vec<(String, String, String)> = Vec::new();
    let mut rossi_eb017 = 0usize;
    for d in &report.diagnostics {
        match d.rule_id {
            Some(rule @ (RuleId::UndischargedProof | RuleId::BrokenProof)) => {
                if !excluded.contains(&d.origin) {
                    rossi_findings.push((
                        rule.code().to_string(),
                        d.origin.clone(),
                        d.message.clone(),
                    ));
                }
            }
            Some(RuleId::ProofFileParseError) => rossi_eb017 += 1,
            _ => {}
        }
    }
    rossi_findings.sort();

    if rossi_findings != rodin.findings {
        let rossi_only: Vec<_> = rossi_findings
            .iter()
            .filter(|f| !rodin.findings.contains(f))
            .take(3)
            .collect();
        let rodin_only: Vec<_> = rodin
            .findings
            .iter()
            .filter(|f| !rossi_findings.contains(f))
            .take(3)
            .collect();
        return Err(format!(
            "findings mismatch ({} vs {}); rossi-only sample {rossi_only:?}; rodin-only sample {rodin_only:?}",
            rossi_findings.len(),
            rodin.findings.len()
        ));
    }
    if rossi_eb017 != rodin.real_eb017 {
        return Err(format!(
            "EB017 count mismatch: rossi {rossi_eb017} vs rodin {}",
            rodin.real_eb017
        ));
    }
    Ok(())
}

struct RodinProofs {
    summary: Counts,
    /// Sorted (ruleId, component, message) for EB015/EB016, with
    /// artifact-excluded components already filtered out.
    findings: Vec<(String, String, String)>,
    /// EB017 findings caused by genuinely unparseable files (rossi must
    /// match these in count).
    real_eb017: usize,
    /// Component stems whose proof files only the JDK parser rejected.
    artifact_components: Vec<String>,
}

/// Run `eventb-checker check -p --format json` and extract everything the
/// gate compares.
fn rodin_proofs(oracle: &str, zip: &Path) -> Result<RodinProofs, String> {
    let output = Command::new(oracle)
        .args(["check", "-p", "--format", "json"])
        .arg(zip)
        .output()
        .map_err(|e| format!("spawn {oracle}: {e}"))?;
    // Exit code 1 just means the model has errors; the JSON is still valid.
    if output.stdout.is_empty() {
        return Err(format!(
            "no oracle output (status {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("oracle json: {e}"))?;

    let summary = json
        .pointer("/summary/proofSummary")
        .and_then(|v| v.as_object())
        .map(|obj| {
            let get = |k: &str| obj.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            Counts {
                total: get("total"),
                discharged: get("discharged"),
                reviewed: get("reviewed"),
                pending: get("pending"),
                unattempted: get("unattempted"),
                broken: get("broken"),
            }
        })
        .unwrap_or_default();

    let errors = json
        .get("errors")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let field = |e: &serde_json::Value, k: &str| {
        e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };

    // First pass: split EB017s into JDK artifacts and real parse failures.
    let mut artifact_components = Vec::new();
    let mut real_eb017 = 0usize;
    for e in &errors {
        if field(e, "ruleId") == "EB017" {
            let message = field(e, "message");
            if message.contains("JAXP") || message.contains("jdk.xml") {
                // `M10_GUI.bpr` (possibly path-qualified) → `M10_GUI`.
                let file = field(e, "file");
                let stem = file
                    .rsplit('/')
                    .next()
                    .unwrap_or(&file)
                    .rsplit_once('.')
                    .map_or(file.as_str(), |(s, _)| s)
                    .to_string();
                if !artifact_components.contains(&stem) {
                    artifact_components.push(stem);
                }
            } else {
                real_eb017 += 1;
            }
        }
    }

    let mut findings = Vec::new();
    for e in &errors {
        let rule = field(e, "ruleId");
        if rule == "EB015" || rule == "EB016" {
            let component = field(e, "file");
            if !artifact_components.contains(&component) {
                findings.push((rule, component, field(e, "message")));
            }
        }
    }
    findings.sort();
    Ok(RodinProofs {
        summary,
        findings,
        real_eb017,
        artifact_components,
    })
}
