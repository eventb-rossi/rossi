//! Corpus integration test — regenerate every model with our static checker,
//! then load it in ProB via the `animate` CLI and compare the outcome
//! (`success` / `invariant_violation` / `load_error` / `deadlock` / `timeout`)
//! against the reference `animate_results.tsv`.
//!
//! `#[ignore]` by default: the corpus, animate jar, and JDK live outside the
//! repo. Run locally:
//!
//!   cargo test -p rossi-build --test animate_corpus -- --ignored --nocapture
//!
//! Environment overrides:
//!   EVENTB_CORPUS_DIR   — external Event-B model corpus directory
//!   EVENTB_ANIMATE_JAR  — path to the animate CLI jar
//!   EVENTB_ANIMATE_TIMEOUT_SECS — default 120
//! Relative paths are resolved from the workspace root.
//!
//! Output:
//!   target/eventb-models-regen/<model>.zip     — regenerated archives
//!   target/rossi-build-animate-corpus.tsv     — model | expected | actual | verdict
//!
//! Verdicts:
//!   match   — actual outcome matches the reference TSV
//!   known   — mismatch is on the known-defective list (does not fail)
//!   regress — unexpected mismatch (fails the test)

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rossi_build::project::infer_project_name_from_archive_bytes;
use rossi_build::repack::repackage_zip_bytes;
use rossi_build::{Project, build};

/// Machines explicitly selected when an archive has multiple independent
/// refinement chains. Mirrors the corpus animation script.
const MACHINE_OVERRIDES: &[(&str, &str)] = &[
    ("abz2020-pitman-controller", "PitmanController2_TIME_MC"),
    ("aman", "M8_Interaction_Events"),
    ("aman_abstraction", "MAbs_helper"),
    ("aman_v7", "M0_AMAN_Update_prob_mc"),
    ("ca648", "Progress"),
    ("ca648_assignment1", "Question4_M0"),
    ("ebike", "M1"),
    ("etcs_s313", "m9_global_inputs"),
    ("evbt_vectors", "Vectors"),
    (
        "landing_gear_vos",
        "R9GearsDoorsHandleValvesControllerSwitchLightsSensorsTime",
    ),
    ("tutorial_fx3-tut2", "OCCUR2"),
    ("tutorial_fx4-tut2", "F-ALGOPC"),
    ("tutorial_fx5-tut2", "E-ALGO"),
    ("tutorial_ggx2-tut3", "test1"),
];

/// Models whose pre-existing reference outcome is itself a load/parse failure
/// (per `animate_results.tsv` and `CHECKER.md`). A mismatch on these is not
/// counted as a regression — they're either known-broken sources or ProB
/// limitations independent of our SC.
const KNOWN_DEFECTIVE: &[&str] = &[
    // animate_results.tsv pre-existing load errors
    "experiments_012_datarefinement",
    "progman",
    "tcb",
    // CHECKER.md known-broken sources (incomplete formulas)
    "tutorial_abk-summation",
    "tutorial_ggx2-tut3",
];

const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[test]
#[ignore]
fn animate_regenerated_corpus_matches_reference() {
    let Some(corpus) = locate_corpus() else {
        eprintln!("EVENTB_CORPUS_DIR is not set or is not a directory — nothing to do");
        return;
    };
    let Some(jar) = locate_jar() else {
        eprintln!("EVENTB_ANIMATE_JAR is not set or is not a file — nothing to do");
        return;
    };
    let reference_tsv = corpus.join("animate_results.tsv");
    let Some(expected) = load_expected(&reference_tsv) else {
        eprintln!("{} unreadable — nothing to do", reference_tsv.display());
        return;
    };

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

    let machine_map: BTreeMap<&str, &str> = MACHINE_OVERRIDES.iter().copied().collect();
    let known: std::collections::BTreeSet<&str> = KNOWN_DEFECTIVE.iter().copied().collect();
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
                &jar,
                &regen_zip,
                machine_map.get(model.as_str()).copied(),
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
        } else if known.contains(model.as_str()) {
            "known"
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
    write_report(&report, &rows);
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

struct Row {
    model: String,
    expected: String,
    actual: String,
    verdict: String,
    notes: String,
}

fn locate_corpus() -> Option<PathBuf> {
    env_path("EVENTB_CORPUS_DIR").filter(|p| p.is_dir())
}

fn locate_jar() -> Option<PathBuf> {
    env_path("EVENTB_ANIMATE_JAR").filter(|p| p.is_file())
}

fn env_path(var: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var(var).ok()?);
    Some(if path.is_absolute() {
        path
    } else {
        workspace_root().join(path)
    })
}

fn workspace_target() -> PathBuf {
    workspace_root().join("target")
}

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn load_expected(tsv: &Path) -> Option<BTreeMap<String, String>> {
    let s = std::fs::read_to_string(tsv).ok()?;
    let mut out = BTreeMap::new();
    for (i, line) in s.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let model = cols.next()?.to_string();
        let _exit = cols.next()?;
        let result = cols.next()?.to_string();
        out.insert(model, result);
    }
    Some(out)
}

fn regen_one(zip: &Path, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(zip)?;
    let name = infer_project_name_from_archive_bytes(&bytes).unwrap_or_else(|| {
        zip.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string()
    });
    let project = Project::from_zip_bytes(&name, &bytes)?;
    let result = build(&project);
    let new_bytes = repackage_zip_bytes(&bytes, &result)?;
    std::fs::write(out, new_bytes)?;
    Ok(())
}

fn animate_one(jar: &Path, zip: &Path, machine: Option<&str>, timeout: Duration) -> Outcome {
    let mut cmd = Command::new("java");
    cmd.arg("-jar")
        .arg(jar)
        .arg("--steps")
        .arg("10")
        .arg("--invariants");
    if let Some(m) = machine {
        cmd.arg("--machine").arg(m);
    }
    cmd.arg(zip);

    let child = match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Outcome::LoadError(format!("spawn: {e}")),
    };
    match wait_with_timeout(child, timeout) {
        Ok((status, stdout, stderr)) => classify(status.code(), &stdout, &stderr),
        Err(WaitError::Timeout) => Outcome::Timeout,
        Err(WaitError::Io(e)) => Outcome::LoadError(format!("wait: {e}")),
    }
}

enum WaitError {
    Timeout,
    Io(std::io::Error),
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<(std::process::ExitStatus, String, String), WaitError> {
    use std::io::Read;
    use std::sync::mpsc;
    use std::thread;

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (tx_out, rx_out) = mpsc::channel();
    let (tx_err, rx_err) = mpsc::channel();
    thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        let _ = tx_out.send(s);
    });
    thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        let _ = tx_err.send(s);
    });

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = rx_out.recv().unwrap_or_default();
                let err = rx_err.recv().unwrap_or_default();
                return Ok((status, out, err));
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(WaitError::Timeout);
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(WaitError::Io(e)),
        }
    }
}

fn classify(exit: Option<i32>, stdout: &str, stderr: &str) -> Outcome {
    if exit == Some(0) {
        return Outcome::Success;
    }
    let combined = format!("{stdout}\n{stderr}");
    let lower = combined.to_lowercase();
    if combined.contains("violated invariants") {
        return Outcome::InvariantViolation;
    }
    if lower.contains("can't find an event")
        || lower.contains("deadlock")
        || lower.contains("no events")
    {
        return Outcome::Deadlock;
    }
    // Pull a short hint for the report.
    let hint = combined
        .lines()
        .rev()
        .find(|l| {
            if l.trim_start().starts_with("at ") {
                return false;
            }
            let lc = l.to_lowercase();
            lc.contains("error") || lc.contains("exception") || lc.contains("failed")
        })
        .unwrap_or("")
        .trim()
        .to_string();
    Outcome::LoadError(hint)
}

fn write_report(path: &Path, rows: &[Row]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("could not write {}: {e}", path.display());
            return;
        }
    };
    let _ = writeln!(f, "model\texpected\tactual\tverdict\tnotes");
    for r in rows {
        let _ = writeln!(
            f,
            "{}\t{}\t{}\t{}\t{}",
            sanitize(&r.model),
            sanitize(&r.expected),
            sanitize(&r.actual),
            sanitize(&r.verdict),
            sanitize(&r.notes)
        );
    }
}

/// Collapse embedded tabs/newlines to single spaces so each TSV row stays on
/// one line — pest's multi-line parse errors are a common source of leakage.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
