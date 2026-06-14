//! Corpus integration test — regenerate every model with our static checker,
//! then load it in ProB via the `eventb-animate` CLI and compare the outcome
//! (`success` / `invariant_violation` / `load_error` / `deadlock` / `timeout`)
//! against the reference `animate_results.tsv`.
//!
//! Requires `eventb-animate` v5.0+, which exits 0 on success and 1 on load
//! error, deadlock, or invariant violation (v4.x treated deadlock as exit 0,
//! so since v5.0 `deadlock` is a first-class reference outcome).
//!
//! `#[ignore]` by default: the corpus and animate executable live outside the
//! repo. Run locally:
//!
//!   cargo test -p rossi-build --test animate_corpus -- --ignored --nocapture
//!
//! Environment overrides:
//!   EVENTB_CORPUS_DIR   — external Event-B model corpus directory
//!   EVENTB_ANIMATE      — eventb-animate executable (default: eventb-animate)
//!   EVENTB_ANIMATE_TIMEOUT_SECS — default 120
//! Relative executable paths are resolved from the workspace root.
//!
//! Per-model metadata comes from the corpus itself: column 4 of
//! `animate_results.tsv` names the machine each reference outcome was
//! recorded with (`(auto)` = let eventb-animate pick), and
//! `model_flags.tsv` flags known-broken (`defective` / `unsupported` /
//! `rodin_rejected`) and `nondeterministic` models.
//!
//! Output:
//!   target/eventb-models-regen/<model>.zip     — regenerated archives
//!   target/rossi-build-animate-corpus.tsv     — model | expected | actual | verdict
//!
//! Verdicts:
//!   match   — actual outcome matches the reference TSV
//!   known   — mismatch on a model flagged `defective` (broken source),
//!             `unsupported` (needs an Event-B extension rossi doesn't
//!             support yet, e.g. the theory plugin), or `rodin_rejected`
//!             (Rodin's own static checker rejects the pristine archive, so
//!             it ships `accurate="false"` artifacts; the pristine animates
//!             only because ProB tolerates Rodin's degraded output, and the
//!             regenerated archive's animate outcome is undefined) in the
//!             corpus `model_flags.tsv` (does not fail)
//!   flaky   — success ↔ invariant_violation ↔ deadlock drift on a model
//!             flagged `nondeterministic` (random animation can hit a
//!             reachable invariant violation, or reach a terminal state
//!             before the requested steps complete) (does not fail)
//!   regress — unexpected mismatch (fails the test)

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use common::{
    Row, WaitError, load_expected, load_flags, load_machines, locate_corpus, log_hint, regen_one,
    resolve_program, spawn_in_group, wait_with_timeout, workspace_target, write_report,
};

const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[test]
#[ignore]
fn animate_regenerated_corpus_matches_reference() {
    let Some(corpus) = locate_corpus() else {
        eprintln!("EVENTB_CORPUS_DIR is not set or is not a directory — nothing to do");
        return;
    };
    let Some(animate) = locate_animate() else {
        let configured =
            std::env::var("EVENTB_ANIMATE").unwrap_or_else(|_| "eventb-animate".into());
        eprintln!(
            "EVENTB_ANIMATE command `{configured}` was not found or is not executable — nothing to do"
        );
        return;
    };
    // Skip-when-unset applies to the *environment* (no corpus, no animate
    // executable); a configured corpus with a missing or malformed reference
    // file is a loud failure — silently returning here would green-light a
    // 0-model "gate".
    let reference_tsv = corpus.join("animate_results.tsv");
    let expected = load_expected(&reference_tsv).unwrap_or_else(|| {
        panic!("{} is missing or malformed", reference_tsv.display());
    });
    let machines = load_machines(&reference_tsv).unwrap_or_else(|| {
        panic!("{} is missing or malformed", reference_tsv.display());
    });
    let flags_tsv = corpus.join("model_flags.tsv");
    let flags = load_flags(&flags_tsv).unwrap_or_else(|| {
        panic!("{} is missing or malformed", flags_tsv.display());
    });

    let regen_dir = workspace_target().join("eventb-models-regen");
    std::fs::create_dir_all(&regen_dir).expect("create regen dir");
    let timeout = Duration::from_secs(
        std::env::var("EVENTB_ANIMATE_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS),
    );

    let mut zips: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("read corpus")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("zip"))
        .collect();
    zips.sort();

    let has_flag = |model: &str, flag: &str| flags.get(model).is_some_and(|f| f.contains(flag));
    let mut rows = Vec::<Row>::new();
    let mut regressions = 0usize;

    for zip in &zips {
        let model = zip
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let regen_zip = regen_dir.join(format!("{model}.zip"));
        let outcome = match regen_one(zip, &regen_zip) {
            Ok(()) => animate_one(
                &animate,
                &regen_zip,
                machines.get(model.as_str()).map(String::as_str),
                timeout,
            ),
            Err(e) => Outcome::Regen(e.to_string()),
        };
        let expected_outcome = expected
            .get(&model)
            .cloned()
            .unwrap_or_else(|| "?".to_string());
        let actual_str = outcome.label();
        let matches = actual_str == expected_outcome;
        let verdict = if matches {
            "match"
        } else if has_flag(&model, "defective")
            || has_flag(&model, "unsupported")
            || has_flag(&model, "rodin_rejected")
        {
            "known"
        } else if has_flag(&model, "nondeterministic")
            && is_tolerated_drift(&expected_outcome, actual_str)
        {
            "flaky"
        } else {
            regressions += 1;
            "regress"
        };
        rows.push(Row {
            model: model.clone(),
            expected: expected_outcome,
            actual: actual_str.to_string(),
            verdict: verdict.to_string(),
            notes: outcome.notes().to_string(),
        });
    }

    let report = workspace_target().join("rossi-build-animate-corpus.tsv");
    write_report(
        &report,
        &["model", "expected", "actual", "verdict", "notes"],
        &rows.iter().map(Row::to_fields).collect::<Vec<_>>(),
    );
    println!(
        "animate-corpus: {} archives, {} regressions (report: {})",
        zips.len(),
        regressions,
        report.display()
    );
    for r in rows.iter().filter(|r| r.verdict == "regress").take(20) {
        eprintln!(
            "  REGRESS  {}: expected {}, got {} — {}",
            r.model, r.expected, r.actual, r.notes
        );
    }
    assert!(
        regressions == 0,
        "{regressions} model(s) regressed (first 20 shown above)"
    );
}

#[derive(Debug, Clone)]
enum Outcome {
    Success,
    InvariantViolation,
    Deadlock,
    LoadError(String),
    Timeout,
    Regen(String),
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::InvariantViolation => "invariant_violation",
            Outcome::Deadlock => "deadlock",
            Outcome::LoadError(_) => "load_error",
            Outcome::Timeout => "timeout",
            Outcome::Regen(_) => "regen_error",
        }
    }

    fn notes(&self) -> &str {
        match self {
            Outcome::LoadError(s) | Outcome::Regen(s) => s.as_str(),
            _ => "",
        }
    }
}

fn locate_animate() -> Option<PathBuf> {
    let configured = std::env::var("EVENTB_ANIMATE").unwrap_or_else(|_| "eventb-animate".into());
    resolve_program(&configured)
}

fn animate_one(animate: &Path, zip: &Path, machine: Option<&str>, timeout: Duration) -> Outcome {
    let mut cmd = Command::new(animate);
    cmd.arg("--steps").arg("10").arg("--invariants");
    if let Some(m) = machine {
        cmd.arg("--machine").arg(m);
    }
    cmd.arg(zip);

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = match spawn_in_group(&mut cmd) {
        Ok(c) => c,
        Err(e) => return Outcome::LoadError(format!("spawn: {e}")),
    };
    match wait_with_timeout(child, timeout) {
        Ok((status, stdout, stderr)) => classify(status.code(), &stdout, &stderr),
        Err(WaitError::Timeout) => Outcome::Timeout,
        Err(WaitError::Io(e)) => Outcome::LoadError(format!("wait: {e}")),
    }
}

/// True when both outcomes are reachable end-states of the same random walk —
/// the drift the `nondeterministic` flag tolerates. `eventb-animate` has no
/// seed flag, so a 10-step walk over identical semantics can finish all steps
/// (`success`), hit a reachable invariant violation (`invariant_violation`),
/// or reach a state with no enabled events (`deadlock`); which one depends on
/// the random path, and the path differs between the pristine and regenerated
/// archives purely because their byte layout differs (element names/ordering).
/// A structural failure (`load_error`/`regen_error`) is never tolerated here.
fn is_tolerated_drift(expected: &str, actual: &str) -> bool {
    const DRIFT: [&str; 3] = ["success", "invariant_violation", "deadlock"];
    DRIFT.contains(&expected) && DRIFT.contains(&actual)
}

fn classify(exit: Option<i32>, stdout: &str, stderr: &str) -> Outcome {
    // eventb-animate v5.0 exit contract: 0 = success; 1 = load error,
    // deadlock, or invariant violation, distinguished by the output text.
    if exit == Some(0) {
        return Outcome::Success;
    }
    let combined = format!("{stdout}\n{stderr}");
    let lower = combined.to_lowercase();
    if combined.contains("violated invariants") {
        return Outcome::InvariantViolation;
    }
    if lower.contains("can't find an event") || lower.contains("deadlock") {
        return Outcome::Deadlock;
    }
    Outcome::LoadError(log_hint(&combined))
}
